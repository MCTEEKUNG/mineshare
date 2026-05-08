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
    CURSORINFO, CURSOR_SHOWING, CallNextHookEx, DispatchMessageW, GetClipCursor, GetCursorInfo,
    GetCursorPos, GetMessageW, GetSystemMetrics, HC_ACTION, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    SM_CXSCREEN, SM_CYSCREEN, SetCursorPos, SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_XBUTTONDOWN, WM_XBUTTONUP,
};
use windows::Win32::Foundation::{CloseHandle, MAX_PATH, RECT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    PROCESS_NAME_FORMAT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

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
/// Hotkey: Ctrl+Alt+L toggles game-mode lock — pins all input to
/// this PC so accidental edge crosses during fullscreen gameplay
/// don't yank focus.
const SCAN_HOTKEY_LOCK: u32 = 0x26; // L

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
    // Restore the local cursor to the edge that faces the peer
    // (per the configured layout). User came back across that
    // edge to leave Remote, so dropping the OS cursor there
    // matches their hand position on the desk. For top/bottom
    // we keep their horizontal anchor, just snap Y to the edge.
    let (restore_x, restore_y) = match super::peer_side() {
        super::PeerSide::Right => (w - 1, restore_y.clamp(0, h - 1)),
        super::PeerSide::Left => (0, restore_y.clamp(0, h - 1)),
        super::PeerSide::Top => (LAST_X.load(Ordering::Relaxed).clamp(0, w - 1), 0),
        super::PeerSide::Bottom => (LAST_X.load(Ordering::Relaxed).clamp(0, w - 1), h - 1),
    };
    unsafe {
        let _ = SetCursorPos(restore_x, restore_y);
    }
    LAST_X.store(restore_x, Ordering::Relaxed);
    LAST_Y.store(restore_y, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
    info!(restore = ?(restore_x, restore_y), "cursor → local");
    super::fire_remote_event(super::RemoteEvent::Exited);
}

pub fn local_in_remote() -> bool {
    CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE
}

pub fn force_exit_remote() {
    if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
        let h = SCREEN_H.load(Ordering::Relaxed);
        info!("force_exit_remote — peer asked us to release");
        exit_remote(h / 2);
    }
}

/// Called when the peer signals it has taken Remote control of us.
/// Warps the local cursor to the boundary edge facing the peer (per
/// the configured layout) so the peer's virt_x model matches the
/// real cursor position — without this the peer's exit threshold
/// fires after a tiny motion in the wrong direction even though the
/// cursor is mid-screen.
pub fn on_peer_take_control() {
    let w = SCREEN_W.load(Ordering::Relaxed);
    let h = SCREEN_H.load(Ordering::Relaxed);
    let (x, y) = match super::peer_side() {
        super::PeerSide::Right => ((w - 1).max(0), (h / 2).clamp(0, (h - 1).max(0))),
        super::PeerSide::Left => (0, (h / 2).clamp(0, (h - 1).max(0))),
        super::PeerSide::Top => ((w / 2).clamp(0, (w - 1).max(0)), 0),
        super::PeerSide::Bottom => ((w / 2).clamp(0, (w - 1).max(0)), (h - 1).max(0)),
    };
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    // Update the hook's "last seen" so HW-motion auto-release doesn't
    // mis-fire on the first injected motion arriving from the peer.
    LAST_X.store(x, Ordering::Relaxed);
    LAST_Y.store(y, Ordering::Relaxed);
    info!(boundary = ?(x, y), side = ?super::peer_side(), "warped cursor to peer-facing edge");
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

        // Game auto-detect: a separate poll thread watches the
        // visible-cursor flag + the clip-cursor rect. When a
        // foreground app hides or confines the cursor (the
        // signature of fullscreen FPS / GTA / Minecraft mouse
        // capture), we auto-engage the input lock so an
        // accidental edge cross during gameplay can't yank focus
        // to the peer. We deliberately only set/clear an
        // *auto* flag, separate from the manual lock — the user's
        // explicit Ctrl+Alt+L override always wins.
        thread::Builder::new()
            .name("win-game-detect".into())
            .spawn(|| game_detect_thread())
            .context("spawn game-detect thread")?;
        Ok(())
    }
}

/// Anti-cheat-protected executables. Foreground match → auto-lock
/// the bridge regardless of cursor state, and surface a red
/// banner on the GUI Status tab so the user *knows* why their
/// keyboard isn't crossing. These games' kernel-level anti-cheat
/// (BattlEye / EAC / Vanguard / RICOCHET / Hyperion) routinely
/// bans accounts for SendInput-style injected events — silently
/// dropping the bridge is the safer default than letting an
/// accidental edge cross arrive as suspect input on the peer.
///
/// Match is case-insensitive on the basename. Add new entries
/// freely — false positives just engage a lock the user can
/// override with Ctrl+Alt+R, false negatives are the dangerous
/// direction.
const RISKY_GAMES: &[&str] = &[
    // Riot Vanguard
    "VALORANT.exe",
    "VALORANT-Win64-Shipping.exe",
    "LeagueClient.exe",
    "League of Legends.exe",
    // BattlEye
    "FortniteClient-Win64-Shipping.exe",
    "RainbowSix.exe",
    "RainbowSix_Vulkan.exe",
    "TslGame.exe", // PUBG
    "destiny2.exe",
    "ArmaReforger.exe",
    "DayZ_x64.exe",
    "Tarkov.exe",
    "EscapeFromTarkov.exe",
    // Easy Anti-Cheat
    "r5apex.exe", // Apex Legends
    "r5apex_dx12.exe",
    "FFXIV_dx11.exe",
    "RustClient.exe",
    "ELDENRING.exe",
    // Activision RICOCHET
    "cod.exe",
    "ModernWarfare.exe",
    "BlackOpsColdWar.exe",
    // Roblox Hyperion
    "RobloxPlayerBeta.exe",
    // FACEIT / ESEA
    "csgo.exe",
    "cs2.exe",
];

fn current_foreground_exe_basename() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let res = QueryFullProcessImageNameW(proc, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(proc);
        if res.is_err() {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    }
}

/// Polls cursor visibility + clip-rect + foreground-process state
/// every 250 ms. Auto-engages the input lock when:
///   * the cursor is hidden (Minecraft-style fullscreen capture)
///   * OR the cursor clip rect is smaller than the screen (FPS
///     mouse-confine)
///   * OR the foreground process matches the anti-cheat-protected
///     `RISKY_GAMES` list — even if cursor is visible (main menu)
///
/// Manual user-engaged locks survive auto-detect releases (user
/// always wins) so alt-tabbing out of a game momentarily doesn't
/// drop a user-set lock.
fn game_detect_thread() {
    use std::sync::atomic::AtomicBool;
    static AUTO_ENGAGED: AtomicBool = AtomicBool::new(false);
    let poll_ms = std::time::Duration::from_millis(250);
    loop {
        std::thread::sleep(poll_ms);
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        let cursor_hidden = unsafe { GetCursorInfo(&mut ci) }.is_ok()
            && (ci.flags.0 & CURSOR_SHOWING.0) == 0;

        // Cursor confine: a real fullscreen game's clip rect is a
        // small fraction of the screen (the playable window).
        // Plain Tauri / browser focus can knock a few pixels off
        // the OS default clip and would otherwise false-positive,
        // so require the clip to be meaningfully smaller —
        // < 70% on BOTH axes — before treating it as a game.
        let mut clip = RECT::default();
        let cursor_clipped = unsafe { GetClipCursor(&mut clip) }.is_ok() && {
            let w = SCREEN_W.load(Ordering::Relaxed);
            let h = SCREEN_H.load(Ordering::Relaxed);
            let cw = clip.right - clip.left;
            let ch = clip.bottom - clip.top;
            cw < w * 7 / 10 && ch < h * 7 / 10
        };

        // Foreground process match — independent of cursor state.
        let exe = current_foreground_exe_basename();
        let anticheat_match = exe.as_deref().and_then(|e| {
            RISKY_GAMES
                .iter()
                .find(|r| r.eq_ignore_ascii_case(e))
                .map(|_| e.to_string())
        });
        super::set_anticheat_warning(anticheat_match.clone());

        let should_lock = cursor_hidden || cursor_clipped || anticheat_match.is_some();
        let was_engaged = AUTO_ENGAGED.load(Ordering::Acquire);
        if should_lock != was_engaged {
            AUTO_ENGAGED.store(should_lock, Ordering::Release);
            if should_lock {
                if !super::is_input_locked() {
                    info!(
                        cursor_hidden,
                        cursor_clipped,
                        anticheat = ?anticheat_match,
                        "game auto-detect — engaging lock"
                    );
                    super::set_input_locked(true);
                }
            } else if super::is_input_locked() && was_engaged {
                info!("game auto-detect — releasing lock");
                super::set_input_locked(false);
            }
        }
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
                // Auto-release peer Remote on real local HW motion (the
                // user is moving Win HW while Ubuntu is driving). Same
                // pattern as Synergy: any local input on the passive
                // side reclaims the cursor.
                if super::peer_in_remote()
                    && last_x != i32::MIN
                    && (x - last_x).abs() + (y - last_y).abs() > 5
                {
                    info!(
                        dx = x - last_x,
                        dy = y - last_y,
                        "local HW motion while peer holds Remote — requesting peer release"
                    );
                    super::fire_remote_event(super::RemoteEvent::RequestPeerExit);
                }
                let h = SCREEN_H.load(Ordering::Relaxed);
                LAST_X.store(x, Ordering::Relaxed);
                LAST_Y.store(y, Ordering::Relaxed);
                // Edge to watch depends on layout. We treat hitting
                // the configured boundary edge as the trigger.
                // Game-mode lock pins input to this PC — skip the
                // edge check entirely so accidental cursor moves
                // during fullscreen play don't yank focus to the
                // peer. Ctrl+Alt+R still works as a manual override.
                let crossed_edge = !super::is_input_locked()
                    && last_x != i32::MIN
                    && match super::peer_side() {
                        super::PeerSide::Right => last_x < w - 1 && x >= w - 1,
                        super::PeerSide::Left => last_x > 0 && x <= 0,
                        super::PeerSide::Top => last_y > 0 && y <= 0,
                        super::PeerSide::Bottom => last_y < h - 1 && y >= h - 1,
                    };
                if crossed_edge {
                    enter_remote(y);
                }
                // local: don't forward, OS handles cursor as usual
            } else {
                // REMOTE: compute delta from anchor, clamp to peer screen,
                // and forward. The "depth" axis (how far INTO the peer
                // we've gone) flips with the layout — right means
                // rightward dx, left means -dx, top means -dy, bottom
                // means dy. Exit fires when depth retreats to
                // -EXIT_BUFFER_PX past the entry edge.
                let dx = x - last_x;
                let dy = y - last_y;
                let depth_dx = match super::peer_side() {
                    super::PeerSide::Right => dx,
                    super::PeerSide::Left => -dx,
                    super::PeerSide::Top => -dy,
                    super::PeerSide::Bottom => dy,
                };
                let peer_w = PEER_W.load(Ordering::Relaxed);
                // Clamp accumulated virt_x to [-EXIT_BUFFER_PX, peer_w-1].
                // The lower floor doubles as the exit trigger; the upper
                // bound stops further depth-direction dx from being
                // absorbed into ever-growing virt_x (which would force
                // the user to drag back for thousands of events to escape).
                let raw = VIRT_X.load(Ordering::Relaxed) + depth_dx;
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

        // Hotkey: Ctrl+Alt+L toggles game-mode lock.
        if down
            && scan == SCAN_HOTKEY_LOCK
            && MOD_CTRL.load(Ordering::Relaxed)
            && MOD_ALT.load(Ordering::Relaxed)
        {
            let next = !super::is_input_locked();
            info!(locked = next, "hotkey Ctrl+Alt+L — game-mode lock");
            super::set_input_locked(next);
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
        // Sign convention mismatch:
        //   * Linux/Win HW: REL_WHEEL / WM_MOUSEWHEEL positive = wheel
        //     rotated forward (away from user) → content scrolls UP.
        //   * enigo's `scroll(length, Vertical)`: positive = scroll
        //     DOWN. (See enigo docs: "positive value means scroll
        //     down/right, negative means scroll up/left".)
        // Without negation, every wheel-up on the controller becomes
        // wheel-down on the Win receiver — exactly the inverted
        // "looking" feel the user reported.
        if dy != 0.0 {
            self.inner
                .lock()
                .scroll(-dy.round() as i32, Axis::Vertical)
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
