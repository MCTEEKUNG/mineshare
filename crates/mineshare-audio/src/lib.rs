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

/// Enumerate cpal output devices on the local host with the
/// default flagged. Used by the GUI's Devices tab to surface
/// what the bridge would render peer audio into. Failures are
/// non-fatal — we return what we got and log the rest.
pub fn list_output_devices() -> Vec<DeviceInfo> {
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
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
