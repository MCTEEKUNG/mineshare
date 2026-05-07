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
//! M2 Slice 2 adds **edge-triggered cursor handover** in the other
//! direction (Ubuntu→Win). Each pump thread maintains a `MODE` state:
//!
//!   * `LOCAL` — events flow to the OS unchanged. We watch our own
//!     accumulated `REL_X` to estimate cursor position and detect when
//!     the user has dragged into the left edge.
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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, Device, EventSummary, EventType, KeyCode as EvKey, PropType, RelativeAxisCode,
    SynchronizationCode,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use super::{Button, InputCapture, InputEvent, InputInject, KeyCode};

const VIRTUAL_DEVICE_NAME: &str = "MineShare Virtual Input";

const MODE_LOCAL: u8 = 0;
const MODE_REMOTE: u8 = 1;

/// Hysteresis past the peer's right edge before we hand control back.
const EXIT_BUFFER_PX: i32 = 100;
/// Maximum forwarded delta per `SYN_REPORT` so coalesced fast motion
/// can't teleport the peer cursor across its screen. 30px keeps the
/// cursor smooth even with high-DPI peers and aggressive acceleration.
const MAX_DELTA_PX: i32 = 30;
/// Cumulative leftward motion at the estimated left edge before we enter
/// Remote. Without a reliable Wayland cursor-pos query we work from a
/// best-effort estimate; the pressure threshold guards against the
/// estimate being off (e.g. the real cursor is already at the left edge
/// when the daemon starts) — the user has to deliberately push past the
/// estimated edge for this many extra pixels to trip a transition.
const ENTER_PRESSURE_PX: i32 = 100;

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
/// Estimated own cursor X (clamped to `[0, SCREEN_W-1]`). We seed it at
/// screen-centre because there is no portable way to query the real
/// cursor position on Wayland; user motion drags it to the truth fast.
static CURSOR_X: AtomicI32 = AtomicI32::new(960);
/// Virtual cursor X on the peer's screen while we're in `REMOTE`.
static VIRT_X: AtomicI32 = AtomicI32::new(0);
/// Cumulative leftward overshoot once `CURSOR_X` has clamped to zero.
/// Resets on rightward motion. When it reaches `ENTER_PRESSURE_PX` we
/// transition to Remote.
#[allow(dead_code)]
static LEFT_PRESSURE: AtomicI32 = AtomicI32::new(0);

pub fn local_screen_geometry() -> (u32, u32) {
    let w = env_i32("MINESHARE_SCREEN_W").unwrap_or(1920).max(1) as u32;
    let h = env_i32("MINESHARE_SCREEN_H").unwrap_or(1080).max(1) as u32;
    SCREEN_W.store(w as i32, Ordering::Relaxed);
    SCREEN_H.store(h as i32, Ordering::Relaxed);
    (w, h)
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
    CURSOR_MODE.store(MODE_REMOTE, Ordering::Release);
    info!("cursor → remote (linux)");
    super::fire_remote_event(super::RemoteEvent::Entered);
}

fn exit_remote() {
    // Reset cursor estimate to "near left edge" so the user can drag right
    // freely without us mis-detecting another edge crossing.
    CURSOR_X.store(40, Ordering::Relaxed);
    LEFT_PRESSURE.store(0, Ordering::Relaxed);
    CURSOR_MODE.store(MODE_LOCAL, Ordering::Release);
    info!(restore_x = 40, "cursor → local (linux)");
    super::fire_remote_event(super::RemoteEvent::Exited);
}

pub struct EvdevCapture {
    devices: Vec<(PathBuf, Device)>,
}

impl EvdevCapture {
    pub fn new() -> Result<Self> {
        // Pull screen geometry from env so the user can override on
        // mismatched setups without rebuilding.
        let screen_w = env_i32("MINESHARE_SCREEN_W").unwrap_or(1920);
        let screen_h = env_i32("MINESHARE_SCREEN_H").unwrap_or(1080);
        let peer_w = env_i32("MINESHARE_PEER_W").unwrap_or(2880);
        SCREEN_W.store(screen_w, Ordering::Relaxed);
        SCREEN_H.store(screen_h, Ordering::Relaxed);
        PEER_W.store(peer_w, Ordering::Relaxed);
        CURSOR_X.store(screen_w / 2, Ordering::Relaxed);
        info!(screen_w, screen_h, peer_w, "evdev capture: screen geometry");

        let mut devices = Vec::new();
        for (path, device) in evdev::enumerate() {
            if device
                .name()
                .map(|n| n.starts_with(VIRTUAL_DEVICE_NAME))
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

impl InputCapture for EvdevCapture {
    fn start(&mut self, sink: UnboundedSender<InputEvent>) -> Result<()> {
        for (path, device) in self.devices.drain(..) {
            let sink = sink.clone();
            thread::Builder::new()
                .name(format!("evdev-{}", path.display()))
                .spawn(move || pump_device(path, device, sink))
                .context("spawn evdev thread")?;
        }
        Ok(())
    }
}

fn pump_device(path: PathBuf, mut device: Device, sink: UnboundedSender<InputEvent>) {
    let mut accum_dx: i32 = 0;
    let mut accum_dy: i32 = 0;
    let mut grabbed = false;

    loop {
        // Grab whenever EITHER side is in Remote — when local capture is
        // forwarding to the peer, OR when the peer is forwarding to us
        // (so the user's real HW doesn't fight the injected cursor on
        // Ubuntu's compositor).
        let want_grab =
            CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE || super::peer_in_remote();
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
                    RelativeAxisCode::REL_X => accum_dx += value,
                    RelativeAxisCode::REL_Y => accum_dy += value,
                    RelativeAxisCode::REL_WHEEL => {
                        if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
                            let _ = sink.send(InputEvent::Scroll {
                                dx: 0.0,
                                dy: value as f32,
                            });
                        }
                    }
                    RelativeAxisCode::REL_HWHEEL => {
                        if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
                            let _ = sink.send(InputEvent::Scroll {
                                dx: value as f32,
                                dy: 0.0,
                            });
                        }
                    }
                    _ => {}
                },
                EventSummary::Key(_, key, value) => {
                    let down = value == 1;

                    // Track modifier state for the emergency-return
                    // hotkey, regardless of mode.
                    if key == EvKey::KEY_LEFTCTRL || key == EvKey::KEY_RIGHTCTRL {
                        MOD_CTRL.store(down, Ordering::Relaxed);
                    }
                    if key == EvKey::KEY_LEFTALT || key == EvKey::KEY_RIGHTALT {
                        MOD_ALT.store(down, Ordering::Relaxed);
                    }

                    // Hotkey: Ctrl+Alt+R toggles Local ⇄ Remote.
                    if down
                        && key == EvKey::KEY_R
                        && MOD_CTRL.load(Ordering::Relaxed)
                        && MOD_ALT.load(Ordering::Relaxed)
                    {
                        match CURSOR_MODE.load(Ordering::Acquire) {
                            MODE_REMOTE => {
                                info!("hotkey Ctrl+Alt+R — forcing exit_remote");
                                exit_remote();
                            }
                            _ => {
                                info!("hotkey Ctrl+Alt+R — entering remote");
                                enter_remote();
                            }
                        }
                        continue;
                    }

                    // Keystrokes and mouse buttons follow the cursor.
                    if CURSOR_MODE.load(Ordering::Acquire) == MODE_REMOTE {
                        if let Some(btn) = button_from_key(key) {
                            let _ = sink.send(InputEvent::MouseButton {
                                btn,
                                down: value != 0,
                            });
                        } else {
                            let _ = sink.send(InputEvent::Key {
                                code: KeyCode(key.0),
                                down: value != 0,
                            });
                        }
                    }
                    // In LOCAL we don't forward; they're already going to
                    // the OS via the un-grabbed kernel path.
                }
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    if accum_dx != 0 || accum_dy != 0 {
                        handle_motion_batch(accum_dx, accum_dy, &sink);
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
fn handle_motion_batch(dx: i32, dy: i32, sink: &UnboundedSender<InputEvent>) {
    use std::sync::atomic::AtomicI32;

    let mode = CURSOR_MODE.load(Ordering::Acquire);
    if mode == MODE_LOCAL {
        // Linux Wayland has no portable cursor-position query, so any
        // estimate-based edge detection eventually drifts and traps the
        // user (the estimate clamps to zero from accumulated leftward
        // drift even when the real cursor is happily centered, then any
        // small leftward push trips REMOTE). For now we rely on the
        // explicit Ctrl+Alt+R hotkey to enter Remote from Linux. M2 Slice
        // 3 will swap in a real Wayland/X11 cursor query and re-enable
        // automatic edge detection.
        let _ = (dx, dy);
        // No forward in LOCAL — OS already moves the cursor.
    } else {
        let peer_w = PEER_W.load(Ordering::Relaxed);
        // For the Ubuntu→Win direction the entry edge is on the *right*
        // of the user's hand motion (they crossed leftward to enter Win),
        // so leftward dx (negative) takes the cursor *deeper* into the
        // peer. Flip the sign so virt_x grows the same way it does in
        // Slice 1 (Win→Ubuntu).
        let raw = VIRT_X.load(Ordering::Relaxed) + (-dx);
        let new_virt_x = raw.clamp(-EXIT_BUFFER_PX, peer_w);
        VIRT_X.store(new_virt_x, Ordering::Relaxed);

        if new_virt_x <= -EXIT_BUFFER_PX {
            exit_remote();
        } else {
            let fdx = dx.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
            let fdy = dy.clamp(-MAX_DELTA_PX, MAX_DELTA_PX);
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
                    "linux sample motion forward"
                );
            }
            let _ = sink.send(InputEvent::MouseMove { dx: fdx, dy: fdy });
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

pub struct UinputInject {
    device: parking_lot::Mutex<VirtualDevice>,
}

impl UinputInject {
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<EvKey>::new();
        for code in 1u16..=255u16 {
            keys.insert(EvKey(code));
        }
        for code in 0x100u16..=0x151u16 {
            keys.insert(EvKey(code));
        }

        let mut rel = AttributeSet::<RelativeAxisCode>::new();
        rel.insert(RelativeAxisCode::REL_X);
        rel.insert(RelativeAxisCode::REL_Y);
        rel.insert(RelativeAxisCode::REL_WHEEL);
        rel.insert(RelativeAxisCode::REL_HWHEEL);

        let mut props = AttributeSet::<PropType>::new();
        props.insert(PropType::POINTER);

        let device = VirtualDevice::builder()
            .context("uinput builder — need /dev/uinput access (group `input`)")?
            .name(VIRTUAL_DEVICE_NAME)
            .with_keys(&keys)?
            .with_relative_axes(&rel)?
            .with_properties(&props)?
            .build()
            .context("create uinput device")?;
        info!(name = VIRTUAL_DEVICE_NAME, "uinput virtual device created");
        Ok(Self {
            device: parking_lot::Mutex::new(device),
        })
    }

    fn emit(&self, ev: &[evdev::InputEvent]) -> Result<()> {
        self.device.lock().emit(ev).context("uinput emit")?;
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
            self.emit(&events)?;
        }
        Ok(())
    }

    fn mouse_button(&self, btn: Button, down: bool) -> Result<()> {
        let key = key_from_button(btn);
        self.emit(&[evdev::InputEvent::new(
            EventType::KEY.0,
            key.0,
            if down { 1 } else { 0 },
        )])
    }

    fn key(&self, code: KeyCode, down: bool) -> Result<()> {
        self.emit(&[evdev::InputEvent::new(
            EventType::KEY.0,
            code.0,
            if down { 1 } else { 0 },
        )])
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
            self.emit(&events)?;
        }
        Ok(())
    }
}
