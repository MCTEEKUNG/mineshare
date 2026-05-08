//! Linux sysout capture via the PipeWire **monitor** of the default
//! sink.
//!
//! ## Why a `parec` subprocess
//!
//! cpal's ALSA backend on Linux only enumerates raw ALSA card
//! devices, not PipeWire/PulseAudio monitor sources — so we'd have
//! to add a native PipeWire client just to access loopback. Spawning
//! `parec --device=@DEFAULT_MONITOR@` (from `pulseaudio-utils`,
//! which pipewire-pulse provides on modern Ubuntu) gives us a
//! self-tracking pipe of f32 PCM from whatever the user's current
//! default sink is — exactly what the Win-side WASAPI loopback
//! produces. Trade-off: an extra process per daemon, but no extra
//! Rust dep and the routing follows the user's sink switches
//! automatically.
//!
//! Format: parec emits raw f32 LE samples at the rate/channel layout
//! we ask for, so we set the canonical 48 kHz / 2-channel / f32
//! shape and skip the resampler entirely on the Linux path.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread;

use anyhow::{Context, Result};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};

use crate::codec::OpusEncoder;
use crate::{
    AudioCapture, AudioFrame, CHANNELS, FRAME_SAMPLES_INTERLEAVED, SAMPLE_RATE, StreamKind,
};

const OPUS_BITRATE_BPS: i32 = 96_000;

pub struct PipewireMonitor {
    started: bool,
    /// Kept on the struct so the subprocess gets cleaned up when the
    /// capture handle is dropped.
    child: Option<Child>,
}

impl PipewireMonitor {
    pub fn new() -> Result<Self> {
        Ok(Self {
            started: false,
            child: None,
        })
    }
}

impl Drop for PipewireMonitor {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl AudioCapture for PipewireMonitor {
    fn start(&mut self, sink: UnboundedSender<AudioFrame>) -> Result<()> {
        if self.started {
            return Ok(());
        }
        self.started = true;

        let mut child = Command::new("parec")
            .args([
                "--device=@DEFAULT_MONITOR@",
                "--format=float32le",
                "--rate=48000",
                "--channels=2",
                "--latency-msec=20",
                "--raw",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context(
                "spawn `parec` (need pulseaudio-utils — `apt install pulseaudio-utils`; \
                 routing follows the user's default sink via @DEFAULT_MONITOR@)",
            )?;

        let stdout = child
            .stdout
            .take()
            .context("parec stdout pipe missing")?;
        let stderr = child
            .stderr
            .take()
            .context("parec stderr pipe missing")?;

        // Surface parec's own diagnostics if it complains about the
        // monitor source not existing, the user not being in the
        // pulse-access group, etc.
        thread::Builder::new()
            .name("parec-stderr".into())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(stderr);
                let mut buf = String::new();
                use std::io::BufRead;
                while reader.read_line(&mut buf).unwrap_or(0) > 0 {
                    let line = buf.trim_end();
                    if !line.is_empty() {
                        warn!(line, "parec stderr");
                    }
                    buf.clear();
                }
            })
            .context("spawn parec-stderr drain thread")?;

        thread::Builder::new()
            .name("parec-pcm-encode".into())
            .spawn(move || {
                if let Err(e) = drive_encode_loop(stdout, sink) {
                    warn!(error = %e, "parec encode thread exited");
                }
            })
            .context("spawn parec-pcm-encode thread")?;

        info!(
            sample_rate = SAMPLE_RATE,
            channels = CHANNELS,
            "PipeWire monitor capture started via parec @DEFAULT_MONITOR@"
        );
        self.child = Some(child);
        Ok(())
    }
}

fn drive_encode_loop<R: Read>(
    mut reader: R,
    sink: UnboundedSender<AudioFrame>,
) -> Result<()> {
    let mut encoder = OpusEncoder::new(OPUS_BITRATE_BPS)?;
    // 20 ms frame = 1920 interleaved f32 samples = 7680 bytes.
    let bytes_per_frame = FRAME_SAMPLES_INTERLEAVED * std::mem::size_of::<f32>();
    let mut byte_buf = vec![0u8; bytes_per_frame];
    let mut pcm = vec![0f32; FRAME_SAMPLES_INTERLEAVED];
    let mut seq: u32 = 0;

    loop {
        if let Err(e) = reader.read_exact(&mut byte_buf) {
            // EOF or pipe broken — caller (parent daemon) is shutting
            // down or parec died. Either way, terminate cleanly.
            warn!(error = %e, "parec stdout closed — stopping monitor capture");
            return Ok(());
        }
        // Reinterpret the byte buffer as f32 little-endian.
        for (i, sample) in pcm.iter_mut().enumerate() {
            let off = i * 4;
            let bytes: [u8; 4] = byte_buf[off..off + 4].try_into().unwrap();
            *sample = f32::from_le_bytes(bytes);
        }

        let opus_bytes = match encoder.encode(&pcm) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, "opus encode failed — skipping frame");
                continue;
            }
        };
        let frame = AudioFrame {
            stream: StreamKind::SysOut,
            seq,
            opus: opus_bytes,
        };
        seq = seq.wrapping_add(1);

        if sink.send(frame).is_err() {
            info!("audio sink closed — stopping PipeWire monitor capture");
            return Ok(());
        }
    }
}
