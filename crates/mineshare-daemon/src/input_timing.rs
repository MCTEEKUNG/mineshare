//! Lock-free stage-duration histograms for the input path.
//!
//! Every stage is timed with a single machine's monotonic clock, so no
//! cross-machine clock calibration is needed and RTT/2 is never used as a
//! latency estimate. Hot paths only do one relaxed `fetch_add`; summaries are
//! drained by the existing 5 s queue-health report.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Bucket upper bounds in microseconds. The last bucket is overflow. Edges
/// bracket the decisions we need: sub-millisecond (healthy), around the
/// 1-2 ms mouse interval, and the ~15.6 ms default Windows timer tick.
const EDGES_US: [u64; 12] = [
    100, 250, 500, 1_000, 2_000, 4_000, 8_000, 12_000, 16_000, 32_000, 64_000, 250_000,
];
const BUCKETS: usize = EDGES_US.len() + 1;

pub struct StageHistogram {
    buckets: [AtomicU64; BUCKETS],
    max_us: AtomicU64,
}

impl StageHistogram {
    pub const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; BUCKETS],
            max_us: AtomicU64::new(0),
        }
    }

    pub fn record(&self, elapsed: Duration) {
        let us = elapsed.as_micros().min(u64::MAX as u128) as u64;
        let bucket = EDGES_US
            .iter()
            .position(|edge| us <= *edge)
            .unwrap_or(EDGES_US.len());
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.max_us.fetch_max(us, Ordering::Relaxed);
    }

    /// Drain and summarise everything recorded since the previous call.
    pub fn take(&self) -> StageSummary {
        let mut counts = [0u64; BUCKETS];
        for (count, bucket) in counts.iter_mut().zip(&self.buckets) {
            *count = bucket.swap(0, Ordering::Relaxed);
        }
        let max_us = self.max_us.swap(0, Ordering::Relaxed);
        StageSummary::from_counts(&counts, max_us)
    }
}

impl Default for StageHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Percentiles are bucket upper bounds (`<=`), `max_us` is exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StageSummary {
    pub count: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

impl StageSummary {
    fn from_counts(counts: &[u64; BUCKETS], max_us: u64) -> Self {
        let count: u64 = counts.iter().sum();
        if count == 0 {
            return Self::default();
        }
        let upper = |index: usize| EDGES_US.get(index).copied().unwrap_or(max_us).min(max_us);
        let quantile = |q: f64| {
            let rank = ((count as f64) * q).ceil().max(1.0) as u64;
            let mut seen = 0;
            for (index, bucket) in counts.iter().enumerate() {
                seen += bucket;
                if seen >= rank {
                    return upper(index);
                }
            }
            max_us
        };
        Self {
            count,
            p50_us: quantile(0.50),
            p95_us: quantile(0.95),
            p99_us: quantile(0.99),
            max_us,
        }
    }
}

impl fmt::Display for StageSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.count == 0 {
            return f.write_str("-");
        }
        write!(
            f,
            "n={} p50<={}us p95<={}us p99<={}us max={}us",
            self.count, self.p50_us, self.p95_us, self.p99_us, self.max_us
        )
    }
}

/// Sender: capture-side enqueue → popped by the send loop.
pub static SEND_QUEUE_AGE: StageHistogram = StageHistogram::new();
/// Sender: encode + encrypt + `send_to` for one input frame.
pub static SEND_CALL: StageHistogram = StageHistogram::new();
/// Receiver: UDP receive enqueue → just before OS injection (includes pacing).
pub static INJECT_QUEUE_AGE: StageHistogram = StageHistogram::new();
/// Receiver: duration of the OS injection call itself.
pub static INJECT_CALL: StageHistogram = StageHistogram::new();
/// Receiver: how late the motion pacer woke past its requested deadline.
/// Large values here mean the OS timer is coarser than the mouse interval.
pub static PACER_OVERSHOOT: StageHistogram = StageHistogram::new();

/// One log-ready snapshot of every stage, drained together.
pub struct InputTimingReport {
    pub send_queue_age: StageSummary,
    pub send_call: StageSummary,
    pub inject_queue_age: StageSummary,
    pub inject_call: StageSummary,
    pub pacer_overshoot: StageSummary,
}

impl InputTimingReport {
    pub fn take() -> Self {
        Self {
            send_queue_age: SEND_QUEUE_AGE.take(),
            send_call: SEND_CALL.take(),
            inject_queue_age: INJECT_QUEUE_AGE.take(),
            inject_call: INJECT_CALL.take(),
            pacer_overshoot: PACER_OVERSHOOT.take(),
        }
    }

    pub fn is_empty(&self) -> bool {
        [
            self.send_queue_age,
            self.send_call,
            self.inject_queue_age,
            self.inject_call,
            self.pacer_overshoot,
        ]
        .iter()
        .all(|stage| stage.count == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_histogram_reports_nothing() {
        let h = StageHistogram::new();
        assert_eq!(h.take(), StageSummary::default());
        assert_eq!(StageSummary::default().to_string(), "-");
    }

    #[test]
    fn percentiles_are_bucket_upper_bounds_capped_by_exact_max() {
        let h = StageHistogram::new();
        for _ in 0..98 {
            h.record(Duration::from_micros(80));
        }
        h.record(Duration::from_micros(15_000));
        h.record(Duration::from_micros(15_500));
        let s = h.take();
        assert_eq!(s.count, 100);
        assert_eq!(s.p50_us, 100);
        assert_eq!(s.p95_us, 100);
        assert_eq!(
            s.p99_us, 15_500,
            "p99 bucket edge 16 ms is capped by the real max"
        );
        assert_eq!(s.max_us, 15_500);
    }

    #[test]
    fn take_drains_so_each_report_covers_one_window() {
        let h = StageHistogram::new();
        h.record(Duration::from_millis(3));
        assert_eq!(h.take().count, 1);
        assert_eq!(h.take().count, 0);
    }

    #[test]
    fn overflow_bucket_reports_exact_max() {
        let h = StageHistogram::new();
        h.record(Duration::from_secs(2));
        let s = h.take();
        assert_eq!((s.p50_us, s.max_us), (2_000_000, 2_000_000));
    }
}
