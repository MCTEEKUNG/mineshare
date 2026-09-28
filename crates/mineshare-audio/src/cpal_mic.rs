//! Cross-platform mic capture via the system default input device.
//!
//! Both Linux (PipeWire-backed via cpal/ALSA) and Windows (WASAPI)
//! expose the user's primary microphone as cpal's
//! `default_input_device()` — so unlike sysout (which needs the
//! platform-specific loopback / monitor trick), the mic path is a
//! straight cpal capture on every OS we ship.
//!
//! Format adaptation: real mics rarely run at our canonical 48 kHz
//! stereo. Cheap headsets are mono 16 kHz, USB mics are typically
//! mono 48 kHz, gaming headsets often 16-bit 44.1 kHz. We resample +
//! channel-map to 48 kHz / 2-channel / f32 in the encode loop, same
//! as the WASAPI loopback path.
//!
//! ## Stream tag
//!
//! Frames carry `StreamKind::Mic` so the receiver can route them to
//! a virtual mic input (PipeWire null-sink monitor / VB-CABLE on
//! Windows) instead of mixing them into the sysout speaker path.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver as WakeReceiver, SyncSender as WakeSender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, StreamTrait};
use parking_lot::Mutex;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use tracing::{debug, info, warn};

use crate::codec::OpusEncoder;
use crate::{
    AudioCapture, AudioFrame, BackendState, BackendStatus, CHANNELS, CaptureSink,
    FRAME_SAMPLES_INTERLEAVED, FRAME_SAMPLES_PER_CHANNEL, SAMPLE_RATE, StreamKind,
};

/// Capture-side ring sized for 96 kHz stereo × 5 frames = covers any
/// realistic mic rate up to 96 kHz with comfortable jitter slack.
const CAPTURE_RING_CAP: usize = 96_000 / 50 * 2 * 5;
/// Speech is fine at 48 kbps stereo Opus — we deliberately under-bit
/// the mic stream relative to sysout (96 kbps) since mics carry
/// monaural voice + room noise, not music.
const OPUS_BITRATE_BPS: i32 = 48_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureControl {
    Continue,
    DemandEnded,
    SinkClosed,
    DeviceChanged,
    StreamError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MicFailureAction {
    Exit,
    WaitForDemand,
    Retry,
}

fn mic_failure_action(active: bool, closed: bool) -> MicFailureAction {
    if closed {
        MicFailureAction::Exit
    } else if active {
        MicFailureAction::Retry
    } else {
        MicFailureAction::WaitForDemand
    }
}

fn capture_control(
    active: bool,
    closed: bool,
    device_changed: bool,
    stream_error: bool,
) -> CaptureControl {
    if closed {
        CaptureControl::SinkClosed
    } else if stream_error {
        CaptureControl::StreamError
    } else if device_changed {
        CaptureControl::DeviceChanged
    } else if !active {
        CaptureControl::DemandEnded
    } else {
        CaptureControl::Continue
    }
}

pub struct CpalMic {
    started: bool,
    status: BackendStatus,
}

impl CpalMic {
    pub fn new() -> Result<Self> {
        Ok(Self {
            started: false,
            status: BackendStatus::new(BackendState::Idle),
        })
    }
}

fn start_once<T>(started: &mut bool, spawn: impl FnOnce() -> Result<T>) -> Result<T> {
    if *started {
        return Err(anyhow::anyhow!("mic capture already started"));
    }
    let value = spawn()?;
    *started = true;
    Ok(value)
}

impl AudioCapture for CpalMic {
    fn backend_status(&self) -> BackendStatus {
        self.status.clone()
    }

    fn start(&mut self, sink: CaptureSink) -> Result<()> {
        let status = self.status.clone();
        let _worker = start_once(&mut self.started, move || {
            thread::Builder::new()
                .name("cpal-mic".into())
                .spawn(move || {
                    if let Err(e) = run_capture_thread(sink, status.clone()) {
                        status.set(BackendState::Degraded);
                        warn!(error = %e, "mic capture thread exited");
                    }
                })
                .context("spawn cpal-mic thread")
        })?;
        Ok(())
    }
}

fn run_capture_thread(sink: CaptureSink, status: BackendStatus) -> Result<()> {
    // Stage 8.4: outer loop rebuilds the cpal stream when the user
    // picks a different mic on the Devices tab. The encode-loop
    // exits cleanly when the version bumps, then we re-enter and
    // start over with the new device. A small audible glitch on
    // device switch is fine — manual operation only.
    let mut encoder = OpusEncoder::new(OPUS_BITRATE_BPS, true)?;
    let mut seq: u32 = 0;
    loop {
        while !sink.is_active() {
            status.set(BackendState::Idle);
            if sink.is_closed() {
                status.set(BackendState::Stopped);
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        }
        status.set(BackendState::Starting);
        let attempt = (|| -> Result<EncodeLoopExit> {
            let last_version = crate::input_device_version();
            let device = crate::resolve_input_device()
                .context("no default audio input device — plug in a mic and try again")?;
            let device_name = device.name().unwrap_or_else(|_| "?".to_string());

            let cfg = device
                .default_input_config()
                .context("query default input config")?;
            let in_rate = cfg.sample_rate().0;
            let in_channels = cfg.channels();
            info!(
                device = %device_name,
                sample_rate = in_rate,
                channels = in_channels,
                sample_format = ?cfg.sample_format(),
                "mic capture device picked"
            );

            if in_channels == 0 {
                anyhow::bail!("input device reports 0 channels — driver issue");
            }

            let rb = HeapRb::<f32>::new(CAPTURE_RING_CAP);
            let (producer, consumer) = rb.split();
            let producer = Arc::new(Mutex::new(producer));
            let producer_cb = producer.clone();
            let (samples_ready_tx, samples_ready_rx) = std::sync::mpsc::sync_channel::<()>(1);
            let samples_ready_cb = samples_ready_tx.clone();
            let stream_error = Arc::new(AtomicBool::new(false));
            let stream_error_cb = stream_error.clone();
            let error_ready_cb = samples_ready_tx.clone();
            let err_fn = move |e| {
                warn!(error = %e, "mic stream error");
                stream_error_cb.store(true, Ordering::Release);
                notify_samples_ready(&error_ready_cb);
            };

            let stream = match cfg.sample_format() {
                SampleFormat::F32 => device.build_input_stream(
                    &cfg.config(),
                    move |data: &[f32], _| {
                        let mut p = producer_cb.lock();
                        let written = p.push_slice(data);
                        if written != data.len() {
                            debug!(
                                dropped = data.len() - written,
                                "mic ring full — dropping samples"
                            );
                        }
                        notify_samples_ready(&samples_ready_cb);
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I16 => device.build_input_stream(
                    &cfg.config(),
                    move |data: &[i16], _| {
                        let mut p = producer_cb.lock();
                        for &s in data {
                            let f = s as f32 / i16::MAX as f32;
                            if p.try_push(f).is_err() {
                                break;
                            }
                        }
                        notify_samples_ready(&samples_ready_cb);
                    },
                    err_fn,
                    None,
                ),
                SampleFormat::I32 => device.build_input_stream(
                    &cfg.config(),
                    move |data: &[i32], _| {
                        let mut p = producer_cb.lock();
                        for &s in data {
                            let f = s as f32 / i32::MAX as f32;
                            if p.try_push(f).is_err() {
                                break;
                            }
                        }
                        notify_samples_ready(&samples_ready_cb);
                    },
                    err_fn,
                    None,
                ),
                other => anyhow::bail!("unsupported mic sample format: {other:?}"),
            }
            .context("build mic input stream")?;
            // Only the live cpal callback should keep the notifier connected.
            drop(samples_ready_tx);
            stream.play().context("cpal play (mic)")?;
            status.set(BackendState::Active);
            info!("mic stream started");

            let outcome = drive_encode_loop(
                consumer,
                sink.clone(),
                (in_rate, in_channels),
                &mut encoder,
                &mut seq,
                last_version,
                MicStreamSignals {
                    samples_ready: samples_ready_rx,
                    stream_error,
                },
            );
            drop(stream);
            Ok(outcome)
        })();
        match attempt {
            Ok(EncodeLoopExit::SinkClosed) => {
                status.set(BackendState::Stopped);
                return Ok(());
            }
            Ok(EncodeLoopExit::DemandEnded) => {
                status.set(BackendState::Idle);
                info!("mic demand ended — releasing capture device");
                continue;
            }
            Ok(EncodeLoopExit::DeviceChanged) => {
                status.set(BackendState::Starting);
                info!("input device selection changed — rebuilding mic stream");
                continue;
            }
            Ok(EncodeLoopExit::Error(error)) | Err(error) => {
                status.set(BackendState::Degraded);
                warn!(%error, "mic capture failed closed");
                match mic_failure_action(sink.is_active(), sink.is_closed()) {
                    MicFailureAction::Exit => {
                        status.set(BackendState::Stopped);
                        return Ok(());
                    }
                    MicFailureAction::WaitForDemand => continue,
                    MicFailureAction::Retry => {
                        let deadline = std::time::Instant::now() + Duration::from_secs(1);
                        while sink.is_active()
                            && !sink.is_closed()
                            && std::time::Instant::now() < deadline
                        {
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
            }
        }
    }
}

enum EncodeLoopExit {
    /// The runtime hung up — daemon is shutting down.
    SinkClosed,
    /// The route was disabled; release the hardware stream and wait lazily.
    DemandEnded,
    /// User picked a new mic on the Devices tab.
    DeviceChanged,
    /// Hard stream/callback failure — bubble up to the bounded retry loop.
    Error(anyhow::Error),
}

struct MicStreamSignals {
    samples_ready: WakeReceiver<()>,
    stream_error: Arc<AtomicBool>,
}

fn drive_encode_loop(
    mut consumer: ringbuf::HeapCons<f32>,
    sink: CaptureSink,
    input_format: (u32, u16),
    encoder: &mut OpusEncoder,
    seq: &mut u32,
    last_version: u64,
    signals: MicStreamSignals,
) -> EncodeLoopExit {
    let (in_rate, in_channels) = input_format;
    let in_frames_per_out = ((in_rate as u64 * FRAME_SAMPLES_PER_CHANNEL as u64
        + SAMPLE_RATE as u64 / 2)
        / SAMPLE_RATE as u64) as usize
        + 1;
    let in_samples_per_out = in_frames_per_out * in_channels as usize;

    let mut in_buf = vec![0f32; in_samples_per_out];
    let mut out_buf = vec![0f32; FRAME_SAMPLES_INTERLEAVED];
    let needs_resample = in_rate != SAMPLE_RATE;
    let needs_chmap = in_channels != CHANNELS;
    if needs_resample || needs_chmap {
        info!(
            in_rate,
            in_channels,
            out_rate = SAMPLE_RATE,
            out_channels = CHANNELS,
            "mic capture will resample/channel-map every frame"
        );
    }

    loop {
        while consumer.occupied_len() < in_samples_per_out {
            match capture_control(
                sink.is_active(),
                sink.is_closed(),
                crate::input_device_version() != last_version,
                signals.stream_error.load(Ordering::Acquire),
            ) {
                CaptureControl::Continue => {}
                CaptureControl::DemandEnded => return EncodeLoopExit::DemandEnded,
                CaptureControl::SinkClosed => return EncodeLoopExit::SinkClosed,
                CaptureControl::DeviceChanged => return EncodeLoopExit::DeviceChanged,
                CaptureControl::StreamError => {
                    return EncodeLoopExit::Error(anyhow!("mic stream callback failed"));
                }
            }
            // A bounded timeout preserves prompt device-switch handling
            // without the previous 2 ms busy poll.
            match signals
                .samples_ready
                .recv_timeout(Duration::from_millis(250))
            {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if sink.is_closed() {
                        return EncodeLoopExit::SinkClosed;
                    }
                    return EncodeLoopExit::Error(anyhow!(
                        "mic stream callbacks disconnected while sink remained live"
                    ));
                }
            }
        }
        let n = consumer.pop_slice(&mut in_buf);
        if n < in_samples_per_out {
            continue;
        }
        match capture_control(
            sink.is_active(),
            sink.is_closed(),
            crate::input_device_version() != last_version,
            signals.stream_error.load(Ordering::Acquire),
        ) {
            CaptureControl::Continue => {}
            CaptureControl::DemandEnded => return EncodeLoopExit::DemandEnded,
            CaptureControl::SinkClosed => return EncodeLoopExit::SinkClosed,
            CaptureControl::DeviceChanged => return EncodeLoopExit::DeviceChanged,
            CaptureControl::StreamError => {
                return EncodeLoopExit::Error(anyhow!("mic stream callback failed"));
            }
        }

        crate::resample::resample_and_chmap(
            &in_buf[..n],
            in_channels,
            in_rate,
            &mut out_buf,
            CHANNELS,
            SAMPLE_RATE,
        );

        let opus_bytes = match encoder.encode(&out_buf) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "mic opus encode failed — skipping frame");
                continue;
            }
        };
        let frame = AudioFrame {
            stream: StreamKind::Mic,
            seq: *seq,
            opus: opus_bytes,
        };
        *seq = seq.wrapping_add(1);

        if !sink.send_lossy(frame) {
            info!("mic sink closed — stopping mic capture");
            return EncodeLoopExit::SinkClosed;
        }
    }
}

fn notify_samples_ready(tx: &WakeSender<()>) {
    // Full means a wake token is already pending; disconnected means the
    // encode thread has exited.
    let _ = tx.try_send(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_mic_demand_releases_the_capture_device() {
        assert_eq!(
            capture_control(false, false, false, false),
            CaptureControl::DemandEnded
        );
    }

    #[test]
    fn demanded_mic_failure_retries_instead_of_killing_the_worker() {
        assert_eq!(mic_failure_action(true, false), MicFailureAction::Retry);
    }

    #[test]
    fn live_stream_error_requests_retry_instead_of_shutdown() {
        assert_eq!(
            capture_control(true, false, false, true),
            CaptureControl::StreamError
        );
    }

    #[test]
    fn failed_mic_worker_spawn_does_not_poison_start_state() {
        let mut started = false;
        let result: Result<()> = start_once(&mut started, || Err(anyhow::anyhow!("failed")));
        assert!(result.is_err());
        assert!(!started);
    }
}
