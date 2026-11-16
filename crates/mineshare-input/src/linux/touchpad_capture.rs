//! Capture indirect type-B touchpads without relying on a desktop gesture API.
use super::{Button, InputEvent, button_from_key, handle_motion_batch};
use crate::{MAX_TOUCHPAD_CONTACTS, TOUCHPAD_CAP_FRAME_INJECT, TouchpadContact, TouchpadEvent};
use anyhow::{Context, Result};
use evdev::{
    AbsInfo, AbsoluteAxisCode as Axis, Device, EventSummary, KeyCode as Key, PropType,
    SynchronizationCode as Syn, raw_stream::RawDevice,
};
use nix::poll::{PollFd, PollFlags, poll};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tracing::{info, warn};

static ACTIVE_PADS: AtomicU32 = AtomicU32::new(0);
static NEXT_STREAM: AtomicU32 = AtomicU32::new(1);

pub(super) fn available() -> bool {
    ACTIVE_PADS.load(Ordering::Acquire) != 0
}

pub(super) fn is_touchpad(device: &Device) -> bool {
    !device.properties().contains(PropType::DIRECT)
        && !device.properties().contains(PropType::SEMI_MT)
        && device.supported_keys().is_some_and(|keys| {
            keys.contains(Key::BTN_TOUCH) && keys.contains(Key::BTN_TOOL_FINGER)
        })
        && device.supported_absolute_axes().is_some_and(|axes| {
            [
                Axis::ABS_MT_SLOT,
                Axis::ABS_MT_TRACKING_ID,
                Axis::ABS_MT_POSITION_X,
                Axis::ABS_MT_POSITION_Y,
            ]
            .into_iter()
            .all(|axis| axes.contains(axis))
        })
}

#[derive(Clone, Copy, Default)]
struct ContactSlot {
    id: Option<u32>,
    x: Option<i32>,
    y: Option<i32>,
}

struct Contacts {
    slots: Vec<ContactSlot>,
    selected: usize,
    x: AbsInfo,
    y: AbsInfo,
}

impl Contacts {
    fn open(device: &RawDevice) -> Result<Self> {
        let axes: Vec<_> = device.get_absinfo()?.collect();
        let axis = |code| {
            axes.iter()
                .find_map(|&(axis, info)| (axis == code).then_some(info))
                .context("missing touchpad axis")
        };
        let slot = axis(Axis::ABS_MT_SLOT)?;
        let x = axis(Axis::ABS_MT_POSITION_X)?;
        let y = axis(Axis::ABS_MT_POSITION_Y)?;
        anyhow::ensure!(
            slot.minimum() == 0
                && (0..64).contains(&slot.maximum())
                && x.minimum() < x.maximum()
                && y.minimum() < y.maximum(),
            "invalid touchpad axis bounds"
        );
        Ok(Self {
            slots: vec![ContactSlot::default(); (slot.maximum() + 1) as usize],
            selected: slot.value() as usize,
            x,
            y,
        })
    }

    fn update(&mut self, axis: Axis, value: i32) {
        if axis == Axis::ABS_MT_SLOT {
            self.selected = value as usize;
            return;
        }
        let Some(slot) = self.slots.get_mut(self.selected) else {
            return;
        };
        match axis {
            Axis::ABS_MT_TRACKING_ID => {
                // The kernel may omit unchanged X/Y values when reusing a
                // slot. Preserve its last coordinates across contact IDs.
                slot.id = u32::try_from(value).ok();
            }
            Axis::ABS_MT_POSITION_X => slot.x = Some(value),
            Axis::ABS_MT_POSITION_Y => slot.y = Some(value),
            _ => {}
        }
    }

    fn snapshot(&self, output: &mut [TouchpadContact; MAX_TOUCHPAD_CONTACTS]) -> Option<usize> {
        let mut count = 0;
        for slot in &self.slots {
            if let Some(id) = slot.id {
                *output.get_mut(count)? = TouchpadContact {
                    id,
                    x: normalize(slot.x?, self.x),
                    y: normalize(slot.y?, self.y),
                };
                count += 1;
            }
        }
        Some(count)
    }

    fn clear(&mut self) {
        for slot in &mut self.slots {
            slot.id = None;
        }
    }
}

fn normalize(value: i32, axis: AbsInfo) -> u16 {
    let minimum = i64::from(axis.minimum());
    let maximum = i64::from(axis.maximum());
    let position = i64::from(value).clamp(minimum, maximum) - minimum;
    (position * i64::from(u16::MAX) / (maximum - minimum)) as u16
}

#[derive(Default)]
struct Motion {
    previous: Option<TouchpadContact>,
    scroll: Option<[TouchpadContact; 2]>,
    residue: [f32; 2],
    stream: Option<u32>,
}

#[derive(Default)]
struct Tap {
    was_touching: bool,
    candidate: Option<(TouchpadContact, Instant)>,
}

impl Tap {
    fn report(&mut self, touching: bool, contacts: &[TouchpadContact], enabled: bool) -> bool {
        let fresh = touching && !self.was_touching;
        self.was_touching = touching;
        if !enabled {
            self.candidate = None;
            return false;
        }
        if !touching {
            return self
                .candidate
                .take()
                .is_some_and(|(_, start)| start.elapsed() <= Duration::from_millis(180));
        }
        match contacts {
            [contact] if fresh => self.candidate = Some((*contact, Instant::now())),
            [contact] => {
                if self.candidate.is_some_and(|(origin, _)| {
                    origin.id != contact.id
                        || origin.x.abs_diff(contact.x) > 1200
                        || origin.y.abs_diff(contact.y) > 1200
                }) {
                    self.candidate = None;
                }
            }
            _ => self.candidate = None,
        }
        false
    }
}

impl Motion {
    fn finish(&mut self, sink: &Sink) {
        self.scroll = None;
        if let Some(stream) = self.stream.take() {
            // Input datagrams are lossy. Match the Windows sender's
            // redundant terminal frames so a lift survives packet loss.
            for _ in 0..3 {
                sink(InputEvent::Touchpad(TouchpadEvent::terminal(stream)));
            }
        }
    }

    fn report(&mut self, contacts: &[TouchpadContact], remote: bool, native: bool, sink: &Sink) {
        if remote
            && native
            && !contacts.is_empty()
            && (contacts.len() >= 2 || self.stream.is_some())
        {
            let stream = *self
                .stream
                .get_or_insert_with(|| NEXT_STREAM.fetch_add(1, Ordering::Relaxed));
            if let Some(frame) = TouchpadEvent::frame(stream, contacts, false) {
                sink(InputEvent::Touchpad(frame));
            }
            self.previous = None;
            self.residue = [0.0; 2];
            return;
        }

        if self.stream.is_some() {
            self.finish(sink);
        }

        if let [a, b] = contacts {
            let previous = self.scroll.replace([*a, *b]);
            if remote
                && let Some([old_a, old_b]) = previous
                && [old_a.id, old_b.id] == [a.id, b.id]
            {
                let [ax, ay] = displacement(old_a, *a);
                let [bx, by] = displacement(old_b, *b);
                let units = crate::touchpad_scroll_speed() * (20.0 / f32::from(u16::MAX));
                let dx = (ax + bx) * units * if crate::invert_scroll_x() { -1.0 } else { 1.0 };
                let dy = (ay + by) * units * if crate::invert_scroll_y() { 1.0 } else { -1.0 };
                if dx != 0.0 || dy != 0.0 {
                    sink(InputEvent::Scroll { dx, dy });
                }
            }
        } else {
            self.scroll = None;
        }

        let [contact] = contacts else {
            self.previous = None;
            self.residue = [0.0; 2];
            return;
        };
        if let Some(previous) = self.previous.replace(*contact)
            && previous.id == contact.id
        {
            let [dx, dy] = displacement(previous, *contact);
            let dx = pointer_delta(dx, 1200.0, &mut self.residue[0]);
            let dy = pointer_delta(dy, 750.0, &mut self.residue[1]);
            if dx != 0 || dy != 0 {
                crate::bump_local_mouse_activity();
                handle_motion_batch(dx, dy, sink);
            }
        } else {
            self.residue = [0.0; 2];
        }
    }
}

fn displacement(previous: TouchpadContact, current: TouchpadContact) -> [f32; 2] {
    [
        f32::from(current.x) - f32::from(previous.x),
        f32::from(current.y) - f32::from(previous.y),
    ]
}

fn pointer_delta(normalized: f32, scale: f32, residue: &mut f32) -> i32 {
    let total = normalized * (scale / f32::from(u16::MAX)) + *residue;
    let delta = total.trunc();
    *residue = total - delta;
    (delta as i32).clamp(-super::MAX_DELTA_PX, super::MAX_DELTA_PX)
}

type Sink = dyn Fn(InputEvent) + Send + Sync + 'static;

struct ActivePad;

impl ActivePad {
    fn new() -> Self {
        ACTIVE_PADS.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for ActivePad {
    fn drop(&mut self) {
        ACTIVE_PADS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn start(path: PathBuf, sink: Arc<Sink>) -> Result<()> {
    let device = RawDevice::open(&path)?;
    let contacts = Contacts::open(&device)?;
    let keys = device.get_key_state()?;
    let touching = keys.contains(Key::BTN_TOUCH);
    let pressed_buttons = keys.iter().filter_map(button_from_key).collect();
    let active = ActivePad::new();
    std::thread::Builder::new()
        .name(format!("touchpad-{}", path.display()))
        .spawn(move || {
            let _active = active;
            pump(&path, device, contacts, touching, pressed_buttons, &*sink);
        })?;
    Ok(())
}

fn pump(
    path: &Path,
    mut device: RawDevice,
    mut contacts: Contacts,
    mut touching: bool,
    mut pressed_buttons: Vec<Button>,
    sink: &Sink,
) {
    let mut motion = Motion::default();
    let mut tap = Tap::default();
    let mut grabbed = false;
    let mut last_grab_attempt: Option<Instant> = None;
    let mut awaiting_lift = touching;
    let mut dropped = false;
    let mut remote_buttons = Vec::new();
    loop {
        let we_drive = super::local_in_remote() || crate::is_game_driving();
        let want_grab = we_drive || crate::peer_in_remote();
        if !we_drive {
            motion.finish(sink);
            tap.candidate = None;
            release_buttons(&mut remote_buttons, sink);
        }
        if want_grab
            && !grabbed
            // The local desktop must see the release of contacts and
            // buttons it received before the handoff.
            && !touching
            && pressed_buttons.is_empty()
            && last_grab_attempt.is_none_or(|last| last.elapsed() >= Duration::from_secs(1))
        {
            last_grab_attempt = Some(Instant::now());
            match device.grab() {
                Ok(()) => grabbed = true,
                Err(error) => warn!(%error, path = %path.display(), "touchpad grab failed"),
            }
        } else if !want_grab && grabbed {
            // Let the desktop see the next complete contact after ungrab.
            // Releasing during an active touch exposes motion with no DOWN.
            if !touching && pressed_buttons.is_empty() {
                if let Err(error) = device.ungrab() {
                    warn!(%error, path = %path.display(), "touchpad ungrab failed");
                    break;
                }
                grabbed = false;
                last_grab_attempt = None;
            }
        }

        let mut fds = [PollFd::new(device.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, super::GRAB_POLL_MS) {
            Ok(0) | Err(nix::errno::Errno::EINTR) => continue,
            Ok(_) => {}
            Err(error) => {
                warn!(%error, path = %path.display(), "touchpad poll failed");
                break;
            }
        }

        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(error) => {
                info!(%error, path = %path.display(), "touchpad disconnected");
                break;
            }
        };
        let mut resync = false;
        for event in events {
            match event.destructure() {
                EventSummary::Synchronization(_, Syn::SYN_DROPPED, _) => {
                    dropped = true;
                    awaiting_lift = true;
                    contacts.slots.fill(ContactSlot::default());
                    motion.finish(sink);
                    motion = Motion::default();
                    tap = Tap::default();
                    release_buttons(&mut remote_buttons, sink);
                }
                EventSummary::Synchronization(_, Syn::SYN_REPORT, _) if dropped => {
                    dropped = false;
                    resync = true;
                }
                _ if dropped || resync => {}
                EventSummary::AbsoluteAxis(_, axis, value) => contacts.update(axis, value),
                EventSummary::Key(_, Key::BTN_TOUCH, value) => {
                    touching = value != 0;
                    if !touching {
                        awaiting_lift = false;
                        motion.previous = None;
                        motion.residue = [0.0; 2];
                    }
                }
                EventSummary::Key(_, key, value) if value == 0 || value == 1 => {
                    if let Some(button) = button_from_key(key) {
                        let down = value == 1;
                        tap.candidate = None;
                        if down {
                            crate::bump_local_click();
                            if !pressed_buttons.contains(&button) {
                                pressed_buttons.push(button);
                            }
                        } else {
                            pressed_buttons.retain(|&held| held != button);
                        }
                        let we_drive = super::local_in_remote() || crate::is_game_driving();
                        let should_route = if down {
                            we_drive && grabbed
                        } else {
                            remote_buttons.contains(&button)
                        };
                        if should_route && crate::route_mouse_button(button, down, we_drive) {
                            if down && !remote_buttons.contains(&button) {
                                remote_buttons.push(button);
                            } else if !down {
                                remote_buttons.retain(|&held| held != button);
                            }
                            sink(InputEvent::MouseButton { btn: button, down });
                        }
                    }
                }
                EventSummary::Synchronization(_, Syn::SYN_REPORT, _) => {
                    let we_drive = super::local_in_remote() || crate::is_game_driving();
                    let mut snapshot = [TouchpadContact::default(); MAX_TOUCHPAD_CONTACTS];
                    let count = if touching && !awaiting_lift {
                        contacts.snapshot(&mut snapshot).unwrap_or(0)
                    } else {
                        if !touching {
                            contacts.clear();
                        }
                        0
                    };
                    // Local one-finger motion still feeds the edge detector.
                    // A failed exclusive grab must never mirror a gesture to
                    // both desktops.
                    if we_drive == grabbed || (!we_drive && crate::peer_in_remote()) {
                        let native =
                            crate::peer_touchpad_capabilities() & TOUCHPAD_CAP_FRAME_INJECT != 0;
                        motion.report(&snapshot[..count], we_drive, native, sink);
                    }
                    if tap.report(
                        touching,
                        &snapshot[..count],
                        we_drive && grabbed && pressed_buttons.is_empty() && !awaiting_lift,
                    ) && !crate::forwarded_input_state().button_is_down(Button::Left)
                    {
                        crate::bump_local_click();
                        for down in [true, false] {
                            if crate::route_mouse_button(Button::Left, down, true) {
                                sink(InputEvent::MouseButton {
                                    btn: Button::Left,
                                    down,
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if resync {
            // Missing MT slot updates cannot be reconstructed from scalar
            // evdev state. Wait for all contacts to lift before resuming.
            let Ok(keys) = device.get_key_state() else {
                break;
            };
            touching = keys.contains(Key::BTN_TOUCH);
            pressed_buttons = keys.iter().filter_map(button_from_key).collect();
            let Ok(axes) = device.get_abs_state() else {
                break;
            };
            contacts.selected = axes[usize::from(Axis::ABS_MT_SLOT.0)].value as usize;
            awaiting_lift = touching;
        }
    }
    motion.finish(sink);
    release_buttons(&mut remote_buttons, sink);
}

fn release_buttons(buttons: &mut Vec<Button>, sink: &Sink) {
    for button in buttons.drain(..) {
        if crate::route_mouse_button(button, false, false) {
            sink(InputEvent::MouseButton {
                btn: button,
                down: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_ranges_normalize_independently() {
        let axis = AbsInfo::new(0, 100, 4100, 0, 0, 0);
        assert_eq!(normalize(0, axis), 0);
        assert_eq!(normalize(2100, axis), 32767);
        assert_eq!(normalize(5000, axis), u16::MAX);
    }

    #[test]
    fn slots_reuse_coordinates_and_release_tracking_ids() {
        let axis = AbsInfo::new(0, 0, 100, 0, 0, 0);
        let mut contacts = Contacts {
            slots: vec![ContactSlot::default(); 3],
            selected: 0,
            x: axis,
            y: axis,
        };
        let mut snapshot = [TouchpadContact::default(); MAX_TOUCHPAD_CONTACTS];
        for id in 0..3 {
            contacts.update(Axis::ABS_MT_SLOT, id);
            contacts.update(Axis::ABS_MT_TRACKING_ID, id + 10);
            contacts.update(Axis::ABS_MT_POSITION_X, id * 30);
            contacts.update(Axis::ABS_MT_POSITION_Y, 50);
        }
        assert_eq!(contacts.snapshot(&mut snapshot), Some(3));
        contacts.update(Axis::ABS_MT_TRACKING_ID, -1);
        assert_eq!(contacts.snapshot(&mut snapshot), Some(2));
        contacts.update(Axis::ABS_MT_TRACKING_ID, 20);
        assert_eq!(contacts.snapshot(&mut snapshot), Some(3));
        assert_eq!(
            snapshot[2],
            TouchpadContact {
                id: 20,
                x: 39321,
                y: 32767
            }
        );
    }

    #[test]
    fn only_a_fresh_stationary_single_contact_becomes_a_tap() {
        let mut tap = Tap::default();
        let contact = TouchpadContact {
            id: 1,
            x: 1000,
            y: 1000,
        };
        let second = TouchpadContact { id: 2, ..contact };
        assert!(!tap.report(true, &[contact], true));
        assert!(tap.report(false, &[], true));
        assert!(!tap.report(true, &[contact], true));
        assert!(!tap.report(true, &[contact, second], true));
        assert!(!tap.report(true, &[contact], true));
        assert!(!tap.report(false, &[], true));
        assert!(!tap.report(true, &[contact], true));
        assert!(!tap.report(true, &[TouchpadContact { x: 4000, ..contact }], true));
        assert!(!tap.report(false, &[], true));
        assert!(!tap.report(true, &[contact], false));
        assert!(!tap.report(true, &[contact], true));
        assert!(!tap.report(false, &[], true));
    }

    #[test]
    fn native_contact_stream_preserves_origin_and_repeats_lift() {
        let mut motion = Motion::default();
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let events_for_sink = events.clone();
        let sink = move |event| events_for_sink.lock().push(event);
        let origin = [
            TouchpadContact {
                id: 10,
                x: 10000,
                y: 1000,
            },
            TouchpadContact {
                id: 11,
                x: 20000,
                y: 1000,
            },
            TouchpadContact {
                id: 12,
                x: 30000,
                y: 1000,
            },
        ];
        motion.report(&origin, true, true, &sink);
        let stream = motion.stream.unwrap();
        let moved = origin.map(|contact| TouchpadContact {
            x: contact.x + 10000,
            ..contact
        });
        motion.report(&moved, true, true, &sink);
        motion.report(&moved[..1], true, true, &sink);
        motion.report(&[], true, true, &sink);
        assert_eq!(motion.stream, None);
        let events = events.lock();
        assert_eq!(events.len(), 6);
        for (event, contacts) in events.iter().zip([&origin[..], &moved[..], &moved[..1]]) {
            assert_eq!(
                *event,
                InputEvent::Touchpad(TouchpadEvent::frame(stream, contacts, false).unwrap())
            );
        }
        assert!(
            events[3..]
                .iter()
                .all(|event| { *event == InputEvent::Touchpad(TouchpadEvent::terminal(stream)) })
        );
    }
}
