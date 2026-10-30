//! Linux receives contact frames as a separate libinput touchpad.
use crate::{MAX_TOUCHPAD_CONTACTS, TouchpadContact, TouchpadEvent};
use anyhow::{Context, Result};
use evdev::{
    AbsInfo, AbsoluteAxisCode as Axis, AttributeSet, InputEvent, KeyCode as Key, PropType,
    UinputAbsSetup, uinput::VirtualDevice,
};
use std::sync::atomic::{AtomicBool, Ordering};

static AVAILABLE: AtomicBool = AtomicBool::new(false);
const TOOLS: [Key; 5] = [
    Key::BTN_TOOL_FINGER,
    Key::BTN_TOOL_DOUBLETAP,
    Key::BTN_TOOL_TRIPLETAP,
    Key::BTN_TOOL_QUADTAP,
    Key::BTN_TOOL_QUINTTAP,
];

pub(super) fn capabilities() -> u32 {
    if AVAILABLE.load(Ordering::Acquire) {
        crate::TOUCHPAD_CAP_FRAME_INJECT
    } else {
        0
    }
}

pub(super) fn scroll_step(delta: f32, residue: &mut f32) -> i32 {
    if !delta.is_finite() {
        return 0;
    }
    let total = delta + *residue;
    let steps = total.trunc() as i32;
    *residue = total - steps as f32;
    steps
}

fn abs(axis: Axis, value: i32) -> InputEvent {
    InputEvent::new(3, axis.0, value)
}
fn key(code: Key, value: bool) -> InputEvent {
    InputEvent::new(1, code.0, i32::from(value))
}

#[derive(Clone, Default)]
struct State {
    stream: Option<u32>,
    slots: [Option<u32>; MAX_TOUCHPAD_CONTACTS],
    tracking: i32,
}

impl State {
    fn frame(
        &mut self,
        stream: u32,
        contacts: &[TouchpadContact],
        terminal: bool,
    ) -> Result<Vec<InputEvent>> {
        anyhow::ensure!(
            contacts.len() <= MAX_TOUCHPAD_CONTACTS && (terminal == contacts.is_empty()),
            "invalid touchpad frame"
        );
        let ids: std::collections::HashSet<_> = contacts.iter().map(|c| c.id).collect();
        anyhow::ensure!(ids.len() == contacts.len(), "duplicate touchpad contact");
        if terminal && self.stream != Some(stream) {
            return Ok(Vec::new());
        }
        let changed = self.stream != Some(stream);
        let mut events = vec![key(Key::BTN_TOUCH, !terminal)];
        for (slot, id) in self.slots.iter_mut().enumerate() {
            if id.is_some_and(|id| changed || !ids.contains(&id)) {
                events.push(abs(Axis::ABS_MT_SLOT, slot as i32));
                events.push(abs(Axis::ABS_MT_TRACKING_ID, -1));
                *id = None;
            }
        }
        for contact in contacts {
            let existing = self.slots.iter().position(|id| *id == Some(contact.id));
            let slot = existing
                .or_else(|| self.slots.iter().position(Option::is_none))
                .context("no free touchpad slot")?;
            events.push(abs(Axis::ABS_MT_SLOT, slot as i32));
            if existing.is_none() {
                self.tracking = if self.tracking == i32::MAX {
                    1
                } else {
                    self.tracking + 1
                };
                self.slots[slot] = Some(contact.id);
                events.push(abs(Axis::ABS_MT_TRACKING_ID, self.tracking));
            }
            events.push(abs(Axis::ABS_MT_POSITION_X, i32::from(contact.x)));
            events.push(abs(Axis::ABS_MT_POSITION_Y, i32::from(contact.y)));
        }
        if let Some(first) = contacts.first() {
            events.push(abs(Axis::ABS_X, i32::from(first.x)));
            events.push(abs(Axis::ABS_Y, i32::from(first.y)));
        }
        for (index, tool) in TOOLS.iter().enumerate() {
            events.push(key(*tool, contacts.len() == index + 1));
        }
        self.stream = (!terminal).then_some(stream);
        Ok(events)
    }
}

pub(super) struct Touchpad {
    device: VirtualDevice,
    state: State,
}

impl Touchpad {
    pub(super) fn new() -> Result<Self> {
        let mut keys = AttributeSet::new();
        for code in TOOLS.into_iter().chain([Key::BTN_TOUCH, Key::BTN_LEFT]) {
            keys.insert(code);
        }
        let mut props = AttributeSet::new();
        props.insert(PropType::POINTER);
        props.insert(PropType::BUTTONPAD);
        let mut builder = VirtualDevice::builder()?
            .name("MineShare Virtual Touchpad")
            .with_keys(&keys)?
            .with_properties(&props)?;
        // 120 x 75 mm pad. Resolution is required for libinput's mm-based gesture thresholds.
        for (axis, max, resolution) in [
            (Axis::ABS_X, 65535, 546),
            (Axis::ABS_Y, 65535, 874),
            (Axis::ABS_MT_POSITION_X, 65535, 546),
            (Axis::ABS_MT_POSITION_Y, 65535, 874),
            (Axis::ABS_MT_SLOT, 4, 0),
            (Axis::ABS_MT_TRACKING_ID, i32::MAX, 0),
        ] {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(
                axis,
                AbsInfo::new(0, 0, max, 0, 0, resolution),
            ))?;
        }
        let device = builder.build().context("create Linux virtual touchpad")?;
        AVAILABLE.store(true, Ordering::Release);
        Ok(Self {
            device,
            state: State::default(),
        })
    }

    pub(super) fn inject(&mut self, event: TouchpadEvent) -> Result<()> {
        let TouchpadEvent::Frame {
            stream_id,
            terminal,
            ..
        } = event
        else {
            anyhow::bail!("Linux does not advertise Windows-specific touchpad actions");
        };
        let contacts = event.contacts().context("invalid touchpad contact count")?;
        let mut next = self.state.clone();
        let events = next.frame(stream_id, contacts, terminal)?;
        if !events.is_empty() {
            self.device.emit(&events)?;
        }
        self.state = next;
        Ok(())
    }

    pub(super) fn release(&mut self) -> Result<()> {
        if let Some(stream) = self.state.stream {
            self.inject(TouchpadEvent::terminal(stream))?;
        }
        Ok(())
    }
}

impl Drop for Touchpad {
    fn drop(&mut self) {
        let _ = self.release();
        AVAILABLE.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_scroll_deltas_are_not_discarded() {
        let mut remainder = 0.0;
        let total: i32 = (0..8).map(|_| scroll_step(0.125, &mut remainder)).sum();
        assert_eq!(total, 1);
        let total: i32 = (0..8).map(|_| scroll_step(-0.125, &mut remainder)).sum();
        assert_eq!(total, -1);
    }

    #[test]
    fn snapshots_repair_missing_contacts_and_ignore_old_terminal() {
        let mut state = State::default();
        let a = TouchpadContact {
            id: 90,
            x: 1000,
            y: 2000,
        };
        let b = TouchpadContact {
            id: 91,
            x: 3000,
            y: 2000,
        };
        state.frame(1, &[a, b], false).unwrap();
        let events = state.frame(1, &[b], false).unwrap();
        assert!(events.contains(&abs(Axis::ABS_MT_TRACKING_ID, -1)));
        assert_eq!(
            state.slots.iter().flatten().copied().collect::<Vec<_>>(),
            vec![91]
        );
        state.frame(2, &[a, b], false).unwrap();
        assert!(state.frame(1, &[], true).unwrap().is_empty());
        assert_eq!(state.stream, Some(2));
        state.frame(2, &[], true).unwrap();
        assert!(state.slots.iter().all(Option::is_none));
    }

    #[test]
    fn invalid_frames_do_not_mutate_contact_state() {
        let mut state = State::default();
        let a = TouchpadContact { id: 1, x: 1, y: 2 };
        assert!(state.frame(1, &[a, a], false).is_err());
        assert!(state.frame(1, &[], false).is_err());
        assert!(state.stream.is_none());
        assert!(state.slots.iter().all(Option::is_none));
    }
}
