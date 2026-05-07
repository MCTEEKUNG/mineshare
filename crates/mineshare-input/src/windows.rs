//! Windows input via low-level hooks (capture) and enigo (inject).
//!
//! Capture uses `SetWindowsHookEx(WH_MOUSE_LL/WH_KEYBOARD_LL)` running on a
//! dedicated thread with a message pump. The hook delivers mouse positions
//! in screen coordinates — we convert to relative deltas by tracking the
//! previous position. (For true raw deltas we'd want `RegisterRawInputDevices`
//! + `WM_INPUT`; that's an M5 polish item.)
//!
//! Inject uses `enigo` 0.6 for cross-platform consistency. We can swap in
//! native `SendInput` later if precision becomes a problem.

use std::mem;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;

use anyhow::{Context, Result};
use enigo::{
    Axis, Button as EButton, Coordinate, Direction, Enigo, Key as EKey, Keyboard, Mouse, Settings,
};
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use super::{Button, InputCapture, InputEvent, InputInject, KeyCode};

static EVENT_SINK: OnceLock<Mutex<Option<UnboundedSender<InputEvent>>>> = OnceLock::new();
static LAST_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_Y: AtomicI32 = AtomicI32::new(i32::MIN);

fn sink_send(ev: InputEvent) {
    if let Some(s) = EVENT_SINK.get()
        && let Some(tx) = s.lock().as_ref()
    {
        let _ = tx.send(ev);
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
    // Install hooks on this thread.
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_mouse_hook), None, 0) };
    let kb = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(low_kb_hook), None, 0) };
    let mouse_ok = matches!(&mouse, Ok(h) if !h.0.is_null());
    let kb_ok = matches!(&kb, Ok(h) if !h.0.is_null());
    if !mouse_ok || !kb_ok {
        warn!(?mouse, ?kb, "SetWindowsHookExW failed (need GUI session)");
        return;
    }
    info!("Win hooks installed (WH_MOUSE_LL + WH_KEYBOARD_LL)");

    // Pump messages forever — the OS calls our hook procs from this thread.
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

// MSLLHOOKSTRUCT::flags — these aren't directly exported as constants
// in `windows-rs`; values match the Win32 SDK (winuser.h).
const LLMHF_INJECTED: u32 = 0x00000001;
const LLMHF_LOWER_IL_INJECTED: u32 = 0x00000002;
const LLKHF_INJECTED: u32 = 0x00000010;
const LLKHF_LOWER_IL_INJECTED: u32 = 0x00000002;

unsafe extern "system" fn low_mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        // Skip events synthesised by SendInput / our own enigo inject —
        // otherwise the hook captures what we just injected and we feed
        // the network into a self-perpetuating loop.
        if info.flags & (LLMHF_INJECTED | LLMHF_LOWER_IL_INJECTED) != 0 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let x = info.pt.x;
        let y = info.pt.y;

        match wparam.0 as u32 {
            WM_MOUSEMOVE => {
                let lx = LAST_X.swap(x, Ordering::Relaxed);
                let ly = LAST_Y.swap(y, Ordering::Relaxed);
                if lx != i32::MIN && ly != i32::MIN {
                    let dx = x - lx;
                    let dy = y - ly;
                    if dx != 0 || dy != 0 {
                        sink_send(InputEvent::MouseMove { dx, dy });
                    }
                }
            }
            WM_LBUTTONDOWN => sink_send(InputEvent::MouseButton {
                btn: Button::Left,
                down: true,
            }),
            WM_LBUTTONUP => sink_send(InputEvent::MouseButton {
                btn: Button::Left,
                down: false,
            }),
            WM_RBUTTONDOWN => sink_send(InputEvent::MouseButton {
                btn: Button::Right,
                down: true,
            }),
            WM_RBUTTONUP => sink_send(InputEvent::MouseButton {
                btn: Button::Right,
                down: false,
            }),
            WM_MBUTTONDOWN => sink_send(InputEvent::MouseButton {
                btn: Button::Middle,
                down: true,
            }),
            WM_MBUTTONUP => sink_send(InputEvent::MouseButton {
                btn: Button::Middle,
                down: false,
            }),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                // Which X button is in high word of mouseData
                let high = (info.mouseData >> 16) as u16;
                let btn = match high {
                    1 => Button::X1,
                    2 => Button::X2,
                    _ => return unsafe { CallNextHookEx(None, code, wparam, lparam) },
                };
                sink_send(InputEvent::MouseButton {
                    btn,
                    down: wparam.0 as u32 == WM_XBUTTONDOWN,
                });
            }
            WM_MOUSEWHEEL => {
                let delta = ((info.mouseData >> 16) as i16) as f32 / 120.0;
                sink_send(InputEvent::Scroll { dx: 0.0, dy: delta });
            }
            _ => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn low_kb_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.flags.0 & (LLKHF_INJECTED | LLKHF_LOWER_IL_INJECTED) != 0 {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        let scan = info.scanCode as u16; // PS/2 set-1 ≈ Linux KEY_*
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        if down || up {
            sink_send(InputEvent::Key {
                code: KeyCode(scan),
                down,
            });
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
        // Use enigo's raw scancode pathway. enigo on Windows interprets
        // `Key::Other(u32)` as a virtual-key code; we map our KeyCode (PS/2
        // scan code, equal to Linux KEY_*) to a virtual key via OS API.
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
    // MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) on Windows; we re-import
    // from `windows` crate.
    use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK_EX, MapVirtualKeyW};
    unsafe {
        let vk = MapVirtualKeyW(scan as u32, MAPVK_VSC_TO_VK_EX);
        if vk == 0 {
            // fallback — reinterpret scan as VK
            scan
        } else {
            (vk & 0xFFFF) as u16
        }
    }
}

// Silence unused warning for VIRTUAL_KEY (used implicitly via MapVirtualKeyW).
#[allow(dead_code)]
fn _force_vk_use(_v: VIRTUAL_KEY) {}

// Ensure mem types are pulled in.
const _: usize = mem::size_of::<MSLLHOOKSTRUCT>();
