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
pub mod playback;
pub mod resample;

#[cfg(target_os = "windows")]
pub mod wasapi_loopback;

#[cfg(target_os = "linux")]
pub mod pipewire_monitor;

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
