//! Cross-platform input capture and injection.
//!
//! Normalized event types are designed to round-trip cleanly between Linux
//! (evdev) and Windows (Raw Input / SendInput). Key codes use the Linux
//! `KEY_*` numbering, which matches PS/2 set-1 scan codes for the common
//! keys — Windows side translates virtual keys to scan codes on the way in
//! and back to virtual keys on the way out.

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Linux KEY_* numbering (also matches PS/2 set-1 scan codes for common keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyCode(pub u16);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { btn: Button, down: bool },
    Key { code: KeyCode, down: bool },
    Scroll { dx: f32, dy: f32 },
}

/// Captures raw HID input. `start` spawns whatever background work the
/// platform needs and pushes events into the provided channel.
///
/// In M1 capture is always *passive* — every local input is also delivered
/// to the local OS as normal. M2 will add `set_grab(true)` to swallow events
/// when the cursor is on a remote monitor.
pub trait InputCapture: Send {
    fn start(&mut self, sink: tokio::sync::mpsc::UnboundedSender<InputEvent>)
    -> anyhow::Result<()>;

    /// Reserved for M2 — block local delivery while cursor is remote.
    fn set_grab(&mut self, _grab: bool) {}
}

pub trait InputInject: Send + Sync {
    fn mouse_move_rel(&self, dx: i32, dy: i32) -> anyhow::Result<()>;
    fn mouse_button(&self, btn: Button, down: bool) -> anyhow::Result<()>;
    fn key(&self, code: KeyCode, down: bool) -> anyhow::Result<()>;
    fn scroll(&self, dx: f32, dy: f32) -> anyhow::Result<()>;

    fn dispatch(&self, event: InputEvent) -> anyhow::Result<()> {
        match event {
            InputEvent::MouseMove { dx, dy } => self.mouse_move_rel(dx, dy),
            InputEvent::MouseButton { btn, down } => self.mouse_button(btn, down),
            InputEvent::Key { code, down } => self.key(code, down),
            InputEvent::Scroll { dx, dy } => self.scroll(dx, dy),
        }
    }
}

/// Returns the local primary screen geometry in **physical** pixels.
///
/// Platform notes:
///   * Windows: triggers per-monitor DPI awareness on first call, then
///     reads `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`. Subsequent calls
///     return the same DPI-aware value.
///   * Linux: read from `MINESHARE_SCREEN_W` / `MINESHARE_SCREEN_H` env
///     vars (defaults `1920x1080`). Slice 3 will query Wayland/X11 directly.
pub fn local_screen_geometry() -> (u32, u32) {
    #[cfg(target_os = "windows")]
    {
        windows::local_screen_geometry()
    }
    #[cfg(target_os = "linux")]
    {
        linux::local_screen_geometry()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        (1920, 1080)
    }
}

/// Stores the peer's primary screen geometry (received via the encrypted
/// control channel) so the platform-specific edge/hysteresis logic can
/// clamp `virt_x` against the real peer width.
pub fn set_peer_screen(w: u32, h: u32) {
    #[cfg(target_os = "windows")]
    {
        windows::set_peer_screen(w, h);
    }
    #[cfg(target_os = "linux")]
    {
        linux::set_peer_screen(w, h);
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = (w, h);
    }
}

/// Construct the platform-specific capture implementation.
pub fn make_capture() -> anyhow::Result<Box<dyn InputCapture>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::EvdevCapture::new()?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::HookCapture::new()?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        anyhow::bail!("input capture is not implemented on this platform")
    }
}

/// Construct the platform-specific injection implementation.
pub fn make_inject() -> anyhow::Result<Box<dyn InputInject>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::UinputInject::new()?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::EnigoInject::new()?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        anyhow::bail!("input injection is not implemented on this platform")
    }
}
