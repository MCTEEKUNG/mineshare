//! Linux input via evdev / uinput.
//!
//! Capture: read raw events from `/dev/input/event*`. Requires the user to
//! be in group `input` (or a udev rule that grants read access). The daemon
//! discovers eligible devices by enumerating and looking for relative axes
//! (mice/touchpads) or KEY_A/KEY_LEFTCTRL (keyboards).
//!
//! Inject: a virtual `uinput` device that the OS treats as a real HID. Needs
//! `/dev/uinput` to be writable by the daemon's user (group `input` is the
//! conventional way; alternatively a udev rule).

use std::path::PathBuf;
use std::thread;

use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, Device, EventSummary, EventType, KeyCode as EvKey, RelativeAxisCode,
    SynchronizationCode,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

use super::{Button, InputCapture, InputEvent, InputInject, KeyCode};

const VIRTUAL_DEVICE_NAME: &str = "MineShare Virtual Input";

pub struct EvdevCapture {
    devices: Vec<(PathBuf, Device)>,
}

impl EvdevCapture {
    pub fn new() -> Result<Self> {
        let mut devices = Vec::new();
        for (path, device) in evdev::enumerate() {
            // Skip our own virtual device to avoid feedback loops.
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
    loop {
        let events = match device.fetch_events() {
            Ok(it) => it,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "evdev fetch_events failed");
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        };
        for ev in events {
            match ev.destructure() {
                EventSummary::RelativeAxis(_, axis, value) => match axis {
                    RelativeAxisCode::REL_X => accum_dx += value,
                    RelativeAxisCode::REL_Y => accum_dy += value,
                    RelativeAxisCode::REL_WHEEL => {
                        let _ = sink.send(InputEvent::Scroll {
                            dx: 0.0,
                            dy: value as f32,
                        });
                    }
                    RelativeAxisCode::REL_HWHEEL => {
                        let _ = sink.send(InputEvent::Scroll {
                            dx: value as f32,
                            dy: 0.0,
                        });
                    }
                    _ => {}
                },
                EventSummary::Key(_, key, value) => {
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
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    if accum_dx != 0 || accum_dy != 0 {
                        let _ = sink.send(InputEvent::MouseMove {
                            dx: accum_dx,
                            dy: accum_dy,
                        });
                        accum_dx = 0;
                        accum_dy = 0;
                    }
                }
                _ => {}
            }
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
        // Advertise the full set of keys + mouse buttons + rel axes that we
        // might need to inject. Over-advertising is harmless.
        let mut keys = AttributeSet::<EvKey>::new();
        for code in 1..=255u16 {
            keys.insert(EvKey(code));
        }
        let mut rel = AttributeSet::<RelativeAxisCode>::new();
        rel.insert(RelativeAxisCode::REL_X);
        rel.insert(RelativeAxisCode::REL_Y);
        rel.insert(RelativeAxisCode::REL_WHEEL);
        rel.insert(RelativeAxisCode::REL_HWHEEL);

        let device = VirtualDevice::builder()
            .context("uinput builder — need /dev/uinput access (group `input`)")?
            .name(VIRTUAL_DEVICE_NAME)
            .with_keys(&keys)?
            .with_relative_axes(&rel)?
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
