//! Per-peer, per-stream audio mailboxes bounded by count and capture age.
use mineshare_audio::{CapturedAudio, StreamKind};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{
    Notify,
    broadcast::error::{RecvError, TryRecvError},
};

pub const MAX_AUDIO_AGE: Duration = Duration::from_millis(60);
/// Input stays first in the select, but continuously-ready input cannot keep
/// audio from getting a bounded service opportunity.
pub struct SendBudget {
    inputs: usize,
    serviced: Instant,
}
impl SendBudget {
    pub fn new(now: Instant) -> Self {
        Self {
            inputs: 0,
            serviced: now,
        }
    }
    pub fn input_sent(&mut self) {
        self.inputs += 1;
    }
    pub fn audio_due(&self, now: Instant) -> bool {
        self.inputs >= 8 || now.saturating_duration_since(self.serviced) >= Duration::from_millis(2)
    }
    pub fn audio_serviced(&mut self, now: Instant) {
        self.inputs = 0;
        self.serviced = now;
    }
}
const CAPACITY_PER_STREAM: usize = 3;
#[derive(Default)]
struct State {
    streams: [VecDeque<Arc<CapturedAudio>>; 2],
    next: usize,
    expired: u64,
    replaced: u64,
    high_water: usize,
    max_age_us: u64,
}
#[derive(Default)]
struct Queue {
    state: Mutex<State>,
    wake: Notify,
}
#[derive(Clone, Default)]
pub struct AudioHub {
    subscribers: Arc<Mutex<Vec<Weak<Queue>>>>,
    capture_expired: Arc<AtomicU64>,
}
pub struct AudioSubscription {
    queue: Arc<Queue>,
}

impl AudioHub {
    pub fn note_capture_expired(&self) {
        self.capture_expired.fetch_add(1, Ordering::Relaxed);
    }
    pub fn capture_expired(&self) -> u64 {
        self.capture_expired.load(Ordering::Relaxed)
    }
    pub fn subscribe(&self) -> AudioSubscription {
        let queue = Arc::new(Queue::default());
        let mut subs = self.subscribers.lock();
        subs.retain(|weak| weak.strong_count() != 0);
        subs.push(Arc::downgrade(&queue));
        AudioSubscription { queue }
    }
    pub fn receiver_count(&self) -> usize {
        let mut subs = self.subscribers.lock();
        subs.retain(|weak| weak.strong_count() != 0);
        subs.len()
    }
    pub fn send(&self, frame: CapturedAudio) {
        let stream = usize::from(frame.stream == StreamKind::Mic);
        let frame = Arc::new(frame);
        self.subscribers.lock().retain(|weak| {
            let Some(queue) = weak.upgrade() else {
                return false;
            };
            let mut state = queue.state.lock();
            if state.streams[stream].len() == CAPACITY_PER_STREAM {
                state.streams[stream].pop_front();
                state.replaced += 1;
            }
            state.streams[stream].push_back(frame.clone());
            state.high_water = state
                .high_water
                .max(state.streams.iter().map(VecDeque::len).sum());
            drop(state);
            queue.wake.notify_one();
            true
        });
    }
}
impl AudioSubscription {
    pub fn try_recv(&mut self) -> Result<Arc<CapturedAudio>, TryRecvError> {
        self.try_recv_at(Instant::now())
    }
    fn try_recv_at(&mut self, now: Instant) -> Result<Arc<CapturedAudio>, TryRecvError> {
        let mut state = self.queue.state.lock();
        for stream in 0..2 {
            while state.streams[stream].front().is_some_and(|frame| {
                now.saturating_duration_since(frame.captured_at) > MAX_AUDIO_AGE
            }) {
                state.streams[stream].pop_front();
                state.expired += 1;
            }
        }
        for lane in [state.next, 1 - state.next] {
            if let Some(frame) = state.streams[lane].pop_front() {
                state.next = 1 - lane;
                state.max_age_us = state
                    .max_age_us
                    .max(now.saturating_duration_since(frame.captured_at).as_micros() as u64);
                return Ok(frame);
            }
        }
        Err(TryRecvError::Empty)
    }
    pub async fn recv(&mut self) -> Result<Arc<CapturedAudio>, RecvError> {
        loop {
            let queue = self.queue.clone();
            let notified = queue.wake.notified();
            match self.try_recv() {
                Ok(frame) => return Ok(frame),
                Err(_) => notified.await,
            }
        }
    }
    pub fn diagnostics(&self) -> (usize, u64, u64, u64) {
        let s = self.queue.state.lock();
        (s.high_water, s.expired, s.replaced, s.max_age_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mineshare_audio::AudioFrame;
    #[test]
    fn continuous_input_cannot_starve_audio() {
        let start = Instant::now();
        let mut budget = SendBudget::new(start);
        let mut serviced = 0;
        for _ in 0..1_000 {
            budget.input_sent();
            if budget.audio_due(start) {
                serviced += 1;
                budget.audio_serviced(start)
            }
        }
        assert_eq!(serviced, 125);
        assert!(budget.audio_due(start + Duration::from_millis(2)));
    }
    fn frame(stream: StreamKind, seq: u32, at: Instant) -> CapturedAudio {
        CapturedAudio {
            frame: AudioFrame {
                stream,
                seq,
                opus: vec![0; 100],
            },
            captured_at: at,
        }
    }
    #[test]
    fn a_stalled_sender_never_replays_a_second_of_old_audio() {
        for stall_ms in [20, 50, 100, 160, 1_000] {
            let hub = AudioHub::default();
            let mut sub = hub.subscribe();
            let start = Instant::now();
            for seq in 0..50 {
                hub.send(frame(StreamKind::SysOut, seq, start));
            }
            let result = sub.try_recv_at(start + Duration::from_millis(stall_ms));
            if stall_ms > 60 {
                assert!(result.is_err())
            } else {
                assert!(result.unwrap().seq >= 47)
            }
            assert!(sub.diagnostics().0 <= 3);
        }
    }
    #[test]
    fn both_streams_make_progress_without_displacing_each_other() {
        let hub = AudioHub::default();
        let mut sub = hub.subscribe();
        let now = Instant::now();
        hub.send(frame(StreamKind::Mic, 1, now));
        for seq in 0..50 {
            hub.send(frame(StreamKind::SysOut, seq, now));
        }
        assert_eq!(sub.try_recv_at(now).unwrap().stream, StreamKind::SysOut);
        assert_eq!(sub.try_recv_at(now).unwrap().stream, StreamKind::Mic);
        assert_eq!(sub.diagnostics().2, 47);
    }
}
