//! Windows side of the **virtual mic** sink — VB-CABLE.
//!
//! VB-CABLE is a free third-party virtual audio device that exposes a
//! pair of WDM endpoints:
//!   * `CABLE Input` — appears as a *playback* device. We render the
//!     peer's mic frames into it.
//!   * `CABLE Output` — the matching *capture* device. Apps like
//!     Discord / Zoom / OBS pick it as their microphone, and what
//!     they "hear" is whatever we wrote into `CABLE Input`.
//!
//! VB-CABLE is donationware (https://vb-audio.com/Cable/) and is not
//! redistributable inside our installer, so the daemon detects it at
//! runtime and surfaces a clear log line with the install URL when
//! it's missing. The bridge keeps working without it — only the
//! "peer's mic shows up in my apps as a mic device" feature is
//! disabled until the user installs VB-CABLE separately.
//!
//! Detection: enumerate cpal's WASAPI output devices and match the
//! one whose name contains `"CABLE Input"` (case-insensitive). The
//! device's friendly name is the user-visible one in Sound Settings,
//! so the match is stable across VB-CABLE versions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use tracing::{debug, info, warn};

use crate::codec::OpusDecoder;
use crate::{
    AudioFrame, AudioPlayback, BackendState, BackendStatus, CHANNELS, FRAME_SAMPLES_INTERLEAVED,
    PlaybackSession, SAMPLE_RATE,
};

const RING_CAPACITY: usize = FRAME_SAMPLES_INTERLEAVED * 10;
const VB_QUEUE_CAPACITY: usize = 8;
const RETRY_BACKOFF: Duration = Duration::from_secs(3);
const IDLE_RELEASE: Duration = Duration::from_secs(2);
const CALLBACK_WATCHDOG_INTERVAL: Duration = Duration::from_millis(500);

fn vb_idle_release_due(last_frame_at: Option<Instant>, now: Instant) -> bool {
    last_frame_at.is_some_and(|last| now.saturating_duration_since(last) >= IDLE_RELEASE)
}

fn transition_build_due(
    observed_epoch: u64,
    current_epoch: u64,
    retry_epoch: u64,
    next_build_attempt: Instant,
    now: Instant,
) -> bool {
    observed_epoch != current_epoch && (retry_epoch != current_epoch || now >= next_build_attempt)
}

fn report_worker_disconnect(failure_reported: &AtomicBool) -> Result<()> {
    if failure_reported.swap(true, Ordering::AcqRel) {
        Ok(())
    } else {
        Err(anyhow::anyhow!("vb-cable-playback thread terminated"))
    }
}

fn retire_vb_session(session_epoch: &AtomicU64, session: &PlaybackSession) -> bool {
    let current = session.epoch();
    session_epoch
        .compare_exchange(
            current,
            current.wrapping_add(1),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn session_is_current(current: &PlaybackSession, candidate: &PlaybackSession) -> bool {
    current.same_identity(candidate)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamDirective {
    Wait,
    Build(u64),
    Keep,
    Drop,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VbStreamHealthAction {
    Keep,
    Rebuild,
}

fn vb_stream_health_action(stream_error: bool) -> VbStreamHealthAction {
    if stream_error {
        VbStreamHealthAction::Rebuild
    } else {
        VbStreamHealthAction::Keep
    }
}

fn vb_callback_stalled(
    inbound_frames: u64,
    previous_tick: u64,
    current_tick: u64,
    elapsed: Duration,
) -> bool {
    inbound_frames > 0 && elapsed >= CALLBACK_WATCHDOG_INTERVAL && previous_tick == current_tick
}

fn stream_directive(
    observed_epoch: Option<u64>,
    current: Option<&PlaybackSession>,
    queued: Option<&PlaybackSession>,
) -> StreamDirective {
    let Some(current) = current else {
        return if observed_epoch.is_some() {
            StreamDirective::Drop
        } else {
            StreamDirective::Wait
        };
    };
    let Some(queued) = queued else {
        return match observed_epoch {
            Some(epoch) if epoch == current.epoch() => StreamDirective::Keep,
            Some(_) => StreamDirective::Drop,
            None => StreamDirective::Wait,
        };
    };
    if !session_is_current(current, queued) {
        return StreamDirective::Reject;
    }
    if observed_epoch == Some(current.epoch()) {
        StreamDirective::Keep
    } else {
        StreamDirective::Build(current.epoch())
    }
}

pub struct VbCablePlayback {
    tx: mpsc::SyncSender<QueuedAudioFrame>,
    session_epoch: Arc<AtomicU64>,
    current_session: Arc<Mutex<Option<PlaybackSession>>>,
    worker_failure_reported: AtomicBool,
    status: BackendStatus,
}

struct QueuedAudioFrame {
    session: PlaybackSession,
    frame: AudioFrame,
}

impl VbCablePlayback {
    pub fn new() -> Result<Self> {
        let device = find_cable_input_device().with_context(|| {
            "VB-CABLE not detected — install from https://vb-audio.com/Cable/ \
             then restart the daemon. Without it, peer mic frames arrive \
             but apps on this machine can't pick them up as a mic input."
        })?;

        let (tx, rx) = mpsc::sync_channel::<QueuedAudioFrame>(VB_QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let session_epoch = Arc::new(AtomicU64::new(0));
        let epoch_for_thread = session_epoch.clone();
        let current_session = Arc::new(Mutex::new(None));
        let current_for_thread = current_session.clone();
        let status = BackendStatus::new(BackendState::Idle);
        let status_for_thread = status.clone();

        thread::Builder::new()
            .name("vb-cable-playback".into())
            .spawn(move || {
                run_playback_thread(
                    device,
                    rx,
                    ready_tx,
                    epoch_for_thread,
                    current_for_thread,
                    status_for_thread,
                );
            })
            .context("spawn vb-cable-playback thread")?;

        ready_rx
            .recv_timeout(Duration::from_secs(3))
            .context("vb-cable-playback thread did not signal readiness")??;
        Ok(Self {
            tx,
            session_epoch,
            current_session,
            worker_failure_reported: AtomicBool::new(false),
            status,
        })
    }
}

impl AudioPlayback for VbCablePlayback {
    fn backend_status(&self) -> BackendStatus {
        self.status.clone()
    }

    fn begin_session(&self) -> PlaybackSession {
        let epoch = self.session_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let session = PlaybackSession::new(epoch);
        *self.current_session.lock() = Some(session.clone());
        session
    }

    fn end_session(&self, session: &PlaybackSession) {
        let mut current = self.current_session.lock();
        if current
            .as_ref()
            .is_some_and(|candidate| session_is_current(candidate, session))
        {
            retire_vb_session(&self.session_epoch, session);
            *current = None;
        }
    }

    fn enqueue(&self, session: &PlaybackSession, frame: AudioFrame) -> Result<()> {
        if self.worker_failure_reported.load(Ordering::Acquire) {
            return Ok(());
        }
        let current = self.current_session.lock();
        if !current
            .as_ref()
            .is_some_and(|candidate| session_is_current(candidate, session))
        {
            return Ok(());
        }
        drop(current);
        match self.tx.try_send(QueuedAudioFrame {
            session: session.clone(),
            frame,
        }) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => Ok(()),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                report_worker_disconnect(&self.worker_failure_reported)
            }
        }
    }
}

fn find_cable_input_device() -> Option<cpal::Device> {
    let host = cpal::default_host();
    let mut all = Vec::new();
    let outs = host.output_devices().ok()?;
    let mut hit: Option<cpal::Device> = None;
    for d in outs {
        let name = d.name().unwrap_or_else(|_| "?".to_string());
        all.push(name.clone());
        if hit.is_none() && name.to_ascii_lowercase().contains("cable input") {
            hit = Some(d);
            info!(device = %name, "matched VB-CABLE Input device for virtual mic");
        }
    }
    if hit.is_none() {
        debug!(devices = ?all, "no 'CABLE Input' device among cpal outputs");
    }
    hit
}

fn run_playback_thread(
    device: cpal::Device,
    rx: mpsc::Receiver<QueuedAudioFrame>,
    ready: mpsc::Sender<Result<()>>,
    session_epoch: Arc<AtomicU64>,
    current_session: Arc<Mutex<Option<PlaybackSession>>>,
    status: BackendStatus,
) {
    let mut decoder = match OpusDecoder::new() {
        Ok(d) => d,
        Err(e) => {
            status.set(BackendState::Degraded);
            warn!(error = %e, "opus decoder init failed inside vb-cable thread");
            let _ = ready.send(Err(e.context("initialize VB-CABLE Opus decoder")));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    let mut stream: Option<StreamCtx> = None;
    let mut last_frame_at: Option<Instant> = None;
    let mut scratch = vec![0f32; FRAME_SAMPLES_INTERLEAVED];
    let mut observed_session_epoch: Option<u64> = None;
    let mut retry_epoch = 0;
    let mut next_build_attempt = Instant::now();
    let mut last_callback_tick = 0;
    let mut frames_since_watchdog = 0;
    let mut last_watchdog = Instant::now();

    loop {
        let now = Instant::now();
        let callback_tick = stream
            .as_ref()
            .map_or(0, |ctx| ctx.callback_ticks.load(Ordering::Acquire));
        let watchdog_errored = stream.is_some()
            && vb_callback_stalled(
                frames_since_watchdog,
                last_callback_tick,
                callback_tick,
                now.saturating_duration_since(last_watchdog),
            );
        if now.saturating_duration_since(last_watchdog) >= CALLBACK_WATCHDOG_INTERVAL {
            last_callback_tick = callback_tick;
            frames_since_watchdog = 0;
            last_watchdog = now;
        }
        let callback_errored = stream
            .as_ref()
            .is_some_and(|ctx| ctx.stream_error.swap(false, Ordering::AcqRel));
        let stream_errored = callback_errored || watchdog_errored;
        if vb_stream_health_action(stream_errored) == VbStreamHealthAction::Rebuild {
            stream = None;
            observed_session_epoch = None;
            last_frame_at = None;
            last_callback_tick = 0;
            frames_since_watchdog = 0;
            last_watchdog = now;
            status.set(BackendState::Degraded);
            if let Some(epoch) = current_session.lock().as_ref().map(PlaybackSession::epoch) {
                retry_epoch = epoch;
                next_build_attempt = Instant::now() + RETRY_BACKOFF;
            }
            warn!("vb-cable stream callback failed — rebuilding after bounded backoff");
        }
        if stream.is_some() && vb_idle_release_due(last_frame_at, Instant::now()) {
            stream = None;
            observed_session_epoch = None;
            last_frame_at = None;
            status.set(BackendState::Idle);
            info!("idle VB-CABLE stream released");
        }
        let idle_directive = {
            let current = current_session.lock();
            stream_directive(observed_session_epoch, current.as_ref(), None)
        };
        if idle_directive == StreamDirective::Drop {
            stream = None;
            observed_session_epoch = None;
            last_frame_at = None;
            status.set(BackendState::Idle);
            info!("retired VB-CABLE stream released");
        }

        let queued = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(queued) => queued,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let directive = {
            let current = current_session.lock();
            stream_directive(
                observed_session_epoch,
                current.as_ref(),
                Some(&queued.session),
            )
        };
        let current_epoch = match directive {
            StreamDirective::Reject | StreamDirective::Wait => continue,
            StreamDirective::Drop => {
                stream = None;
                observed_session_epoch = None;
                last_frame_at = None;
                continue;
            }
            StreamDirective::Build(epoch) => epoch,
            StreamDirective::Keep => queued.session.epoch(),
        };
        let now = Instant::now();
        if directive == StreamDirective::Build(current_epoch)
            && !transition_build_due(
                observed_session_epoch.unwrap_or(0),
                current_epoch,
                retry_epoch,
                next_build_attempt,
                now,
            )
        {
            continue;
        }
        if directive == StreamDirective::Build(current_epoch) {
            status.set(BackendState::Starting);
            retry_epoch = current_epoch;
            stream = match build_stream(&device, current_epoch, session_epoch.clone()) {
                Ok(stream) => Some(stream),
                Err(e) => {
                    status.set(BackendState::Degraded);
                    next_build_attempt = Instant::now() + RETRY_BACKOFF;
                    warn!(error = %e, "vb-cable session stream rebuild failed — retrying in 3 s");
                    continue;
                }
            };
            decoder = match OpusDecoder::new() {
                Ok(decoder) => decoder,
                Err(e) => {
                    status.set(BackendState::Degraded);
                    warn!(error = %e, "vb-cable decoder reset failed");
                    return;
                }
            };
            observed_session_epoch = Some(current_epoch);
            last_callback_tick = stream
                .as_ref()
                .map_or(0, |ctx| ctx.callback_ticks.load(Ordering::Acquire));
            frames_since_watchdog = 0;
            last_watchdog = Instant::now();
            status.set(BackendState::Active);
            last_frame_at = Some(Instant::now());
            next_build_attempt = Instant::now();
        }
        let n = match decoder.decode(&queued.frame.opus, &mut scratch) {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "opus decode failed (mic) — dropping frame");
                continue;
            }
        };
        let current = current_session.lock();
        if session_epoch.load(Ordering::Acquire) != current_epoch
            || stream_directive(
                observed_session_epoch,
                current.as_ref(),
                Some(&queued.session),
            ) != StreamDirective::Keep
        {
            continue;
        }
        drop(current);
        let Some(stream) = stream.as_mut() else {
            continue;
        };
        let pushed = stream.producer.push_slice(&scratch[..n]);
        frames_since_watchdog = frames_since_watchdog.saturating_add(1);
        last_frame_at = Some(Instant::now());
        if pushed != n {
            warn!(
                dropped = n - pushed,
                "vb-cable ring full — dropping mic samples"
            );
        }
    }
    drop(stream);
    status.set(BackendState::Stopped);
    info!("vb-cable-playback thread exiting");
}

struct StreamCtx {
    _stream: cpal::Stream,
    producer: ringbuf::HeapProd<f32>,
    stream_error: Arc<AtomicBool>,
    callback_ticks: Arc<AtomicU64>,
}

fn build_stream(
    device: &cpal::Device,
    stream_epoch: u64,
    session_epoch: Arc<AtomicU64>,
) -> Result<StreamCtx> {
    let device_name = device.name().unwrap_or_else(|_| "?".to_string());
    let config = pick_config(device)?;
    info!(
        device = %device_name,
        sample_rate = config.sample_rate().0,
        channels = config.channels(),
        sample_format = ?config.sample_format(),
        "vb-cable playback config picked"
    );

    let rb = HeapRb::<f32>::new(RING_CAPACITY);
    let (producer, mut consumer) = rb.split();

    let stream_error = Arc::new(AtomicBool::new(false));
    let stream_error_cb = stream_error.clone();
    let callback_ticks = Arc::new(AtomicU64::new(0));
    let callback_ticks_f32 = callback_ticks.clone();
    let callback_ticks_i16 = callback_ticks.clone();
    let err_fn = move |e| {
        warn!(error = %e, "vb-cable stream error");
        stream_error_cb.store(true, Ordering::Release);
    };
    let session_epoch_f32 = session_epoch.clone();
    let session_epoch_i16 = session_epoch;
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_output_stream(
            &config.config(),
            move |out: &mut [f32], _| {
                fill_callback_for_epoch(
                    out,
                    &mut consumer,
                    stream_epoch,
                    &session_epoch_f32,
                    &callback_ticks_f32,
                );
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => {
            let mut tmp = vec![0f32; 0];
            device.build_output_stream(
                &config.config(),
                move |out: &mut [i16], _| {
                    tmp.resize(out.len(), 0.0);
                    fill_callback_for_epoch(
                        &mut tmp,
                        &mut consumer,
                        stream_epoch,
                        &session_epoch_i16,
                        &callback_ticks_i16,
                    );
                    for (dst, &src) in out.iter_mut().zip(tmp.iter()) {
                        *dst = (src.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                    }
                },
                err_fn,
                None,
            )
        }
        other => anyhow::bail!("unsupported vb-cable sample format: {other:?}"),
    }
    .context("build vb-cable output stream")?;
    stream.play().context("cpal play (vb-cable)")?;

    Ok(StreamCtx {
        _stream: stream,
        producer,
        stream_error,
        callback_ticks,
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
            "vb-cable ring underrun (filled with silence)"
        );
    }
}

fn fill_callback_for_epoch(
    out: &mut [f32],
    consumer: &mut ringbuf::HeapCons<f32>,
    stream_epoch: u64,
    session_epoch: &AtomicU64,
    callback_ticks: &AtomicU64,
) {
    callback_ticks.fetch_add(1, Ordering::Relaxed);
    if session_epoch.load(Ordering::Acquire) != stream_epoch {
        out.fill(0.0);
        return;
    }
    fill_callback(out, consumer);
}

fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let supported = device
        .supported_output_configs()
        .context("query VB-CABLE configs")?
        .collect::<Vec<_>>();
    let target_rate = cpal::SampleRate(SAMPLE_RATE);
    if let Some(matched) = supported.iter().find(|c| {
        c.channels() == CHANNELS
            && c.min_sample_rate() <= target_rate
            && target_rate <= c.max_sample_rate()
            && c.sample_format() == SampleFormat::F32
    }) {
        return Ok((*matched).with_sample_rate(target_rate));
    }
    if let Some(matched) = supported.iter().find(|c| {
        c.channels() == CHANNELS
            && c.min_sample_rate() <= target_rate
            && target_rate <= c.max_sample_rate()
    }) {
        return Ok((*matched).with_sample_rate(target_rate));
    }
    let default = device
        .default_output_config()
        .context("VB-CABLE has no default output config")?;
    warn!(
        rate = default.sample_rate().0,
        channels = default.channels(),
        format = ?default.sample_format(),
        "VB-CABLE doesn't expose 48 kHz stereo natively — falling back to default"
    );
    Ok(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = assert!(VB_QUEUE_CAPACITY <= 8);

    #[test]
    fn vb_cable_transition_retry_is_bounded_and_session_scoped() {
        let now = Instant::now();
        let retry_deadline = now + RETRY_BACKOFF;

        assert!(!transition_build_due(0, 1, 1, retry_deadline, now));
        assert!(transition_build_due(0, 2, 1, retry_deadline, now));
    }

    #[test]
    fn vb_cable_worker_disconnect_is_reported_only_once() {
        let failure_reported = AtomicBool::new(false);

        assert!(report_worker_disconnect(&failure_reported).is_err());
        assert!(report_worker_disconnect(&failure_reported).is_ok());
    }

    #[test]
    fn vb_stream_error_requests_bounded_rebuild() {
        assert_eq!(vb_stream_health_action(true), VbStreamHealthAction::Rebuild);
        assert_eq!(vb_stream_health_action(false), VbStreamHealthAction::Keep);
    }

    #[test]
    fn vb_callback_watchdog_detects_stalled_stream_with_inbound_frames() {
        assert!(vb_callback_stalled(1, 7, 7, CALLBACK_WATCHDOG_INTERVAL));
        assert!(!vb_callback_stalled(1, 7, 8, CALLBACK_WATCHDOG_INTERVAL));
        assert!(!vb_callback_stalled(0, 7, 7, CALLBACK_WATCHDOG_INTERVAL));
    }

    #[test]
    fn ending_current_vb_session_advances_epoch_only_once() {
        let epoch = AtomicU64::new(8);
        let session = PlaybackSession::new(8);

        assert!(retire_vb_session(&epoch, &session));
        assert_eq!(epoch.load(Ordering::Acquire), 9);
        assert!(!retire_vb_session(&epoch, &session));
        assert_eq!(epoch.load(Ordering::Acquire), 9);
    }

    #[test]
    fn equal_epoch_from_another_backend_is_not_current() {
        let current = PlaybackSession::new(3);
        let foreign = PlaybackSession::new(3);

        assert!(!session_is_current(&current, &foreign));
    }

    #[test]
    fn vb_stream_exists_only_for_a_queued_current_session() {
        let current = PlaybackSession::new(1);
        let foreign = PlaybackSession::new(1);
        assert_eq!(stream_directive(Some(0), None, None), StreamDirective::Drop);
        assert_eq!(
            stream_directive(None, Some(&current), None),
            StreamDirective::Wait
        );
        let next = PlaybackSession::new(2);
        assert_eq!(
            stream_directive(Some(1), Some(&next), None),
            StreamDirective::Drop
        );
        assert_eq!(
            stream_directive(None, Some(&current), Some(&current)),
            StreamDirective::Build(1)
        );
        assert_eq!(
            stream_directive(Some(1), Some(&current), Some(&foreign)),
            StreamDirective::Reject
        );
        assert_eq!(stream_directive(Some(1), None, None), StreamDirective::Drop);
    }

    #[test]
    fn vb_stream_releases_after_the_idle_budget() {
        let start = Instant::now();
        assert!(!vb_idle_release_due(
            Some(start),
            start + IDLE_RELEASE - Duration::from_millis(1)
        ));
        assert!(vb_idle_release_due(Some(start), start + IDLE_RELEASE));
    }
}
