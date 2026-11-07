//! Linux receives contact frames as a separate libinput touchpad.
use crate::{MAX_TOUCHPAD_CONTACTS, TouchpadContact, TouchpadEvent};
use anyhow::{Context, Result};
use evdev::{
    AbsInfo, AbsoluteAxisCode as Axis, AttributeSet, InputEvent, KeyCode as Key, PropType,
    UinputAbsSetup, uinput::VirtualDevice,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static AVAILABLE: AtomicBool = AtomicBool::new(false);
const TOOLS: [Key; 5] = [
    Key::BTN_TOOL_FINGER,
    Key::BTN_TOOL_DOUBLETAP,
    Key::BTN_TOOL_TRIPLETAP,
    Key::BTN_TOOL_QUADTAP,
    Key::BTN_TOOL_QUINTTAP,
];

// Start a clear horizontal swipe at about 4.6 mm on the normalized 120 mm
// pad. Keep later detents about 9 mm apart so cycling stays controllable.
const SWITCH_START: i32 = 2_500;
const SWITCH_STEP: i32 = 5_000;
const SWITCH_IDLE: Duration = Duration::from_millis(400);

#[derive(Default)]
pub(super) struct WindowSwipe {
    stream: Option<u32>,
    phase: SwipePhase,
}

#[derive(Default)]
enum SwipePhase {
    #[default]
    Waiting,
    Tracking([TouchpadContact; 3]),
    Native,
    Switching {
        fingers: [u32; 3],
        detent: i32,
        updated: Instant,
    },
    Consumed,
}

pub(super) enum SwipeOutput {
    Ignore,
    Native,
    Replay(TouchpadEvent),
    Hold,
    Step { reverse: bool, count: usize },
    Finish,
}

impl WindowSwipe {
    pub(super) fn expired(&mut self, now: Instant) -> bool {
        if matches!(self.phase, SwipePhase::Switching { updated, .. }
            if now.saturating_duration_since(updated) >= SWITCH_IDLE)
        {
            self.phase = SwipePhase::Consumed;
        }
        matches!(self.phase, SwipePhase::Consumed)
    }

    pub(super) fn update(&mut self, event: TouchpadEvent, now: Instant) -> Result<SwipeOutput> {
        use SwipeOutput::*;
        let TouchpadEvent::Frame {
            stream_id,
            terminal,
            ..
        } = event
        else {
            anyhow::bail!("Linux does not advertise Windows-specific touchpad actions");
        };
        let contacts = event.contacts().context("invalid touchpad contact count")?;
        anyhow::ensure!(
            contacts
                .iter()
                .enumerate()
                .all(|(i, c)| contacts[..i].iter().all(|p| p.id != c.id)),
            "duplicate touchpad contact"
        );
        if terminal {
            if self.stream != Some(stream_id) {
                return Ok(Ignore);
            }
            *self = Self::default();
            return Ok(Finish);
        }
        if self.stream != Some(stream_id) {
            *self = Self {
                stream: Some(stream_id),
                ..Self::default()
            };
        }
        let Ok(three) = <&[TouchpadContact; 3]>::try_from(contacts) else {
            return Ok(match self.phase {
                SwipePhase::Switching { .. } | SwipePhase::Consumed => {
                    self.phase = SwipePhase::Consumed;
                    Finish
                }
                _ => {
                    self.phase = SwipePhase::Waiting;
                    Native
                }
            });
        };
        let x = three.iter().map(|c| i32::from(c.x)).sum::<i32>() / 3;
        match &mut self.phase {
            SwipePhase::Waiting => {
                self.phase = SwipePhase::Tracking(*three);
                Ok(Hold)
            }
            SwipePhase::Tracking(origin) => {
                let mut dx = [0; 3];
                let mut dy = [0; 3];
                for (i, contact) in three.iter().enumerate() {
                    let Some(start) = origin.iter().find(|c| c.id == contact.id) else {
                        *origin = *three;
                        return Ok(Hold);
                    };
                    dx[i] = i32::from(contact.x) - i32::from(start.x);
                    dy[i] = i32::from(contact.y) - i32::from(start.y);
                }
                let horizontal = dx.iter().sum::<i32>() / 3;
                let vertical = dy.iter().sum::<i32>() / 3;
                let distance = horizontal.abs();
                let clear_direction = distance > vertical.abs()
                    && (distance >= SWITCH_STEP || distance * 4 >= vertical.abs() * 5);
                if distance >= SWITCH_START
                    && clear_direction
                    && dx
                        .iter()
                        .all(|d| d * horizontal.signum() >= SWITCH_START / 2)
                {
                    let travel = distance - SWITCH_START;
                    self.phase = SwipePhase::Switching {
                        fingers: three.map(|c| c.id),
                        detent: x - horizontal.signum() * (travel % SWITCH_STEP),
                        updated: now,
                    };
                    Ok(Step {
                        reverse: horizontal < 0,
                        count: (1 + travel / SWITCH_STEP).min(3) as usize,
                    })
                } else if dx.into_iter().chain(dy).any(|d| d.abs() >= SWITCH_STEP) {
                    let first = TouchpadEvent::frame(stream_id, origin, false).unwrap();
                    self.phase = SwipePhase::Native;
                    Ok(Replay(first))
                } else {
                    Ok(Hold)
                }
            }
            SwipePhase::Native => Ok(Native),
            SwipePhase::Switching {
                fingers,
                detent,
                updated,
            } => {
                if !three.iter().all(|c| fingers.contains(&c.id)) {
                    self.phase = SwipePhase::Consumed;
                    return Ok(Finish);
                }
                let steps = (x - *detent) / SWITCH_STEP;
                *updated = now;
                if steps == 0 {
                    return Ok(Hold);
                }
                *detent += steps * SWITCH_STEP;
                Ok(Step {
                    reverse: steps < 0,
                    count: steps.unsigned_abs().min(3) as usize,
                })
            }
            SwipePhase::Consumed => Ok(Hold),
        }
    }

    pub(super) fn switching(&self) -> bool {
        matches!(self.phase, SwipePhase::Switching { .. })
    }
}

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

    fn three_fingers(stream: u32, x: u16, y: u16) -> TouchpadEvent {
        TouchpadEvent::frame(
            stream,
            &[
                TouchpadContact { id: 1, x, y },
                TouchpadContact {
                    id: 2,
                    x: x + 1500,
                    y,
                },
                TouchpadContact {
                    id: 3,
                    x: x + 3000,
                    y,
                },
            ],
            false,
        )
        .unwrap()
    }

    #[test]
    fn short_horizontal_swipe_starts_within_twelve_reports_in_both_directions() {
        for direction in [-1, 1] {
            let mut swipe = WindowSwipe::default();
            let start = Instant::now();
            // 50 mm/s at 125 Hz on the normalized 120 mm pad.
            let first_step = (0..=24).find(|&sample| {
                let x = (24_000 + direction * 218 * sample as i32) as u16;
                let now = start + Duration::from_millis(8 * sample);
                matches!(
                    swipe.update(three_fingers(1, x, 20_000), now).unwrap(),
                    SwipeOutput::Step { reverse, count: 1 } if reverse == (direction < 0)
                )
            });
            eprintln!(
                "direction={direction}, first_step_ms={:?}",
                first_step.map(|n| n * 8)
            );
            assert_eq!(first_step, Some(12));
        }
    }

    #[test]
    fn horizontal_swipe_steps_and_reverses_until_release() {
        use SwipeOutput::*;
        let mut swipe = WindowSwipe::default();
        let now = Instant::now();
        assert!(matches!(
            swipe.update(three_fingers(1, 10000, 12000), now).unwrap(),
            Hold
        ));
        assert!(matches!(
            swipe.update(three_fingers(1, 15800, 12400), now).unwrap(),
            Step {
                reverse: false,
                count: 1
            }
        ));
        assert!(matches!(
            swipe.update(three_fingers(1, 26300, 12400), now).unwrap(),
            Step {
                reverse: false,
                count: 2
            }
        ));
        assert!(matches!(
            swipe.update(three_fingers(1, 16800, 12000), now).unwrap(),
            Step {
                reverse: true,
                count: 1
            }
        ));
        assert!(matches!(
            swipe.update(TouchpadEvent::terminal(2), now).unwrap(),
            Ignore
        ));
        assert!(swipe.switching());
        assert!(matches!(
            swipe.update(TouchpadEvent::terminal(1), now).unwrap(),
            Finish
        ));
        assert!(!swipe.switching());
    }

    #[test]
    fn vertical_swipe_replays_native_origin_and_stays_native() {
        use SwipeOutput::*;
        let mut swipe = WindowSwipe::default();
        let now = Instant::now();
        let origin = three_fingers(1, 12000, 12000);
        assert!(matches!(swipe.update(origin, now).unwrap(), Hold));
        assert!(matches!(
            swipe.update(three_fingers(1, 12100, 18000), now).unwrap(),
            Replay(event) if event == origin
        ));
        assert!(matches!(
            swipe.update(three_fingers(1, 25000, 21000), now).unwrap(),
            Native
        ));
    }

    #[test]
    fn replacing_a_finger_finishes_switching_even_without_a_count_change() {
        let mut swipe = WindowSwipe::default();
        let now = Instant::now();
        swipe.update(three_fingers(1, 12000, 12000), now).unwrap();
        swipe.update(three_fingers(1, 18000, 12000), now).unwrap();
        let frame = three_fingers(1, 24000, 12000);
        let mut contacts = frame.contacts().unwrap().to_vec();
        contacts[0].id = 4;
        let replacement = TouchpadEvent::frame(1, &contacts, false).unwrap();
        assert!(matches!(
            swipe.update(replacement, now).unwrap(),
            SwipeOutput::Finish
        ));
        assert!(!swipe.switching());
    }

    #[test]
    fn lost_terminal_expires_switcher_without_restarting_same_swipe() {
        let mut swipe = WindowSwipe::default();
        let now = Instant::now();
        swipe.update(three_fingers(1, 12000, 12000), now).unwrap();
        swipe.update(three_fingers(1, 18000, 12000), now).unwrap();
        assert!(!swipe.expired(now + SWITCH_IDLE - Duration::from_millis(1)));
        assert!(swipe.expired(now + SWITCH_IDLE));
        assert!(matches!(
            swipe
                .update(three_fingers(1, 26000, 12000), now + SWITCH_IDLE)
                .unwrap(),
            SwipeOutput::Hold
        ));
        swipe.update(three_fingers(2, 12000, 12000), now).unwrap();
        assert!(matches!(
            swipe.update(three_fingers(2, 6000, 12000), now).unwrap(),
            SwipeOutput::Step {
                reverse: true,
                count: 1
            }
        ));
    }

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
