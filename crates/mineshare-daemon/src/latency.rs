//! Round-trip latency tracking for the encrypted control channel.
//!
//! Stage 8.5 adds a periodic Ping/Pong heartbeat on top of the
//! existing TLS-style ControlMsg pipe — every ~500 ms each side
//! sends a `Ping { ts_ms }` carrying its own monotonic timestamp,
//! the peer echoes it back as `Pong { ts_ms }` verbatim, and the
//! original sender computes `now - ts_ms` to learn the
//! round-trip-time. Because the Pong's `ts_ms` came from *our*
//! clock, RTT calculation needs no clock sync between peers.
//!
//! Samples are kept in a fixed-size ring buffer (last 128 RTTs ≈
//! the past 64 seconds at 2 Hz) so the GUI can render a histogram
//! and surface min / median / p95 without growing forever. The
//! buffer plus snapshot routine all live behind a single mutex —
//! contention is negligible at 2 Hz.

use parking_lot::Mutex;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Number of RTT samples retained for histogram + percentile
/// stats. 128 × 500 ms ≈ 64 s; long enough to smooth network
/// blips, short enough for the GUI to feel "live".
const RING: usize = 128;

/// Bin upper bounds in milliseconds. Anything ≥ the last edge
/// goes into the overflow bin. Bin count == [`HISTOGRAM_BINS`].
const BIN_EDGES_MS: &[f32] = &[2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0];
const HISTOGRAM_BINS: usize = BIN_EDGES_MS.len() + 1;

struct State {
    /// Ring buffer of RTTs in milliseconds; `head` is the next
    /// write position, `len` <= RING tracks fill.
    samples: [f32; RING],
    head: usize,
    len: usize,
}

static STATE: Mutex<State> = Mutex::new(State {
    samples: [0.0; RING],
    head: 0,
    len: 0,
});

/// Wall-clock millisecond timestamp used as the `ts_ms` payload of
/// a Ping. Monotonicity isn't required since we never compare two
/// peers' clocks; we only ever subtract our own past timestamp
/// from our own current one.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Called when a Pong arrives carrying the timestamp we put on the
/// outbound Ping. Folds the new RTT into the rolling buffer.
pub fn record_rtt_ms(rtt_ms: f32) {
    if !rtt_ms.is_finite() || rtt_ms < 0.0 {
        return;
    }
    let mut s = STATE.lock();
    let head = s.head;
    s.samples[head] = rtt_ms;
    s.head = (head + 1) % RING;
    if s.len < RING {
        s.len += 1;
    }
    // Sample the recorded RTT once every ~10 seconds (every 20th
    // 500ms-spaced ping) so users / debuggers can see live values
    // in the daemon log without flooding it. Cheap proxy for "is
    // NODELAY working" — a healthy LAN should be < 5 ms here.
    if s.len % 20 == 0 {
        tracing::info!(rtt_ms = %format!("{:.1}", rtt_ms), "RTT sample");
    }
}

/// Wipe the ring on session boundaries so a healthy reconnect
/// doesn't show p95 dragged up by stale samples from the dropped
/// session.
pub fn reset() {
    let mut s = STATE.lock();
    s.head = 0;
    s.len = 0;
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencySnapshot {
    /// Number of RTT samples currently in the ring (0..=RING).
    pub samples: u32,
    /// Most recently observed RTT.
    pub last_ms: Option<f32>,
    /// Min / mean / percentiles over the ring contents.
    pub min_ms: Option<f32>,
    pub avg_ms: Option<f32>,
    pub p50_ms: Option<f32>,
    pub p95_ms: Option<f32>,
    pub max_ms: Option<f32>,
    /// Counts per histogram bin in left-to-right order.
    pub histogram: Vec<u32>,
    /// Upper bound (in ms) of each bin except the last (overflow).
    pub bin_edges_ms: Vec<f32>,
}

pub fn snapshot() -> LatencySnapshot {
    let s = STATE.lock();
    let n = s.len;
    if n == 0 {
        return LatencySnapshot {
            samples: 0,
            last_ms: None,
            min_ms: None,
            avg_ms: None,
            p50_ms: None,
            p95_ms: None,
            max_ms: None,
            histogram: vec![0; HISTOGRAM_BINS],
            bin_edges_ms: BIN_EDGES_MS.to_vec(),
        };
    }
    // Snapshot in-order copy so the percentile sort doesn't
    // mutate the live ring.
    let mut samples: Vec<f32> = (0..n)
        .map(|i| {
            let idx = (s.head + RING - n + i) % RING;
            s.samples[idx]
        })
        .collect();
    let last_ms = samples.last().copied();
    drop(s);

    let mut hist = vec![0u32; HISTOGRAM_BINS];
    for &v in &samples {
        let bin = BIN_EDGES_MS
            .iter()
            .position(|edge| v < *edge)
            .unwrap_or(BIN_EDGES_MS.len());
        hist[bin] = hist[bin].saturating_add(1);
    }

    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let min_ms = samples.first().copied();
    let max_ms = samples.last().copied();
    let avg_ms = Some(samples.iter().sum::<f32>() / samples.len() as f32);
    let p50_ms = Some(percentile(&samples, 0.50));
    let p95_ms = Some(percentile(&samples, 0.95));

    LatencySnapshot {
        samples: n as u32,
        last_ms,
        min_ms,
        avg_ms,
        p50_ms,
        p95_ms,
        max_ms,
        histogram: hist,
        bin_edges_ms: BIN_EDGES_MS.to_vec(),
    }
}

/// Linear-interpolation percentile on a pre-sorted slice. `p` in
/// [0.0, 1.0]. Returns NaN-free results because the caller has
/// already filtered non-finite samples.
fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = p * (sorted.len() - 1) as f32;
    let lo = pos.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    let frac = pos - lo as f32;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}
