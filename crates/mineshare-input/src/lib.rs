//! Cross-platform input capture and injection.
//!
//! Normalized event types are designed to round-trip cleanly between Linux
//! (evdev) and Windows (Raw Input / SendInput). Key codes use the Linux
//! `KEY_*` numbering, which matches PS/2 set-1 scan codes for the common
//! keys — Windows side translates virtual keys to scan codes on the way in
//! and back to virtual keys on the way out.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "windows")]
pub mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

pub const MAX_TOUCHPAD_CONTACTS: usize = 5;

/// Device-relative contact position. Both axes cover the complete physical
/// touchpad range, independent of either computer's DPI or display layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TouchpadContact {
    pub id: u32,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TouchpadAction {
    Tap,
    Press,
    Release,
}

/// Authoritative Precision Touchpad stream carried over the encrypted input
/// datagram channel. A frame contains the complete active-contact snapshot;
/// receivers derive DOWN/UPDATE/UP transitions from consecutive snapshots so
/// the next delivered frame repairs a lost UDP begin/update packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TouchpadEvent {
    Frame {
        stream_id: u32,
        count: u8,
        contacts: [TouchpadContact; MAX_TOUCHPAD_CONTACTS],
        terminal: bool,
    },
    Action {
        fingers: u8,
        action: TouchpadAction,
    },
}

impl TouchpadEvent {
    pub fn frame(stream_id: u32, contacts: &[TouchpadContact], terminal: bool) -> Option<Self> {
        if contacts.len() > MAX_TOUCHPAD_CONTACTS
            || (terminal && !contacts.is_empty())
            || (!terminal && contacts.is_empty())
        {
            return None;
        }
        let mut packed = [TouchpadContact::default(); MAX_TOUCHPAD_CONTACTS];
        packed[..contacts.len()].copy_from_slice(contacts);
        Some(Self::Frame {
            stream_id,
            count: contacts.len() as u8,
            contacts: packed,
            terminal,
        })
    }

    pub fn terminal(stream_id: u32) -> Self {
        Self::Frame {
            stream_id,
            count: 0,
            contacts: [TouchpadContact::default(); MAX_TOUCHPAD_CONTACTS],
            terminal: true,
        }
    }

    pub fn contacts(&self) -> Option<&[TouchpadContact]> {
        match self {
            Self::Frame {
                count,
                contacts,
                terminal,
                ..
            } => {
                let count = usize::from(*count);
                (count <= MAX_TOUCHPAD_CONTACTS
                    && ((*terminal && count == 0) || (!*terminal && count > 0)))
                    .then(|| &contacts[..count])
            }
            Self::Action { .. } => None,
        }
    }

    pub fn coalescible_stream(&self) -> Option<u32> {
        match self {
            Self::Frame {
                stream_id,
                terminal: false,
                ..
            } if self.contacts().is_some() => Some(*stream_id),
            _ => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Frame { terminal: true, .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { btn: Button, down: bool },
    Key { code: KeyCode, down: bool },
    Scroll { dx: f32, dy: f32 },
    Touchpad(TouchpadEvent),
}

/// Compact authoritative state for keys/buttons currently routed to the
/// peer. Periodic snapshots heal a lost UDP key/button edge without putting
/// every input event behind TCP retransmission latency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardedInputState {
    key_words: [u64; 16],
    mouse_buttons: u8,
}

impl ForwardedInputState {
    pub fn any_held(&self) -> bool {
        self.mouse_buttons != 0 || self.key_words.iter().any(|word| *word != 0)
    }

    pub fn apply_event(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key { code, down } => self.set_key(code.0, down),
            InputEvent::MouseButton { btn, down } => self.set_button(btn, down),
            InputEvent::MouseMove { .. } | InputEvent::Scroll { .. } | InputEvent::Touchpad(_) => {}
        }
    }

    pub fn reconciliation_events(&self, desired: &Self) -> Vec<InputEvent> {
        let mut events = Vec::new();
        for code in 0..1024u16 {
            let current = self.key_is_down(code);
            let want = desired.key_is_down(code);
            if current != want {
                events.push(InputEvent::Key {
                    code: KeyCode(code),
                    down: want,
                });
            }
        }
        for btn in [
            Button::Left,
            Button::Right,
            Button::Middle,
            Button::X1,
            Button::X2,
        ] {
            let current = self.button_is_down(btn);
            let want = desired.button_is_down(btn);
            if current != want {
                events.push(InputEvent::MouseButton { btn, down: want });
            }
        }
        events
    }

    fn key_is_down(&self, code: u16) -> bool {
        let index = code as usize;
        index < 1024 && self.key_words[index / 64] & (1u64 << (index % 64)) != 0
    }

    fn set_key(&mut self, code: u16, down: bool) {
        let index = code as usize;
        if index >= 1024 {
            return;
        }
        let mask = 1u64 << (index % 64);
        if down {
            self.key_words[index / 64] |= mask;
        } else {
            self.key_words[index / 64] &= !mask;
        }
    }

    fn button_is_down(&self, btn: Button) -> bool {
        self.mouse_buttons & (1u8 << btn_index(btn)) != 0
    }

    fn set_button(&mut self, btn: Button, down: bool) {
        let mask = 1u8 << btn_index(btn);
        if down {
            self.mouse_buttons |= mask;
        } else {
            self.mouse_buttons &= !mask;
        }
    }
}

/// Captures raw HID input. `start` spawns whatever background work the
/// platform needs and calls the provided callback for each event.
///
/// The callback is invoked directly from the OS hook / evdev pump thread,
/// with no intermediate MPSC channel — eliminating one async scheduler
/// round-trip from the critical mouse-event path.
///
/// In M1 capture is always *passive* — every local input is also delivered
/// to the local OS as normal. M2 will add `set_grab(true)` to swallow events
/// when the cursor is on a remote monitor.
pub trait InputCapture: Send {
    fn start(
        &mut self,
        sink: std::sync::Arc<dyn Fn(InputEvent) + Send + Sync + 'static>,
    ) -> anyhow::Result<()>;

    /// Reserved for M2 — block local delivery while cursor is remote.
    fn set_grab(&mut self, _grab: bool) {}
}

/// Hard ceiling on the per-event relative mouse delta we will inject.
///
/// Defense-in-depth against a single pathological MouseMove teleporting a
/// mouse-look game's camera. The proven failure (daemon log): at peer
/// take-control the cursor is warped to the screen edge and the *first*
/// forwarded delta is computed against a stale anchor, yielding a
/// ~screen-half delta (e.g. `MouseMove { dx: -969, dy: 453 }`). Injected
/// as one relative move that flings the in-game camera to the sky.
///
/// Real aiming peaks around a few dozen pixels per 500 Hz forward event
/// (observed ~64), so 200 leaves generous headroom while neutralising the
/// half-screen artifact regardless of which layer produced it. Tunable; a
/// future handover-suppression fix can make this purely a safety net.
pub(crate) const MAX_INJECT_DELTA_PX: i32 = 200;

pub trait InputInject: Send + Sync {
    fn mouse_move_rel(&self, dx: i32, dy: i32) -> anyhow::Result<()>;
    fn mouse_button(&self, btn: Button, down: bool) -> anyhow::Result<()>;
    fn key(&self, code: KeyCode, down: bool) -> anyhow::Result<()>;
    fn scroll(&self, dx: f32, dy: f32) -> anyhow::Result<()>;
    fn touchpad(&self, _event: TouchpadEvent) -> anyhow::Result<()> {
        Ok(())
    }

    fn dispatch(&self, event: InputEvent) -> anyhow::Result<()> {
        match event {
            InputEvent::MouseMove { dx, dy } => self.mouse_move_rel(
                dx.clamp(-MAX_INJECT_DELTA_PX, MAX_INJECT_DELTA_PX),
                dy.clamp(-MAX_INJECT_DELTA_PX, MAX_INJECT_DELTA_PX),
            ),
            InputEvent::MouseButton { btn, down } => self.mouse_button(btn, down),
            InputEvent::Key { code, down } => self.key(code, down),
            InputEvent::Scroll { dx, dy } => self.scroll(dx, dy),
            InputEvent::Touchpad(event) => self.touchpad(event),
        }
    }

    /// Emit synthetic release events for any keys / mouse buttons
    /// the peer has injected without their matching up-event. The
    /// daemon calls this when a peer session ends so the OS doesn't
    /// stay convinced a remote-injected key (typical: WASD held
    /// during a game when the network drops) is still pressed.
    /// Default no-op for impls that don't track held state.
    fn release_all_held(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub const TOUCHPAD_CAP_FRAME_CAPTURE: u32 = 1 << 0;
pub const TOUCHPAD_CAP_ACTION_CAPTURE: u32 = 1 << 1;
pub const TOUCHPAD_CAP_FRAME_INJECT: u32 = 1 << 2;
pub const TOUCHPAD_CAP_ACTION_INJECT: u32 = 1 << 3;

static PEER_TOUCHPAD_CAPABILITIES: AtomicU32 = AtomicU32::new(0);

pub fn local_touchpad_capabilities() -> u32 {
    #[cfg(target_os = "windows")]
    {
        windows::touchpad_capabilities()
    }
    #[cfg(not(target_os = "windows"))]
    {
        0
    }
}

pub fn set_peer_touchpad_capabilities(capabilities: u32) {
    PEER_TOUCHPAD_CAPABILITIES.store(capabilities, Ordering::Release);
}

pub fn peer_touchpad_capabilities() -> u32 {
    PEER_TOUCHPAD_CAPABILITIES.load(Ordering::Acquire)
}

pub fn native_touchpad_pair_available(local: u32, peer: u32) -> bool {
    local & TOUCHPAD_CAP_FRAME_CAPTURE != 0 && peer & TOUCHPAD_CAP_FRAME_INJECT != 0
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
    /// User toggled Game Drive ON locally → ask the peer to start receiving.
    /// Translates to `ControlMsg::GameDrive { active: true }`.
    GameDriveStart,
    /// User toggled Game Drive OFF → ask the peer to stop receiving.
    /// Translates to `ControlMsg::GameDrive { active: false }`.
    GameDriveStop,
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

/// Game Drive mode — a user-toggled mode that forwards raw relative
/// mouse + keyboard continuously to the peer, bypassing cursor-crossing,
/// warp, handover, and game-lock so a cursor-locked game running on the
/// peer can be driven smoothly from the local machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameDrive {
    /// Normal cursor-crossing behavior.
    Off = 0,
    /// This machine forwards raw mouse+keyboard continuously to the peer.
    Driving = 1,
    /// This machine injects the peer's pure-relative input (game runs here).
    Receiving = 2,
}

static GAME_DRIVE: AtomicU8 = AtomicU8::new(0);

pub fn game_drive() -> GameDrive {
    match GAME_DRIVE.load(Ordering::Acquire) {
        1 => GameDrive::Driving,
        2 => GameDrive::Receiving,
        _ => GameDrive::Off,
    }
}

pub fn set_game_drive(s: GameDrive) {
    GAME_DRIVE.store(s as u8, Ordering::Release);
}

pub fn is_game_driving() -> bool {
    matches!(game_drive(), GameDrive::Driving)
}

pub fn is_game_receiving() -> bool {
    matches!(game_drive(), GameDrive::Receiving)
}

// ---------------------------------------------------------------------------
// Stage 10 game-compat polish: capture-side knobs.
//
// `MOUSE_SENS_BITS` stores an `f32` multiplier as raw bits (Atomic
// f32 isn't in std). The capture path multiplies forwarded mouse
// deltas by this before sending, so the user can dial in cursor
// speed when local + peer DPIs disagree without messing with OS
// settings on either box.
//
// `INVERT_SCROLL_X / Y` flip the sign of forwarded wheel events.
// "Natural scroll" mismatch is the #1 cross-OS gripe; one
// checkbox per axis keeps it cheap to fix per machine.
// ---------------------------------------------------------------------------

static MOUSE_SENS_BITS: AtomicU32 = AtomicU32::new(0x3F80_0000); // f32 1.0
static TOUCHPAD_SCROLL_SPEED_BITS: AtomicU32 = AtomicU32::new(0x3F80_0000); // f32 1.0
static INVERT_SCROLL_X: AtomicBool = AtomicBool::new(false);
static INVERT_SCROLL_Y: AtomicBool = AtomicBool::new(false);

pub fn mouse_sensitivity() -> f32 {
    f32::from_bits(MOUSE_SENS_BITS.load(Ordering::Relaxed))
}

pub fn set_mouse_sensitivity(v: f32) {
    let clamped = if v.is_finite() {
        v.clamp(0.25, 4.0)
    } else {
        1.0
    };
    MOUSE_SENS_BITS.store(clamped.to_bits(), Ordering::Relaxed);
}

/// Multiplier for outgoing two-finger touchpad translation and legacy wheel
/// deltas. Native contact scaling preserves each contact's offset from the
/// centroid, so pinch distance and three/four-finger gestures are unaffected.
pub fn touchpad_scroll_speed() -> f32 {
    f32::from_bits(TOUCHPAD_SCROLL_SPEED_BITS.load(Ordering::Relaxed))
}

pub fn set_touchpad_scroll_speed(v: f32) {
    let clamped = if v.is_finite() {
        v.clamp(0.25, 4.0)
    } else {
        1.0
    };
    TOUCHPAD_SCROLL_SPEED_BITS.store(clamped.to_bits(), Ordering::Relaxed);
}

pub fn invert_scroll_x() -> bool {
    INVERT_SCROLL_X.load(Ordering::Relaxed)
}

pub fn invert_scroll_y() -> bool {
    INVERT_SCROLL_Y.load(Ordering::Relaxed)
}

pub fn set_invert_scroll(x: bool, y: bool) {
    INVERT_SCROLL_X.store(x, Ordering::Relaxed);
    INVERT_SCROLL_Y.store(y, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Runtime-tunable mouse rate (Phase 1).
//
// The user picks a forward/inject rate (60..=1000 Hz) from the GUI;
// `set_mouse_rate_hz` converts it to a flush interval in microseconds
// and stores it in `TARGET_FLUSH_US`. The Windows forward watchdog and
// the Linux capture-forward coalescer read `target_flush_us()` each
// cycle so a slider drag takes effect live. Replaces the old
// compile-time `FLUSH_INTERVAL_MS` constants on both platforms.
//
// `FWD_EVENTS` / `INJ_EVENTS` count actual forwarded (Windows) and
// injected (Linux/receiver) mouse motions so the GUI can show the
// *measured* rate next to the *set* rate — if the live rate stays
// below the setting, the hardware / inject path is the ceiling.
// ---------------------------------------------------------------------------

use std::sync::atomic::AtomicU64;

/// Runtime flush interval in microseconds, derived from the user's
/// mouse-rate setting. Read each cycle by the Windows forward
/// watchdog and the Linux inject coalescer. Default 2000 us = 500 Hz.
static TARGET_FLUSH_US: AtomicU64 = AtomicU64::new(2_000);

/// Pure mapping Hz -> flush microseconds, clamped to 60..=1000 Hz.
pub(crate) fn hz_to_flush_us(hz: u32) -> u64 {
    let hz = hz.clamp(60, 1000) as u64;
    1_000_000 / hz
}

/// Set the target mouse rate. Called from `settings::push_to_input_layer`.
pub fn set_mouse_rate_hz(hz: u32) {
    TARGET_FLUSH_US.store(hz_to_flush_us(hz), Ordering::Relaxed);
}

/// Current mouse-motion interval in microseconds. Platform capture loops and
/// the daemon's receive-side injection pacer share this deadline so neither
/// side can burst faster than the rate selected by the user.
pub fn target_flush_us() -> u64 {
    TARGET_FLUSH_US.load(Ordering::Relaxed)
}

static FWD_EVENTS: AtomicU64 = AtomicU64::new(0); // forwarded MouseMove flushes (Windows)
static INJ_EVENTS: AtomicU64 = AtomicU64::new(0); // injected mouse moves (Linux/receiver)

pub(crate) fn bump_fwd_events() {
    FWD_EVENTS.fetch_add(1, Ordering::Relaxed);
}
#[cfg(target_os = "linux")]
pub(crate) fn bump_inj_events() {
    INJ_EVENTS.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct MouseRateStats {
    pub set_hz: u32,
    pub fwd_total: u64,
    pub inj_total: u64,
}

pub fn mouse_rate_stats() -> MouseRateStats {
    MouseRateStats {
        set_hz: (1_000_000 / target_flush_us().max(1)) as u32,
        fwd_total: FWD_EVENTS.load(Ordering::Relaxed),
        inj_total: INJ_EVENTS.load(Ordering::Relaxed),
    }
}

// ---------------------------------------------------------------------------
// Auto-focus assist for remote keyboard handoff.
//
// On GNOME-Wayland (Ubuntu's default desktop), keyboard focus is
// click-to-focus by default — moving the mouse cursor over a
// window does NOT make subsequent typing land on it. So when the
// peer drives the local cursor across, our uinput keyboard does
// emit keys, but no window has focus and the keys vanish.
//
// Windows activates the window under the incoming boundary cursor without a
// synthetic click as part of TakeControl itself. Motion / first-key retries
// cover the rare case where Windows temporarily rejects foreground transfer.
// Linux retains its platform-specific take-control behavior.
// ---------------------------------------------------------------------------

static AUTO_FOCUS_ON_TAKE: AtomicBool = AtomicBool::new(false);

pub fn auto_focus_on_take_control() -> bool {
    AUTO_FOCUS_ON_TAKE.load(Ordering::Relaxed)
}

pub fn set_auto_focus_on_take_control(v: bool) {
    AUTO_FOCUS_ON_TAKE.store(v, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Diagnostic counters: see how many keys we actually forward (Win
// side, in MODE_REMOTE) vs inject (Linux side, on uinput
// keyboard). Surfaced on the GUI Advanced tab so the user can
// tell at a glance whether their keystrokes are crossing the
// bridge or getting eaten somewhere along the way.
// ---------------------------------------------------------------------------

static KEYS_FORWARDED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static KEYS_INJECTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn note_key_forwarded_with_code(code: u16, down: bool) {
    let n = KEYS_FORWARDED.fetch_add(1, Ordering::Relaxed);
    // Rate-limited INFO log so users debugging "phantom keys
    // appear when I cross the cursor" can see what scancode is
    // actually flying. First 10 keys → log every one. After that
    // → every 50th. KEY_SPACE (57) and KEY_ENTER (28) always
    // log because they're the most common "what just typed
    // that?!" suspects.
    if n < 10 || n.is_multiple_of(50) || code == 57 || code == 28 {
        tracing::info!(
            scancode = code,
            down,
            total = n + 1,
            "key forwarded to peer"
        );
    }
}

pub(crate) fn note_key_injected_with_code(code: u16, down: bool) {
    let n = KEYS_INJECTED.fetch_add(1, Ordering::Relaxed);
    if n < 10 || n.is_multiple_of(50) || code == 57 || code == 28 {
        tracing::info!(
            scancode = code,
            down,
            total = n + 1,
            "key injected from peer"
        );
    }
}

pub fn keys_forwarded() -> u64 {
    KEYS_FORWARDED.load(Ordering::Relaxed)
}

pub fn keys_injected() -> u64 {
    KEYS_INJECTED.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Keyboard target: decouple keyboard direction from mouse cursor.
//
// The previous "keys follow cursor" behaviour breaks the very
// common workflow of "leave mouse on machine A while typing into
// a window on machine B" — e.g., reading a doc on Windows while
// typing a Discord message on a Ubuntu app. Mouse + keyboard are
// physically two devices; users want them routed independently
// when they have to.
//
// Three states:
//   * Auto       — keys follow the mouse cursor (legacy behaviour, default)
//   * ForcePeer  — keys ALWAYS go to the peer, regardless of cursor
//   * ForceLocal — keys ALWAYS stay local, regardless of cursor
//
// Cycled via Ctrl+Alt+K hotkey on the capture side; the GUI's
// status pill shows the current target so the user can tell at
// a glance where their next keystroke will land.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardTarget {
    /// Default: forward when our cursor is on the peer's screen
    /// (covers the "cursor-cross + drive peer" workflow) AND
    /// also when the peer's mouse is being actively used while
    /// ours is idle (covers the "two-mouse-one-keyboard"
    /// workflow — Linux mouse on the Linux PC, Win keyboard on
    /// the Win PC, type into a Linux app without crossing the
    /// cursor over).
    #[default]
    Smart,
    /// Strict cursor-following only — keys go where our cursor
    /// points and nowhere else. The pre-Smart legacy behaviour;
    /// kept for users who don't want any activity-based magic.
    Auto,
    /// Pin keys to the peer regardless of cursor or activity.
    ForcePeer,
    /// Pin keys to this machine regardless of cursor or activity.
    ForceLocal,
}

static KEYBOARD_TARGET: AtomicU8 = AtomicU8::new(0); // 0 == Smart (default)

pub fn keyboard_target() -> KeyboardTarget {
    match KEYBOARD_TARGET.load(Ordering::Relaxed) {
        1 => KeyboardTarget::Auto,
        2 => KeyboardTarget::ForcePeer,
        3 => KeyboardTarget::ForceLocal,
        _ => KeyboardTarget::Smart,
    }
}

pub fn set_keyboard_target(t: KeyboardTarget) {
    let v = match t {
        KeyboardTarget::Smart => 0,
        KeyboardTarget::Auto => 1,
        KeyboardTarget::ForcePeer => 2,
        KeyboardTarget::ForceLocal => 3,
    };
    let prev = KEYBOARD_TARGET.swap(v, Ordering::AcqRel);
    if prev != v {
        tracing::info!(target = ?t, "keyboard target changed");
    }
}

/// Cycle Smart → ForcePeer → ForceLocal → Auto → Smart. The
/// hotkey itself is consumed and never forwarded — otherwise
/// users would lose the ability to switch back once they've
/// forced keys to the other side.
pub fn cycle_keyboard_target() {
    let next = match keyboard_target() {
        KeyboardTarget::Smart => KeyboardTarget::ForcePeer,
        KeyboardTarget::ForcePeer => KeyboardTarget::ForceLocal,
        KeyboardTarget::ForceLocal => KeyboardTarget::Auto,
        KeyboardTarget::Auto => KeyboardTarget::Smart,
    };
    set_keyboard_target(next);
}

/// Sticky memory of the last concrete Smart-mode routing
/// decision. Read when both local and peer mouse activity are
/// stale (user is typing without touching either mouse — common
/// on Linux apps that auto-hide the cursor after idle, where
/// "cursor disappeared" used to fool Smart into rolling back to
/// local mid-paragraph).
static LAST_SMART_TO_PEER: AtomicBool = AtomicBool::new(false);

/// Public for the daemon to call on session-start so a fresh
/// connection doesn't inherit stale stickiness from the previous
/// peer.
pub fn reset_smart_decision() {
    LAST_SMART_TO_PEER.store(false, Ordering::Relaxed);
    // Drop stale peer-activity timing so a new session doesn't race
    // against the previous peer's last-seen activity.
    PEER_ACTIVITY_AT.store(0, Ordering::Relaxed);
    // Note: local stamps (LOCAL_MOUSE_AT/LOCAL_CLICK_AT) are intentionally
    // NOT cleared, so a fresh session defaults to routing local (peer reads
    // as "never", local may still read as recent).
    clear_held_forwarded();
    clear_held_buttons();
    #[cfg(target_os = "windows")]
    windows::reset_forwarded_keyboard_latches();
}

/// Held-key tracker for "key currently held down whose press was
/// forwarded to the peer". Each scancode that we routed away
/// from the local OS gets a `true` here; on key-up we look it
/// up and forward the release **to the same destination** even
/// if Smart's current decision would route the press locally.
///
/// Without this, Smart flipping mid-keypress (user moved the
/// other-side mouse between Shift-down and Shift-up, etc.)
/// strands the modifier on the peer side: Linux thinks Shift is
/// still held → every subsequent letter from this side comes
/// out uppercase even though the user never turned on Caps Lock.
///
/// Sized for both Win PS/2 set-1 scancodes (0..255) and Linux
/// `KEY_*` codes (0..767). 1024 is a small constant array that
/// const-initialises and avoids any HashSet allocation in the
/// hot path.
static HELD_FORWARDED: parking_lot::Mutex<[bool; 1024]> = parking_lot::const_mutex([false; 1024]);

/// Was the down event for `code` forwarded? If so, the matching
/// up MUST also be forwarded so the peer's modifier state stays
/// consistent. Atomic-light: a single Mutex lock per keystroke
/// is well within budget on the WH_KEYBOARD_LL hot path.
/// Wipe held-state on session boundaries so the new session
/// doesn't think a key is still in flight from the old one.
fn clear_held_forwarded() {
    let mut g = HELD_FORWARDED.lock();
    for h in g.iter_mut() {
        *h = false;
    }
}

/// Combined held-state-aware forwarding decision. Capture sites
/// call this instead of [`should_forward_keys`] directly:
///
///   * If the key is already held forwarded → forward (continuation
///     of an ongoing press; routes auto-repeats and the eventual
///     release to the same destination as the original down).
///   * Else if `down`, evaluate Smart and remember the decision.
///   * Else (up of an un-tracked key) → don't forward; the
///     local OS gets it as usual.
pub fn route_keystroke(code: u16, down: bool, cursor_in_remote: bool) -> bool {
    let mut held = HELD_FORWARDED.lock();
    let i = code as usize;
    let was_held = i < 1024 && held[i];
    let forward = if was_held {
        // Continuation — must follow through.
        if !down && i < 1024 {
            held[i] = false;
        }
        true
    } else if down {
        let decide = should_forward_keys(cursor_in_remote);
        if decide && i < 1024 {
            held[i] = true;
        }
        decide
    } else {
        // Up of a key whose down was never forwarded.
        false
    };
    drop(held);
    forward
}

/// Same held-state-aware logic as [`route_keystroke`] but for
/// mouse buttons. Without it, the user clicking on the peer
/// (cursor in REMOTE), then crossing the cursor back BEFORE
/// releasing, leaves the button stuck-down on the peer's
/// uinput / enigo device — every subsequent operation on that
/// machine acts as if a button is held (text gets selected
/// while moving the mouse, drag-drop fires, etc.).
///
/// Routing rule for buttons is simpler than for keys: there's
/// no "Smart" — buttons always follow the cursor. So a fresh
/// DOWN forwards iff `cursor_in_remote`.
static MOUSE_BTNS_FORWARDED: parking_lot::Mutex<[bool; 8]> = parking_lot::const_mutex([false; 8]);

fn btn_index(btn: Button) -> usize {
    match btn {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::X1 => 3,
        Button::X2 => 4,
    }
}

pub fn route_mouse_button(btn: Button, down: bool, cursor_in_remote: bool) -> bool {
    let i = btn_index(btn);
    let mut held = MOUSE_BTNS_FORWARDED.lock();
    let was_held = held[i];
    let forward = if was_held {
        if !down {
            held[i] = false;
        }
        true
    } else if down {
        if cursor_in_remote {
            held[i] = true;
        }
        cursor_in_remote
    } else {
        false
    };
    drop(held);
    forward
}

fn clear_held_buttons() {
    let mut g = MOUSE_BTNS_FORWARDED.lock();
    for h in g.iter_mut() {
        *h = false;
    }
}

/// Snapshot the state that capture routing currently intends the peer to
/// hold. The two small locks are never held together, avoiding lock-order
/// coupling with the keyboard and mouse hook paths.
pub fn forwarded_input_state() -> ForwardedInputState {
    let mut state = ForwardedInputState::default();
    {
        let held = HELD_FORWARDED.lock();
        for (code, down) in held.iter().copied().enumerate() {
            if down {
                state.set_key(code as u16, true);
            }
        }
    }
    {
        let held = MOUSE_BTNS_FORWARDED.lock();
        for (index, down) in held.iter().copied().enumerate().take(5) {
            if down {
                state.mouse_buttons |= 1u8 << index;
            }
        }
    }
    state
}

/// Should this side forward an incoming keystroke to the peer?
/// `cursor_in_remote` is the capture-side mouse-mode flag.
///
/// Smart rule — "the keyboard follows the screen whose mouse was
/// active most recently":
///   1. Peer cursor crossed onto this screen → local. Hard override:
///      the local physical keyboard must not be captured and reflected
///      back to the peer while both keyboards are targeting this screen.
///   2. Cursor crossed onto the peer screen → peer. Hard override:
///      the physical local mouse keeps emitting HW motion while it
///      drives the peer cursor, so a raw motion race would wrongly
///      say local.
///   3. Both sides stale (idle past `ACTIVITY_FRESH_MS`, or never
///      active) → keep the sticky decision (defaults to local).
///   4. Otherwise the more-recently-active side wins.
///
/// "Active" = a deliberate drag (see `bump_local_mouse_activity`) or
/// any click. A brief nudge does NOT count, so it can't steal the
/// keyboard from a peer you are actively using — that debounce, not
/// a timing margin, is what keeps the routing stable.
///
/// Step 2 must check *staleness*, not just "never active": the peer's
/// age is capped at 60 s on the wire while the local age is uncapped,
/// so without it a minute of mutual idle would let the capped peer age
/// "win" the race and silently flip the keyboard across.
pub fn should_forward_keys(cursor_in_remote: bool) -> bool {
    // Game Drive: while we are driving the peer's game, every key goes to
    // the peer regardless of cursor position or Smart routing.
    if is_game_driving() {
        return true;
    }
    /// Once both sides have been idle longer than this, stop racing
    /// ages and hold the last decision. Comfortably below the 60 s
    /// wire cap so a capped peer age always reads as stale, and above
    /// any normal pause between mouse moves while actively working.
    const ACTIVITY_FRESH_MS: u64 = 10_000;

    match keyboard_target() {
        KeyboardTarget::Smart => {
            let to_peer = if cursor_in_remote {
                // The physical cursor owned by this machine has already
                // crossed to the peer. Treat that direct capture-side fact as
                // authoritative even if a delayed TakeControl temporarily
                // leaves PEER_IN_REMOTE set during collision resolution.
                // Giving the stale inbound flag priority here leaked the
                // first physical key (commonly K) into the source PC.
                true
            } else if peer_in_remote() {
                // The peer has crossed onto this screen, so this screen is
                // the active keyboard destination. Keep this machine's
                // physical keyboard local; otherwise the peer-activity
                // heuristic reflects it back to the peer and consumes it
                // from the window the user is looking at.
                false
            } else {
                let la = local_activity_age();
                let pa = peer_activity_age();
                if la >= ACTIVITY_FRESH_MS && pa >= ACTIVITY_FRESH_MS {
                    // Both idle (or never active) — hold the last call.
                    LAST_SMART_TO_PEER.load(Ordering::Relaxed)
                } else {
                    // Smaller age = more recent. A tie falls to local.
                    pa < la
                }
            };
            LAST_SMART_TO_PEER.store(to_peer, Ordering::Relaxed);
            to_peer
        }
        KeyboardTarget::Auto => cursor_in_remote,
        KeyboardTarget::ForcePeer => true,
        KeyboardTarget::ForceLocal => false,
    }
}

// ---------------------------------------------------------------------------
// Mouse-activity tracking for Smart keyboard routing.
//
// Local activity is recorded as wall-clock millisecond timestamps:
// the most recent deliberate hardware mouse drag (`LOCAL_MOUSE_AT`)
// and the most recent local mouse-button down (`LOCAL_CLICK_AT`).
// Peer activity lives in a single `PEER_ACTIVITY_AT`, back-dated onto
// our own clock from the *age* the daemon's periodic ActivityBeacon
// ControlMsg reports (no clock-sync needed). Smart's heuristic reads
// recency on both sides via `local_activity_age` / `peer_activity_age`.
// ---------------------------------------------------------------------------

/// Wall-clock ms of the most recent *deliberate* local mouse drag
/// (see the debounce in `bump_local_mouse_activity`). 0 = never.
static LOCAL_MOUSE_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// Debounce state for distinguishing a deliberate drag from an
// accidental nudge. A "run" of motion must last past `DEBOUNCE_MS`
// before it counts as routing activity — otherwise a single twitch
// of the resting hand would yank the keyboard back from a peer the
// user is actively driving.
static MOTION_RUN_START: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LAST_MOTION_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A motion run must be sustained at least this long to count as a
/// deliberate drag. Shorter bursts (flicks / resting-hand nudges)
/// are ignored for keyboard routing.
const DEBOUNCE_MS: u64 = 120;
/// A gap longer than this ends the current run; the next motion
/// begins a fresh one, so two separate nudges don't accumulate into
/// a false "drag".
const IDLE_RESET_MS: u64 = 250;

pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Called from the OS-specific capture path on each genuine hardware
/// mouse motion. Does NOT fire on injected events because our hooks
/// only see real HW input (Win: `SetCursorPos` doesn't trigger
/// WH_MOUSE_LL; Linux: our virtual uinput device is excluded from the
/// evdev grab).
///
/// Only a *sustained* run (≥ `DEBOUNCE_MS`) advances the routing
/// activity clock `LOCAL_MOUSE_AT`; a brief nudge is dropped. Cheap:
/// a couple of atomic ops + one SystemTime read.
///
/// Edge: a deliberate drag whose motion events are spaced wider than
/// `IDLE_RESET_MS` (250 ms) restarts the run on every event, so it never
/// accumulates past `DEBOUNCE_MS` and never advances the routing clock.
/// Acceptable in practice — real HW motion fires far faster than 4
/// events/sec, and clicks always count regardless.
pub(crate) fn bump_local_mouse_activity() {
    let now = now_ms();
    let last = LAST_MOTION_AT.swap(now, Ordering::Relaxed);
    if last == 0 || now.saturating_sub(last) > IDLE_RESET_MS {
        // First motion, or a fresh run after an idle gap.
        MOTION_RUN_START.store(now, Ordering::Relaxed);
    }
    let run_start = MOTION_RUN_START.load(Ordering::Relaxed);
    if now.saturating_sub(run_start) >= DEBOUNCE_MS {
        LOCAL_MOUSE_AT.store(now, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Activity recency — the single signal Smart routing races on.
//
// A click always counts (it focuses a window — the strongest
// "user attention" signal); a mouse drag counts once it's
// deliberate (debounced above). Each side tracks its own local
// stamps; the periodic ActivityBeacon carries the *age* of the
// peer's most recent activity so the receiver can back-date a
// single `PEER_ACTIVITY_AT` and compare apples to apples on its
// own clock, with no clock-sync.
// ---------------------------------------------------------------------------

/// Wall-clock ms of the most recent local mouse-button down. 0 = never.
static LOCAL_CLICK_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Most recent peer activity, expressed on OUR clock (back-dated from
/// the age the beacon reports). 0 = never this session.
static PEER_ACTIVITY_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn bump_local_click() {
    LOCAL_CLICK_AT.store(now_ms(), Ordering::Relaxed);
}

/// Called by the daemon when an ActivityBeacon arrives carrying the
/// age (ms) of the peer's most recent activity (deliberate drag OR
/// click). We back-date our own clock by that age so the recency
/// race stays accurate to network latency, no clock-sync needed.
pub fn note_peer_activity(age_ms: u32) {
    let stamp = now_ms().saturating_sub(age_ms as u64);
    PEER_ACTIVITY_AT.store(stamp, Ordering::Relaxed);
}

/// Raw ms of the most recent local activity (drag or click), 0 if none.
fn local_activity_at() -> u64 {
    LOCAL_MOUSE_AT
        .load(Ordering::Relaxed)
        .max(LOCAL_CLICK_AT.load(Ordering::Relaxed))
}

/// Age (ms) of the most recent local activity; `u64::MAX` = never.
fn local_activity_age() -> u64 {
    let at = local_activity_at();
    if at == 0 {
        u64::MAX
    } else {
        now_ms().saturating_sub(at)
    }
}

/// Age (ms) of the most recent peer activity; `u64::MAX` = never.
fn peer_activity_age() -> u64 {
    let at = PEER_ACTIVITY_AT.load(Ordering::Relaxed);
    if at == 0 {
        u64::MAX
    } else {
        now_ms().saturating_sub(at)
    }
}

/// ms since the local user's most recent activity (drag or click),
/// capped at 60 000. `None` if neither has happened this session.
/// Feeds the daemon's ActivityBeacon sender.
pub fn local_input_age_ms() -> Option<u32> {
    let at = local_activity_at();
    if at == 0 {
        return None;
    }
    Some(now_ms().saturating_sub(at).min(60_000) as u32)
}

/// Apply the mouse sensitivity multiplier with sub-pixel residue
/// retained between calls — without this, sensitivity < 1.0 throws
/// away every other delta when raw HW motion is small (a 1-pixel
/// motion × 0.6 rounds to 0 and the cursor stalls). Each call
/// returns the integer to forward; the caller keeps the residue
/// across the session.
pub fn scale_delta(d: i32, residue: &mut f32) -> i32 {
    let scaled = d as f32 * mouse_sensitivity() + *residue;
    let whole = scaled.trunc();
    *residue = scaled - whole;
    whole as i32
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
    if v {
        // A peer crossing onto this screen makes this screen the concrete
        // Smart-keyboard destination. Clear any peer-directed decision left
        // over from earlier activity so releasing control cannot resurrect
        // a stale route before either mouse moves again.
        LAST_SMART_TO_PEER.store(false, Ordering::Relaxed);
    }
}

/// Immediately revoke peer mouse ownership when genuine local hardware
/// motion is detected. The control-plane `RequestPeerExit` still tells the
/// peer to stop forwarding, but the local data plane must not wait for that
/// round trip: otherwise queued `SendInput` motion fights the user's
/// touchpad/mouse for the cursor until the acknowledgement arrives.
///
/// Returns `true` only for the first caller that performs the takeover so a
/// burst of low-level-hook callbacks emits one control request, not one per
/// hardware sample.
pub(crate) fn reclaim_from_peer_hardware() -> bool {
    PEER_IN_REMOTE
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

pub(crate) fn fire_remote_event(ev: RemoteEvent) {
    if let Some(tx) = REMOTE_EVT_TX.lock().as_ref() {
        let _ = tx.send(ev);
    }
}

/// Toggle Game Drive from this machine. When turning ON we become `Driving`
/// and signal the peer to `Receiving`; when turning OFF we reset and signal
/// stop. No-op semantics if there is no peer are handled by the daemon
/// (the RemoteEvent simply isn't delivered).
pub fn toggle_game_drive() {
    // Either role (Driving or Receiving) turns OFF on toggle so the person at
    // EITHER machine — including the one running the anti-cheat game — can
    // abort the session. Firing Stop makes the peer exit too.
    if is_game_driving() || is_game_receiving() {
        set_game_drive(GameDrive::Off);
        fire_remote_event(RemoteEvent::GameDriveStop);
    } else {
        // Start from a clean LOCAL state: if the cursor had crossed into
        // REMOTE, the REMOTE motion branch would run virt_x + the anchor
        // warp and re-introduce the in-game camera fling. Force LOCAL first.
        force_local_exit_remote();
        set_game_drive(GameDrive::Driving);
        fire_remote_event(RemoteEvent::GameDriveStart);
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
        windows::on_peer_take_control();
        let _ = inject;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;

    /// All routing state lives in process-global statics, so the tests
    /// must run one at a time. Each test takes this lock and resets the
    /// world first.
    static TEST_LOCK: parking_lot::Mutex<()> = parking_lot::const_mutex(());

    fn reset() {
        LOCAL_MOUSE_AT.store(0, Relaxed);
        LOCAL_CLICK_AT.store(0, Relaxed);
        PEER_ACTIVITY_AT.store(0, Relaxed);
        MOTION_RUN_START.store(0, Relaxed);
        LAST_MOTION_AT.store(0, Relaxed);
        LAST_SMART_TO_PEER.store(false, Relaxed);
        set_peer_in_remote(false);
        set_keyboard_target(KeyboardTarget::Smart);
        set_game_drive(GameDrive::Off);
        clear_held_forwarded();
        clear_held_buttons();
    }

    #[test]
    fn touchpad_frame_rejects_malformed_contact_counts() {
        let contact = TouchpadContact { id: 1, x: 2, y: 3 };
        let frame = TouchpadEvent::frame(9, &[contact], false).expect("valid contact frame");
        assert_eq!(frame.contacts(), Some(&[contact][..]));
        assert!(TouchpadEvent::frame(9, &[], false).is_none());
        assert!(TouchpadEvent::frame(9, &[contact], true).is_none());

        let malformed = TouchpadEvent::Frame {
            stream_id: 9,
            count: (MAX_TOUCHPAD_CONTACTS + 1) as u8,
            contacts: [TouchpadContact::default(); MAX_TOUCHPAD_CONTACTS],
            terminal: false,
        };
        assert!(malformed.contacts().is_none());
        assert_eq!(TouchpadEvent::terminal(9).contacts(), Some(&[][..]));
    }

    #[test]
    fn touchpad_capability_falls_back_without_both_frame_sides() {
        assert!(native_touchpad_pair_available(
            TOUCHPAD_CAP_FRAME_CAPTURE,
            TOUCHPAD_CAP_FRAME_INJECT
        ));
        assert!(!native_touchpad_pair_available(
            TOUCHPAD_CAP_FRAME_CAPTURE,
            0
        ));
        assert!(!native_touchpad_pair_available(
            0,
            TOUCHPAD_CAP_FRAME_INJECT
        ));
    }

    #[test]
    fn game_drive_state_roundtrips() {
        let _g = TEST_LOCK.lock();
        set_game_drive(GameDrive::Off);
        assert_eq!(game_drive(), GameDrive::Off);
        assert!(!is_game_driving());
        assert!(!is_game_receiving());

        set_game_drive(GameDrive::Driving);
        assert_eq!(game_drive(), GameDrive::Driving);
        assert!(is_game_driving());
        assert!(!is_game_receiving());

        set_game_drive(GameDrive::Receiving);
        assert!(!is_game_driving());
        assert!(is_game_receiving());

        set_game_drive(GameDrive::Off); // reset for other tests
    }

    #[test]
    fn toggle_game_drive_receiver_can_stop() {
        let _g = TEST_LOCK.lock();
        reset();
        // The machine running the game is `Receiving`. Toggling Game Drive
        // there (Stop button / Ctrl+Alt+G) must turn it OFF — not flip it to
        // Driving (role-swap), so the person at the anti-cheat game can abort.
        set_game_drive(GameDrive::Receiving);
        toggle_game_drive();
        assert_eq!(game_drive(), GameDrive::Off);
    }

    #[test]
    fn toggle_game_drive_flips_state() {
        let _g = TEST_LOCK.lock();
        reset();
        set_game_drive(GameDrive::Off);
        toggle_game_drive();
        assert!(is_game_driving(), "toggle from Off enters Driving");
        toggle_game_drive();
        assert_eq!(
            game_drive(),
            GameDrive::Off,
            "toggle from Driving returns Off"
        );
    }

    #[test]
    fn game_drive_remote_events_exist() {
        // Compile-time proof the toggle signal variants exist and are distinct.
        let a = RemoteEvent::GameDriveStart;
        let b = RemoteEvent::GameDriveStop;
        assert_ne!(format!("{a:?}"), format!("{b:?}"));
    }

    #[test]
    fn game_driving_forces_keys_to_peer() {
        let _g = TEST_LOCK.lock();
        reset();
        set_keyboard_target(KeyboardTarget::ForceLocal); // even pinned-local…
        set_game_drive(GameDrive::Driving);
        assert!(
            should_forward_keys(false),
            "Driving must forward keys to peer"
        );
        set_game_drive(GameDrive::Off);
        assert!(!should_forward_keys(false), "Off + ForceLocal stays local");
    }

    #[test]
    fn hz_maps_to_flush_micros() {
        assert_eq!(hz_to_flush_us(1000), 1_000);
        assert_eq!(hz_to_flush_us(500), 2_000);
        assert_eq!(hz_to_flush_us(125), 8_000);
        // clamp guards against div-by-zero / absurd values
        assert_eq!(hz_to_flush_us(0), hz_to_flush_us(60));
        assert_eq!(hz_to_flush_us(99999), hz_to_flush_us(1000));
    }

    #[test]
    fn dispatch_clamps_oversized_mouse_move() {
        use std::sync::Mutex;
        // Mock injector that records the (dx, dy) actually handed to the
        // OS — lets us assert what `dispatch` forwards after clamping.
        struct Rec(Mutex<(i32, i32)>);
        impl InputInject for Rec {
            fn mouse_move_rel(&self, dx: i32, dy: i32) -> anyhow::Result<()> {
                *self.0.lock().unwrap() = (dx, dy);
                Ok(())
            }
            fn mouse_button(&self, _: Button, _: bool) -> anyhow::Result<()> {
                Ok(())
            }
            fn key(&self, _: KeyCode, _: bool) -> anyhow::Result<()> {
                Ok(())
            }
            fn scroll(&self, _: f32, _: f32) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let r = Rec(Mutex::new((0, 0)));

        // The proven fling: a single ~screen-half delta generated at the
        // peer-take-control warp (see log: "warped cursor" then
        // MouseMove { dx: -969, dy: 453 }). It MUST be clamped so one bad
        // event can't teleport a mouse-look game's camera.
        r.dispatch(InputEvent::MouseMove { dx: -969, dy: 453 })
            .unwrap();
        assert_eq!(
            *r.0.lock().unwrap(),
            (-MAX_INJECT_DELTA_PX, MAX_INJECT_DELTA_PX),
            "oversized handover delta must be clamped"
        );

        // Normal in-game motion (observed peak ~64 px/event) passes through
        // untouched — the clamp must not throttle real aiming.
        r.dispatch(InputEvent::MouseMove { dx: 64, dy: -18 })
            .unwrap();
        assert_eq!(
            *r.0.lock().unwrap(),
            (64, -18),
            "normal motion must be unaffected"
        );
    }

    #[test]
    fn local_activity_newer_routes_local() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        PEER_ACTIVITY_AT.store(now - 1000, Relaxed);
        LOCAL_MOUSE_AT.store(now - 50, Relaxed);
        assert!(!should_forward_keys(false));
    }

    #[test]
    fn peer_activity_newer_routes_peer() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        LOCAL_MOUSE_AT.store(now - 1000, Relaxed);
        PEER_ACTIVITY_AT.store(now - 50, Relaxed);
        assert!(should_forward_keys(false));
    }

    #[test]
    fn peer_driving_local_screen_keeps_laptop_keyboard_local() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        LOCAL_MOUSE_AT.store(now - 1000, Relaxed);
        PEER_ACTIVITY_AT.store(now - 50, Relaxed);
        set_peer_in_remote(true);

        assert!(
            !route_keystroke(30, true, false),
            "when the peer cursor is on this screen, this machine's physical keyboard must remain local"
        );
        assert!(
            !route_keystroke(30, false, false),
            "the matching local key-up must also remain local"
        );
    }

    #[test]
    fn peer_control_handoff_clears_stale_smart_peer_route() {
        let _g = TEST_LOCK.lock();
        reset();
        LAST_SMART_TO_PEER.store(true, Relaxed);

        set_peer_in_remote(true);
        set_peer_in_remote(false);

        assert!(
            !should_forward_keys(false),
            "a completed peer-control handoff must not resurrect the previous peer-directed sticky route"
        );
    }

    #[test]
    fn click_counts_as_activity() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        PEER_ACTIVITY_AT.store(now - 1000, Relaxed);
        // Local click is more recent than peer → keyboard stays local.
        LOCAL_CLICK_AT.store(now - 20, Relaxed);
        assert!(!should_forward_keys(false));
    }

    #[test]
    fn cursor_in_remote_overrides_to_peer() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        // Local mouse just moved, but the cursor has crossed onto the
        // peer screen — that wins.
        LOCAL_MOUSE_AT.store(now - 5, Relaxed);
        assert!(should_forward_keys(true));
    }

    #[test]
    fn physical_pc_key_follows_its_remote_cursor_during_stale_inbound_overlap() {
        let _g = TEST_LOCK.lock();
        reset();

        // A delayed TakeControl can briefly leave both ownership flags set
        // while collision resolution crosses the TCP control link. The real
        // PC cursor is already on the Laptop, so its physical K key must not
        // leak into the PC's foreground window during that overlap.
        set_peer_in_remote(true);
        assert!(
            route_keystroke(37, true, true),
            "physical PC K-down must follow the PC cursor to the Laptop"
        );
        assert!(
            route_keystroke(37, false, true),
            "physical PC K-up must follow the same routed press"
        );
    }

    #[test]
    fn both_idle_keeps_sticky_default_local() {
        let _g = TEST_LOCK.lock();
        reset();
        // Never active this session, sticky defaults to local.
        assert!(!should_forward_keys(false));
        // Sticky honours a prior peer decision.
        LAST_SMART_TO_PEER.store(true, Relaxed);
        assert!(should_forward_keys(false));
    }

    #[test]
    fn both_stale_keeps_sticky_not_capped_peer() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        // Last decision was local. The peer's reported age is capped
        // at 60 s on the wire, so PEER_ACTIVITY_AT tops out ~60 s old
        // while the uncapped local age keeps climbing. Without a
        // "both stale → sticky" guard the race wrongly picks peer
        // (60_000 < 70_000) after a minute of mutual idle.
        LAST_SMART_TO_PEER.store(false, Relaxed);
        LOCAL_MOUSE_AT.store(now - 70_000, Relaxed);
        PEER_ACTIVITY_AT.store(now - 60_000, Relaxed);
        assert!(!should_forward_keys(false));

        // And it honours a prior peer decision the same way.
        reset();
        LAST_SMART_TO_PEER.store(true, Relaxed);
        LOCAL_MOUSE_AT.store(now - 70_000, Relaxed);
        PEER_ACTIVITY_AT.store(now - 60_000, Relaxed);
        assert!(should_forward_keys(false));
    }

    #[test]
    fn manual_targets_ignore_activity() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        LOCAL_MOUSE_AT.store(now - 5, Relaxed); // local most recent

        set_keyboard_target(KeyboardTarget::ForcePeer);
        assert!(should_forward_keys(false));

        set_keyboard_target(KeyboardTarget::ForceLocal);
        assert!(!should_forward_keys(true));

        set_keyboard_target(KeyboardTarget::Auto);
        assert!(should_forward_keys(true));
        assert!(!should_forward_keys(false));
    }

    #[test]
    fn held_key_release_follows_the_press() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        // Cursor on peer → the down is forwarded and remembered.
        LOCAL_MOUSE_AT.store(now - 5, Relaxed);
        assert!(route_keystroke(30, true, true)); // KEY 'a' down → peer
        // Decision flips back to local (cursor home), but the up of the
        // already-forwarded key must still go to the peer.
        assert!(route_keystroke(30, false, false));
        // A fresh key now follows the current (local) decision.
        assert!(!route_keystroke(31, true, false));
    }

    #[test]
    fn nudge_does_not_claim_keyboard() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        // A motion run that just started (< DEBOUNCE_MS) is a nudge.
        MOTION_RUN_START.store(now, Relaxed);
        LAST_MOTION_AT.store(now, Relaxed);
        bump_local_mouse_activity();
        assert_eq!(
            LOCAL_MOUSE_AT.load(Relaxed),
            0,
            "a brief nudge must not advance the routing activity clock"
        );
    }

    #[test]
    fn sustained_drag_claims_keyboard() {
        let _g = TEST_LOCK.lock();
        reset();
        let now = now_ms();
        // A run that has lasted well past DEBOUNCE_MS, still ongoing.
        MOTION_RUN_START.store(now - 200, Relaxed);
        LAST_MOTION_AT.store(now - 10, Relaxed);
        bump_local_mouse_activity();
        assert!(
            LOCAL_MOUSE_AT.load(Relaxed) > 0,
            "a sustained drag must advance the routing activity clock"
        );
    }

    #[test]
    fn reset_zeroes_peer_activity() {
        let _g = TEST_LOCK.lock();
        reset();
        PEER_ACTIVITY_AT.store(now_ms(), Relaxed);
        reset_smart_decision();
        assert_eq!(
            PEER_ACTIVITY_AT.load(Relaxed),
            0,
            "reset_smart_decision must clear stale peer-activity timing"
        );
    }

    #[test]
    fn held_state_snapshot_reconciles_lost_edges() {
        let mut injected = ForwardedInputState::default();
        let mut desired = ForwardedInputState::default();
        desired.apply_event(InputEvent::Key {
            code: KeyCode(30),
            down: true,
        });
        desired.apply_event(InputEvent::MouseButton {
            btn: Button::Left,
            down: true,
        });

        let presses = injected.reconciliation_events(&desired);
        assert_eq!(presses.len(), 2);
        for event in presses {
            injected.apply_event(event);
        }
        assert_eq!(injected, desired);

        let released = ForwardedInputState::default();
        let releases = injected.reconciliation_events(&released);
        assert_eq!(releases.len(), 2);
        assert!(releases.iter().all(|event| matches!(
            event,
            InputEvent::Key { down: false, .. } | InputEvent::MouseButton { down: false, .. }
        )));
    }

    #[test]
    fn forwarded_snapshot_matches_routing_tracker() {
        let _g = TEST_LOCK.lock();
        reset();
        assert!(route_keystroke(42, true, true));
        assert!(route_mouse_button(Button::Right, true, true));

        let snapshot = forwarded_input_state();
        let events = ForwardedInputState::default().reconciliation_events(&snapshot);
        assert!(events.contains(&InputEvent::Key {
            code: KeyCode(42),
            down: true,
        }));
        assert!(events.contains(&InputEvent::MouseButton {
            btn: Button::Right,
            down: true,
        }));

        reset_smart_decision();
    }

    #[test]
    fn forwarded_snapshot_preserves_normalized_extended_keycode() {
        let _g = TEST_LOCK.lock();
        reset();
        // KEY_LEFTMETA is the Linux evdev code emitted for the Windows logo
        // key. It must not regress to the raw Windows 0x5B scan code in a
        // held-state snapshot.
        assert!(route_keystroke(125, true, true));

        let snapshot = forwarded_input_state();
        let events = ForwardedInputState::default().reconciliation_events(&snapshot);
        assert!(events.contains(&InputEvent::Key {
            code: KeyCode(125),
            down: true,
        }));

        reset_smart_decision();
    }

    #[test]
    fn local_hardware_takeover_is_immediate_and_one_shot() {
        let _g = TEST_LOCK.lock();
        set_peer_in_remote(true);
        assert!(
            reclaim_from_peer_hardware(),
            "first local hardware motion must revoke remote ownership"
        );
        assert!(!peer_in_remote(), "remote motion must be gated immediately");
        assert!(
            !reclaim_from_peer_hardware(),
            "one hardware burst must emit only one peer-exit request"
        );
    }
}
