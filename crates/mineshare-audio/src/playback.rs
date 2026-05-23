//! `cpal` playback sink — receives Opus frames, decodes, pushes into
//! a ring buffer, and a `cpal` output stream pulls samples from it on
//! the audio device's callback thread.
//!
//! On Windows `cpal::Stream` is `!Send` (it holds a COM pointer), so
//! the stream lives on a dedicated OS thread and the public handle
//! holds only the `Sender` half of an mpsc channel — that part *is*
//! `Send + Sync` and so satisfies our [`AudioPlayback`] trait bound.
//!
//! Underrun handling: if the ring buffer empties before more frames
//! arrive, the callback fills with silence (zeros). Better than
//! introducing latency by blocking the device.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use tracing::{debug, info, warn};

use crate::codec::OpusDecoder;
use crate::{AudioFrame, AudioPlayback, CHANNELS, FRAME_SAMPLES_INTERLEAVED, SAMPLE_RATE};

/// Ring-buffer capacity in interleaved samples. ~10 frames @ 20 ms =
/// 200 ms. Generous enough to absorb network jitter, tight enough not
/// to feel laggy.
const RING_CAPACITY: usize = FRAME_SAMPLES_INTERLEAVED * 10;

pub struct CpalPlayback {
    tx: mpsc::Sender<AudioFrame>,
}

impl CpalPlayback {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::channel::<AudioFrame>();
        // Asynchronous bring-up: the playback thread spawns and
        // begins building the cpal stream in the background. We
        // do NOT block on a readiness signal here — Win laptops
        // with slow audio drivers (Cirrus Logic SoundWire,
        // Realtek HDA on first wake, USB DACs) can take 3-10 s
        // for `Device::build_output_stream` to return, and the
        // pre-Stage-10 synchronous wait would silently fall back
        // to NullPlayback whenever it hit that ceiling, leaving
        // the user with no audio for the rest of the session.
        //
        // The thread runs an outer build/run loop instead: if
        // build fails it logs and retries every few seconds, so
        // a hot-plugged device or a slow driver eventually wakes
        // up and audio "appears" without restarting the daemon.
        thread::Builder::new()
            .name("cpal-playback".into())
            .spawn(move || run_playback_thread(rx))
            .context("spawn cpal-playback thread")?;
        Ok(Self { tx })
    }
}

impl AudioPlayback for CpalPlayback {
    fn enqueue(&self, frame: AudioFrame) -> Result<()> {
        // mpsc::Sender::send only fails if the receiver is dropped,
        // which means the playback thread is gone — surface that as
        // an error so the caller stops trying.
        self.tx
            .send(frame)
            .map_err(|_| anyhow::anyhow!("cpal-playback thread terminated"))
    }
}

fn run_playback_thread(rx: mpsc::Receiver<AudioFrame>) {
    let mut decoder = match OpusDecoder::new() {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, "opus decoder init failed inside playback thread");
            return;
        }
    };
    let mut scratch = vec![0f32; FRAME_SAMPLES_INTERLEAVED];

    // Lazy stream: starts as `None`, builds on the first iteration
    // (or after a version bump). Retry cadence is bounded so a
    // permanently-broken device doesn't hot-loop calling cpal.
    let mut stream_ctx: Option<StreamCtx> = None;
    let mut last_version = crate::output_device_version();
    let mut next_build_attempt = Instant::now();
    const RETRY_BACKOFF: Duration = Duration::from_secs(3);
    let mut dropped_frames_since_warn: u64 = 0;
    let mut last_drop_warn = Instant::now();

    // Device-loss watchdog state. The cpal data callback bumps
    // `callback_ticks` every time the device pulls samples; if frames
    // keep arriving from the peer but the tick stops advancing, the
    // OS audio callback has silently stopped (device unplugged, default
    // switched, SoundWire/Bluetooth dropout) WITHOUT firing `err_fn`.
    // We detect that and force a rebuild — `resolve_output_device()`
    // then re-picks whatever the current default/selection is.
    const WATCHDOG_INTERVAL: Duration = Duration::from_millis(500);
    let mut last_watchdog = Instant::now();
    let mut last_tick_snapshot: u64 = 0;
    let mut frames_since_watchdog: u64 = 0;

    loop {
        // (Re)build the stream when we need one:
        //   * none yet (startup or a previous build failed), OR
        //   * the user picked a different device on the GUI tab, OR
        //   * the cpal error callback fired (device error), OR
        //   * the device-loss watchdog flagged a stalled callback.
        let v = crate::output_device_version();
        let errored = stream_ctx
            .as_ref()
            .is_some_and(|c| c.stream_error.load(Ordering::Acquire));
        let want_rebuild = stream_ctx.is_none() || v != last_version || errored;
        if want_rebuild && Instant::now() >= next_build_attempt {
            if errored {
                warn!("cpal playback stream errored — rebuilding");
            }
            // Drop any old stream first so the device handle is
            // released before we ask cpal for it back — some
            // Windows drivers refuse exclusive re-acquire
            // otherwise.
            stream_ctx = None;
            match build_stream() {
                Ok(s) => {
                    last_tick_snapshot = s.callback_ticks.load(Ordering::Relaxed);
                    stream_ctx = Some(s);
                    last_version = v;
                    last_watchdog = Instant::now();
                    frames_since_watchdog = 0;
                    info!("cpal playback ready");
                }
                Err(e) => {
                    warn!(error = %e, "cpal playback build failed — retrying in 3 s");
                    last_version = v; // don't busy-retry on the same version
                    next_build_attempt = Instant::now() + RETRY_BACKOFF;
                }
            }
        }

        // Drain frames with a short timeout so the next iteration
        // can notice version bumps and retry timers without
        // waiting on a frame that may not come (silent stream
        // periods, peer paused playback, etc.).
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => {
                let n = match decoder.decode(&frame.opus, &mut scratch) {
                    Ok(n) => n,
                    Err(e) => {
                        warn!(error = %e, "opus decode failed — dropping frame");
                        continue;
                    }
                };
                if let Some(ctx) = stream_ctx.as_mut() {
                    let pushed = ctx.producer.push_slice(&scratch[..n]);
                    frames_since_watchdog += 1;
                    if pushed != n {
                        warn!(
                            dropped = n - pushed,
                            "cpal playback ring full — dropping samples"
                        );
                    }

                    // Device-loss watchdog: if frames have been arriving
                    // for a full interval but the callback tick hasn't
                    // moved, the device callback is dead — tear the
                    // stream down so the top of the loop rebuilds it
                    // against the current default device.
                    if last_watchdog.elapsed() >= WATCHDOG_INTERVAL {
                        let tick_now = ctx.callback_ticks.load(Ordering::Relaxed);
                        if frames_since_watchdog > 0 && tick_now == last_tick_snapshot {
                            warn!(
                                "cpal playback callback stalled \
                                 (device removed / default changed) — rebuilding"
                            );
                            stream_ctx = None;
                            next_build_attempt = Instant::now();
                            last_watchdog = Instant::now();
                            frames_since_watchdog = 0;
                            continue;
                        }
                        last_tick_snapshot = tick_now;
                        last_watchdog = Instant::now();
                        frames_since_watchdog = 0;
                    }
                } else {
                    // Stream not built yet (or last build failed).
                    // Drop the frame; we'd rather lose samples
                    // than block the recv loop. Surface a
                    // throttled warning so the user sees that
                    // audio is arriving but cpal isn't ready.
                    dropped_frames_since_warn += 1;
                    if last_drop_warn.elapsed() >= Duration::from_secs(2) {
                        warn!(
                            count = dropped_frames_since_warn,
                            "cpal stream not built yet — dropping incoming audio"
                        );
                        dropped_frames_since_warn = 0;
                        last_drop_warn = std::time::Instant::now();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // No new frame — fall through to top of loop for
                // the version-check / retry-timer.
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    drop(stream_ctx);
    info!("cpal playback thread exiting");
}

/// Wraps the cpal stream with the producer-half of the ring buffer
/// the decoded-frame loop pushes into. Both fields stay on the
/// playback thread — `Stream` is `!Send` on Windows. The
/// `stream` field is held only for its drop semantics (cpal stops
/// the audio device when it goes out of scope), so the compiler
/// can't see it being read; the `#[allow]` keeps the warning
/// quiet without disabling dead-code lints elsewhere.
struct StreamCtx {
    #[allow(dead_code)]
    stream: cpal::Stream,
    producer: ringbuf::HeapProd<f32>,
    /// Bumped by the cpal data callback every time the device pulls
    /// samples. The playback thread's watchdog samples this to tell a
    /// live device callback from one that has silently stopped.
    callback_ticks: Arc<AtomicU64>,
    /// Set by the cpal error callback when the OS reports a stream
    /// error (device disconnected, format change, exclusive grab).
    stream_error: Arc<AtomicBool>,
}

fn build_stream() -> Result<StreamCtx> {
    // Honour the user's runtime device pick (Stage 8.4); falls
    // back to the OS default when no selection is set or the
    // selected device has been unplugged since.
    let device =
        crate::resolve_output_device().context("no audio output device available (default or selected)")?;
    let device_name = device.name().unwrap_or_else(|_| "?".to_string());

    let config = pick_config(&device)?;
    info!(
        device = %device_name,
        sample_rate = config.sample_rate().0,
        channels = config.channels(),
        sample_format = ?config.sample_format(),
        "cpal playback device picked"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY);
    let (producer, mut consumer) = rb.split();

    let callback_ticks = Arc::new(AtomicU64::new(0));
    let stream_error = Arc::new(AtomicBool::new(false));

    let err_flag = stream_error.clone();
    let err_fn = move |e| {
        warn!(error = %e, "cpal playback stream error");
        err_flag.store(true, Ordering::Release);
    };

    let ticks_f32 = callback_ticks.clone();
    let ticks_i16 = callback_ticks.clone();
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_output_stream(
            &config.config(),
            move |out: &mut [f32], _| {
                ticks_f32.fetch_add(1, Ordering::Relaxed);
                fill_callback(out, &mut consumer);
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => {
            let mut tmp = vec![0f32; 0];
            device.build_output_stream(
                &config.config(),
                move |out: &mut [i16], _| {
                    ticks_i16.fetch_add(1, Ordering::Relaxed);
                    tmp.resize(out.len(), 0.0);
                    fill_callback(&mut tmp, &mut consumer);
                    for (dst, &src) in out.iter_mut().zip(tmp.iter()) {
                        *dst = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    }
                },
                err_fn,
                None,
            )
        }
        other => anyhow::bail!("unsupported cpal sample format: {other:?}"),
    }
    .context("build cpal output stream")?;
    stream.play().context("cpal play")?;

    Ok(StreamCtx {
        stream,
        producer,
        callback_ticks,
        stream_error,
    })
}

fn fill_callback(out: &mut [f32], consumer: &mut ringbuf::HeapCons<f32>) {
    let popped = consumer.pop_slice(out);
    for s in &mut out[popped..] {
        *s = 0.0;
    }
    if popped < out.len() {
        debug!(
            underrun = out.len() - popped,
            "cpal playback ring underrun (filled with silence)"
        );
    }
}

fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    // Prefer 48 kHz / 2-channel / f32 — the canonical bridge format.
    // Fall back to whatever the device's default is if we can't get
    // an exact match.
    let supported = device
        .supported_output_configs()
        .context("query supported output configs")?
        .collect::<Vec<_>>();

    let target_rate = cpal::SampleRate(SAMPLE_RATE);
    if let Some(matched) = supported.iter().find(|c| {
        c.channels() == CHANNELS
            && c.min_sample_rate() <= target_rate
            && target_rate <= c.max_sample_rate()
            && c.sample_format() == SampleFormat::F32
    }) {
        return Ok(matched.clone().with_sample_rate(target_rate));
    }
    if let Some(matched) = supported.iter().find(|c| {
        c.channels() == CHANNELS
            && c.min_sample_rate() <= target_rate
            && target_rate <= c.max_sample_rate()
    }) {
        return Ok(matched.clone().with_sample_rate(target_rate));
    }
    let default = device
        .default_output_config()
        .context("no default output config")?;
    warn!(
        rate = default.sample_rate().0,
        channels = default.channels(),
        format = ?default.sample_format(),
        "no exact match for 48 kHz stereo f32 — falling back to device default"
    );
    Ok(default)
}
