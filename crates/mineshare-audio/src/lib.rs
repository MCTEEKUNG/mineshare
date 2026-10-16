//! Cross-platform audio bridge.
//!
//! M3 Slice 1 covers Win→Ubuntu **sysout**: the controller's WASAPI
//! loopback capture is encoded with Opus and forwarded over the same
//! encrypted UDP channel as `mineshare-input` events; the receiver
//! decodes and plays back through `cpal`.
//!
//! Slice 2 will reverse the direction (PipeWire monitor on Linux),
//! Slice 3 covers mic forwarding (capture mic on side A, route into a
//! virtual source / VB-CABLE on side B).
//!
//! ## Format
//!
//! All Opus frames carry the same canonical PCM shape:
//!   * 48 kHz sample rate
//!   * 2 channels (interleaved stereo)
//!   * 20 ms frames (= 960 samples per channel)
//!
//! Capture devices that don't natively produce 48 kHz stereo are
//! resampled / channel-mapped at the platform-specific capture
//! boundary so the wire format stays uniform.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub mod codec;
pub mod cpal_mic;
pub mod playback;
#[cfg(target_os = "windows")]
mod process_loopback_win;
pub mod resample;

#[cfg(target_os = "windows")]
pub mod virtual_mic_win;

#[cfg(any(target_os = "linux", test))]
pub mod pipewire_monitor;
#[cfg(any(target_os = "linux", test))]
pub mod virtual_mic_linux;

/// Audio kind tag — both directions of the bridge ride the same wire,
/// so the receiver needs to know whether a frame goes to the speakers
/// (sysout) or to a virtual mic source (mic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamKind {
    SysOut,
    Mic,
}

/// One Opus-encoded 20 ms frame on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub stream: StreamKind,
    /// Monotonically increasing per-stream counter — used by the jitter
    /// buffer to detect drops/reordering. Wraps at u32 max (≈ 24 days
    /// at 50 fps).
    pub seq: u32,
    /// Raw Opus payload. Decoder yields 48 kHz / 2-channel PCM.
    pub opus: Vec<u8>,
}

/// Canonical capture / playback shape — see module docs.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
/// 20 ms @ 48 kHz = 960 samples per channel = 1920 interleaved.
pub const FRAME_SAMPLES_PER_CHANNEL: usize = 960;
pub const FRAME_SAMPLES_INTERLEAVED: usize = FRAME_SAMPLES_PER_CHANNEL * CHANNELS as usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum BackendState {
    Idle = 0,
    Starting = 1,
    Active = 2,
    Degraded = 3,
    Stopped = 4,
}

#[derive(Clone)]
pub struct BackendStatus {
    state: Arc<AtomicU8>,
}

impl BackendStatus {
    pub fn new(initial: BackendState) -> Self {
        Self {
            state: Arc::new(AtomicU8::new(initial as u8)),
        }
    }

    pub fn set(&self, state: BackendState) {
        self.state.store(state as u8, Ordering::Release);
    }

    pub fn get(&self) -> BackendState {
        match self.state.load(Ordering::Acquire) {
            0 => BackendState::Idle,
            1 => BackendState::Starting,
            2 => BackendState::Active,
            3 => BackendState::Degraded,
            _ => BackendState::Stopped,
        }
    }
}

impl Default for BackendStatus {
    fn default() -> Self {
        Self::new(BackendState::Degraded)
    }
}

/// Construct the platform-specific sysout capture: process-isolated WASAPI
/// on Windows, PipeWire monitor on Linux.
pub fn make_sysout_capture() -> anyhow::Result<Box<dyn AudioCapture>> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(process_loopback_win::ProcessLoopbackCapture::new()))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(pipewire_monitor::PipewireMonitor::new()?))
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("sysout capture is not implemented on this platform")
    }
}

/// Construct a mic capture handle on the system default input
/// device. Cross-platform via cpal.
pub fn make_mic_capture() -> anyhow::Result<Box<dyn AudioCapture>> {
    Ok(Box::new(cpal_mic::CpalMic::new()?))
}

/// 5-second cache for the output / input device lists so the
/// GUI's Devices tab can poll cheaply without re-entering cpal's
/// COM-heavy enumeration on every tick. Win cpal enumeration
/// takes 100–500 ms per call (each device.name() rounds-trips
/// through IMMDeviceEnumerator), and the GUI used to call this
/// every 2.5 s — that alone pinned ~10–20 % of CPU on slower
/// laptops and made the WebView feel laggy. The cache also
/// short-circuits repeat calls during a device-switch flurry
/// when the playback thread rebuilds and cross-checks the list.
const DEVICE_CACHE_TTL: Duration = Duration::from_secs(5);

static OUTPUT_DEVICE_CACHE: parking_lot::Mutex<Option<(Instant, Vec<DeviceInfo>)>> =
    parking_lot::Mutex::new(None);
static INPUT_DEVICE_CACHE: parking_lot::Mutex<Option<(Instant, Vec<DeviceInfo>)>> =
    parking_lot::Mutex::new(None);

fn cached_or_compute(
    cache: &parking_lot::Mutex<Option<(Instant, Vec<DeviceInfo>)>>,
    compute: impl FnOnce() -> Vec<DeviceInfo>,
) -> Vec<DeviceInfo> {
    {
        let g = cache.lock();
        if let Some((ts, ref v)) = *g
            && ts.elapsed() < DEVICE_CACHE_TTL
        {
            return v.clone();
        }
    }
    let fresh = compute();
    *cache.lock() = Some((Instant::now(), fresh.clone()));
    fresh
}

/// Enumerate cpal output devices on the local host with the
/// default flagged. Used by the GUI's Devices tab to surface
/// what the bridge would render peer audio into. Failures are
/// non-fatal — we return what we got and log the rest.
pub fn list_output_devices() -> Vec<DeviceInfo> {
    cached_or_compute(&OUTPUT_DEVICE_CACHE, list_output_devices_uncached)
}

/// Bypass the cache. Called by `resolve_output_device()` when the
/// runtime needs to find a freshly-selected device by name and
/// can't risk a stale 5 s cache miss.
pub fn list_output_devices_uncached() -> Vec<DeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Ok(iter) = host.output_devices() {
        for d in iter {
            let name = d.name().unwrap_or_else(|_| "?".to_string());
            out.push(DeviceInfo {
                is_default: name == default_name,
                name,
            });
        }
    }
    out
}

/// Enumerate cpal input devices on the local host (microphones).
pub fn list_input_devices() -> Vec<DeviceInfo> {
    cached_or_compute(&INPUT_DEVICE_CACHE, list_input_devices_uncached)
}

pub fn list_input_devices_uncached() -> Vec<DeviceInfo> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Ok(iter) = host.input_devices() {
        for d in iter {
            let name = d.name().unwrap_or_else(|_| "?".to_string());
            out.push(DeviceInfo {
                is_default: name == default_name,
                name,
            });
        }
    }
    out
}

/// Wipe the device-list cache. Called from the GUI's "↻ refresh"
/// button so a freshly-plugged device appears even within the 5 s
/// TTL window.
pub fn invalidate_device_cache() {
    *OUTPUT_DEVICE_CACHE.lock() = None;
    *INPUT_DEVICE_CACHE.lock() = None;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
}

// ----------------------------------------------------------------------------
// Runtime device selection (Stage 8.4)
//
// `None` means "follow the OS default", which is the behaviour the
// daemon shipped with up to M5. When the user picks a device on the
// Devices tab, we stash its name here and bump the version; the
// playback / mic capture threads notice the bump on their next
// poll, drop the cpal stream, and rebuild against the new device.
// Stream re-build pauses audio for ~50 ms — fine for a manual
// device switch.
// ----------------------------------------------------------------------------

static SELECTED_OUTPUT: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
static SELECTED_INPUT: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
static SELECTED_SYSOUT_CAPTURE: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);
static OUTPUT_VERSION: AtomicU64 = AtomicU64::new(0);
static INPUT_VERSION: AtomicU64 = AtomicU64::new(0);
static SYSOUT_CAPTURE_VERSION: AtomicU64 = AtomicU64::new(0);

/// Set the preferred output device by name. Pass `None` to revert
/// to the system default. The change takes effect within ~200 ms,
/// when the playback thread next polls [`output_device_version`].
pub fn set_output_device(name: Option<String>) {
    *SELECTED_OUTPUT.lock() = name;
    OUTPUT_VERSION.fetch_add(1, Ordering::Release);
}

/// Mirror of [`set_output_device`] for the mic capture path.
pub fn set_input_device(name: Option<String>) {
    *SELECTED_INPUT.lock() = name;
    INPUT_VERSION.fetch_add(1, Ordering::Release);
}

/// Select the render endpoint whose mixer is captured as local system audio.
/// This is deliberately separate from peer-audio playback routing.
pub fn set_sysout_capture_device(name: Option<String>) {
    *SELECTED_SYSOUT_CAPTURE.lock() = name;
    SYSOUT_CAPTURE_VERSION.fetch_add(1, Ordering::Release);
}

pub fn selected_output_device() -> Option<String> {
    SELECTED_OUTPUT.lock().clone()
}

pub fn selected_input_device() -> Option<String> {
    SELECTED_INPUT.lock().clone()
}

pub fn selected_sysout_capture_device() -> Option<String> {
    SELECTED_SYSOUT_CAPTURE.lock().clone()
}

pub fn output_device_version() -> u64 {
    OUTPUT_VERSION.load(Ordering::Acquire)
}

pub fn input_device_version() -> u64 {
    INPUT_VERSION.load(Ordering::Acquire)
}

pub fn sysout_capture_device_version() -> u64 {
    SYSOUT_CAPTURE_VERSION.load(Ordering::Acquire)
}

/// Resolve the cpal output device matching the user's selection, or
/// the system default if no selection / not found. Used by the
/// playback thread when (re)building a stream.
pub fn resolve_output_device() -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Some(want) = selected_output_device()
        && let Ok(iter) = host.output_devices()
    {
        for d in iter {
            if d.name().ok().as_deref() == Some(want.as_str()) {
                return Some(d);
            }
        }
        // selection no longer present — fall through to default.
    }
    host.default_output_device()
}

/// Cheap probe used by the playback module while it is following the OS
/// default. Unlike full device enumeration this asks cpal for one endpoint, so
/// polling it at a human-scale interval does not reintroduce the old COM/CPU
/// problem from the Devices page.
pub(crate) fn default_output_device_name() -> Option<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
}

/// Mirror of [`resolve_output_device`] for the mic capture path.
pub fn resolve_input_device() -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Some(want) = selected_input_device()
        && let Ok(iter) = host.input_devices()
    {
        for d in iter {
            if d.name().ok().as_deref() == Some(want.as_str()) {
                return Some(d);
            }
        }
    }
    host.default_input_device()
}

/// Resolve the endpoint used for WASAPI system-audio loopback. A missing
/// explicit selection falls back safely to the current OS default while the
/// preference remains intact for a future hot-plug recovery.
pub fn resolve_sysout_capture_device() -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Some(want) = selected_sysout_capture_device()
        && let Ok(iter) = host.output_devices()
    {
        for d in iter {
            if d.name().ok().as_deref() == Some(want.as_str()) {
                return Some(d);
            }
        }
    }
    host.default_output_device()
}

/// Construct a virtual-mic playback sink — peer mic frames flow into
/// this and apps on the local machine see a "MineShare Mic" input
/// device.
///
///   * Linux: PipeWire `module-null-sink` named `mineshare_mic`
///     (always available).
///   * Windows: VB-CABLE `CABLE Input` (only if user has
///     installed VB-CABLE — returns Err otherwise so the caller can
///     log instructions and continue without virtual-mic playback).
pub fn make_virtual_mic_playback() -> anyhow::Result<Box<dyn AudioPlayback>> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(virtual_mic_win::VbCablePlayback::new()?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(virtual_mic_linux::PipewireVirtualMic::new()?))
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        anyhow::bail!("virtual mic playback is not implemented on this platform")
    }
}

/// Construct a `cpal` playback handle on the system default output
/// device. Works on Windows + Linux (and macOS for free, though
/// untested).
pub fn make_playback() -> anyhow::Result<Box<dyn AudioPlayback>> {
    Ok(Box::new(playback::CpalPlayback::new()?))
}

/// Bounded hand-off from a capture thread to the async runtime, plus a cheap
/// demand probe. Capture backends release their hardware/COM resources when
/// there is no connected peer or the user disabled that stream.
#[derive(Clone)]
pub struct CaptureSink {
    tx: tokio::sync::mpsc::Sender<AudioFrame>,
    active: std::sync::Arc<dyn Fn() -> bool + Send + Sync + 'static>,
}

impl CaptureSink {
    pub fn new<F>(tx: tokio::sync::mpsc::Sender<AudioFrame>, active: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        Self {
            tx,
            active: std::sync::Arc::new(active),
        }
    }

    pub fn is_active(&self) -> bool {
        (self.active)()
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    /// Returns false only when the runtime receiver has closed. A full queue is
    /// an intentional lossy drop: stale real-time audio must not accumulate.
    pub fn send_lossy(&self, frame: AudioFrame) -> bool {
        match self.tx.try_send(frame) {
            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

pub trait AudioCapture: Send {
    fn backend_name(&self) -> &'static str {
        "unknown"
    }

    fn backend_status(&self) -> BackendStatus {
        BackendStatus::default()
    }

    /// Spawn whatever background work the platform needs and push
    /// encoded frames into the bounded, lossy `sink`. Returns immediately.
    fn start(&mut self, sink: CaptureSink) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
struct PlaybackSessionIngress {
    accepted: AtomicU64,
    consumed: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct PlaybackSession {
    epoch: u64,
    ingress: Arc<PlaybackSessionIngress>,
}

impl PartialEq for PlaybackSession {
    fn eq(&self, other: &Self) -> bool {
        self.epoch == other.epoch
    }
}

impl Eq for PlaybackSession {}

impl PlaybackSession {
    pub(crate) fn new(epoch: u64) -> Self {
        Self {
            epoch,
            ingress: Arc::new(PlaybackSessionIngress::default()),
        }
    }

    pub(crate) const fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        self.epoch == other.epoch && Arc::ptr_eq(&self.ingress, &other.ingress)
    }

    pub(crate) fn note_accepted_frame(&self) {
        self.ingress.accepted.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn note_consumed_frame(&self) {
        self.ingress.consumed.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn has_pending_ingress(&self) -> bool {
        self.ingress.accepted.load(Ordering::Acquire)
            != self.ingress.consumed.load(Ordering::Acquire)
    }
}

pub trait AudioPlayback: Send + Sync {
    fn backend_status(&self) -> BackendStatus {
        BackendStatus::default()
    }

    /// Establish an opaque receive identity for one peer session. Implementations
    /// must reject frames submitted with any older identity.
    fn begin_session(&self) -> PlaybackSession {
        PlaybackSession::new(0)
    }

    /// Retire a receive identity when its peer connection ends. Implementations
    /// must make in-flight builds and queued frames for this identity stale.
    fn end_session(&self, _session: &PlaybackSession) {}

    /// Decode and enqueue one frame for playback. Lossy: drops on
    /// buffer overflow (better latency than blocking).
    fn enqueue(&self, session: &PlaybackSession, frame: AudioFrame) -> anyhow::Result<()>;
}

pub struct PlaybackSessionLease<'a> {
    playback: &'a dyn AudioPlayback,
    token: PlaybackSession,
}

impl<'a> PlaybackSessionLease<'a> {
    pub fn begin(playback: &'a dyn AudioPlayback) -> Self {
        Self {
            playback,
            token: playback.begin_session(),
        }
    }

    pub fn token(&self) -> &PlaybackSession {
        &self.token
    }
}

impl Drop for PlaybackSessionLease<'_> {
    fn drop(&mut self) {
        self.playback.end_session(&self.token);
    }
}

#[cfg(test)]
mod playback_session_lease_tests {
    use super::*;

    struct RecordingPlayback {
        ended: AtomicU64,
    }

    impl AudioPlayback for RecordingPlayback {
        fn begin_session(&self) -> PlaybackSession {
            PlaybackSession::new(7)
        }

        fn end_session(&self, session: &PlaybackSession) {
            if session.epoch() == 7 {
                self.ended.fetch_add(1, Ordering::AcqRel);
            }
        }

        fn enqueue(&self, _session: &PlaybackSession, _frame: AudioFrame) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn dropping_session_lease_retires_the_backend_identity() {
        let playback = RecordingPlayback {
            ended: AtomicU64::new(0),
        };
        {
            let lease = PlaybackSessionLease::begin(&playback);
            assert_eq!(lease.token().epoch(), 7);
        }
        assert_eq!(playback.ended.load(Ordering::Acquire), 1);
    }
}

#[cfg(test)]
mod capture_sink_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{AudioFrame, CaptureSink, StreamKind};

    fn frame(seq: u32) -> AudioFrame {
        AudioFrame {
            stream: StreamKind::Mic,
            seq,
            opus: vec![seq as u8],
        }
    }

    #[test]
    fn capture_sink_is_dynamic_bounded_and_lossy() {
        let enabled = Arc::new(AtomicBool::new(false));
        let enabled_for_sink = enabled.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let sink = CaptureSink::new(tx, move || enabled_for_sink.load(Ordering::Relaxed));

        assert!(!sink.is_active());
        enabled.store(true, Ordering::Relaxed);
        assert!(sink.is_active());
        assert!(sink.send_lossy(frame(1)));
        for seq in 2..=100_001 {
            assert!(sink.send_lossy(frame(seq)), "a full queue is a lossy drop");
        }
        assert_eq!(rx.len(), 1);
        drop(rx);
        assert!(sink.is_closed());
        assert!(!sink.send_lossy(frame(3)));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_sysout_factory_is_process_isolated() {
        let capture = super::make_sysout_capture().expect("factory must be lazy and hardware-free");
        assert_eq!(capture.backend_name(), "windows-process-loopback");
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "manual hardware/CPU gate; requires working loopback and mic devices"]
    fn windows_capture_backends_skip_frames_without_demand() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut sysout = super::make_sysout_capture().expect("WASAPI loopback must be available");
        let mut mic = super::make_mic_capture().expect("default mic must be available");
        sysout
            .start(CaptureSink::new(tx.clone(), || false))
            .expect("start WASAPI loopback");
        mic.start(CaptureSink::new(tx, || false))
            .expect("start mic capture");

        // Long enough for both 20 ms pipelines to produce hundreds of source
        // frames if the demand gate regresses.
        std::thread::sleep(std::time::Duration::from_secs(8));
        assert!(
            matches!(
                rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "inactive capture encoded or queued audio"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "manual comparison probe; requires working loopback and mic devices"]
    fn windows_capture_backends_encode_with_demand() {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let mut sysout = super::make_sysout_capture().expect("WASAPI loopback must be available");
        let mut mic = super::make_mic_capture().expect("default mic must be available");
        sysout
            .start(CaptureSink::new(tx.clone(), || true))
            .expect("start WASAPI loopback");
        mic.start(CaptureSink::new(tx, || true))
            .expect("start mic capture");

        std::thread::sleep(std::time::Duration::from_secs(8));
        assert!(!rx.is_empty(), "active capture did not encode any audio");
    }
}

#[cfg(test)]
mod backend_status_tests {
    use super::*;

    #[test]
    fn backend_status_reports_worker_transitions() {
        let status = BackendStatus::new(BackendState::Idle);
        status.set(BackendState::Active);
        assert_eq!(status.get(), BackendState::Active);
    }
}
