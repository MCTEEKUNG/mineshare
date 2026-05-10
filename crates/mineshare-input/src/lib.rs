//! Cross-platform input capture and injection.
//!
//! Normalized event types are designed to round-trip cleanly between Linux
//! (evdev) and Windows (Raw Input / SendInput). Key codes use the Linux
//! `KEY_*` numbering, which matches PS/2 set-1 scan codes for the common
//! keys — Windows side translates virtual keys to scan codes on the way in
//! and back to virtual keys on the way out.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { btn: Button, down: bool },
    Key { code: KeyCode, down: bool },
    Scroll { dx: f32, dy: f32 },
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
// Phase 2: Auto-focus click on peer take-control (opt-in).
//
// On GNOME-Wayland (Ubuntu's default desktop), keyboard focus is
// click-to-focus by default — moving the mouse cursor over a
// window does NOT make subsequent typing land on it. So when the
// peer drives the local cursor across, our uinput keyboard does
// emit keys, but no window has focus and the keys vanish.
//
// When this flag is true, the local side fires a synthetic left-
// click in place right after the cursor-warp slam in
// `on_peer_take_control`. The click activates whatever window is
// under the cursor and gives it keyboard focus. Cost: it really
// is a click — buttons / menu items / drag handles under the
// cursor get hit. Default OFF; users on focus-follows-mouse
// compositors don't need this and shouldn't enable it.
// ---------------------------------------------------------------------------

static AUTO_FOCUS_ON_TAKE: AtomicBool = AtomicBool::new(false);
/// Wall-clock ms of the last auto-focus click. Used to
/// rate-limit so rapid cursor crossings (e.g. user wiggling the
/// mouse across the edge while reading something) don't fire a
/// click on every single cross — that's the "phantom spacebar
/// every time I move between screens" bug, since the click on
/// any focusable element behaves identically to pressing Space
/// (play/pause, button activation, etc.).
static LAST_AUTO_CLICK_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const AUTO_CLICK_COOLDOWN_MS: u64 = 30_000;

pub fn auto_focus_on_take_control() -> bool {
    if !AUTO_FOCUS_ON_TAKE.load(Ordering::Relaxed) {
        return false;
    }
    // Rate-limited consume: returns true at most once every
    // 30 seconds even when the underlying setting is on, so the
    // click only fires when the user is actually returning to
    // the peer after a quiet period — exactly the "I haven't
    // been here in a while, please grab focus" scenario.
    let now = now_ms();
    let prev = LAST_AUTO_CLICK_AT.load(Ordering::Relaxed);
    if prev != 0 && now.saturating_sub(prev) < AUTO_CLICK_COOLDOWN_MS {
        tracing::debug!(
            since_last_ms = now.saturating_sub(prev),
            "auto-focus click suppressed by cooldown"
        );
        return false;
    }
    LAST_AUTO_CLICK_AT.store(now, Ordering::Relaxed);
    tracing::info!("auto-focus click WILL fire on take-control");
    true
}

pub fn set_auto_focus_on_take_control(v: bool) {
    AUTO_FOCUS_ON_TAKE.store(v, Ordering::Relaxed);
    // Reset cooldown when the user toggles, so turning it on
    // doesn't have to wait 30 s for the first click.
    LAST_AUTO_CLICK_AT.store(0, Ordering::Relaxed);
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
    if n < 10 || n % 50 == 0 || code == 57 || code == 28 {
        tracing::info!(scancode = code, down, total = n + 1, "key forwarded to peer");
    }
}

pub(crate) fn note_key_forwarded() {
    KEYS_FORWARDED.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_key_injected_with_code(code: u16, down: bool) {
    let n = KEYS_INJECTED.fetch_add(1, Ordering::Relaxed);
    if n < 10 || n % 50 == 0 || code == 57 || code == 28 {
        tracing::info!(scancode = code, down, total = n + 1, "key injected from peer");
    }
}

pub(crate) fn note_key_injected() {
    KEYS_INJECTED.fetch_add(1, Ordering::Relaxed);
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
    clear_held_forwarded();
    clear_held_buttons();
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
static HELD_FORWARDED: parking_lot::Mutex<[bool; 1024]> =
    parking_lot::const_mutex([false; 1024]);

/// Was the down event for `code` forwarded? If so, the matching
/// up MUST also be forwarded so the peer's modifier state stays
/// consistent. Atomic-light: a single Mutex lock per keystroke
/// is well within budget on the WH_KEYBOARD_LL hot path.
pub(crate) fn is_held_forwarded(code: u16) -> bool {
    let i = code as usize;
    if i >= 1024 {
        return false;
    }
    HELD_FORWARDED.lock()[i]
}

pub(crate) fn set_held_forwarded(code: u16, held: bool) {
    let i = code as usize;
    if i >= 1024 {
        return;
    }
    HELD_FORWARDED.lock()[i] = held;
}

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
pub fn route_keystroke(
    code: u16,
    down: bool,
    cursor_in_remote: bool,
) -> bool {
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
static MOUSE_BTNS_FORWARDED: parking_lot::Mutex<[bool; 8]> =
    parking_lot::const_mutex([false; 8]);

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

/// Should this side forward an incoming keystroke to the peer?
/// `cursor_in_remote` is the capture-side mouse-mode flag.
///
/// Smart-mode decision tree (priority order):
///   1. Cursor crossed to peer       → peer  (legacy Auto behaviour)
///   2. Recent click on either side  → most-recent-click wins  (focus signal!)
///   3. Local mouse moved < 1.5 s    → local (immediate claim-back)
///   4. Peer mouse moved < 2.5 s     → peer  (peer is being used)
///   5. Both mice idle               → keep the last decision (sticky)
///
/// Step 2 is the focus-based upgrade: a click is a stronger
/// "user attention" signal than motion, because it focuses a
/// window. As long as either side clicked within the last 30 s,
/// we follow whichever click is more recent and ignore motion.
///
/// Step 5 is the sticky fix for the "user types for 30 s without
/// touching mouse → Smart times out and routes back" gotcha.
pub fn should_forward_keys(cursor_in_remote: bool) -> bool {
    /// Window where a click is treated as a fresh focus signal.
    /// Long enough to cover sustained typing into the just-
    /// clicked field; short enough that an old click from
    /// minutes ago doesn't keep hijacking the keyboard.
    const CLICK_FRESH_MS: u64 = 30_000;

    match keyboard_target() {
        KeyboardTarget::Smart => {
            let to_peer = if cursor_in_remote {
                true
            } else {
                let lc = local_click_age();
                let pc = peer_click_age();
                if lc < CLICK_FRESH_MS || pc < CLICK_FRESH_MS {
                    // Either side has a fresh click — most-recent
                    // wins. (Lower age = more recent.)
                    pc < lc
                } else if local_mouse_active_within(1500) {
                    false
                } else if peer_mouse_active_within(2500) {
                    true
                } else {
                    LAST_SMART_TO_PEER.load(Ordering::Relaxed)
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
// Each side records the wall-clock millisecond timestamp of its
// most recent local hardware mouse motion (`LOCAL_MOUSE_AT`),
// and what the peer reported via the daemon's periodic
// ActivityBeacon ControlMsg (`PEER_MOUSE_AT` — wall-clock as
// observed *here* when the beacon arrived). Smart's heuristic
// reads both via `local_mouse_active_within` /
// `peer_mouse_active_within`.
// ---------------------------------------------------------------------------

static LOCAL_MOUSE_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PEER_MOUSE_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Called from the OS-specific capture path on each genuine
/// hardware mouse motion. Cheap (one atomic store + one
/// SystemTime call). Does NOT fire on injected events because
/// our hooks only see real HW input (Win: `SetCursorPos` doesn't
/// trigger WH_MOUSE_LL; Linux: our virtual uinput device is
/// excluded from the evdev grab).
pub(crate) fn bump_local_mouse_activity() {
    LOCAL_MOUSE_AT.store(now_ms(), Ordering::Relaxed);
}

/// Called by the daemon when an ActivityBeacon arrives from the
/// peer reporting their mouse was just active. We stamp our own
/// clock (not the peer's) so age comparisons stay sane without
/// any clock-sync.
pub fn note_peer_mouse_active() {
    PEER_MOUSE_AT.store(now_ms(), Ordering::Relaxed);
}

pub fn local_mouse_active_within(ms: u64) -> bool {
    let a = LOCAL_MOUSE_AT.load(Ordering::Relaxed);
    a > 0 && now_ms().saturating_sub(a) < ms
}

pub fn peer_mouse_active_within(ms: u64) -> bool {
    let a = PEER_MOUSE_AT.load(Ordering::Relaxed);
    a > 0 && now_ms().saturating_sub(a) < ms
}

// ---------------------------------------------------------------------------
// Click-based focus signal — stronger than motion for Smart routing.
//
// When the user clicks on a window, that window takes keyboard
// focus. A click is therefore the strongest available "where is
// the user's attention" signal short of querying the OS for the
// focused window class (which is hard cross-platform).
//
// Each side stamps `LOCAL_CLICK_AT` on every local mouse-button
// down. The activity beacon carries the age of that click
// (capped at 60 s) so the receiver can fold it into its own
// "peer click recency" tracker. Smart's decision tree consults
// both before falling back to motion.
// ---------------------------------------------------------------------------

static LOCAL_CLICK_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PEER_CLICK_AT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn bump_local_click() {
    LOCAL_CLICK_AT.store(now_ms(), Ordering::Relaxed);
}

/// Called by the daemon when a beacon arrives carrying a fresh
/// click age (`Some(age_ms)`). We back-date PEER_CLICK_AT by the
/// reported age so comparisons stay accurate across network jitter.
pub fn note_peer_click(age_ms: u32) {
    let stamp = now_ms().saturating_sub(age_ms as u64);
    PEER_CLICK_AT.store(stamp, Ordering::Relaxed);
}

/// ms since the local user clicked, capped at 60 000. Used by
/// the daemon's beacon sender. None if no click recorded this
/// session.
pub fn local_click_age_ms() -> Option<u32> {
    let at = LOCAL_CLICK_AT.load(Ordering::Relaxed);
    if at == 0 {
        return None;
    }
    let age = now_ms().saturating_sub(at).min(60_000) as u32;
    Some(age)
}

fn local_click_age() -> u64 {
    let at = LOCAL_CLICK_AT.load(Ordering::Relaxed);
    if at == 0 { u64::MAX } else { now_ms().saturating_sub(at) }
}

fn peer_click_age() -> u64 {
    let at = PEER_CLICK_AT.load(Ordering::Relaxed);
    if at == 0 { u64::MAX } else { now_ms().saturating_sub(at) }
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
        windows::on_peer_take_control();
        // Phase 2 auto-focus: cursor warp alone doesn't shift
        // keyboard focus on Windows (foreground window stays put
        // when SetCursorPos moves the cursor without a click).
        // The opt-in click here fires AFTER the warp so the
        // window now under the cursor takes focus and subsequent
        // key inject calls land on it.
        if auto_focus_on_take_control() {
            let _ = inject.mouse_button(Button::Left, true);
            let _ = inject.mouse_button(Button::Left, false);
            tracing::info!("auto-focus click fired on TakeControl");
        }
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
