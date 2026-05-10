//! Linux input via evdev / uinput.
//!
//! Capture: read raw events from `/dev/input/event*`. Requires the user to
//! be in group `input` (or a udev rule that grants read access). The daemon
//! discovers eligible devices by enumerating and looking for relative axes
//! (mice/touchpads) or `KEY_A`/`KEY_LEFTCTRL` (keyboards).
//!
//! Inject: a virtual `uinput` device that the OS treats as a real HID. Needs
//! `/dev/uinput` to be writable by the daemon's user.
//!
//! M2 Slice 2 adds **edge-press cursor handover** in the other
//! direction (Ubuntu→Win). Each pump thread maintains a `MODE` state:
//!
//!   * `LOCAL` — events flow to the OS unchanged. We integrate the
//!     `REL_X` deltas into a clamped `CURSOR_X` estimate; the OS
//!     clamps the real cursor at the screen edge while evdev keeps
//!     reporting the overshoot, so any time the user drags into the
//!     real left edge our estimate self-syncs to 0. Sustained leftward
//!     overshoot past 0 (≥ `ENTER_PRESSURE_PX`) is the signal that the
//!     user is actively pushing into the edge → enter `REMOTE`. Any
//!     rightward dx cancels an in-flight press.
//!   * `REMOTE` — every relevant device is `EVIOCGRAB`-ed so the OS no
//!     longer sees motion or keystrokes; we forward them to the peer
//!     instead. A virtual `VIRT_X` tracks the cursor's position on the
//!     peer's screen. When it overshoots the peer's right edge by
//!     `EXIT_BUFFER_PX` we ungrab and hand back local control.
//!
//! Screen geometry is read from env vars at startup:
//!   * `MINESHARE_SCREEN_W` (default 1920) — own screen width
//!   * `MINESHARE_SCREEN_H` (default 1080) — own screen height
//!   * `MINESHARE_PEER_W`   (default 2880) — peer screen width (matches the
//!     test rig's 200%-DPI Win laptop). Slice 2.5 will negotiate this over
//!     the encrypted control channel.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, Device, EventSummary, EventType, KeyCode as EvKey, PropType, RelativeAxisCode,
    SynchronizationCode,
};
use tracing::{debug, info, warn};

use super::{Button, InputCapture, InputEvent, InputInject, KeyCode};

/// Common prefix for our two virtual input devices. The pump
/// loop uses this to skip them during enumeration so we don't
/// pump our own injected events back as captured input.
const VIRTUAL_DEVICE_PREFIX: &str = "MineShare Virtual";
const VIRTUAL_MOUSE_NAME: &str = "MineShare Virtual Mouse";
const VIRTUAL_KEYBOARD_NAME: &str = "MineShare Virtual Keyboard";

const MODE_LOCAL: u8 = 0;
const MODE_REMOTE: u8 = 1;

/// Hysteresis past the peer's right edge before we hand control back.
const EXIT_BUFFER_PX: i32 = 100;
/// Maximum forwarded delta per `SYN_REPORT` so coalesced fast motion
/// can't teleport the peer cursor across its screen. 30px keeps the
/// cursor smooth even with high-DPI peers and aggressive acceleration.
const MAX_DELTA_PX: i32 = 30;
/// Cumulative overshoot past the configured boundary edge before we
/// hand control to the peer. The OS clamps the real cursor at the
/// physical screen edge but evdev keeps reporting deltas from the
/// still-moving HW; we mirror that with clamped CURSOR_X / CURSOR_Y
/// estimates and treat sustained overshoot as the user actively
/// pushing against the edge.
///
/// Two thresholds so the gesture feels symmetric on both axes —
/// the vertical extent of a 1920×1080 desktop is roughly half the
/// horizontal one, so the same 200 px feels twice as sticky on
/// top/bottom edges. Scale the trigger by the actual extent.
const ENTER_PRESSURE_HORIZ_PX: i32 = 200;
const ENTER_PRESSURE_VERT_PX: i32 = 110;

// Modifier-key tracking for the emergency-return hotkey (Ctrl+Alt+R).
static MOD_CTRL: AtomicBool = AtomicBool::new(false);
static MOD_ALT: AtomicBool = AtomicBool::new(false);

// --- shared cursor / grab state across all evdev pump threads ---------

static CURSOR_MODE: AtomicU8 = AtomicU8::new(MODE_LOCAL);
static SCREEN_W: AtomicI32 = AtomicI32::new(1920);
static SCREEN_H: AtomicI32 = AtomicI32::new(1080);
/// Approximate peer screen width — used to clamp `VIRT_X` and detect the
/// peer-side right edge. Slice 2.5 negotiates the real value via
/// `PortAnnounce`; for now defaults to the user's 2880-wide Win laptop.
static PEER_W: AtomicI32 = AtomicI32::new(2880);
/// Estimated own cursor X / Y (clamped to screen bounds). Seeded at
/// screen-centre because Wayland has no portable cursor-pos query;
/// user motion drags both to the truth fast via the clamp self-sync.
static CURSOR_X: AtomicI32 = AtomicI32::new(960);
static CURSOR_Y: AtomicI32 = AtomicI32::new(540);
/// Virtual cursor depth into the peer's screen while we're in `REMOTE`.
/// Generic across horizontal/vertical layouts — reused under the same
/// `VIRT_X` name to avoid renaming the entire FSM.
static VIRT_X: AtomicI32 = AtomicI32::new(0);

/// Stage 10 sub-pixel residue for the sensitivity multiplier.
/// `f32` bits packed into `AtomicU32` because std lacks
/// `AtomicF32`. Single-writer (the evdev forwarding thread)
/// so the relaxed loads are fine.
static SENS_RESIDUE_X: AtomicU32 = AtomicU32::new(0);
static SENS_RESIDUE_Y: AtomicU32 = AtomicU32::new(0);
/// Cumulative overshoot once the cursor estimate has clamped at the
/// configured boundary edge. Resets on motion in the opposite
/// direction. Hits `ENTER_PRESSURE_PX` → enter Remote.
static LEFT_PRESSURE: AtomicI32 = AtomicI32::new(0);

// ---------------------------------------------------------------------------
// Mouse-motion coalescing window (Stage: jitter fix).
//
// Mouse hardware polls at ~1000 Hz, but Windows refreshes the cursor
// at the display rate (~125 Hz). Forwarding 1 kHz of `MouseMove`
// events to the peer makes the receiver call `SendInput` 1000×/sec —
// the OS visually batches those into 8 ms display frames, which the
// user perceives as **stutter** even on a 0 ms RTT link (the
// jitter the user reported).
//
// We aggregate dx/dy here for `FLUSH_INTERVAL_MS` then forward a
// single combined delta. Aligning the wire rate with the receiver's
// display rate cuts UDP traffic ~8×, ~8× the SendInput calls, and
// removes the visible stutter. 8 ms ≈ 125 Hz which matches Windows'
// default cursor update rate.
//
// Static atomics rather than per-thread state because multiple
// pump threads (one per device) all feed a single peer cursor —
// summing across devices is the correct combined motion.
// ---------------------------------------------------------------------------
const FLUSH_INTERVAL_MS: u64 = 8;
static PENDING_DX: AtomicI32 = AtomicI32::new(0);
static PENDING_DY: AtomicI32 = AtomicI32::new(0);
static LAST_FLUSH_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FLUSH_WATCHDOG_STARTED: AtomicBool = AtomicBool::new(false);
static FWD_COUNT: AtomicI32 = AtomicI32::new(0);

pub fn local_screen_geometry() -> (u32, u32) {
    // Stage 9 priority order:
    //   1. Explicit env vars (legacy, also lets users override on
    //      headless setups where no display server is reachable).
    //   2. `xrandr --query` output — works on X11 and XWayland,
    //      reports the bounding rectangle around every connected
    //      monitor under "Screen 0: ... current W x H".
    //   3. 1920x1080 fallback for the no-display-server-available
    //      path (matches the pre-Stage-9 default).
    let env_w = env_i32("MINESHARE_SCREEN_W");
    let env_h = env_i32("MINESHARE_SCREEN_H");
    let (w, h) = if let (Some(w), Some(h)) = (env_w, env_h) {
        (w.max(1) as u32, h.max(1) as u32)
    } else if let Some((w, h)) = detect_via_xrandr() {
        info!(width = w, height = h, "detected screen geometry via xrandr");
        (w, h)
    } else {
        warn!("no env override and xrandr query failed — falling back to 1920x1080");
        (1920, 1080)
    };
    SCREEN_W.store(w as i32, Ordering::Relaxed);
    SCREEN_H.store(h as i32, Ordering::Relaxed);
    (w, h)
}

/// Parse the bounding rectangle out of `xrandr --query` stdout.
/// On a 2-monitor setup the "current ..." line reports the
/// combined dimensions (e.g. `Screen 0: minimum 320 x 200, current
/// 3840 x 1080, maximum 8192 x 8192`), so this naturally handles
/// multi-monitor without any per-output enumeration.
fn detect_via_xrandr() -> Option<(u32, u32)> {
    use std::process::Command;
    let out = Command::new("xrandr").arg("--query").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        // Look for "current 1920 x 1080" anywhere on the first
        // line — xrandr formatting is stable across versions.
        if let Some(rest) = line.split("current ").nth(1) {
            let mut parts = rest.split(|c: char| !c.is_ascii_digit());
            let w: u32 = parts.find(|p| !p.is_empty()).and_then(|p| p.parse().ok())?;
            let h: u32 = parts.find(|p| !p.is_empty()).and_then(|p| p.parse().ok())?;
            if w > 0 && h > 0 {
                return Some((w, h));
            }
        }
    }
    None
}

pub fn set_peer_screen(w: u32, _h: u32) {
    PEER_W.store(w.max(1) as i32, Ordering::Relaxed);
    info!(peer_w = w, "peer screen geometry stored");
}

fn enter_remote() {
    // Refuse if the peer signalled it's already driving Remote.
    if super::peer_in_remote() {
        debug!("enter_remote refused — peer holds Remote");
        return;
    }
    // virt_x is "distance dragged INTO the peer from the edge we crossed".
    // It grows as the user moves further into the peer's screen, and falls
    // back toward zero (then negative) as they head back toward the entry
    // edge. Mirrors Win→Ubuntu's meaning so the exit hysteresis is
    // symmetric: same EXIT_BUFFER_PX of grace before flipping back.
    VIRT_X.store(0, Ordering::Relaxed);
    // Wipe any residual coalescing state so the new Remote session
    // doesn't inherit pending dx/dy from a previous one — and seed
    // last-flush time to "now" so the first 8 ms accumulate freshly.
    PENDING_DX.store(0, Ordering::Release);
    PENDING_DY.store(0, Ordering::Release);
    LAST_FLUSH_MS.store(super::now_ms(), Ordering::Release);
    CURSOR_MODE.store(MODE_REMOTE, Ordering::Release);
    info!("cursor → remote (linux)");
    super::fire_remote_event(super::RemoteEvent::Entered);
}

fn exit_remote() {
    // Reset cursor-position estimate to "just inside the boundary
    // edge" so the user can pull back freely without us
    // mis-detecting another edge crossing. Side depends on the
    // configured layout.
    let w = SCREEN_W.load(Ordering::Relaxed);
    let h = SCREEN_H.load(Ordering::Relaxed);
    let (rx, ry) = match super::peer_side() {
        super::PeerSide::Left => (40, CURSOR_Y.load(Ordering::Relaxed)),
        super::PeerSide::Right => ((w - 41).max(0), CURSOR_Y.load(Ordering::Relaxed)),
        super::PeerSide::Top => (CURSOR_X.load(Ordering::Relaxed), 40),
        super::PeerSide::Bottom => (CURSOR_X.load(Ordering::Relaxed), (h - 41).max(0)),
    };
    CURSOR_X.store(rx, Ordering::Relaxed);
    CURSOR_Y.store(ry, Ordering::Relaxed);
    LEFT_PRESSURE.store(0, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
    info!(restore = ?(rx, ry), "cursor → local (linux)");
    super::fire_remote_event(super::RemoteEvent::Exited);
}

pub fn local_in_remote() -> bool {
    CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE
}

/// Called by `mineshare_input::set_peer_side` whenever the user
/// flips the layout via the GUI. Wipes the press counter and re-
/// centres the cursor-position estimate so the next overshoot
/// detection has a sane baseline — without this the user would
/// have to traverse most of the screen on the new axis to
/// re-trigger.
pub fn reset_after_side_change() {
    let w = SCREEN_W.load(Ordering::Relaxed);
    let h = SCREEN_H.load(Ordering::Relaxed);
    CURSOR_X.store(w / 2, Ordering::Relaxed);
    CURSOR_Y.store(h / 2, Ordering::Relaxed);
    LEFT_PRESSURE.store(0, Ordering::Relaxed);
}

pub fn force_exit_remote() {
    if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
        info!("force_exit_remote — peer asked us to release");
        exit_remote();
    }
}

/// Atomically drain the pending dx/dy accumulator and forward the
/// combined delta to the peer. Caller is responsible for only
/// invoking this in `MODE_REMOTE`. Skips the call entirely when both
/// axes are zero so we don't spam no-op packets.
fn flush_pending<F: Fn(InputEvent) + ?Sized>(sink: &F) {
    // Two `swap` rather than one because there's no atomic "swap two
    // values together" — but the watchdog and pump paths both run
    // this same function, so any partial read on one side just gets
    // forwarded as a smaller delta and the other axis follows on the
    // very next tick. No data loss, just at most 8 ms of split per
    // axis under contention (which is below human perception).
    let dx = PENDING_DX.swap(0, Ordering::AcqRel);
    let dy = PENDING_DY.swap(0, Ordering::AcqRel);
    if dx == 0 && dy == 0 {
        return;
    }
    LAST_FLUSH_MS.store(super::now_ms(), Ordering::Release);
    sink(InputEvent::MouseMove { dx, dy });
    let n = FWD_COUNT.fetch_add(1, Ordering::Relaxed);
    if n % 100 == 0 {
        let virt = VIRT_X.load(Ordering::Relaxed);
        info!(
            dx,
            dy,
            virt_x = virt,
            n,
            "linux coalesced motion forward (8ms-window)"
        );
    }
}

/// Spawn the periodic flush thread once. Wakes every
/// `FLUSH_INTERVAL_MS` to deliver any pending motion that the pump
/// thread couldn't dispatch (e.g. user moved 2 px then paused —
/// without this the residual would sit in `PENDING_*` until the
/// next motion event, producing a perceptible "phantom step" when
/// the user resumes). Cheap: ~125 wakeups/sec, only does atomic
/// loads when nothing is pending.
fn start_flush_watchdog(sink: std::sync::Arc<dyn Fn(InputEvent) + Send + Sync + 'static>) {
    if FLUSH_WATCHDOG_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Err(e) = thread::Builder::new()
        .name("evdev-flush-watchdog".to_string())
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(FLUSH_INTERVAL_MS));
                if CURSOR_MODE.load(Ordering::Acquire) != MODE_REMOTE {
                    continue;
                }
                let n = super::now_ms();
                let last = LAST_FLUSH_MS.load(Ordering::Relaxed);
                if n.saturating_sub(last) >= FLUSH_INTERVAL_MS {
                    flush_pending(&*sink);
                }
            }
        })
    {
        warn!(error = %e, "failed to spawn evdev flush watchdog — motion will still flow but with sub-window granularity loss on pause");
        FLUSH_WATCHDOG_STARTED.store(false, Ordering::Release);
    }
}

/// Called when the peer signals it has taken Remote control of us.
/// Wayland has no portable cursor-warp, so we slam the cursor into
/// the boundary-edge OS clamp by injecting a wide relative delta —
/// the side (and thus the sign) depends on the configured layout.
pub fn on_peer_take_control(inject: &dyn InputInject) {
    let w = SCREEN_W.load(Ordering::Relaxed).max(1);
    let h = SCREEN_H.load(Ordering::Relaxed).max(1);
    let (slam_dx, slam_dy, rx, ry) = match super::peer_side() {
        super::PeerSide::Left => (-(w * 2), 0, 0, h / 2),
        super::PeerSide::Right => (w * 2, 0, w - 1, h / 2),
        super::PeerSide::Top => (0, -(h * 2), w / 2, 0),
        super::PeerSide::Bottom => (0, h * 2, w / 2, h - 1),
    };
    if let Err(e) = inject.mouse_move_rel(slam_dx, slam_dy) {
        warn!(error = %e, "boundary-edge slam failed");
        return;
    }

    // Phase 2 auto-focus: GNOME-Wayland (Ubuntu's default) is
    // click-to-focus, so the cursor warp alone doesn't direct the
    // peer's keystrokes anywhere — they get emitted via uinput
    // but no window has keyboard focus and the typing vanishes.
    // When this opt-in is enabled we fire a single left-click in
    // place after the slam, which activates whatever window the
    // cursor landed on. Side effect: any button / link / drag
    // handle under the cursor gets clicked, which is why this is
    // off by default.
    if super::auto_focus_on_take_control() {
        if let Err(e) = inject.mouse_button(super::Button::Left, true) {
            warn!(error = %e, "auto-focus click press failed");
        } else if let Err(e) = inject.mouse_button(super::Button::Left, false) {
            warn!(error = %e, "auto-focus click release failed");
        } else {
            info!("auto-focus click fired on TakeControl");
        }
    }

    // Resync our own LOCAL-mode tracking so we don't false-trigger
    // re-entry once the peer releases and HW motion resumes here.
    CURSOR_X.store(rx, Ordering::Relaxed);
    CURSOR_Y.store(ry, Ordering::Relaxed);
    LEFT_PRESSURE.store(0, Ordering::Relaxed);
    info!(
        slam = ?(slam_dx, slam_dy),
        side = ?super::peer_side(),
        "boundary-edge slam on TakeControl (linux)"
    );
}

pub struct EvdevCapture {
    devices: Vec<(PathBuf, Device)>,
}

impl EvdevCapture {
    pub fn new() -> Result<Self> {
        // Stage 9: prefer the auto-detected (xrandr / env) values
        // already populated by `local_screen_geometry()`. The env
        // override path still works for headless / weird-Wayland
        // boxes since `local_screen_geometry()` honours it first.
        let (auto_w, auto_h) = local_screen_geometry();
        let screen_w = auto_w as i32;
        let screen_h = auto_h as i32;
        // Peer width still honours `MINESHARE_PEER_W` for the
        // "haven't received PortAnnounce yet" startup window;
        // `set_peer_screen` overwrites it as soon as the
        // handshake completes.
        let peer_w = env_i32("MINESHARE_PEER_W").unwrap_or(2880);
        PEER_W.store(peer_w, Ordering::Relaxed);
        CURSOR_X.store(screen_w / 2, Ordering::Relaxed);
        info!(screen_w, screen_h, peer_w, "evdev capture: screen geometry");

        let mut devices = Vec::new();
        for (path, device) in evdev::enumerate() {
            if device
                .name()
                .map(|n| n.starts_with(VIRTUAL_DEVICE_PREFIX))
                .unwrap_or(false)
            {
                continue;
            }
            if is_relevant(&device) {
                let name = device.name().unwrap_or("?").to_string();
                debug!(path = %path.display(), name, "evdev capture: opened device");
                devices.push((path, device));
            }
        }
        if devices.is_empty() {
            anyhow::bail!(
                "no evdev mouse/keyboard found. Add user to group `input` and re-login, \
                 or install a udev rule granting read access to /dev/input/event*"
            );
        }
        info!(count = devices.len(), "evdev capture initialised");
        Ok(Self { devices })
    }
}

fn env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

fn is_relevant(d: &Device) -> bool {
    let has_rel = d
        .supported_relative_axes()
        .map(|a| a.contains(RelativeAxisCode::REL_X) || a.contains(RelativeAxisCode::REL_WHEEL))
        .unwrap_or(false);
    let has_keyboard = d
        .supported_keys()
        .map(|k| k.contains(EvKey::KEY_A) || k.contains(EvKey::KEY_SPACE))
        .unwrap_or(false);
    let has_mouse_btns = d
        .supported_keys()
        .map(|k| k.contains(EvKey::BTN_LEFT) || k.contains(EvKey::BTN_RIGHT))
        .unwrap_or(false);
    has_rel || has_keyboard || has_mouse_btns
}

/// Classify a device as "pointer-like" — has actual relative-axis
/// motion. Used to scope EVIOCGRAB more narrowly when the peer
/// is driving us: only mouse-class devices need to be grabbed
/// (to prevent the user's HW cursor fighting the injected one).
/// Keyboards stay free so the user can keep typing on their own
/// machine even while the peer's cursor is borrowing the screen.
///
/// We deliberately key off REL_X / REL_Y *only*, not BTN_LEFT —
/// many gaming keyboards expose media-key BTN_* codes the kernel
/// happens to share with mouse buttons, and the original
/// "has_mouse_btns" branch mis-classified them as pointers.
fn is_pointer_device(d: &Device) -> bool {
    d.supported_relative_axes()
        .map(|a| a.contains(RelativeAxisCode::REL_X) || a.contains(RelativeAxisCode::REL_Y))
        .unwrap_or(false)
}

impl InputCapture for EvdevCapture {
    fn start(
        &mut self,
        sink: std::sync::Arc<dyn Fn(InputEvent) + Send + Sync + 'static>,
    ) -> Result<()> {
        // Periodic flush so the user's "moved 2 px then paused" case
        // doesn't strand pending motion in the coalescer. Idempotent —
        // safe to call across reconnects.
        start_flush_watchdog(sink.clone());
        for (path, device) in self.devices.drain(..) {
            let sink = sink.clone(); // cheap Arc ref-count bump
            let is_pointer = is_pointer_device(&device);
            let name = device.name().unwrap_or("?").to_string();
            info!(
                path = %path.display(),
                name,
                is_pointer,
                "evdev pump classification (pointer = grabbed when peer drives; non-pointer = stays free for typing)"
            );
            thread::Builder::new()
                .name(format!("evdev-{}", path.display()))
                .spawn(move || pump_device(path, device, is_pointer, sink))
                .context("spawn evdev thread")?;
        }
        Ok(())
    }
}

fn pump_device(
    path: PathBuf,
    mut device: Device,
    is_pointer: bool,
    sink: std::sync::Arc<dyn Fn(InputEvent) + Send + Sync + 'static>,
) {
    let mut accum_dx: i32 = 0;
    let mut accum_dy: i32 = 0;
    let mut grabbed = false;

    loop {
        // Two grab regimes:
        //   * MODE_REMOTE (we drive the peer): grab everything —
        //     mouse + keyboard — so local OS doesn't double-process
        //     events we're forwarding over the wire.
        //   * peer_in_remote (peer drives us): grab only POINTER
        //     devices to prevent the user's HW mouse motion fighting
        //     our injected cursor. Keyboards stay free so the user
        //     can keep typing on their own machine while the peer's
        //     cursor borrows the screen — keystrokes don't visually
        //     "fight" the way cursor motion does.
        let we_drive = CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE;
        let peer_drives = super::peer_in_remote();
        let want_grab = we_drive || (peer_drives && is_pointer);
        if want_grab != grabbed {
            if want_grab {
                match device.grab() {
                    Ok(()) => {
                        grabbed = true;
                        debug!(path = %path.display(), "evdev grabbed");
                    }
                    Err(e) => warn!(error = %e, path = %path.display(), "evdev grab failed"),
                }
            } else {
                let _ = device.ungrab();
                grabbed = false;
                debug!(path = %path.display(), "evdev ungrabbed");
            }
        }

        let events = match device.fetch_events() {
            Ok(it) => it,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "evdev fetch_events failed");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        for ev in events {
            match ev.destructure() {
                EventSummary::RelativeAxis(_, axis, value) => match axis {
                    RelativeAxisCode::REL_X => {
                        accum_dx += value;
                        // Stage 11 Smart-keyboard: bump local
                        // mouse activity on real HW motion.
                        // Our virtual uinput mouse is excluded
                        // from this capture by name prefix, so
                        // injected events don't loop back here.
                        super::bump_local_mouse_activity();
                    }
                    RelativeAxisCode::REL_Y => {
                        accum_dy += value;
                        super::bump_local_mouse_activity();
                    }
                    RelativeAxisCode::REL_WHEEL => {
                        if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
                            // Stage 10: optional Y-axis flip.
                            let dy = if super::invert_scroll_y() {
                                -(value as f32)
                            } else {
                                value as f32
                            };
                            sink(InputEvent::Scroll { dx: 0.0, dy });
                        }
                    }
                    RelativeAxisCode::REL_HWHEEL => {
                        if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
                            let dx = if super::invert_scroll_x() {
                                -(value as f32)
                            } else {
                                value as f32
                            };
                            sink(InputEvent::Scroll { dx, dy: 0.0 });
                        }
                    }
                    _ => {}
                },
                EventSummary::Key(_, key, value) => {
                    // evdev key value semantics: 0 = release, 1 = press,
                    // 2 = auto-repeat. The previous code treated 2 as
                    // "released" which broke modifier tracking — holding
                    // Ctrl past the auto-repeat threshold flipped
                    // MOD_CTRL back to false, and the hotkey never saw
                    // both modifiers held simultaneously.
                    let down = value == 1;
                    let up = value == 0;

                    // Update modifier state ONLY on real press / release,
                    // not on auto-repeat (which would otherwise overwrite
                    // a held modifier with `false`).
                    if key == EvKey::KEY_LEFTCTRL || key == EvKey::KEY_RIGHTCTRL {
                        if down {
                            MOD_CTRL.store(true, Ordering::Relaxed);
                        } else if up {
                            MOD_CTRL.store(false, Ordering::Relaxed);
                        }
                    }
                    if key == EvKey::KEY_LEFTALT || key == EvKey::KEY_RIGHTALT {
                        if down {
                            MOD_ALT.store(true, Ordering::Relaxed);
                        } else if up {
                            MOD_ALT.store(false, Ordering::Relaxed);
                        }
                    }

                    // Hotkey: Ctrl+Alt+R toggles Local ⇄ Remote, or asks
                    // the peer to release if the peer holds Remote.
                    if down
                        && key == EvKey::KEY_R
                        && MOD_CTRL.load(Ordering::Relaxed)
                        && MOD_ALT.load(Ordering::Relaxed)
                    {
                        let mode = CURSOR_MODE.load(Ordering::Acquire);
                        if mode == MODE_REMOTE {
                            info!("hotkey Ctrl+Alt+R — forcing exit_remote");
                            exit_remote();
                        } else if super::peer_in_remote() {
                            info!("hotkey Ctrl+Alt+R — requesting peer to release");
                            super::fire_remote_event(super::RemoteEvent::RequestPeerExit);
                        } else {
                            info!("hotkey Ctrl+Alt+R — entering remote");
                            enter_remote();
                        }
                        continue;
                    }

                    // Hotkey: Ctrl+Alt+L toggles game-mode lock —
                    // pins input to this PC so accidental edge
                    // crosses during gameplay don't yank focus.
                    if down
                        && key == EvKey::KEY_L
                        && MOD_CTRL.load(Ordering::Relaxed)
                        && MOD_ALT.load(Ordering::Relaxed)
                    {
                        let next = !super::is_input_locked();
                        info!(locked = next, "hotkey Ctrl+Alt+L — game-mode lock");
                        super::set_input_locked(next);
                        continue;
                    }

                    // Hotkey: Ctrl+Alt+K cycles keyboard target. ALWAYS
                    // handled regardless of current target, so the
                    // user can always switch back.
                    if down
                        && key == EvKey::KEY_K
                        && MOD_CTRL.load(Ordering::Relaxed)
                        && MOD_ALT.load(Ordering::Relaxed)
                    {
                        super::cycle_keyboard_target();
                        info!(target = ?super::keyboard_target(), "hotkey Ctrl+Alt+K — keyboard target");
                        continue;
                    }

                    // Keystrokes routed via `route_keystroke` — combines
                    // user's keyboard target preference, cursor side,
                    // AND a held-key tracker that ensures key
                    // releases follow their press to the same
                    // destination (fixes stuck-modifier "everything's
                    // uppercase even though Caps Lock is off" bug
                    // when Smart flips mid-press).
                    //
                    // Mouse buttons still follow the cursor (a pinned
                    // keyboard doesn't stop the user from clicking
                    // wherever the mouse is pointing).
                    let cursor_in_remote =
                        CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE;
                    if let Some(btn) = button_from_key(key) {
                        // Local click → focus signal (regardless of
                        // whether we forward it).
                        if value != 0 {
                            super::bump_local_click();
                        }
                        // Held-aware routing — UP follows DOWN's
                        // destination so the peer never ends up
                        // with a stuck-down button after a cursor
                        // cross-back happens between press and
                        // release.
                        if super::route_mouse_button(btn, value != 0, cursor_in_remote) {
                            sink(InputEvent::MouseButton {
                                btn,
                                down: value != 0,
                            });
                        }
                    } else if super::route_keystroke(key.0, value != 0, cursor_in_remote) {
                        sink(InputEvent::Key {
                            code: KeyCode(key.0),
                            down: value != 0,
                        });
                        super::note_key_forwarded_with_code(key.0, value != 0);
                    }
                    // In LOCAL we don't forward; they're already going to
                    // the OS via the un-grabbed kernel path.
                }
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    if accum_dx != 0 || accum_dy != 0 {
                        handle_motion_batch(accum_dx, accum_dy, &*sink);
                        accum_dx = 0;
                        accum_dy = 0;
                    }
                }
                _ => {}
            }
        }
    }
}

/// Apply one synced (dx, dy) batch — either tracks our own cursor and
/// triggers `enter_remote`, or forwards the delta to the peer and tracks
/// `VIRT_X` for the right-edge exit.
fn handle_motion_batch<F: Fn(InputEvent) + ?Sized>(dx: i32, dy: i32, sink: &F) {
    let mode = CURSOR_MODE.load(Ordering::Acquire);
    if mode == MODE_LOCAL {
        // When the peer is currently driving (peer_in_remote) we *do*
        // see the real HW events (the device is grabbed for us), so any
        // meaningful motion means the local user is trying to take
        // control back. Forward that intent as `RequestPeerExit` so
        // the peer's daemon will exit_remote and unfreeze our cursor.
        if super::peer_in_remote() {
            if (dx.abs() + dy.abs()) > 5 {
                info!(
                    dx,
                    dy, "local HW motion while peer holds Remote — requesting peer release"
                );
                super::fire_remote_event(super::RemoteEvent::RequestPeerExit);
            }
            return;
        }

        // Track an estimate of the real cursor X *and* Y. Wayland
        // has no portable cursor-pos query, but the OS clamps at
        // the screen edges — and evdev keeps reporting the
        // overshoot. Mirroring with clamped CURSOR_X / CURSOR_Y
        // means any time the user drags into a real screen edge our
        // estimate self-syncs; drift can't accumulate beyond one
        // screen.
        let screen_w = SCREEN_W.load(Ordering::Relaxed);
        let screen_h = SCREEN_H.load(Ordering::Relaxed);
        let prev_x = CURSOR_X.load(Ordering::Relaxed);
        let prev_y = CURSOR_Y.load(Ordering::Relaxed);
        let raw_x = prev_x + dx;
        let raw_y = prev_y + dy;
        CURSOR_X.store(raw_x.clamp(0, (screen_w - 1).max(0)), Ordering::Relaxed);
        CURSOR_Y.store(raw_y.clamp(0, (screen_h - 1).max(0)), Ordering::Relaxed);

        // Edge press: which clamp signals "user is pushing into the
        // boundary" depends on the layout. We pick one axis (X for
        // left/right sides, Y for top/bottom) and detect overshoot
        // past the configured edge of the screen. The opposite-
        // direction motion cancels an in-flight press (user pulled
        // back from the edge).
        let (overshoot, cancel) = match super::peer_side() {
            super::PeerSide::Left => (
                if raw_x < 0 { Some(-raw_x) } else { None },
                dx > 0,
            ),
            super::PeerSide::Right => (
                if raw_x > screen_w - 1 {
                    Some(raw_x - (screen_w - 1))
                } else {
                    None
                },
                dx < 0,
            ),
            super::PeerSide::Top => (
                if raw_y < 0 { Some(-raw_y) } else { None },
                dy > 0,
            ),
            super::PeerSide::Bottom => (
                if raw_y > screen_h - 1 {
                    Some(raw_y - (screen_h - 1))
                } else {
                    None
                },
                dy < 0,
            ),
        };
        if let Some(over) = overshoot {
            // Game-mode lock: pretend the press never happened.
            // We still update CURSOR_X/Y above so the estimate
            // self-syncs at the clamp; we just don't trip the
            // FSM into Remote.
            if super::is_input_locked() {
                LEFT_PRESSURE.store(0, Ordering::Relaxed);
                return;
            }
            let threshold = if super::peer_side().is_horizontal() {
                ENTER_PRESSURE_HORIZ_PX
            } else {
                ENTER_PRESSURE_VERT_PX
            };
            let pressure = LEFT_PRESSURE.fetch_add(over, Ordering::Relaxed) + over;
            if pressure >= threshold {
                info!(
                    pressure,
                    threshold,
                    side = ?super::peer_side(),
                    "edge press — entering remote (linux)"
                );
                LEFT_PRESSURE.store(0, Ordering::Relaxed);
                enter_remote();
            }
        } else if cancel {
            LEFT_PRESSURE.store(0, Ordering::Relaxed);
        }
        // No forward in LOCAL — OS already moves the cursor.
    } else {
        let peer_w = PEER_W.load(Ordering::Relaxed);
        // Depth-direction delta: how far INTO the peer the latest
        // HW motion takes us. Left means -dx, right means dx, top
        // means -dy, bottom means dy. virt_x grows on depth and
        // retreats toward -EXIT_BUFFER_PX as the user pulls back.
        // (This bookkeeping has to run on EVERY SYN_REPORT so the
        // exit-edge detection stays accurate, even though we only
        // *forward* an aggregate every FLUSH_INTERVAL_MS below.)
        let depth_dx = match super::peer_side() {
            super::PeerSide::Left => -dx,
            super::PeerSide::Right => dx,
            super::PeerSide::Top => -dy,
            super::PeerSide::Bottom => dy,
        };
        let raw = VIRT_X.load(Ordering::Relaxed) + depth_dx;
        let new_virt_x = raw.clamp(-EXIT_BUFFER_PX, peer_w);
        VIRT_X.store(new_virt_x, Ordering::Relaxed);

        if new_virt_x <= -EXIT_BUFFER_PX {
            // Hand any buffered motion to the peer before flipping
            // back to LOCAL — otherwise the last few px the user
            // dragged before crossing back would never reach them.
            flush_pending(sink);
            exit_remote();
            return;
        }

        // Stage 10 sensitivity scaling with sub-pixel residue —
        // applied PER SYN_REPORT (same as before) so the residue
        // tracker stays accurate at high frequencies.
        let mut rx = f32::from_bits(SENS_RESIDUE_X.load(Ordering::Relaxed));
        let mut ry = f32::from_bits(SENS_RESIDUE_Y.load(Ordering::Relaxed));
        let scaled_dx = super::scale_delta(dx, &mut rx);
        let scaled_dy = super::scale_delta(dy, &mut ry);
        SENS_RESIDUE_X.store(rx.to_bits(), Ordering::Relaxed);
        SENS_RESIDUE_Y.store(ry.to_bits(), Ordering::Relaxed);
        let fdx = scaled_dx.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
        let fdy = scaled_dy.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);

        // Coalesce into the pending accumulator. The sum across an
        // 8 ms window can grow well past MAX_DELTA_PX (that cap
        // exists to bound a SINGLE evdev event, not the aggregate),
        // and that's exactly what we want — fast flicks should still
        // travel a fast distance, just delivered as one packet
        // instead of eight.
        PENDING_DX.fetch_add(fdx, Ordering::AcqRel);
        PENDING_DY.fetch_add(fdy, Ordering::AcqRel);

        // Opportunistic flush: if the window has already elapsed,
        // dispatch immediately on this SYN rather than waiting for
        // the watchdog tick. Keeps the worst-case latency bounded
        // by the time between SYN_REPORTs (~1 ms) when the user is
        // actively moving — only when motion *stops* does the
        // watchdog's 8 ms tick determine the final-fragment delay.
        let now_ms = super::now_ms();
        let last = LAST_FLUSH_MS.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= FLUSH_INTERVAL_MS {
            flush_pending(sink);
        }
    }
}

fn button_from_key(k: EvKey) -> Option<Button> {
    match k {
        EvKey::BTN_LEFT => Some(Button::Left),
        EvKey::BTN_RIGHT => Some(Button::Right),
        EvKey::BTN_MIDDLE => Some(Button::Middle),
        EvKey::BTN_SIDE => Some(Button::X1),
        EvKey::BTN_EXTRA => Some(Button::X2),
        _ => None,
    }
}

fn key_from_button(btn: Button) -> EvKey {
    match btn {
        Button::Left => EvKey::BTN_LEFT,
        Button::Right => EvKey::BTN_RIGHT,
        Button::Middle => EvKey::BTN_MIDDLE,
        Button::X1 => EvKey::BTN_SIDE,
        Button::X2 => EvKey::BTN_EXTRA,
    }
}

/// Two virtual devices, one per input class. Earlier we shipped a
/// single combo device that registered REL_X/Y + POINTER prop +
/// keys 1..=255 + mouse buttons in one `uinput` node. libinput
/// hates that — depending on which capability it sees first the
/// device gets classified as either a pointer or a keyboard, and
/// in the GNOME-Wayland case it sometimes ended up bound as the
/// "keyboard seat" while not actually being the user's typing
/// device. The result was a stuck state where letter keys from
/// the real keyboard stopped reaching apps until the user logged
/// out and back in.
///
/// Splitting into a clean mouse-only node + keyboard-only node
/// lets each one be classified unambiguously, the same way real
/// HID hardware presents itself.
/// Tag tracking which virtual node a held code belongs to so
/// `release_all_held` knows which uinput sink to emit the
/// synthetic key-up on.
#[derive(Hash, PartialEq, Eq, Clone, Copy)]
enum HeldKey {
    /// BTN_* mouse-button code emitted on the mouse node.
    MouseBtn(u16),
    /// KEY_* keyboard code emitted on the keyboard node.
    KbKey(u16),
}

pub struct UinputInject {
    mouse: parking_lot::Mutex<VirtualDevice>,
    keyboard: parking_lot::Mutex<VirtualDevice>,
    /// Set of codes the peer has injected `down` without a matching
    /// `up`. Maintained per inject call; drained on
    /// `release_all_held` at session teardown.
    held: parking_lot::Mutex<HashSet<HeldKey>>,
}

impl UinputInject {
    pub fn new() -> Result<Self> {
        // Mouse node: REL axes + POINTER prop + only the BTN_*
        // mouse-button range. No typing keys here.
        let mut mouse_keys = AttributeSet::<EvKey>::new();
        for code in 0x100u16..=0x151u16 {
            mouse_keys.insert(EvKey(code));
        }
        let mut rel = AttributeSet::<RelativeAxisCode>::new();
        rel.insert(RelativeAxisCode::REL_X);
        rel.insert(RelativeAxisCode::REL_Y);
        rel.insert(RelativeAxisCode::REL_WHEEL);
        rel.insert(RelativeAxisCode::REL_HWHEEL);
        let mut mouse_props = AttributeSet::<PropType>::new();
        mouse_props.insert(PropType::POINTER);
        let mouse = VirtualDevice::builder()
            .context("uinput builder — need /dev/uinput access (group `input`)")?
            .name(VIRTUAL_MOUSE_NAME)
            .with_keys(&mouse_keys)?
            .with_relative_axes(&rel)?
            .with_properties(&mouse_props)?
            .build()
            .context("create uinput mouse device")?;
        info!(name = VIRTUAL_MOUSE_NAME, "uinput virtual mouse created");

        // Keyboard node: typing keys (1..=255). Mouse-button codes
        // overlap (BTN_LEFT == 0x110 falls in the 1..=255 range)
        // but it's harmless to register them here too — the mouse
        // device is the one we actually emit them on.
        let mut kb_keys = AttributeSet::<EvKey>::new();
        for code in 1u16..=255u16 {
            kb_keys.insert(EvKey(code));
        }
        let keyboard = VirtualDevice::builder()
            .context("uinput builder — need /dev/uinput access (group `input`)")?
            .name(VIRTUAL_KEYBOARD_NAME)
            .with_keys(&kb_keys)?
            .build()
            .context("create uinput keyboard device")?;
        info!(name = VIRTUAL_KEYBOARD_NAME, "uinput virtual keyboard created");

        Ok(Self {
            mouse: parking_lot::Mutex::new(mouse),
            keyboard: parking_lot::Mutex::new(keyboard),
            held: parking_lot::Mutex::new(HashSet::new()),
        })
    }

    fn emit_mouse(&self, ev: &[evdev::InputEvent]) -> Result<()> {
        self.mouse.lock().emit(ev).context("uinput emit (mouse)")?;
        Ok(())
    }

    fn emit_keyboard(&self, ev: &[evdev::InputEvent]) -> Result<()> {
        self.keyboard
            .lock()
            .emit(ev)
            .context("uinput emit (keyboard)")?;
        Ok(())
    }
}

impl InputInject for UinputInject {
    fn mouse_move_rel(&self, dx: i32, dy: i32) -> Result<()> {
        let mut events = Vec::with_capacity(2);
        if dx != 0 {
            events.push(evdev::InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_X.0,
                dx,
            ));
        }
        if dy != 0 {
            events.push(evdev::InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_Y.0,
                dy,
            ));
        }
        if !events.is_empty() {
            self.emit_mouse(&events)?;
        }
        Ok(())
    }

    fn mouse_button(&self, btn: Button, down: bool) -> Result<()> {
        let key = key_from_button(btn);
        self.emit_mouse(&[evdev::InputEvent::new(
            EventType::KEY.0,
            key.0,
            if down { 1 } else { 0 },
        )])?;
        let mut held = self.held.lock();
        if down {
            held.insert(HeldKey::MouseBtn(key.0));
        } else {
            held.remove(&HeldKey::MouseBtn(key.0));
        }
        Ok(())
    }

    fn key(&self, code: KeyCode, down: bool) -> Result<()> {
        self.emit_keyboard(&[evdev::InputEvent::new(
            EventType::KEY.0,
            code.0,
            if down { 1 } else { 0 },
        )])?;
        super::note_key_injected_with_code(code.0, down);
        let mut held = self.held.lock();
        if down {
            held.insert(HeldKey::KbKey(code.0));
        } else {
            held.remove(&HeldKey::KbKey(code.0));
        }
        Ok(())
    }

    fn release_all_held(&self) -> Result<()> {
        let drained: Vec<HeldKey> = self.held.lock().drain().collect();
        if drained.is_empty() {
            return Ok(());
        }
        let count = drained.len();
        for h in drained {
            let res = match h {
                HeldKey::KbKey(code) => self.emit_keyboard(&[evdev::InputEvent::new(
                    EventType::KEY.0,
                    code,
                    0,
                )]),
                HeldKey::MouseBtn(code) => self.emit_mouse(&[evdev::InputEvent::new(
                    EventType::KEY.0,
                    code,
                    0,
                )]),
            };
            if let Err(e) = res {
                warn!(error = %e, "failed to release held key on session end");
            }
        }
        info!(
            count,
            "released stale held keys at session end (preventing stuck-key after peer disconnect)"
        );
        Ok(())
    }

    fn scroll(&self, dx: f32, dy: f32) -> Result<()> {
        let mut events = Vec::new();
        if dy != 0.0 {
            events.push(evdev::InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_WHEEL.0,
                dy.round() as i32,
            ));
        }
        if dx != 0.0 {
            events.push(evdev::InputEvent::new(
                EventType::RELATIVE.0,
                RelativeAxisCode::REL_HWHEEL.0,
                dx.round() as i32,
            ));
        }
        if !events.is_empty() {
            self.emit_mouse(&events)?;
        }
        Ok(())
    }
}
