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
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig, SupportedBufferSize, SupportedStreamConfig};
use parking_lot::Mutex;
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use tracing::{debug, info, warn};

use crate::codec::OpusDecoder;
use crate::{
    AudioFrame, AudioPlayback, BackendState, BackendStatus, CHANNELS, FRAME_SAMPLES_INTERLEAVED,
    PlaybackSession, SAMPLE_RATE, StreamKind,
};

/// Ring-buffer capacity in interleaved samples. ~10 frames @ 20 ms =
/// 200 ms. Generous enough to absorb network jitter, tight enough not
/// to feel laggy.
const RING_CAPACITY: usize = FRAME_SAMPLES_INTERLEAVED * 10;
/// Three 20 ms frames reserve 60 ms against delivery jitter. A smaller 40 ms
/// reserve previously underran during device scheduling stalls. Native audio
/// acceptance, including quiet/sparse sounds, gates this intermediate setting.
const JITTER_TARGET_SAMPLES: usize = FRAME_SAMPLES_INTERLEAVED * 3;
/// A burst must not turn the 200 ms safety capacity into steady audio delay.
/// Return to the 60 ms reserve once decoded audio exceeds 80 ms.
/// A 120 ms plateau still added a device period in the real Wi-Fi measurement.
const JITTER_MAX_SAMPLES: usize = FRAME_SAMPLES_INTERLEAVED * 4;

/// Keep one ring-buffer's worth of ingress slack. Production logs showed the
/// Windows playback worker occasionally being descheduled for 80–160 ms while
/// the UDP receiver continued delivering a steady 50 frames/s. A four-frame
/// queue therefore discarded current audio even though the decoded PCM ring
/// had room (`ring_samples=0`). Eight frames absorb that observed burst while
/// the existing lossy boundary still prevents unbounded latency.
const PLAYBACK_QUEUE_CAPACITY: usize = 8;
const PLAYBACK_HEALTH_REPORT_INTERVAL: Duration = Duration::from_secs(5);
const IDLE_PAUSE_AFTER: Duration = Duration::from_secs(2);
const DEFAULT_DEVICE_PROBE_INTERVAL: Duration = Duration::from_millis(500);
/// MMCSS protects the decoder/callback threads, and device discovery runs on
/// a separate worker. The 20 ms device request does not reduce the separate
/// 80 ms PCM reserve used to absorb network delivery stalls.
#[cfg(target_os = "windows")]
const PLAYBACK_BUFFER_FRAMES: u32 = 960;
#[cfg(not(target_os = "windows"))]
const PLAYBACK_BUFFER_FRAMES: u32 = 1_920;

static PLAYBACK_UNDERRUN_SAMPLES: AtomicU64 = AtomicU64::new(0);
static PLAYBACK_CATCHUP_SAMPLES: AtomicU64 = AtomicU64::new(0);
static PLAYBACK_EXPIRED_FRAMES: AtomicU64 = AtomicU64::new(0);

fn queued_frame_expired(queued_at: Instant, built_at: Option<Instant>, now: Instant) -> bool {
    // Device creation may take longer than one jitter reserve. Preserve the
    // initial short cue, with a bounded cold-start grace; steady playback must
    // not replay frames retained while the decoder was descheduled.
    let active = built_at.is_some_and(|built| queued_at >= built);
    let limit = if active {
        Duration::from_millis(80)
    } else {
        Duration::from_millis(300)
    };
    now.saturating_duration_since(queued_at) > limit
}

fn effective_playback_idle(route_active: bool, elapsed: Duration) -> Duration {
    if route_active {
        Duration::ZERO
    } else {
        elapsed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlaybackLifecycleAction {
    None,
    Build,
    Release,
}

fn playback_lifecycle_action(
    stream_present: bool,
    frame_pending: bool,
    rebuild_requested: bool,
    idle_elapsed: Duration,
) -> PlaybackLifecycleAction {
    if stream_present && idle_elapsed >= IDLE_PAUSE_AFTER {
        PlaybackLifecycleAction::Release
    } else if (stream_present && rebuild_requested)
        || (!stream_present && frame_pending && idle_elapsed < IDLE_PAUSE_AFTER)
    {
        PlaybackLifecycleAction::Build
    } else {
        PlaybackLifecycleAction::None
    }
}

fn current_session_playback_demand(pending_epoch: Option<u64>, current_epoch: u64) -> bool {
    pending_epoch == Some(current_epoch)
}

fn expire_idle_build_state(
    stream_present: bool,
    idle_elapsed: Duration,
    pending_build_epoch: &mut Option<u64>,
    next_build_attempt: &mut Instant,
    now: Instant,
) {
    if !stream_present && pending_build_epoch.is_some() && idle_elapsed >= IDLE_PAUSE_AFTER {
        *pending_build_epoch = None;
        *next_build_attempt = now;
    }
}

fn record_current_frame_demand(
    stream_present: bool,
    current_epoch: u64,
    frame_received_at: Instant,
    last_frame_received: &mut Instant,
    pending_build_epoch: &mut Option<u64>,
    next_build_attempt: &mut Instant,
) {
    expire_idle_build_state(
        stream_present,
        frame_received_at.saturating_duration_since(*last_frame_received),
        pending_build_epoch,
        next_build_attempt,
        frame_received_at,
    );
    *last_frame_received = frame_received_at;
    *pending_build_epoch = Some(current_epoch);
}

fn accepted_session_epoch(requested: &PlaybackSession, current: &PlaybackSession) -> Option<u64> {
    requested.same_identity(current).then_some(current.epoch())
}

fn retire_current_playback_session(
    current_session: &Mutex<PlaybackSession>,
    session_epoch: &AtomicU64,
    session_active: &AtomicBool,
    requested: &PlaybackSession,
) -> bool {
    let mut current = current_session.lock();
    if !requested.same_identity(&current) {
        return false;
    }
    let retired_epoch = current.epoch().wrapping_add(1);
    *current = PlaybackSession::new(retired_epoch);
    session_active.store(false, Ordering::Release);
    session_epoch.store(retired_epoch, Ordering::Release);
    true
}

fn synchronize_session_epoch(observed_epoch: &mut u64, current_epoch: u64) -> bool {
    if *observed_epoch == current_epoch {
        false
    } else {
        *observed_epoch = current_epoch;
        true
    }
}

fn synchronize_decoder_session(
    decoder: &mut OpusDecoder,
    observed_epoch: &mut u64,
    current_epoch: u64,
) -> Result<bool> {
    let changed = synchronize_session_epoch(observed_epoch, current_epoch);
    if changed {
        *decoder = OpusDecoder::new()?;
    }
    Ok(changed)
}

pub struct CpalPlayback {
    tx: mpsc::SyncSender<QueuedAudioFrame>,
    queue_dropped_frames: Arc<AtomicU64>,
    session_epoch: Arc<AtomicU64>,
    ingress_order: Arc<Mutex<()>>,
    current_session: Arc<Mutex<PlaybackSession>>,
    session_active: Arc<AtomicBool>,
    status: BackendStatus,
}

struct QueuedAudioFrame {
    session: PlaybackSession,
    frame: AudioFrame,
    queued_at: Instant,
}

fn publish_current_session_frame(
    tx: &mpsc::SyncSender<QueuedAudioFrame>,
    ingress_order: &Mutex<()>,
    session: &PlaybackSession,
    current_session: &PlaybackSession,
    frame: AudioFrame,
) -> std::result::Result<bool, mpsc::TrySendError<QueuedAudioFrame>> {
    let _guard = ingress_order.lock();
    if accepted_session_epoch(session, current_session).is_none() {
        return Ok(false);
    }
    match tx.try_send(QueuedAudioFrame {
        session: session.clone(),
        frame,
        queued_at: Instant::now(),
    }) {
        Ok(()) => {
            session.note_accepted_frame();
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

fn consume_published_frame(ingress_order: &Mutex<()>, queued: &QueuedAudioFrame) {
    let _guard = ingress_order.lock();
    queued.session.note_consumed_frame();
}

struct BuildDemandState<'a> {
    last_frame_received: &'a mut Instant,
    pending_build_epoch: &'a mut Option<u64>,
    next_build_attempt: &'a mut Instant,
}

fn refresh_build_demand_from_session_ingress(
    ingress_order: &Mutex<()>,
    session: &PlaybackSession,
    current_epoch: u64,
    stream_present: bool,
    now: Instant,
    state: BuildDemandState<'_>,
) -> bool {
    let _guard = ingress_order.lock();
    if session.epoch() != current_epoch || !session.has_pending_ingress() {
        return false;
    }
    record_current_frame_demand(
        stream_present,
        current_epoch,
        now,
        state.last_frame_received,
        state.pending_build_epoch,
        state.next_build_attempt,
    );
    true
}

impl CpalPlayback {
    pub fn new() -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<QueuedAudioFrame>(PLAYBACK_QUEUE_CAPACITY);
        let queue_dropped_frames = Arc::new(AtomicU64::new(0));
        let dropped_for_thread = queue_dropped_frames.clone();
        let session_epoch = Arc::new(AtomicU64::new(0));
        let epoch_for_thread = session_epoch.clone();
        let ingress_order = Arc::new(Mutex::new(()));
        let order_for_thread = ingress_order.clone();
        let current_session = Arc::new(Mutex::new(PlaybackSession::new(0)));
        let session_for_thread = current_session.clone();
        let session_active = Arc::new(AtomicBool::new(false));
        let active_for_thread = session_active.clone();
        let status = BackendStatus::new(BackendState::Idle);
        let status_for_thread = status.clone();
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
            .spawn(move || {
                run_playback_thread(
                    rx,
                    dropped_for_thread,
                    epoch_for_thread,
                    order_for_thread,
                    session_for_thread,
                    active_for_thread,
                    status_for_thread,
                )
            })
            .context("spawn cpal-playback thread")?;
        Ok(Self {
            tx,
            queue_dropped_frames,
            session_epoch,
            ingress_order,
            current_session,
            session_active,
            status,
        })
    }
}

impl AudioPlayback for CpalPlayback {
    fn backend_status(&self) -> BackendStatus {
        self.status.clone()
    }

    fn begin_session(&self) -> PlaybackSession {
        let mut current_session = self.current_session.lock();
        let epoch = self.session_epoch.load(Ordering::Relaxed).wrapping_add(1);
        let session = PlaybackSession::new(epoch);
        *current_session = session.clone();
        self.session_active.store(true, Ordering::Release);
        self.session_epoch.store(epoch, Ordering::Release);
        session
    }

    fn end_session(&self, session: &PlaybackSession) {
        retire_current_playback_session(
            &self.current_session,
            &self.session_epoch,
            &self.session_active,
            session,
        );
    }

    fn enqueue(&self, session: &PlaybackSession, frame: AudioFrame) -> Result<()> {
        let current_session = self.current_session.lock();
        match publish_current_session_frame(
            &self.tx,
            &self.ingress_order,
            session,
            &current_session,
            frame,
        ) {
            Ok(_) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => {
                // Audio is time-sensitive: retaining a stale backlog sounds
                // worse than dropping it. The playback thread emits one
                // aggregate diagnostic instead of logging on this hot path.
                self.queue_dropped_frames.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Err(anyhow::anyhow!("cpal-playback thread terminated"))
            }
        }
    }
}

#[derive(Default)]
struct PlaybackSequence {
    last: Option<u32>,
}

impl PlaybackSequence {
    fn last(&self) -> Option<u32> {
        self.last
    }

    fn reset(&mut self) {
        self.last = None;
    }

    fn accept(&mut self, current: u32) -> bool {
        let accepted = match self.last {
            None => true,
            Some(last) => {
                let distance = current.wrapping_sub(last);
                distance != 0 && distance < (1 << 31)
            }
        };
        if accepted {
            self.last = Some(current);
        }
        accepted
    }
}

struct PlaybackWorkerStatusGuard {
    status: BackendStatus,
    graceful: bool,
}

impl PlaybackWorkerStatusGuard {
    fn new(status: BackendStatus) -> Self {
        Self {
            status,
            graceful: false,
        }
    }

    fn mark_graceful(&mut self) {
        self.graceful = true;
    }
}

impl Drop for PlaybackWorkerStatusGuard {
    fn drop(&mut self) {
        if !self.graceful {
            self.status.set(BackendState::Degraded);
        }
    }
}

/// Device discovery can block inside the OS audio service. Neither submitting
/// a request nor reading its result may wait on the decoder thread.
struct DefaultDeviceProbe {
    requests: mpsc::SyncSender<()>,
    results: mpsc::Receiver<Option<String>>,
}

#[cfg(target_os = "windows")]
struct ProbeComApartment;

#[cfg(target_os = "windows")]
impl ProbeComApartment {
    fn initialize() -> Result<Self> {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .context("initialize default device probe COM apartment")?;
        Ok(Self)
    }
}

#[cfg(target_os = "windows")]
impl Drop for ProbeComApartment {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

impl DefaultDeviceProbe {
    fn spawn<F>(mut query: F) -> Result<Self>
    where
        F: FnMut() -> Option<String> + Send + 'static,
    {
        let (requests, pending) = mpsc::sync_channel(1);
        let (completed, results) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("audio-default-probe".into())
            .spawn(move || {
                #[cfg(target_os = "windows")]
                let _com_apartment = match ProbeComApartment::initialize() {
                    Ok(apartment) => apartment,
                    Err(error) => {
                        warn!(%error, "default audio endpoint probe COM unavailable");
                        return;
                    }
                };
                while pending.recv().is_ok() {
                    let started = Instant::now();
                    let current = query();
                    if started.elapsed() >= Duration::from_millis(50) {
                        info!(
                            elapsed_ms = started.elapsed().as_millis(),
                            "default audio endpoint probe delayed outside decoder"
                        );
                    }
                    if completed.send(current).is_err() {
                        break;
                    }
                }
            })
            .context("spawn default audio endpoint probe")?;
        Ok(Self { requests, results })
    }

    fn request(&self) -> bool {
        self.requests.try_send(()).is_ok()
    }

    fn poll(&self) -> Option<String> {
        self.results.try_recv().ok().flatten()
    }
}

fn run_playback_thread(
    rx: mpsc::Receiver<QueuedAudioFrame>,
    queue_dropped_frames: Arc<AtomicU64>,
    session_epoch: Arc<AtomicU64>,
    ingress_order: Arc<Mutex<()>>,
    current_session: Arc<Mutex<PlaybackSession>>,
    session_active: Arc<AtomicBool>,
    status: BackendStatus,
) {
    #[cfg(target_os = "windows")]
    crate::audio_thread_win::enter();
    let mut decoder = match OpusDecoder::new() {
        Ok(d) => d,
        Err(e) => {
            status.set(BackendState::Degraded);
            warn!(error = %e, "opus decoder init failed inside playback thread");
            return;
        }
    };
    let mut exit_status = PlaybackWorkerStatusGuard::new(status.clone());
    let mut scratch = vec![0f32; FRAME_SAMPLES_INTERLEAVED];
    // Last sequence accepted in the current peer session. Serial-number
    // arithmetic handles natural u32 wrap-around; the explicit session epoch
    // handles a peer process restart that begins its counter at zero.
    let mut sequence = PlaybackSequence::default();
    #[cfg(target_os = "windows")]
    let mut reported_mmcss_threads = (0, 0, 0);
    // The worker can start after begin_session(). Starting at the published
    // epoch would swallow that first transition and skip output prewarming.
    let mut observed_session_epoch = 0;

    // The stream starts as `None`, then prewarms when an enabled peer route
    // begins. Retry cadence is bounded so a
    // permanently-broken device doesn't hot-loop calling cpal.
    let mut stream_ctx: Option<StreamCtx> = None;
    let mut pending_build_epoch = None;
    let mut last_version = crate::output_device_version();
    let mut next_build_attempt = Instant::now();
    let mut last_default_probe = Instant::now() - DEFAULT_DEVICE_PROBE_INTERVAL;
    let default_probe = match DefaultDeviceProbe::spawn(crate::default_output_device_name) {
        Ok(probe) => Some(probe),
        Err(error) => {
            warn!(%error, "default audio endpoint probing unavailable");
            None
        }
    };
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
    let mut last_health_report = Instant::now();
    let mut ring_dropped_samples = 0u64;
    let mut queue_dropped_since_report = 0u64;
    let mut last_frame_received = Instant::now();

    loop {
        let (worker_session, route_active) = {
            let current = current_session.lock();
            (current.clone(), session_active.load(Ordering::Acquire))
        };
        let worker_epoch = worker_session.epoch();
        let session_changed = match synchronize_decoder_session(
            &mut decoder,
            &mut observed_session_epoch,
            worker_epoch,
        ) {
            Ok(changed) => changed,
            Err(e) => {
                status.set(BackendState::Degraded);
                warn!(error = %e, "opus decoder reset failed for new peer session");
                return;
            }
        };
        if session_changed {
            pending_build_epoch = route_active.then_some(worker_epoch);
            stream_ctx = None;
            status.set(BackendState::Idle);
            sequence.reset();
            next_build_attempt = Instant::now();
            last_frame_received = Instant::now();
            info!(
                session_epoch = observed_session_epoch,
                "cpal playback invalidated for new peer session"
            );
        } else if !current_session_playback_demand(pending_build_epoch, worker_epoch) {
            pending_build_epoch = None;
        }
        refresh_build_demand_from_session_ingress(
            &ingress_order,
            &worker_session,
            worker_epoch,
            stream_ctx.is_some(),
            Instant::now(),
            BuildDemandState {
                last_frame_received: &mut last_frame_received,
                pending_build_epoch: &mut pending_build_epoch,
                next_build_attempt: &mut next_build_attempt,
            },
        );
        let idle_elapsed = effective_playback_idle(route_active, last_frame_received.elapsed());
        expire_idle_build_state(
            stream_ctx.is_some(),
            idle_elapsed,
            &mut pending_build_epoch,
            &mut next_build_attempt,
            Instant::now(),
        );

        // In "Follow system default" mode, Windows can move the default render
        // endpoint while the old stream remains perfectly healthy. A callback
        // watchdog cannot detect that, so compare the active endpoint with the
        // current default at a low-frequency internal seam and rebuild when it
        // changes. Explicit device selections never follow this path.
        let follows_default = crate::selected_output_device().is_none();
        if follows_default
            && stream_ctx.is_some()
            && last_default_probe.elapsed() >= DEFAULT_DEVICE_PROBE_INTERVAL
        {
            last_default_probe = Instant::now();
            if let Some(probe) = default_probe.as_ref() {
                probe.request();
            }
        }
        let default_change = default_probe.as_ref().and_then(|probe| {
            probe.poll().and_then(|current| {
                stream_ctx.as_ref().and_then(|ctx| {
                    default_device_changed(follows_default, Some(&ctx.device_name), Some(&current))
                        .then(|| (ctx.device_name.clone(), current))
                })
            })
        });

        // (Re)build the stream when we need one:
        //   * a peer frame arrived while no stream exists, OR
        //   * the user picked a different device on the GUI tab, OR
        //   * the cpal error callback fired (device error), OR
        //   * the device-loss watchdog flagged a stalled callback.
        let v = crate::output_device_version();
        let errored = stream_ctx
            .as_ref()
            .is_some_and(|c| c.stream_error.load(Ordering::Acquire));
        let rebuild_requested =
            stream_ctx.is_some() && (v != last_version || errored || default_change.is_some());
        match playback_lifecycle_action(
            stream_ctx.is_some(),
            current_session_playback_demand(pending_build_epoch, worker_epoch),
            rebuild_requested,
            idle_elapsed,
        ) {
            PlaybackLifecycleAction::Release => {
                stream_ctx = None;
                pending_build_epoch = None;
                sequence.reset();
                status.set(BackendState::Idle);
                info!("cpal playback output device released while idle");
            }
            PlaybackLifecycleAction::Build if Instant::now() >= next_build_attempt => {
                status.set(BackendState::Starting);
                if session_epoch.load(Ordering::Acquire) != worker_epoch {
                    pending_build_epoch = None;
                    continue;
                }
                if errored {
                    warn!("cpal playback stream errored — rebuilding");
                }
                if let Some((old, new)) = &default_change {
                    info!(
                        previous = %old,
                        current = %new,
                        "Windows default playback device changed — rebuilding"
                    );
                }
                // Drop any old stream first so the device handle is
                // released before we ask cpal for it back — some
                // Windows drivers refuse exclusive re-acquire
                // otherwise.
                pending_build_epoch = Some(worker_epoch);
                stream_ctx = None;
                match build_stream(worker_epoch, session_epoch.clone()) {
                    Ok(s) => {
                        refresh_build_demand_from_session_ingress(
                            &ingress_order,
                            &worker_session,
                            worker_epoch,
                            false,
                            Instant::now(),
                            BuildDemandState {
                                last_frame_received: &mut last_frame_received,
                                pending_build_epoch: &mut pending_build_epoch,
                                next_build_attempt: &mut next_build_attempt,
                            },
                        );
                        let current_epoch = session_epoch.load(Ordering::Acquire);
                        if current_epoch != worker_epoch
                            || (!session_active.load(Ordering::Acquire)
                                && (!current_session_playback_demand(
                                    pending_build_epoch,
                                    current_epoch,
                                ) || last_frame_received.elapsed() >= IDLE_PAUSE_AFTER))
                        {
                            pending_build_epoch = None;
                            next_build_attempt = Instant::now();
                            continue;
                        }
                        if let Err(e) = s.stream.play() {
                            status.set(BackendState::Degraded);
                            warn!(error = %e, "cpal playback start failed — retrying in 3 s");
                            next_build_attempt = Instant::now() + RETRY_BACKOFF;
                            continue;
                        }
                        if session_epoch.load(Ordering::Acquire) != worker_epoch {
                            pending_build_epoch = None;
                            next_build_attempt = Instant::now();
                            continue;
                        }
                        last_tick_snapshot = s.callback_ticks.load(Ordering::Relaxed);
                        stream_ctx = Some(s);
                        status.set(BackendState::Active);
                        pending_build_epoch = None;
                        last_version = v;
                        last_watchdog = Instant::now();
                        frames_since_watchdog = 0;
                        info!("cpal playback ready");
                    }
                    Err(e) => {
                        status.set(BackendState::Degraded);
                        warn!(error = %e, "cpal playback build failed — retrying in 3 s");
                        last_version = v; // don't busy-retry on the same version
                        next_build_attempt = Instant::now() + RETRY_BACKOFF;
                    }
                }
            }
            PlaybackLifecycleAction::None | PlaybackLifecycleAction::Build => {}
        }

        // Drain frames with a short timeout so the next iteration
        // can notice version bumps and retry timers without
        // waiting on a frame that may not come (silent stream
        // periods, peer paused playback, etc.).
        let received = rx.recv_timeout(Duration::from_millis(200));
        match received {
            Ok(queued) => {
                consume_published_frame(&ingress_order, &queued);
                let current_session_epoch = session_epoch.load(Ordering::Acquire);
                if queued.session.epoch() != current_session_epoch {
                    // This frame was already queued when the previous peer
                    // session ended. Never let it seed the new sequence state.
                    continue;
                }
                let session_changed = match synchronize_decoder_session(
                    &mut decoder,
                    &mut observed_session_epoch,
                    current_session_epoch,
                ) {
                    Ok(changed) => changed,
                    Err(e) => {
                        warn!(error = %e, "opus decoder reset failed for queued peer session");
                        return;
                    }
                };
                if session_changed {
                    stream_ctx = None;
                    sequence.reset();
                    next_build_attempt = Instant::now();
                    info!(
                        session_epoch = observed_session_epoch,
                        "cpal playback invalidated by new-session frame"
                    );
                }
                if queued_frame_expired(
                    queued.queued_at,
                    stream_ctx.as_ref().map(|ctx| ctx.built_at),
                    Instant::now(),
                ) {
                    // Advance ordering even for intentionally discarded audio:
                    // never rewind on late packets or recreate this backlog as PLC.
                    sequence.accept(queued.frame.seq);
                    PLAYBACK_EXPIRED_FRAMES.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                record_current_frame_demand(
                    stream_ctx.is_some(),
                    current_session_epoch,
                    Instant::now(),
                    &mut last_frame_received,
                    &mut pending_build_epoch,
                    &mut next_build_attempt,
                );
                let frame = queued.frame;
                let queue_dropped = queue_dropped_frames.swap(0, Ordering::Relaxed);
                if queue_dropped > 0 {
                    queue_dropped_since_report =
                        queue_dropped_since_report.saturating_add(queue_dropped);
                    // A bounded-queue drop was intentional latency control,
                    // not network loss. Do not synthesize the discarded
                    // backlog with PLC when the next current frame arrives.
                    sequence.reset();
                }
                // Drop duplicates and reordered-late frames without treating
                // natural u32 wrap-around as a rewind.
                let previous_seq = sequence.last();
                if !sequence.accept(frame.seq) {
                    continue;
                }
                if session_epoch.load(Ordering::Acquire) != current_session_epoch {
                    pending_build_epoch = None;
                    continue;
                }
                if let Some(ctx) = stream_ctx.as_mut() {
                    pending_build_epoch = None;
                    // Conceal any gap between the last accepted seq and
                    // this one. Cap the fill so a large gap (long stall,
                    // seq reset) doesn't blast a burst of synthetic
                    // frames into the ring.
                    let lost = crate::codec::frames_lost(previous_seq, frame.seq);
                    let conceal =
                        concealment_frames(previous_seq, frame.seq, ctx.producer.occupied_len());
                    if frame.stream == StreamKind::Mic && lost == 1 && conceal == 1 {
                        // Single-frame voice gap: try to *recover* the
                        // missing frame from the in-band FEC carried by
                        // this (the next) packet. Fall back to PLC if the
                        // packet has no usable LBRR data.
                        match decoder.decode_fec(&frame.opus, &mut scratch) {
                            Ok(n) => {
                                push_samples(
                                    &mut ctx.producer,
                                    &scratch[..n],
                                    &mut ring_dropped_samples,
                                );
                            }
                            Err(_) => {
                                if let Ok(n) = decoder.decode_plc(&mut scratch) {
                                    push_samples(
                                        &mut ctx.producer,
                                        &scratch[..n],
                                        &mut ring_dropped_samples,
                                    );
                                }
                            }
                        }
                    } else {
                        // Multi-frame gap, or a non-voice stream where
                        // LBRR yields little — synthesize each missing
                        // frame with Opus PLC instead of hard silence.
                        for _ in 0..conceal {
                            if let Ok(n) = decoder.decode_plc(&mut scratch) {
                                push_samples(
                                    &mut ctx.producer,
                                    &scratch[..n],
                                    &mut ring_dropped_samples,
                                );
                            }
                        }
                    }

                    let n = match decoder.decode(&frame.opus, &mut scratch) {
                        Ok(n) => n,
                        Err(e) => {
                            warn!(error = %e, "opus decode failed — dropping frame");
                            continue;
                        }
                    };
                    push_samples(&mut ctx.producer, &scratch[..n], &mut ring_dropped_samples);
                    frames_since_watchdog += 1;

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
                            pending_build_epoch = Some(current_session_epoch);
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
                // the lifecycle / version / retry checks.
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if last_health_report.elapsed() >= PLAYBACK_HEALTH_REPORT_INTERVAL {
            #[cfg(target_os = "windows")]
            {
                let threads = crate::audio_thread_win::registered_threads();
                if threads != reported_mmcss_threads {
                    info!(
                        threads = threads.0,
                        pro_audio = threads.1,
                        high_priority = threads.2,
                        "playback MMCSS audio tasks"
                    );
                    reported_mmcss_threads = threads;
                }
            }
            let catchup = PLAYBACK_CATCHUP_SAMPLES.swap(0, Ordering::Relaxed);
            let expired = PLAYBACK_EXPIRED_FRAMES.swap(0, Ordering::Relaxed);
            if expired > 0 {
                info!(frames = expired, "cpal discarded expired ingress frames");
            }
            if catchup > 0 {
                info!(
                    samples = catchup,
                    "cpal playback caught up after delivery burst"
                );
            }
            let samples = PLAYBACK_UNDERRUN_SAMPLES.swap(0, Ordering::Relaxed);
            if samples > 0 {
                debug!(samples, "cpal playback underrun summary");
            }
            queue_dropped_since_report = queue_dropped_since_report
                .saturating_add(queue_dropped_frames.swap(0, Ordering::Relaxed));
            if ring_dropped_samples > 0 || queue_dropped_since_report > 0 {
                warn!(
                    ring_samples = ring_dropped_samples,
                    queue_frames = queue_dropped_since_report,
                    "cpal playback backlog drop summary"
                );
                ring_dropped_samples = 0;
                queue_dropped_since_report = 0;
            }
            last_health_report = Instant::now();
        }
    }

    drop(stream_ctx);
    status.set(BackendState::Stopped);
    exit_status.mark_graceful();
    info!("cpal playback thread exiting");
}

/// Wraps the cpal stream with the producer-half of the ring buffer
/// the decoded-frame loop pushes into. Both fields stay on the
/// playback thread — `Stream` is `!Send` on Windows. The stream is started
/// only after post-build epoch validation and retained for its drop semantics.
struct StreamCtx {
    stream: cpal::Stream,
    built_at: Instant,
    device_name: String,
    producer: ringbuf::HeapProd<f32>,
    /// Bumped by the cpal data callback every time the device pulls
    /// samples. The playback thread's watchdog samples this to tell a
    /// live device callback from one that has silently stopped.
    callback_ticks: Arc<AtomicU64>,
    /// Set by the cpal error callback when the OS reports a stream
    /// error (device disconnected, format change, exclusive grab).
    stream_error: Arc<AtomicBool>,
}

fn build_stream(stream_epoch: u64, session_epoch: Arc<AtomicU64>) -> Result<StreamCtx> {
    // Honour the user's runtime device pick (Stage 8.4); falls
    // back to the OS default when no selection is set or the
    // selected device has been unplugged since.
    let device = crate::resolve_output_device()
        .context("no audio output device available (default or selected)")?;
    let device_name = device.name().unwrap_or_else(|_| "?".to_string());

    let config = pick_config(&device)?;
    #[cfg(target_os = "windows")]
    let low_latency_available = crate::audio_thread_win::enter();
    #[cfg(not(target_os = "windows"))]
    let low_latency_available = false;
    let stream_config = playback_stream_config(&config, low_latency_available);
    info!(
        device = %device_name,
        sample_rate = config.sample_rate().0,
        channels = config.channels(),
        sample_format = ?config.sample_format(),
        buffer_size = ?stream_config.buffer_size,
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
    let session_epoch_f32 = session_epoch.clone();
    let session_epoch_i16 = session_epoch;
    let stream = match config.sample_format() {
        SampleFormat::F32 => device.build_output_stream(
            &stream_config,
            {
                let mut playback_started = PlayoutState::default();
                move |out: &mut [f32], _| {
                    #[cfg(target_os = "windows")]
                    crate::audio_thread_win::enter();
                    ticks_f32.fetch_add(1, Ordering::Relaxed);
                    fill_callback_for_epoch(
                        out,
                        &mut consumer,
                        &mut playback_started,
                        stream_epoch,
                        &session_epoch_f32,
                    );
                }
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => {
            let mut tmp = vec![0f32; 0];
            let mut playback_started = PlayoutState::default();
            device.build_output_stream(
                &stream_config,
                move |out: &mut [i16], _| {
                    #[cfg(target_os = "windows")]
                    crate::audio_thread_win::enter();
                    ticks_i16.fetch_add(1, Ordering::Relaxed);
                    tmp.resize(out.len(), 0.0);
                    fill_callback_for_epoch(
                        &mut tmp,
                        &mut consumer,
                        &mut playback_started,
                        stream_epoch,
                        &session_epoch_i16,
                    );
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
    Ok(StreamCtx {
        stream,
        built_at: Instant::now(),
        device_name,
        producer,
        callback_ticks,
        stream_error,
    })
}

fn playback_stream_config(
    config: &SupportedStreamConfig,
    low_latency_available: bool,
) -> StreamConfig {
    let mut stream_config = config.config();
    let frames = if low_latency_available {
        PLAYBACK_BUFFER_FRAMES
    } else {
        1_920
    };
    stream_config.buffer_size = match *config.buffer_size() {
        SupportedBufferSize::Range { min, max } => BufferSize::Fixed(frames.clamp(min, max)),
        SupportedBufferSize::Unknown => BufferSize::Default,
    };
    stream_config
}

#[derive(Default)]
struct PlayoutState {
    started: bool,
    buffering_since: Option<Instant>,
}

fn fill_callback(out: &mut [f32], consumer: &mut ringbuf::HeapCons<f32>, state: &mut PlayoutState) {
    fill_callback_at(out, consumer, state, Instant::now());
}

fn fill_callback_at(
    out: &mut [f32],
    consumer: &mut ringbuf::HeapCons<f32>,
    state: &mut PlayoutState,
    now: Instant,
) {
    // Only the consumer advances the read cursor: the producer remains SPSC.
    // Dropping oldest PCM here keeps new Wi-Fi bursts close to live instead
    // of rejecting newer audio while a full ring slowly plays old samples.
    let buffered = consumer.occupied_len();
    let target = JITTER_TARGET_SAMPLES.max(out.len());
    // A callback may legitimately consume 40 ms at once. Keep one callback
    // quantum above the target so a normal steady cycle is not called a burst.
    if buffered > JITTER_MAX_SAMPLES.max(target + out.len()) {
        let skip = (buffered - target) / CHANNELS as usize * CHANNELS as usize;
        let skipped = consumer.skip(skip);
        PLAYBACK_CATCHUP_SAMPLES.fetch_add(skipped as u64, Ordering::Relaxed);
    }
    if consumer.is_empty() {
        out.fill(0.0);
        if state.started {
            PLAYBACK_UNDERRUN_SAMPLES.fetch_add(out.len() as u64, Ordering::Relaxed);
        }
        *state = PlayoutState::default();
        return;
    }
    if !state.started {
        // DTX/sparse audio may contain less than 60 ms in total. Bound the
        // wait in wall time instead of retaining a short sound until the next
        // sound arrives. Continuous audio builds the normal 60 ms reserve.
        let since = state.buffering_since.get_or_insert(now);
        if consumer.occupied_len() < JITTER_TARGET_SAMPLES
            && now.saturating_duration_since(*since) < Duration::from_millis(60)
        {
            out.fill(0.0);
            return;
        }
        state.started = true;
        state.buffering_since = None;
    }
    let popped = consumer.pop_slice(out);
    for s in &mut out[popped..] {
        *s = 0.0;
    }
    if popped < out.len() {
        PLAYBACK_UNDERRUN_SAMPLES.fetch_add((out.len() - popped) as u64, Ordering::Relaxed);
        *state = PlayoutState::default();
    }
}

fn fill_callback_for_epoch(
    out: &mut [f32],
    consumer: &mut ringbuf::HeapCons<f32>,
    state: &mut PlayoutState,
    stream_epoch: u64,
    session_epoch: &AtomicU64,
) {
    if session_epoch.load(Ordering::Acquire) != stream_epoch {
        out.fill(0.0);
        *state = PlayoutState::default();
        return;
    }
    fill_callback(out, consumer, state);
}

/// Recover small losses only while buffered audio can still preserve continuity.
/// Long Wi-Fi stalls have already elapsed in wall time; replaying up to 100 ms
/// of PLC after that stall would put current video audio further behind.
fn concealment_frames(previous: Option<u32>, seq: u32, buffered: usize) -> u32 {
    let lost = crate::codec::frames_lost(previous, seq);
    if buffered == 0 || lost > 2 {
        return 0;
    }
    let room = (JITTER_MAX_SAMPLES.saturating_sub(buffered) / FRAME_SAMPLES_INTERLEAVED)
        .saturating_sub(1) as u32;
    lost.min(room)
}

fn push_samples(producer: &mut ringbuf::HeapProd<f32>, samples: &[f32], dropped_samples: &mut u64) {
    if producer.vacant_len() < samples.len() {
        *dropped_samples = dropped_samples.saturating_add(samples.len() as u64);
        return;
    }
    let pushed = producer.push_slice(samples);
    debug_assert_eq!(pushed, samples.len());
}

fn default_device_changed(
    following_default: bool,
    active: Option<&str>,
    observed_default: Option<&str>,
) -> bool {
    following_default
        && matches!(
            (active, observed_default),
            (Some(active), Some(observed)) if active != observed
        )
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
        .context("no default output config")?;
    warn!(
        rate = default.sample_rate().0,
        channels = default.channels(),
        format = ?default.sample_format(),
        "no exact match for 48 kHz stereo f32 — falling back to device default"
    );
    Ok(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_default_device_query_does_not_block_decoder_or_queue_requests() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut first = true;
        let probe = DefaultDeviceProbe::spawn(move || {
            if first {
                first = false;
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            Some("new endpoint".to_string())
        })
        .unwrap();
        assert!(probe.request());
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        // The injected OS query is still blocked. The audio worker must be
        // able to poll and continue decoding, with at most one queued query.
        assert_eq!(probe.poll(), None);
        assert!(probe.request());
        assert!(!probe.request());
        release_tx.send(()).unwrap();
        assert_eq!(
            probe.results.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some("new endpoint".to_string())
        );
    }

    #[test]
    fn default_device_probe_shutdown_does_not_wait_for_os_query() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let probe = DefaultDeviceProbe::spawn(move || {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            finished_tx.send(()).unwrap();
            Some("endpoint".to_string())
        })
        .unwrap();
        assert!(probe.request());
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(probe);
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn session_started_before_worker_is_still_a_prewarm_transition() {
        let mut decoder = OpusDecoder::new().unwrap();
        let mut observed_epoch = 0;
        assert!(synchronize_decoder_session(&mut decoder, &mut observed_epoch, 1).unwrap());
        assert_eq!(
            playback_lifecycle_action(false, true, false, Duration::ZERO),
            PlaybackLifecycleAction::Build
        );
        assert!(!synchronize_decoder_session(&mut decoder, &mut observed_epoch, 1).unwrap());
    }
    use crate::codec::OpusEncoder;

    #[test]
    fn unexpected_playback_worker_exit_marks_backend_degraded() {
        let status = BackendStatus::new(BackendState::Active);
        {
            let _guard = PlaybackWorkerStatusGuard::new(status.clone());
        }
        assert_eq!(status.get(), BackendState::Degraded);
    }

    #[test]
    fn idle_receiver_does_not_hold_output_device_open() {
        assert_eq!(
            playback_lifecycle_action(false, false, false, Duration::ZERO),
            PlaybackLifecycleAction::None,
            "startup without peer audio must not power up the output device"
        );
        assert_eq!(
            playback_lifecycle_action(false, true, false, Duration::ZERO),
            PlaybackLifecycleAction::Build,
            "the first real peer frame must lazily request playback"
        );
        assert_eq!(
            playback_lifecycle_action(true, false, false, IDLE_PAUSE_AFTER),
            PlaybackLifecycleAction::Release,
            "idle must release the stream handle so SoundWire/headphone DACs power down"
        );
    }

    #[test]
    fn active_route_keeps_output_warm_between_sounds() {
        assert_eq!(
            effective_playback_idle(true, IDLE_PAUSE_AFTER),
            Duration::ZERO,
            "an enabled peer route must not cold-start WASAPI after every short silence"
        );
        assert_eq!(
            effective_playback_idle(false, IDLE_PAUSE_AFTER),
            IDLE_PAUSE_AFTER,
            "retiring the route must still release the output device"
        );
    }

    #[test]
    fn stale_session_demand_does_not_wake_playback() {
        let stale_demand = current_session_playback_demand(Some(7), 8);

        assert_eq!(
            playback_lifecycle_action(false, stale_demand, false, Duration::ZERO),
            PlaybackLifecycleAction::None,
            "a failed build requested by an old peer session must not open the device after reset"
        );
    }

    #[test]
    fn old_session_enqueue_is_never_retagged_as_current() {
        let stale_session = PlaybackSession::new(7);
        let current_session = PlaybackSession::new(8);
        assert_eq!(
            accepted_session_epoch(&stale_session, &current_session),
            None
        );
        assert_eq!(
            accepted_session_epoch(&current_session, &current_session),
            Some(8)
        );
    }

    #[test]
    fn stale_session_callback_never_renders_old_ring() {
        let ring = HeapRb::<f32>::new(RING_CAPACITY);
        let (mut producer, mut consumer) = ring.split();
        assert_eq!(
            producer.push_slice(&vec![0.25; JITTER_TARGET_SAMPLES]),
            JITTER_TARGET_SAMPLES
        );
        let mut out = vec![f32::NAN; FRAME_SAMPLES_INTERLEAVED];
        let mut playback_started = PlayoutState::default();
        let session_epoch = AtomicU64::new(8);

        fill_callback_for_epoch(
            &mut out,
            &mut consumer,
            &mut playback_started,
            7,
            &session_epoch,
        );

        assert!(
            out.iter().all(|&sample| sample == 0.0),
            "a callback created for an old peer session must fail closed after reset"
        );
    }

    #[test]
    fn failed_build_demand_expires_while_idle() {
        assert_eq!(
            playback_lifecycle_action(false, true, false, IDLE_PAUSE_AFTER),
            PlaybackLifecycleAction::None,
            "a failed lazy build must stop retrying after the receiver becomes idle"
        );
    }

    #[test]
    fn idle_expiry_clears_pending_build_and_retry_deadline() {
        let now = Instant::now();
        let mut pending_build_epoch = Some(8);
        let mut next_build_attempt = now + Duration::from_secs(3);

        expire_idle_build_state(
            false,
            IDLE_PAUSE_AFTER,
            &mut pending_build_epoch,
            &mut next_build_attempt,
            now,
        );

        assert_eq!(pending_build_epoch, None);
        assert_eq!(next_build_attempt, now);
    }

    #[test]
    fn frame_crossing_idle_boundary_gets_a_fresh_build_deadline() {
        let started = Instant::now();
        let frame_received_at = started + IDLE_PAUSE_AFTER + Duration::from_millis(100);
        let mut last_frame_received = started;
        let mut pending_build_epoch = Some(8);
        let mut next_build_attempt = started + Duration::from_secs(3);

        record_current_frame_demand(
            false,
            8,
            frame_received_at,
            &mut last_frame_received,
            &mut pending_build_epoch,
            &mut next_build_attempt,
        );

        assert_eq!(last_frame_received, frame_received_at);
        assert_eq!(pending_build_epoch, Some(8));
        assert_eq!(next_build_attempt, frame_received_at);
    }

    #[test]
    fn current_frame_queued_during_blocking_build_preserves_demand() {
        let ingress_order = Mutex::new(());
        let session = PlaybackSession::new(8);
        session.note_accepted_frame();
        session.note_accepted_frame();
        session.note_consumed_frame();
        let now = Instant::now();
        let mut last_frame_received = now - IDLE_PAUSE_AFTER - Duration::from_millis(100);
        let mut pending_build_epoch = Some(8);
        let mut next_build_attempt = now + Duration::from_millis(900);

        let saw_current = refresh_build_demand_from_session_ingress(
            &ingress_order,
            &session,
            8,
            false,
            now,
            BuildDemandState {
                last_frame_received: &mut last_frame_received,
                pending_build_epoch: &mut pending_build_epoch,
                next_build_attempt: &mut next_build_attempt,
            },
        );

        assert!(saw_current);
        assert_eq!(last_frame_received, now);
        assert_eq!(pending_build_epoch, Some(8));
        assert_eq!(next_build_attempt, now);
    }

    #[test]
    fn new_session_frame_queued_during_old_build_is_deferred() {
        let ingress_order = Mutex::new(());
        let new_session = PlaybackSession::new(9);
        new_session.note_accepted_frame();
        let now = Instant::now();
        let old_last_frame_received = now - Duration::from_millis(100);
        let mut last_frame_received = old_last_frame_received;
        let mut pending_build_epoch = Some(8);
        let old_deadline = now + Duration::from_millis(900);
        let mut next_build_attempt = old_deadline;

        let saw_current = refresh_build_demand_from_session_ingress(
            &ingress_order,
            &new_session,
            8,
            false,
            now,
            BuildDemandState {
                last_frame_received: &mut last_frame_received,
                pending_build_epoch: &mut pending_build_epoch,
                next_build_attempt: &mut next_build_attempt,
            },
        );

        assert!(!saw_current);
        assert!(new_session.has_pending_ingress());
        assert_eq!(last_frame_received, old_last_frame_received);
        assert_eq!(pending_build_epoch, Some(8));
        assert_eq!(next_build_attempt, old_deadline);
    }

    #[test]
    fn current_session_pending_ingress_preserves_demand_without_draining_stale_queue() {
        let ingress_order = Mutex::new(());
        let stale_session = PlaybackSession::new(8);
        let current_session = PlaybackSession::new(9);
        current_session.note_accepted_frame();
        current_session.note_accepted_frame();
        current_session.note_consumed_frame();

        let (tx, rx) = mpsc::sync_channel(PLAYBACK_QUEUE_CAPACITY);
        tx.send(QueuedAudioFrame {
            session: stale_session,
            queued_at: Instant::now(),
            frame: AudioFrame {
                stream: StreamKind::SysOut,
                seq: 1,
                opus: Vec::new(),
            },
        })
        .unwrap();
        let now = Instant::now();
        let mut last_frame_received = now - IDLE_PAUSE_AFTER - Duration::from_millis(100);
        let mut pending_build_epoch = Some(9);
        let mut next_build_attempt = now + Duration::from_millis(900);

        assert!(refresh_build_demand_from_session_ingress(
            &ingress_order,
            &current_session,
            9,
            false,
            now,
            BuildDemandState {
                last_frame_received: &mut last_frame_received,
                pending_build_epoch: &mut pending_build_epoch,
                next_build_attempt: &mut next_build_attempt,
            },
        ));
        assert_eq!(last_frame_received, now);
        assert_eq!(pending_build_epoch, Some(9));
        assert_eq!(next_build_attempt, now);
        assert_eq!(rx.try_recv().unwrap().session.epoch(), 8);
    }

    #[test]
    fn successful_enqueue_stays_pending_until_the_worker_consumes_it() {
        let (tx, rx) = mpsc::sync_channel(PLAYBACK_QUEUE_CAPACITY);
        let ingress_order = Arc::new(parking_lot::Mutex::new(()));
        let session = PlaybackSession::new(8);
        let frame = AudioFrame {
            stream: StreamKind::SysOut,
            seq: 1,
            opus: Vec::new(),
        };

        assert!(
            publish_current_session_frame(&tx, &ingress_order, &session, &session, frame).is_ok()
        );
        assert!(session.has_pending_ingress());

        let queued = rx.recv().unwrap();
        consume_published_frame(&ingress_order, &queued);

        assert!(!session.has_pending_ingress());
    }

    #[test]
    fn full_queue_does_not_publish_phantom_session_demand() {
        let (tx, rx) = mpsc::sync_channel(1);
        let ingress_order = Mutex::new(());
        let session = PlaybackSession::new(8);
        let frame = || AudioFrame {
            stream: StreamKind::SysOut,
            seq: 1,
            opus: Vec::new(),
        };

        assert!(
            publish_current_session_frame(&tx, &ingress_order, &session, &session, frame()).is_ok()
        );
        assert!(matches!(
            publish_current_session_frame(&tx, &ingress_order, &session, &session, frame()),
            Err(mpsc::TrySendError::Full(_))
        ));
        let queued = rx.recv().unwrap();
        consume_published_frame(&ingress_order, &queued);

        assert!(!session.has_pending_ingress());
    }

    #[test]
    fn disconnected_queue_does_not_publish_session_demand() {
        let (tx, rx) = mpsc::sync_channel(1);
        let ingress_order = Mutex::new(());
        let session = PlaybackSession::new(8);
        drop(rx);

        assert!(matches!(
            publish_current_session_frame(
                &tx,
                &ingress_order,
                &session,
                &session,
                AudioFrame {
                    stream: StreamKind::SysOut,
                    seq: 1,
                    opus: Vec::new(),
                },
            ),
            Err(mpsc::TrySendError::Disconnected(_))
        ));
        assert!(!session.has_pending_ingress());
    }

    #[test]
    fn stale_token_does_not_publish_into_the_current_session() {
        let (tx, rx) = mpsc::sync_channel(1);
        let ingress_order = Mutex::new(());
        let stale_session = PlaybackSession::new(8);
        let current_session = PlaybackSession::new(9);

        assert!(
            !publish_current_session_frame(
                &tx,
                &ingress_order,
                &stale_session,
                &current_session,
                AudioFrame {
                    stream: StreamKind::SysOut,
                    seq: 1,
                    opus: Vec::new(),
                },
            )
            .unwrap()
        );
        assert!(!stale_session.has_pending_ingress());
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn different_token_with_same_epoch_cannot_publish_as_current() {
        let (tx, rx) = mpsc::sync_channel(1);
        let ingress_order = Mutex::new(());
        let current_session = PlaybackSession::new(8);
        let other_backend_session = PlaybackSession::new(8);

        assert!(
            !publish_current_session_frame(
                &tx,
                &ingress_order,
                &other_backend_session,
                &current_session,
                AudioFrame {
                    stream: StreamKind::SysOut,
                    seq: 1,
                    opus: Vec::new(),
                },
            )
            .unwrap()
        );
        assert!(!other_backend_session.has_pending_ingress());
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn ending_current_session_invalidates_a_blocking_build_before_install() {
        let session_epoch = AtomicU64::new(8);
        let session_active = AtomicBool::new(true);
        let session = PlaybackSession::new(8);
        let current_session = Mutex::new(session.clone());
        session.note_accepted_frame();

        assert!(retire_current_playback_session(
            &current_session,
            &session_epoch,
            &session_active,
            &session,
        ));
        let retired_epoch = session_epoch.load(Ordering::Acquire);
        assert_eq!(retired_epoch, 9);
        assert!(!session_active.load(Ordering::Acquire));
        assert!(!session.same_identity(&current_session.lock()));
        assert!(!current_session_playback_demand(
            Some(session.epoch()),
            retired_epoch,
        ));
    }

    #[test]
    fn post_build_demand_read_linearizes_with_enqueue_publication() {
        let ingress_order = Arc::new(Mutex::new(()));
        let session = PlaybackSession::new(8);
        let publication_guard = ingress_order.lock();
        let (started_tx, started_rx) = mpsc::channel();
        let worker_order = ingress_order.clone();
        let worker_session = session.clone();

        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let now = Instant::now();
            let mut last_frame_received = now - IDLE_PAUSE_AFTER;
            let mut pending_build_epoch = Some(8);
            let mut next_build_attempt = now + Duration::from_secs(3);
            refresh_build_demand_from_session_ingress(
                &worker_order,
                &worker_session,
                8,
                false,
                now,
                BuildDemandState {
                    last_frame_received: &mut last_frame_received,
                    pending_build_epoch: &mut pending_build_epoch,
                    next_build_attempt: &mut next_build_attempt,
                },
            )
        });

        started_rx.recv().unwrap();
        session.note_accepted_frame();
        drop(publication_guard);

        assert!(worker.join().unwrap());
    }

    #[test]
    fn new_session_frame_requires_old_stream_invalidation() {
        let mut observed_epoch = 7;

        assert!(
            synchronize_session_epoch(&mut observed_epoch, 8),
            "a new-session frame must invalidate the old stream before decode"
        );
        assert_eq!(observed_epoch, 8);
    }

    #[test]
    fn new_session_resets_opus_predictive_history() {
        let mut encoder = OpusEncoder::new(96_000, StreamKind::SysOut).unwrap();
        let tone = (0..FRAME_SAMPLES_INTERLEAVED)
            .map(|sample| (std::f32::consts::TAU * 1_000.0 * sample as f32 / 48_000.0).sin())
            .collect::<Vec<_>>();
        let payload = encoder.encode(&tone).unwrap();
        let mut decoder = OpusDecoder::new().unwrap();
        let mut scratch = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        decoder.decode(&payload, &mut scratch).unwrap();
        decoder.decode_plc(&mut scratch).unwrap();
        assert!(scratch.iter().any(|sample| sample.abs() > 1.0e-4));

        let mut observed_epoch = 7;
        assert!(synchronize_decoder_session(&mut decoder, &mut observed_epoch, 8).unwrap());
        scratch.fill(f32::NAN);
        decoder.decode_plc(&mut scratch).unwrap();

        assert!(
            scratch.iter().all(|sample| sample.abs() <= 1.0e-6),
            "new session inherited Opus PLC history from the old peer"
        );
    }

    #[test]
    fn playback_buffer_uses_the_platform_period_when_supported() {
        let supported = SupportedStreamConfig::new(
            2,
            cpal::SampleRate(48_000),
            SupportedBufferSize::Range {
                min: 128,
                max: 2_048,
            },
            SampleFormat::F32,
        );
        assert_eq!(
            playback_stream_config(&supported, true).buffer_size,
            BufferSize::Fixed(if cfg!(target_os = "windows") {
                960
            } else {
                1_920
            })
        );
    }

    #[test]
    fn playback_buffer_clamps_to_device_range() {
        let supported = SupportedStreamConfig::new(
            2,
            cpal::SampleRate(48_000),
            SupportedBufferSize::Range {
                min: 2_048,
                max: 4_096,
            },
            SampleFormat::F32,
        );
        assert_eq!(
            playback_stream_config(&supported, true).buffer_size,
            BufferSize::Fixed(2_048)
        );
    }

    #[test]
    fn playback_keeps_the_forty_ms_device_cushion_without_audio_scheduling() {
        let supported = SupportedStreamConfig::new(
            2,
            cpal::SampleRate(48_000),
            SupportedBufferSize::Range {
                min: 128,
                max: 2048,
            },
            SampleFormat::F32,
        );
        assert_eq!(
            playback_stream_config(&supported, false).buffer_size,
            BufferSize::Fixed(1920)
        );
    }

    #[test]
    fn push_samples_drops_a_whole_frame_when_ring_is_full() {
        let ring = HeapRb::<f32>::new(2);
        let (mut producer, consumer) = ring.split();
        let mut dropped = 0;

        push_samples(&mut producer, &[0.0, 0.0, 0.0], &mut dropped);

        assert_eq!(dropped, 3);
        assert_eq!(consumer.occupied_len(), 0);
    }

    #[test]
    fn default_device_change_rebuilds_only_when_following_default() {
        assert!(default_device_changed(
            true,
            Some("Speakers"),
            Some("HyperX Cloud III")
        ));
        assert!(!default_device_changed(
            true,
            Some("Speakers"),
            Some("Speakers")
        ));
        assert!(!default_device_changed(
            false,
            Some("Speakers"),
            Some("HyperX Cloud III")
        ));
        assert!(!default_device_changed(true, Some("Speakers"), None));
    }

    #[test]
    fn concealment_does_not_replay_elapsed_wifi_outages() {
        assert_eq!(concealment_frames(Some(1), 10, 0), 0);
        assert_eq!(
            concealment_frames(Some(1), 10, FRAME_SAMPLES_INTERLEAVED * 2),
            0
        );
        assert_eq!(
            concealment_frames(Some(1), 3, FRAME_SAMPLES_INTERLEAVED * 2),
            1
        );
        assert_eq!(
            concealment_frames(Some(1), 4, FRAME_SAMPLES_INTERLEAVED * 5),
            0
        );
    }

    #[test]
    fn catchup_preserves_stereo_alignment_and_normal_reserve() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
        let stereo: Vec<_> = (0..RING_CAPACITY / 2).flat_map(|_| [0.25, -0.5]).collect();
        producer.push_slice(&stereo);
        let mut started = PlayoutState {
            started: true,
            buffering_since: None,
        };
        let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        fill_callback(&mut out, &mut consumer, &mut started);
        assert!(out.chunks_exact(2).all(|pair| pair == [0.25, -0.5]));
        assert_eq!(consumer.occupied_len(), JITTER_TARGET_SAMPLES - out.len());
    }

    #[test]
    fn empty_audio_resets_the_short_sound_deadline() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
        let now = Instant::now();
        let mut state = PlayoutState {
            started: false,
            buffering_since: Some(now - Duration::from_secs(1)),
        };
        let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED * 2];
        fill_callback_at(&mut out, &mut consumer, &mut state, now);
        producer.push_slice(&vec![0.25; out.len()]);
        fill_callback_at(&mut out, &mut consumer, &mut state, now);
        assert!(out.iter().all(|&sample| sample == 0.0));
        fill_callback_at(
            &mut out,
            &mut consumer,
            &mut state,
            now + Duration::from_millis(60),
        );
        assert!(out.iter().all(|&sample| sample == 0.25));
    }

    #[test]
    fn a_short_sound_does_not_wait_for_the_next_sound_to_start_playing() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
        producer.push_slice(&vec![0.25; FRAME_SAMPLES_INTERLEAVED * 2]);
        let mut started = PlayoutState::default();
        let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED * 2];
        let now = Instant::now();
        fill_callback_at(&mut out, &mut consumer, &mut started, now);
        assert!(out.iter().all(|&sample| sample == 0.0));
        fill_callback_at(
            &mut out,
            &mut consumer,
            &mut started,
            now + Duration::from_millis(60),
        );
        assert!(
            out.iter().all(|&sample| sample == 0.25),
            "40 ms sound is still waiting after the jitter reserve's 60 ms deadline"
        );
    }

    #[test]
    fn normal_forty_ms_callbacks_do_not_skip_audio_at_the_high_water_mark() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
        let mut dropped = 0;
        let mut next_frame = 0;
        let mut enqueue = |producer: &mut ringbuf::HeapProd<f32>, count: usize| {
            for _ in 0..count {
                push_samples(
                    producer,
                    &vec![next_frame as f32; FRAME_SAMPLES_INTERLEAVED],
                    &mut dropped,
                );
                next_frame += 1;
            }
        };
        enqueue(&mut producer, 5);
        let mut started = PlayoutState {
            started: true,
            buffering_since: None,
        };
        for callback in 0..50 {
            let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED * 2];
            fill_callback(&mut out, &mut consumer, &mut started);
            assert!(
                out[..FRAME_SAMPLES_INTERLEAVED]
                    .iter()
                    .all(|&v| v == (callback * 2) as f32)
            );
            assert!(
                out[FRAME_SAMPLES_INTERLEAVED..]
                    .iter()
                    .all(|&v| v == (callback * 2 + 1) as f32)
            );
            enqueue(&mut producer, 2);
        }
        assert_eq!(dropped, 0);
    }

    #[test]
    fn a_six_frame_delivery_burst_returns_to_the_normal_reserve() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
        let mut dropped = 0;
        for frame in 0..6 {
            push_samples(
                &mut producer,
                &vec![frame as f32; FRAME_SAMPLES_INTERLEAVED],
                &mut dropped,
            );
        }
        let mut started = PlayoutState {
            started: true,
            buffering_since: None,
        };
        let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED * 2];
        fill_callback(&mut out, &mut consumer, &mut started);
        let audible_age_ms = (5.0 - out[0]) * 20.0;
        assert!(
            audible_age_ms <= 80.0,
            "moderate Wi-Fi burst left {audible_age_ms} ms of queued age"
        );
    }

    #[test]
    fn wifi_delivery_burst_does_not_keep_playing_old_audio() {
        // 200 ms of packets can arrive together after a Wi-Fi scheduling stall.
        // Samples carry capture-frame markers, so this checks audible age on
        // the real producer -> device callback path, without an audio device.
        let (mut producer, mut consumer) = HeapRb::<f32>::new(RING_CAPACITY).split();
        let mut dropped = 0;
        for frame in 0..10 {
            push_samples(
                &mut producer,
                &vec![frame as f32; FRAME_SAMPLES_INTERLEAVED],
                &mut dropped,
            );
        }
        let mut started = PlayoutState {
            started: true,
            buffering_since: None,
        };
        let mut out = vec![0.0; FRAME_SAMPLES_INTERLEAVED];
        fill_callback(&mut out, &mut consumer, &mut started);
        let audible_age_ms = (9.0 - out[0]) * 20.0;
        assert!(
            audible_age_ms <= 100.0,
            "Wi-Fi catch-up audio is {audible_age_ms} ms behind the newest received frame"
        );
    }

    #[test]
    fn jitter_reserve_survives_a_sixty_millisecond_delivery_gap() {
        // SoundWire/USB drivers can delay the decoder thread for 50–60 ms
        // without reporting a device error. Once playback has started, the
        // prebuffer must cover that gap without emitting a zero-filled callback
        // (an audible click/crackle).
        let ring = HeapRb::<f32>::new(RING_CAPACITY);
        let (mut producer, mut consumer) = ring.split();
        let tone = vec![0.25; JITTER_TARGET_SAMPLES];
        assert_eq!(producer.push_slice(&tone), tone.len());

        let mut playback_started = PlayoutState::default();
        let callback_samples = SAMPLE_RATE as usize / 100 * CHANNELS as usize; // 10 ms
        for callback in 0..6 {
            let mut out = vec![f32::NAN; callback_samples];
            fill_callback(&mut out, &mut consumer, &mut playback_started);
            assert!(
                out.iter().all(|&sample| sample == 0.25),
                "callback {callback} underruns before a 60 ms scheduling gap ends"
            );
        }
    }

    #[test]
    fn decoder_stalls_expire_old_ingress_but_not_normal_delivery() {
        let queued = Instant::now();
        let built = queued - Duration::from_secs(1);
        for delay in [20, 50] {
            assert!(!super::queued_frame_expired(
                queued,
                Some(built),
                queued + Duration::from_millis(delay)
            ));
        }
        for delay in [100, 160] {
            assert!(super::queued_frame_expired(
                queued,
                Some(built),
                queued + Duration::from_millis(delay)
            ));
        }
    }

    #[test]
    fn cold_device_creation_preserves_the_first_short_sound_with_a_finite_grace() {
        let queued = Instant::now();
        let ready = queued + Duration::from_millis(160);
        assert!(!super::queued_frame_expired(queued, Some(ready), ready));
        assert!(super::queued_frame_expired(
            queued,
            Some(ready),
            queued + Duration::from_millis(301)
        ));
        assert!(super::queued_frame_expired(
            queued,
            None,
            queued + Duration::from_millis(301)
        ));
    }

    #[test]
    fn expired_frames_advance_ordering_without_concealing_the_discarded_backlog() {
        let mut sequence = super::PlaybackSequence::default();
        assert!(sequence.accept(10));
        assert!(sequence.accept(11)); // expired; advance without decoding
        assert!(sequence.accept(12)); // expired
        assert!(!sequence.accept(9)); // late UDP cannot rewind after expiry
        assert_eq!(
            super::concealment_frames(sequence.last(), 13, FRAME_SAMPLES_INTERLEAVED),
            0
        );
    }

    #[test]
    fn playback_ingress_absorbs_observed_eight_frame_burst() {
        let (tx, _rx) = mpsc::sync_channel::<u32>(PLAYBACK_QUEUE_CAPACITY);
        for frame in 0..8 {
            tx.try_send(frame).unwrap_or_else(|_| {
                panic!("playback queue dropped frame {frame} in an 8-frame burst")
            });
        }
    }

    #[test]
    fn new_peer_session_accepts_sequence_restart_and_u32_wrap() {
        let mut sequence = PlaybackSequence::default();
        assert!(sequence.accept(120_000));
        assert!(
            !sequence.accept(3),
            "old session state must reject a rewind"
        );

        sequence.reset();
        assert!(
            sequence.accept(3),
            "a restarted peer begins at zero and must become audible immediately"
        );

        sequence.reset();
        assert!(sequence.accept(u32::MAX));
        assert!(
            sequence.accept(0),
            "serial-number comparison must accept natural u32 wrap-around"
        );
    }
}
