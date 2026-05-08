//! Cross-platform input capture and injection.
//!
//! Normalized event types are designed to round-trip cleanly between Linux
//! (evdev) and Windows (Raw Input / SendInput). Key codes use the Linux
//! `KEY_*` numbering, which matches PS/2 set-1 scan codes for the common
//! keys — Windows side translates virtual keys to scan codes on the way in
//! and back to virtual keys on the way out.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

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

/// Lifecycle events the platform-specific capture modules emit when they
/// switch into or out of Remote mode. The daemon listens for these and
/// translates them into `ControlMsg`s over the encrypted TCP control
/// channel so the two peers can coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteEvent {
    /// Local capture has just entered Remote mode. Translates to
    /// `ControlMsg::TakeControl`.
    Entered,
    /// Local capture has just left Remote mode. Translates to
    /// `ControlMsg::ReleaseControl`.
    Exited,
    /// User asked the *peer* to leave Remote (hotkey pressed locally
    /// while the peer holds Remote). Translates to
    /// `ControlMsg::ForceRelease`.
    RequestPeerExit,
}

static REMOTE_EVT_TX: Mutex<Option<UnboundedSender<RemoteEvent>>> = Mutex::new(None);
static PEER_IN_REMOTE: AtomicBool = AtomicBool::new(false);

/// "Game mode" — when on, edge detection / press gestures / auto
/// cursor-handover are all disabled and the bridge stays pinned
/// to the local machine. The Ctrl+Alt+R hotkey still works as a
/// deliberate escape hatch (the user can ALWAYS leave or enter
/// Remote with the keyboard) and peer-driven auto-release on
/// real local HW also still fires. Persisted via the daemon's
/// AppData config, toggled via the Ctrl+Alt+L hotkey or the
/// Status tab in the GUI.
static INPUT_LOCKED: AtomicBool = AtomicBool::new(false);

/// Name of the foreground anti-cheat-protected game when the
/// Win-side game-detect thread has flagged one. `None` when no
/// risky title is in the foreground. The GUI surfaces this as a
/// red banner on the Status tab so the user knows *why* the
/// input lock auto-engaged. Anti-cheat engines like BattlEye /
/// EAC / Vanguard / RICOCHET ban accounts for using injected
/// input — flagging matters more than just cursor capture.
static ANTICHEAT_WARNING: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

pub fn anticheat_warning() -> Option<String> {
    ANTICHEAT_WARNING.lock().clone()
}

pub(crate) fn set_anticheat_warning(name: Option<String>) {
    let mut g = ANTICHEAT_WARNING.lock();
    let changed = g.as_ref() != name.as_ref();
    *g = name.clone();
    if changed {
        if let Some(ref n) = name {
            tracing::warn!(game = %n, "anti-cheat-protected game in foreground — input locked");
        } else {
            tracing::info!("anti-cheat foreground cleared");
        }
    }
}

pub fn is_input_locked() -> bool {
    INPUT_LOCKED.load(Ordering::Acquire)
}

pub fn set_input_locked(v: bool) {
    let prev = INPUT_LOCKED.swap(v, Ordering::AcqRel);
    if prev != v {
        tracing::info!(locked = v, "input lock toggled");
        if v {
            // Engaging the lock while in Remote drops us back to
            // local immediately so the user isn't stuck driving
            // the peer with no edge-crossing way back.
            force_local_exit_remote();
        }
    }
}

/// Which side of the local screen the peer monitor is "stuck to"
/// in the user's physical desk arrangement. The platform-specific
/// capture modules read this to decide which edge of our display
/// triggers entry into Remote, where to warp the cursor on
/// TakeControl, and which sign convention `virt_x` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSide {
    Left = 0,
    Right = 1,
    Top = 2,
    Bottom = 3,
}

impl PeerSide {
    /// True when the side runs along a vertical edge of the local
    /// screen (left or right). Tracking depth then comes from
    /// horizontal HW deltas (`dx`).
    pub fn is_horizontal(self) -> bool {
        matches!(self, PeerSide::Left | PeerSide::Right)
    }
}

static PEER_SIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1); // Right

pub fn peer_side() -> PeerSide {
    match PEER_SIDE.load(Ordering::Acquire) {
        0 => PeerSide::Left,
        1 => PeerSide::Right,
        2 => PeerSide::Top,
        _ => PeerSide::Bottom,
    }
}

pub fn set_peer_side(side: PeerSide) {
    let prev = PEER_SIDE.swap(side as u8, Ordering::AcqRel);
    if prev != side as u8 {
        tracing::info!(?side, "peer side updated");
        // Stale press accumulator from the previous side would
        // otherwise leak: a left-edge counter doesn't mean
        // anything once the user has switched to top, etc. Same
        // for the cursor-position estimate on Linux — re-seed it
        // to mid-screen so the next overshoot detection has a
        // sane baseline regardless of where the user's real
        // cursor actually is right now.
        #[cfg(target_os = "linux")]
        linux::reset_after_side_change();
    }
}

/// Daemon registers a channel here once the encrypted control session is
/// up. Capture modules call `fire_remote_event` on each transition.
pub fn set_remote_event_sender(tx: UnboundedSender<RemoteEvent>) {
    *REMOTE_EVT_TX.lock() = Some(tx);
}

pub fn clear_remote_event_sender() {
    *REMOTE_EVT_TX.lock() = None;
}

/// Returns true if the peer has signalled it is currently driving Remote
/// mode, in which case the local capture must refuse to enter Remote
/// itself (otherwise both ends would forward each other's HW input and
/// the cursors fight on both screens).
/// Returns true if *we* (the local capture) have entered Remote
/// mode and are forwarding HW input to the peer. The platform
/// modules each maintain their own cursor-mode state machine; this
/// helper queries whichever one is compiled in. Used by the GUI
/// shell's status snapshot.
pub fn local_in_remote() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows::local_in_remote()
    }
    #[cfg(target_os = "linux")]
    {
        linux::local_in_remote()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        false
    }
}

pub fn peer_in_remote() -> bool {
    PEER_IN_REMOTE.load(Ordering::Acquire)
}

pub fn set_peer_in_remote(v: bool) {
    PEER_IN_REMOTE.store(v, Ordering::Release);
}

pub(crate) fn fire_remote_event(ev: RemoteEvent) {
    if let Some(tx) = REMOTE_EVT_TX.lock().as_ref() {
        let _ = tx.send(ev);
    }
}

/// Forces the local capture to leave Remote mode (used when the peer
/// asks us to release control via `ControlMsg::ForceRelease`).
pub fn force_local_exit_remote() {
    #[cfg(target_os = "windows")]
    {
        windows::force_exit_remote();
    }
    #[cfg(target_os = "linux")]
    {
        linux::force_exit_remote();
    }
}

/// Called when the peer signals it has taken Remote control of us
/// (`ControlMsg::TakeControl`). Warps the local cursor to the boundary
/// edge that faces the peer so the peer's `virt_x` model matches the
/// real cursor position — without this the peer's exit hysteresis
/// fires after a few pixels of rightward motion because their model
/// thinks we're already at the boundary while reality has the cursor
/// somewhere mid-screen.
pub fn on_peer_take_control(inject: &dyn InputInject) {
    #[cfg(target_os = "windows")]
    {
        let _ = inject;
        windows::on_peer_take_control();
    }
    #[cfg(target_os = "linux")]
    {
        linux::on_peer_take_control(inject);
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = inject;
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
