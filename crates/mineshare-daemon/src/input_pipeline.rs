//! Bounded capture ingress. Merge before capacity can discard an input edge.
use mineshare_input::{InputEvent, MAX_INJECT_DELTA_PX};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{
    Notify,
    broadcast::error::{RecvError, TryRecvError},
};

const CAPACITY: usize = 1_024;
const MAX_MERGED: usize = 4;
const MAX_INPUT_AGE: Duration = Duration::from_millis(250);

pub fn merge_motion(a: InputEvent, b: InputEvent) -> Option<InputEvent> {
    match (a, b) {
        (InputEvent::MouseMove { dx, dy }, InputEvent::MouseMove { dx: bx, dy: by }) => {
            let x = dx.checked_add(bx)?;
            let y = dy.checked_add(by)?;
            (x.unsigned_abs() <= MAX_INJECT_DELTA_PX as u32
                && y.unsigned_abs() <= MAX_INJECT_DELTA_PX as u32)
                .then_some(InputEvent::MouseMove { dx: x, dy: y })
        }
        (InputEvent::Touchpad(a), InputEvent::Touchpad(b)) if a.can_coalesce_with(&b) => {
            Some(InputEvent::Touchpad(b))
        }
        _ => None,
    }
}

struct Entry {
    event: InputEvent,
    merged: usize,
    at: Instant,
}
#[derive(Default)]
struct State {
    pending: VecDeque<Entry>,
    failed: bool,
    coalesced: u64,
    high_water: usize,
    max_age_us: u64,
}
#[derive(Default)]
struct Queue {
    state: Mutex<State>,
    wake: Notify,
}
#[derive(Clone, Default)]
pub struct InputHub {
    subscribers: Arc<Mutex<Vec<Weak<Queue>>>>,
}
pub struct InputSubscription {
    queue: Arc<Queue>,
}

impl InputHub {
    pub fn subscribe(&self) -> InputSubscription {
        // Allocate on session setup, never while publishing a hardware edge.
        let queue = Arc::new(Queue {
            state: Mutex::new(State {
                pending: VecDeque::with_capacity(CAPACITY),
                ..State::default()
            }),
            wake: Notify::new(),
        });
        let mut subs = self.subscribers.lock();
        subs.retain(|weak| weak.strong_count() != 0);
        subs.push(Arc::downgrade(&queue));
        InputSubscription { queue }
    }
    pub fn send(&self, event: InputEvent) {
        let event = match event {
            InputEvent::MouseMove { dx, dy } => InputEvent::MouseMove {
                dx: dx.clamp(-MAX_INJECT_DELTA_PX, MAX_INJECT_DELTA_PX),
                dy: dy.clamp(-MAX_INJECT_DELTA_PX, MAX_INJECT_DELTA_PX),
            },
            other => other,
        };
        self.subscribers.lock().retain(|weak| {
            let Some(queue) = weak.upgrade() else {
                return false;
            };
            let mut state = queue.state.lock();
            if state.failed {
                return true;
            }
            if let Some(last) = state.pending.back_mut()
                && last.merged < MAX_MERGED
                && let Some(merged) = merge_motion(last.event, event)
            {
                last.event = merged;
                last.merged += 1;
                state.coalesced += 1;
            } else if state.pending.len() == CAPACITY {
                // Never silently discard a press/release. End the subscriber's
                // session and let its existing teardown release held input.
                state.failed = true;
                state.pending.clear();
            } else {
                state.pending.push_back(Entry {
                    event,
                    merged: 1,
                    at: Instant::now(),
                });
                state.high_water = state.high_water.max(state.pending.len());
            }
            drop(state);
            queue.wake.notify_one();
            true
        });
    }
}

impl InputSubscription {
    pub fn try_recv(&mut self) -> Result<InputEvent, TryRecvError> {
        self.try_recv_at(Instant::now())
    }
    fn try_recv_at(&mut self, now: Instant) -> Result<InputEvent, TryRecvError> {
        let mut state = self.queue.state.lock();
        if state.failed {
            return Err(TryRecvError::Closed);
        }
        match state.pending.pop_front() {
            Some(entry) => {
                if now.saturating_duration_since(entry.at) > MAX_INPUT_AGE {
                    state.failed = true;
                    state.pending.clear();
                    return Err(TryRecvError::Closed);
                }
                let age = now.saturating_duration_since(entry.at);
                state.max_age_us = state.max_age_us.max(age.as_micros() as u64);
                crate::input_timing::SEND_QUEUE_AGE.record(age);
                Ok(entry.event)
            }
            None => Err(TryRecvError::Empty),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.queue.state.lock().pending.is_empty()
    }
    pub async fn recv(&mut self) -> Result<InputEvent, RecvError> {
        loop {
            // notify_one retains a permit if publishing races the await.
            let queue = self.queue.clone();
            let notified = queue.wake.notified();
            match self.try_recv() {
                Ok(event) => return Ok(event),
                Err(TryRecvError::Closed) => return Err(RecvError::Closed),
                Err(_) => notified.await,
            }
        }
    }
    pub fn diagnostics(&self) -> (usize, u64, u64) {
        let state = self.queue.state.lock();
        (state.high_water, state.coalesced, state.max_age_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mineshare_input::KeyCode;
    #[tokio::test]
    async fn a_thousand_moves_and_short_key_tap_survive_a_stalled_subscriber() {
        let hub = InputHub::default();
        let mut sub = hub.subscribe();
        for _ in 0..500 {
            hub.send(InputEvent::MouseMove { dx: 2, dy: -1 });
        }
        hub.send(InputEvent::Key {
            code: KeyCode(30),
            down: true,
        });
        hub.send(InputEvent::Key {
            code: KeyCode(30),
            down: false,
        });
        for _ in 0..500 {
            hub.send(InputEvent::MouseMove { dx: 2, dy: -1 });
        }
        let mut sum = (0, 0);
        let mut keys = Vec::new();
        while let Ok(event) = sub.try_recv() {
            match event {
                InputEvent::MouseMove { dx, dy } => {
                    sum.0 += dx;
                    sum.1 += dy;
                    assert!(dx.abs() <= 200)
                }
                InputEvent::Key { down, .. } => {
                    keys.push((sum.0, down));
                }
                _ => panic!(),
            }
        }
        assert_eq!(sum, (2000, -1000));
        assert_eq!(keys, vec![(1000, true), (1000, false)]);
        assert!(sub.diagnostics().0 <= 252);
    }
    #[test]
    fn merging_never_turns_legal_moves_into_a_clamped_wire_event() {
        assert!(
            merge_motion(
                InputEvent::MouseMove { dx: 150, dy: 0 },
                InputEvent::MouseMove { dx: 150, dy: 0 }
            )
            .is_none()
        );
    }
    #[test]
    fn stale_keyboard_input_ends_the_session_instead_of_typing_late() {
        let hub = InputHub::default();
        let mut sub = hub.subscribe();
        hub.send(InputEvent::Key {
            code: KeyCode(30),
            down: true,
        });
        assert!(matches!(
            sub.try_recv_at(Instant::now() + Duration::from_millis(300)),
            Err(TryRecvError::Closed)
        ));
    }
    #[tokio::test]
    async fn discrete_overload_fails_closed_instead_of_losing_an_arbitrary_edge() {
        let hub = InputHub::default();
        let mut sub = hub.subscribe();
        for i in 0..=CAPACITY {
            hub.send(InputEvent::Key {
                code: KeyCode(30),
                down: i % 2 == 0,
            });
        }
        assert!(matches!(sub.recv().await, Err(RecvError::Closed)));
    }
    #[tokio::test]
    async fn subscribers_are_independent_and_wake_without_polling() {
        let hub = InputHub::default();
        let mut a = hub.subscribe();
        let mut b = hub.subscribe();
        let event = InputEvent::Key {
            code: KeyCode(30),
            down: true,
        };
        let task = tokio::spawn(async move { a.recv().await.unwrap() });
        tokio::task::yield_now().await;
        hub.send(event);
        assert_eq!(task.await.unwrap(), event);
        assert_eq!(b.recv().await.unwrap(), event);
    }

    /// Manual benchmark: `cargo test -p mineshare-daemon --release --lib --
    /// --ignored --nocapture --test-threads=1 hub_wake_latency_bench`.
    /// A paced non-runtime producer publishes 1 kHz jittered motion while a
    /// tokio task awaits `recv()`, as the UDP writer does. Prints raw
    /// microsecond age at dequeue (publish -> task running); does not assert.
    #[test]
    #[ignore = "manual wake-latency benchmark; prints, does not assert"]
    fn hub_wake_latency_bench() {
        use std::sync::atomic::{AtomicU64, Ordering};
        const TOTAL: usize = 20_000;
        for rep in 0..3 {
            // Two workers, like the production transport runtime. HUB_RT=1
            // promotes them through rtkit exactly as the app does.
            let realtime = std::env::var_os("HUB_RT").is_some();
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .on_thread_start(move || {
                    if realtime {
                        #[cfg(target_os = "windows")]
                        let granted = mineshare_audio::register_realtime_thread();
                        #[cfg(not(target_os = "windows"))]
                        let granted =
                            mineshare_input::promote_current_thread_realtime("bench-worker");
                        eprintln!("realtime granted={granted}");
                    }
                })
                .enable_all()
                .build()
                .unwrap();
            // HUB_HOGS=n adds n normal-priority busy threads (a loaded desktop).
            let hogs: usize = std::env::var("HUB_HOGS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let stop_hogs = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let hog_threads: Vec<_> = (0..hogs)
                .map(|_| {
                    let stop = stop_hogs.clone();
                    std::thread::spawn(move || {
                        while !stop.load(Ordering::Relaxed) {
                            std::hint::spin_loop();
                        }
                    })
                })
                .collect();
            let hub = Arc::new(InputHub::default());
            let mut sub = hub.subscribe();
            let t0 = Instant::now();
            let sent: Arc<Vec<AtomicU64>> =
                Arc::new((0..TOTAL).map(|_| AtomicU64::new(0)).collect());
            let ages = rt.block_on(async {
                let sent_c = sent.clone();
                let consumer = tokio::spawn(async move {
                    let mut cum = 0usize;
                    let mut ages = Vec::with_capacity(TOTAL);
                    while cum < TOTAL {
                        match sub.recv().await {
                            Ok(InputEvent::MouseMove { dx, .. }) => {
                                let first = sent_c[cum].load(Ordering::Acquire);
                                cum += dx as usize;
                                let now = t0.elapsed().as_nanos() as u64;
                                ages.push(now.saturating_sub(first) / 1_000);
                            }
                            _ => break,
                        }
                    }
                    ages
                });
                let hub_p = hub.clone();
                let sent_p = sent.clone();
                std::thread::spawn(move || {
                    let start = Instant::now();
                    for i in 0..TOTAL {
                        let at = start
                            + Duration::from_micros(i as u64 * 1_000 + (i as u64 * 7_919) % 300);
                        while Instant::now() < at {
                            std::hint::spin_loop();
                        }
                        sent_p[i].store(t0.elapsed().as_nanos() as u64, Ordering::Release);
                        hub_p.send(InputEvent::MouseMove { dx: 1, dy: 0 });
                    }
                })
                .join()
                .unwrap();
                consumer.await.unwrap()
            });
            stop_hogs.store(true, Ordering::Relaxed);
            for hog in hog_threads {
                hog.join().unwrap();
            }
            let mut v = ages.clone();
            v.sort_unstable();
            let q = |p: usize| v[(v.len() - 1) * p / 100];
            println!(
                "HUB rt={realtime} hogs={hogs} rep={rep} events={} p50={} p90={} p99={} p99.9={} max={}",
                v.len(),
                q(50),
                q(90),
                q(99),
                v[(v.len() - 1) * 999 / 1000],
                v[v.len() - 1]
            );
        }
    }
}
