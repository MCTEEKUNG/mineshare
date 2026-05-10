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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub mod codec;
pub mod cpal_mic;
pub mod playback;
pub mod resample;

#[cfg(target_os = "windows")]
pub mod wasapi_loopback;
#[cfg(target_os = "windows")]
pub mod virtual_mic_win;

#[cfg(target_os = "linux")]
pub mod pipewire_monitor;
#[cfg(target_os = "linux")]
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

/// Construct the platform-specific sysout capture: WASAPI loopback
/// on Windows, PipeWire monitor on Linux.
pub fn make_sysout_capture() -> anyhow::Result<Box<dyn AudioCapture>> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(wasapi_loopback::WasapiLoopback::new()?))
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
static OUTPUT_VERSION: AtomicU64 = AtomicU64::new(0);
static INPUT_VERSION: AtomicU64 = AtomicU64::new(0);

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

pub fn selected_output_device() -> Option<String> {
    SELECTED_OUTPUT.lock().clone()
}

pub fn selected_input_device() -> Option<String> {
    SELECTED_INPUT.lock().clone()
}

pub fn output_device_version() -> u64 {
    OUTPUT_VERSION.load(Ordering::Acquire)
}

pub fn input_device_version() -> u64 {
    INPUT_VERSION.load(Ordering::Acquire)
}

/// Resolve the cpal output device matching the user's selection, or
/// the system default if no selection / not found. Used by the
/// playback thread when (re)building a stream.
pub fn resolve_output_device() -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Some(want) = selected_output_device() {
        if let Ok(iter) = host.output_devices() {
            for d in iter {
                if d.name().ok().as_deref() == Some(want.as_str()) {
                    return Some(d);
                }
            }
        }
        // selection no longer present — fall through to default.
    }
    host.default_output_device()
}

/// Mirror of [`resolve_output_device`] for the mic capture path.
pub fn resolve_input_device() -> Option<cpal::Device> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Some(want) = selected_input_device() {
        if let Ok(iter) = host.input_devices() {
            for d in iter {
                if d.name().ok().as_deref() == Some(want.as_str()) {
                    return Some(d);
                }
            }
        }
    }
    host.default_input_device()
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

pub trait AudioCapture: Send {
    /// Spawn whatever background work the platform needs and push
    /// encoded frames into `sink`. Returns immediately.
    fn start(
        &mut self,
        sink: tokio::sync::mpsc::UnboundedSender<AudioFrame>,
    ) -> anyhow::Result<()>;
}

pub trait AudioPlayback: Send + Sync {
    /// Decode and enqueue one frame for playback. Lossy: drops on
    /// buffer overflow (better latency than blocking).
    fn enqueue(&self, frame: AudioFrame) -> anyhow::Result<()>;
}
