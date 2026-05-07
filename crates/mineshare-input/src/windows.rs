//! Windows input via low-level hooks (capture) and enigo (inject).
//!
//! M2 layer adds **edge-triggered cursor handover**: when the cursor reaches
//! the right edge of the local screen we enter a "remote" mode where
//!  * the local cursor is warped to a centre anchor every event so subsequent
//!    HW motion keeps producing fresh deltas (the WH_MOUSE_LL hook needs
//!    cursor position changes to fire);
//!  * captured deltas, button events, and keystrokes are forwarded to the
//!    peer instead of being processed locally;
//!  * a virtual `(virt_x, virt_y)` cursor position is tracked in remote
//!    space — when `virt_x` falls below zero we hand control back and the
//!    real Windows cursor is restored at the right edge.
//!
//! `SetCursorPos` does not trigger `WH_MOUSE_LL` (only HW interrupts do), so
//! the warp is invisible to the hook.

use std::mem;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::thread;

use anyhow::{Context, Result};
use enigo::{
    Axis, Button as EButton, Coordinate, Direction, Enigo, Key as EKey, Keyboard, Mouse, Settings,
};
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, HC_ACTION,
    KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CYSCREEN, SetCursorPos,
    SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use super::{Button, InputCapture, InputEvent, InputInject, KeyCode};

const MODE_LOCAL: u8 = 0;
const MODE_REMOTE: u8 = 1;

static EVENT_SINK: OnceLock<Mutex<Option<UnboundedSender<InputEvent>>>> = OnceLock::new();
static LAST_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_Y: AtomicI32 = AtomicI32::new(i32::MIN);
static CURSOR_MODE: AtomicU8 = AtomicU8::new(MODE_LOCAL);
static SCREEN_W: AtomicI32 = AtomicI32::new(1920);
static SCREEN_H: AtomicI32 = AtomicI32::new(1080);
/// Approximate peer screen width — used to clamp `VIRT_X` so that pushing
/// past the peer's right edge stops accumulating instead of letting the
/// virtual cursor race off into infinity (which makes it impossible to
/// drag back to negative virt_x and exit Remote mode).
///
/// 1920 is a sensible default until M2 Slice 2 negotiates the real width
/// over the encrypted control channel.
static PEER_W: AtomicI32 = AtomicI32::new(1920);
static VIRT_X: AtomicI32 = AtomicI32::new(0);
static VIRT_Y: AtomicI32 = AtomicI32::new(0);

/// Hysteresis buffer in pixels at the left edge of the peer screen. The
/// user has to drag this much further left than virt_x = 0 before we hand
/// control back to the local desktop. Without it, any tiny leftward jitter
/// (or natural left-tracking inside the peer screen) bounces the cursor
/// back out of Remote mode immediately.
const EXIT_BUFFER_PX: i32 = 100;

// Modifier-key tracking (PS/2 set-1 scan codes — left/right both produce
// the same scancode here, so we ignore the LLKHF_EXTENDED bit).
const SCAN_CTRL: u32 = 0x1D;
const SCAN_ALT: u32 = 0x38;
/// Hotkey: Ctrl+Alt+R forces exit_remote regardless of cursor position.
/// Useful when remote-mode gets stuck (e.g. peer disconnected mid-session).
const SCAN_HOTKEY: u32 = 0x13; // R

static MOD_CTRL: AtomicBool = AtomicBool::new(false);
static MOD_ALT: AtomicBool = AtomicBool::new(false);

/// Per-event delta cap. Windows coalesces fast HW motion into a single
/// `WM_MOUSEMOVE` whose pt-delta can be hundreds or thousands of pixels —
/// large enough to throw the peer cursor straight to a screen edge in one
/// frame. Cap each forwarded delta so the peer sees a smooth stream of
/// reasonable steps. 30px keeps Ubuntu cursor smooth even with Win at
/// 200% DPI and Linux's default acceleration profile.
const MAX_DELTA_PX: i32 = 30;

fn sink_send(ev: InputEvent) {
    if let Some(s) = EVENT_SINK.get()
        && let Some(tx) = s.lock().as_ref()
    {
        let _ = tx.send(ev);
    }
}

pub fn local_screen_geometry() -> (u32, u32) {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| unsafe {
        // Idempotent — failures here just mean DPI awareness is already
        // set. Calling before HookCapture::start() lets a `--no-capture`
        // daemon still report DPI-aware physical pixels.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
    let w = unsafe { GetSystemMetrics(SM_CXSCREEN).max(1) as u32 };
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN).max(1) as u32 };
    SCREEN_W.store(w as i32, Ordering::Relaxed);
    SCREEN_H.store(h as i32, Ordering::Relaxed);
    (w, h)
}

pub fn set_peer_screen(w: u32, _h: u32) {
    PEER_W.store(w.max(1) as i32, Ordering::Relaxed);
    info!(peer_w = w, "peer screen geometry stored");
}

fn anchor() -> (i32, i32) {
    (
        SCREEN_W.load(Ordering::Relaxed) / 2,
        SCREEN_H.load(Ordering::Relaxed) / 2,
    )
}

fn enter_remote(entry_y: i32) {
    // Refuse if the peer signalled it's already driving Remote — otherwise
    // both ends forward each other's HW input simultaneously and we end
    // up with cursors fighting on both screens.
    if super::peer_in_remote() {
        debug!("enter_remote refused — peer holds Remote");
        return;
    }
    let (ax, ay) = anchor();
    VIRT_X.store(0, Ordering::Relaxed);
    VIRT_Y.store(entry_y, Ordering::Relaxed);
    unsafe {
        let _ = SetCursorPos(ax, ay);
    }
    LAST_X.store(ax, Ordering::Relaxed);
    LAST_Y.store(ay, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_REMOTE, Ordering::Release);
    info!(entry_y, anchor = ?(ax, ay), "cursor → remote");
    super::fire_remote_event(super::RemoteEvent::Entered);
}

fn exit_remote(restore_y: i32) {
    let w = SCREEN_W.load(Ordering::Relaxed);
    let h = SCREEN_H.load(Ordering::Relaxed);
    let y = restore_y.clamp(0, h - 1);
    unsafe {
        let _ = SetCursorPos(w - 1, y);
    }
    LAST_X.store(w - 1, Ordering::Relaxed);
    LAST_Y.store(y, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
    info!(restore = ?(w - 1, y), "cursor → local");
    super::fire_remote_event(super::RemoteEvent::Exited);
}

pub fn force_exit_remote() {
    if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
        let h = SCREEN_H.load(Ordering::Relaxed);
        info!("force_exit_remote — peer asked us to release");
        exit_remote(h / 2);
    }
}

pub struct HookCapture {
    started: bool,
}

impl HookCapture {
    pub fn new() -> Result<Self> {
        Ok(Self { started: false })
    }
}

impl InputCapture for HookCapture {
    fn start(&mut self, sink: UnboundedSender<InputEvent>) -> Result<()> {
        if self.started {
            return Ok(());
        }
        self.started = true;

        // Make this process per-monitor-DPI-aware **before** querying
        // screen geometry. Without it, GetSystemMetrics returns logical
        // (DPI-virtualised) pixels while WH_MOUSE_LL delivers physical
        // pixels — a 200% scale display then reports cursor coords up to
        // 2× our screen-width assumption, producing dx values of 2000+
        // that warp Ubuntu's cursor straight to the right edge.
        unsafe {
            // Best-effort: a process can only set this once, so failure
            // here usually just means a previous setter already ran.
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        // Probe primary screen geometry once at start. Multi-monitor +
        // hot-plug come in M2 Slice 3.
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w > 0 && h > 0 {
            SCREEN_W.store(w, Ordering::Relaxed);
            SCREEN_H.store(h, Ordering::Relaxed);
            info!(width = w, height = h, "primary screen geometry");
        } else {
            warn!("GetSystemMetrics returned 0 — falling back to 1920x1080");
        }

        let cell = EVENT_SINK.get_or_init(|| Mutex::new(None));
        *cell.lock() = Some(sink);

        thread::Builder::new()
            .name("win-input-hooks".into())
            .spawn(|| unsafe { hook_thread() })
            .context("spawn hook thread")?;
        Ok(())
    }
}

unsafe fn hook_thread() {
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_mouse_hook), None, 0) };
    let kb = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_kb_hook), None, 0) };
    let mouse_ok = matches!(&mouse, Ok(h) if !h.0.is_null());
    let kb_ok = matches!(&kb, Ok(h) if !h.0.is_null());
    if !mouse_ok || !kb_ok {
        warn!(?mouse, ?kb, "SetWindowsHookExW failed (need GUI session)");
        return;
    }
    info!("Win hooks installed (WH_MOUSE_LL + WH_KEYBOARD_LL)");

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 == 0 || r.0 == -1 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

const LLMHF_INJECTED: u32 = 0x00000001;
const LLMHF_LOWER_IL_INJECTED: u32 = 0x00000002;
const LLKHF_INJECTED: u32 = 0x00000010;
const LLKHF_LOWER_IL_INJECTED: u32 = 0x00000002;

unsafe extern "system" fn low_mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code != HC_ACTION as i32 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    if info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let x = info.pt.x;
    let y = info.pt.y;
    let mode = CURSOR_MODE.load(Ordering::Acquire);
    let last_x = LAST_X.load(Ordering::Relaxed);
    let last_y = LAST_Y.load(Ordering::Relaxed);

    match wparam.0 as u32 {
        WM_MOUSEMOVE => {
            if mode == MODE_LOCAL {
                let w = SCREEN_W.load(Ordering::Relaxed);
                LAST_X.store(x, Ordering::Relaxed);
                LAST_Y.store(y, Ordering::Relaxed);
                if last_x != i32::MIN && last_x < w - 1 && x >= w - 1 {
                    enter_remote(y);
                }
                // local: don't forward, OS handles cursor as usual
            } else {
                // REMOTE: compute delta from anchor, clamp to peer screen,
                // and forward.
                let dx = x - last_x;
                let dy = y - last_y;
                let peer_w = PEER_W.load(Ordering::Relaxed);
                // Clamp accumulated virt_x to [-EXIT_BUFFER_PX, peer_w-1].
                // The lower floor doubles as the exit trigger; the upper
                // bound stops further rightward dx from being absorbed
                // into ever-growing virt_x (which would force the user
                // to drag left for thousands of events to escape).
                let raw = VIRT_X.load(Ordering::Relaxed) + dx;
                let new_virt_x = raw.clamp(-EXIT_BUFFER_PX, peer_w - 1);
                VIRT_X.store(new_virt_x, Ordering::Relaxed);
                VIRT_Y.fetch_add(dy, Ordering::Relaxed);

                if new_virt_x <= -EXIT_BUFFER_PX {
                    exit_remote(y);
                } else {
                    // Cap each forwarded step so coalesced HW motion can't
                    // teleport the peer cursor across its screen.
                    let fdx = dx.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
                    let fdy = dy.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
                    if fdx != 0 || fdy != 0 {
                        static FWD_COUNT: AtomicI32 = AtomicI32::new(0);
                        let n = FWD_COUNT.fetch_add(1, Ordering::Relaxed);
                        if n % 200 == 0 {
                            info!(
                                raw_dx = dx,
                                raw_dy = dy,
                                fdx,
                                fdy,
                                virt_x = new_virt_x,
                                n,
                                "sample motion forward"
                            );
                        }
                        sink_send(InputEvent::MouseMove { dx: fdx, dy: fdy });
                    }
                    let (ax, ay) = anchor();
                    unsafe {
                        let _ = SetCursorPos(ax, ay);
                    }
                    LAST_X.store(ax, Ordering::Relaxed);
                    LAST_Y.store(ay, Ordering::Relaxed);
                }
                // Consume the event so the OS doesn't process it locally.
                return LRESULT(1);
            }
        }
        WM_LBUTTONDOWN if mode == MODE_REMOTE => sink_send(InputEvent::MouseButton {
            btn: Button::Left,
            down: true,
        }),
        WM_LBUTTONUP if mode == MODE_REMOTE => sink_send(InputEvent::MouseButton {
            btn: Button::Left,
            down: false,
        }),
        WM_RBUTTONDOWN if mode == MODE_REMOTE => sink_send(InputEvent::MouseButton {
            btn: Button::Right,
            down: true,
        }),
        WM_RBUTTONUP if mode == MODE_REMOTE => sink_send(InputEvent::MouseButton {
            btn: Button::Right,
            down: false,
        }),
        WM_MBUTTONDOWN if mode == MODE_REMOTE => sink_send(InputEvent::MouseButton {
            btn: Button::Middle,
            down: true,
        }),
        WM_MBUTTONUP if mode == MODE_REMOTE => sink_send(InputEvent::MouseButton {
            btn: Button::Middle,
            down: false,
        }),
        WM_XBUTTONDOWN | WM_XBUTTONUP if mode == MODE_REMOTE => {
            let high = (info.mouseData >> 16) as u16;
            if let Some(btn) = match high {
                1 => Some(Button::X1),
                2 => Some(Button::X2),
                _ => None,
            } {
                sink_send(InputEvent::MouseButton {
                    btn,
                    down: wparam.0 as u32 == WM_XBUTTONDOWN,
                });
            }
        }
        WM_MOUSEWHEEL if mode == MODE_REMOTE => {
            let delta = ((info.mouseData >> 16) as i16) as f32 / 120.0;
            sink_send(InputEvent::Scroll { dx: 0.0, dy: delta });
        }
        _ => {}
    }
    // In remote mode every mouse event has been forwarded; consume it so
    // the OS doesn't double-process it locally.
    if mode == MODE_REMOTE {
        return LRESULT(1);
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn low_kb_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.flags.0 & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let scan = info.scanCode;
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);

        // Track modifier state regardless of mode so the hotkey works
        // even after a half-pressed transition.
        if scan == SCAN_CTRL && (down || up) {
            MOD_CTRL.store(down, Ordering::Relaxed);
        }
        if scan == SCAN_ALT && (down || up) {
            MOD_ALT.store(down, Ordering::Relaxed);
        }

        let mode = CURSOR_MODE.load(Ordering::Acquire);

        // Hotkey: Ctrl+Alt+R toggles Local ⇄ Remote, or asks the peer to
        // release if the peer is the one currently driving.
        if down
            && scan == SCAN_HOTKEY
            && MOD_CTRL.load(Ordering::Relaxed)
            && MOD_ALT.load(Ordering::Relaxed)
        {
            if mode == MODE_REMOTE {
                info!("hotkey Ctrl+Alt+R — forcing exit_remote");
                let h = SCREEN_H.load(Ordering::Relaxed);
                exit_remote(h / 2);
            } else if super::peer_in_remote() {
                info!("hotkey Ctrl+Alt+R — requesting peer to release");
                super::fire_remote_event(super::RemoteEvent::RequestPeerExit);
            } else {
                info!("hotkey Ctrl+Alt+R — entering remote");
                let mut pt = POINT::default();
                let entry_y = unsafe {
                    if GetCursorPos(&mut pt).is_ok() {
                        pt.y
                    } else {
                        SCREEN_H.load(Ordering::Relaxed) / 2
                    }
                };
                enter_remote(entry_y);
            }
            return LRESULT(1);
        }

        // Keystrokes follow the cursor — only forward when remote.
        if mode == MODE_REMOTE && (down || up) {
            sink_send(InputEvent::Key {
                code: KeyCode(scan as u16),
                down,
            });
            // Consume so Windows doesn't also act on the keystroke.
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

pub struct EnigoInject {
    inner: Mutex<Enigo>,
}

impl EnigoInject {
    pub fn new() -> Result<Self> {
        let inner = Enigo::new(&Settings::default()).context("init enigo")?;
        debug!("enigo inject ready");
        Ok(Self {
            inner: Mutex::new(inner),
        })
    }
}

impl InputInject for EnigoInject {
    fn mouse_move_rel(&self, dx: i32, dy: i32) -> Result<()> {
        self.inner
            .lock()
            .move_mouse(dx, dy, Coordinate::Rel)
            .context("enigo move_mouse")?;
        Ok(())
    }

    fn mouse_button(&self, btn: Button, down: bool) -> Result<()> {
        let b = match btn {
            Button::Left => EButton::Left,
            Button::Right => EButton::Right,
            Button::Middle => EButton::Middle,
            Button::X1 => EButton::Back,
            Button::X2 => EButton::Forward,
        };
        let dir = if down {
            Direction::Press
        } else {
            Direction::Release
        };
        self.inner.lock().button(b, dir).context("enigo button")?;
        Ok(())
    }

    fn key(&self, code: KeyCode, down: bool) -> Result<()> {
        let vk = scancode_to_vk(code.0);
        let dir = if down {
            Direction::Press
        } else {
            Direction::Release
        };
        self.inner
            .lock()
            .key(EKey::Other(vk as u32), dir)
            .context("enigo key")?;
        Ok(())
    }

    fn scroll(&self, _dx: f32, dy: f32) -> Result<()> {
        if dy != 0.0 {
            self.inner
                .lock()
                .scroll(dy.round() as i32, Axis::Vertical)
                .context("enigo scroll v")?;
        }
        Ok(())
    }
}

fn scancode_to_vk(scan: u16) -> u16 {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK_EX, MapVirtualKeyW};
    unsafe {
        let vk = MapVirtualKeyW(scan as u32, MAPVK_VSC_TO_VK_EX);
        if vk == 0 { scan } else { (vk & 0xFFFF) as u16 }
    }
}

#[allow(dead_code)]
fn _force_vk_use(_v: VIRTUAL_KEY) {}

const _: usize = mem::size_of::<MSLLHOOKSTRUCT>();
