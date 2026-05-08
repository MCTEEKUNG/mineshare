//! Linux side of the **virtual mic** sink.
//!
//! When the daemon starts on Linux we ask PipeWire (via the
//! pipewire-pulse compatibility layer's `pactl`) to load a
//! `module-null-sink` named `mineshare_mic`. PipeWire automatically
//! exposes the sink's monitor as a *source*, which is what apps like
//! Discord / OBS / Zoom see in their input-device picker as
//! "**Monitor of MineShare Mic**".
//!
//! Decoded peer-mic frames are written into that sink via a `pacat`
//! subprocess piped from this side — same pattern as the
//! `parec`-based monitor capture, just in reverse. When the daemon
//! exits, `Drop` kills `pacat` and unloads the null-sink module so
//! the user's `wpctl status` doesn't keep a stale entry around.
//!
//! ## Why a subprocess instead of cpal
//!
//! cpal's ALSA backend can address PipeWire devices via the
//! `pipewire-alsa` shim, but selecting a *specific* PipeWire sink
//! through that pathway is fiddly (you'd have to set
//! `PIPEWIRE_NODE` or use undocumented device names). `pacat` takes
//! `--device=<name>` directly and is part of the same
//! `pulseaudio-utils` package we already require for `parec`.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tracing::{info, warn};

use crate::codec::OpusDecoder;
use crate::{AudioFrame, AudioPlayback, FRAME_SAMPLES_INTERLEAVED};

/// PipeWire sink-name used both for `pactl load-module` and for
/// pacat's `--device=` flag.
const SINK_NAME: &str = "mineshare_mic";
/// Human-readable description shown in app pickers. **Must not
/// contain spaces** — PipeWire's `pactl` compatibility shim splits
/// every form of `sink_properties=…` value on whitespace before the
/// property-list parser can honor quoting/escapes (we tried both
/// `"foo bar"` and `foo\040bar`; both truncate). Hyphenated reads
/// cleanly and survives the round trip intact.
const SINK_DESCRIPTION: &str = "MineShare-Mic";

pub struct PipewireVirtualMic {
    /// Module index returned by `pactl load-module` — needed for
    /// the matching `pactl unload-module` on shutdown.
    module_index: Option<String>,
    /// pacat subprocess we pipe decoded PCM into.
    pacat: Mutex<Option<Child>>,
    decoder: Mutex<OpusDecoder>,
    /// Reused per-frame scratch buffer.
    scratch: Mutex<Vec<f32>>,
    /// Flips to true after the first failed write so we stop
    /// spamming logs if pacat dies.
    pacat_dead: AtomicBool,
}

impl PipewireVirtualMic {
    pub fn new() -> Result<Self> {
        // Step 0: cleanup stale `mineshare_mic` modules left behind
        // by a previous daemon that didn't shut down cleanly
        // (SIGKILL / OOM / panic before Drop could run). Without
        // this we accumulate duplicate sinks across restarts and
        // pactl name resolution picks the wrong one when we later
        // set the description.
        cleanup_stale_modules();

        // Step 1: create the null-sink with the description baked
        // into module-args. PipeWire's pactl shim *does* honor the
        // value as long as it has no internal whitespace — see the
        // SINK_DESCRIPTION doc comment for the gory details.
        let out = Command::new("pactl")
            .args([
                "load-module",
                "module-null-sink",
                &format!("sink_name={SINK_NAME}"),
                &format!("sink_properties=device.description={SINK_DESCRIPTION}"),
                "channels=2",
                "rate=48000",
            ])
            .output()
            .context(
                "spawn `pactl load-module` (pulseaudio-utils — \
                 `apt install pulseaudio-utils`)",
            )?;
        if !out.status.success() {
            anyhow::bail!(
                "pactl load-module failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let module_index = String::from_utf8_lossy(&out.stdout).trim().to_string();
        info!(
            sink = SINK_NAME,
            description = SINK_DESCRIPTION,
            module = %module_index,
            "PipeWire null-sink loaded for virtual mic"
        );

        // Step 2: spawn pacat targeting the new sink. We feed it
        // raw f32 LE 48 kHz stereo and it forwards into PipeWire.
        let pacat = Command::new("pacat")
            .args([
                &format!("--device={SINK_NAME}"),
                "--format=float32le",
                "--rate=48000",
                "--channels=2",
                "--latency-msec=20",
                "--raw",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn `pacat` for virtual-mic playback")?;
        info!(sink = SINK_NAME, "pacat playback into virtual mic started");

        Ok(Self {
            module_index: Some(module_index),
            pacat: Mutex::new(Some(pacat)),
            decoder: Mutex::new(OpusDecoder::new()?),
            scratch: Mutex::new(vec![0f32; FRAME_SAMPLES_INTERLEAVED]),
            pacat_dead: AtomicBool::new(false),
        })
    }
}

impl Drop for PipewireVirtualMic {
    fn drop(&mut self) {
        if let Some(mut child) = self.pacat.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(idx) = self.module_index.take() {
            let _ = Command::new("pactl")
                .args(["unload-module", &idx])
                .status();
            info!(module = %idx, "unloaded PipeWire null-sink");
        }
    }
}

impl AudioPlayback for PipewireVirtualMic {
    fn enqueue(&self, frame: AudioFrame) -> Result<()> {
        if self.pacat_dead.load(Ordering::Relaxed) {
            // pacat already terminated; silently drop further frames
            // rather than log-flooding.
            return Ok(());
        }
        let mut scratch = self.scratch.lock();
        let n = self.decoder.lock().decode(&frame.opus, &mut scratch)?;
        let pcm = &scratch[..n];

        // Reinterpret f32 slice as little-endian bytes for pacat's
        // `--format=float32le`. f32 is LE on every platform we ship.
        let bytes: &[u8] = bytemuck_le_f32(pcm);

        let mut pacat_guard = self.pacat.lock();
        let Some(pacat) = pacat_guard.as_mut() else {
            return Ok(());
        };
        let stdin = pacat
            .stdin
            .as_mut()
            .context("pacat stdin missing — child terminated")?;
        if let Err(e) = stdin.write_all(bytes) {
            warn!(error = %e, "pacat stdin write failed — virtual mic disabled until restart");
            self.pacat_dead.store(true, Ordering::Relaxed);
            return Err(e.into());
        }
        Ok(())
    }
}

/// Find any leftover `module-null-sink sink_name=mineshare_mic` from
/// a previous daemon and unload them. Best-effort; we ignore errors
/// so a missing pactl during cleanup doesn't block startup.
fn cleanup_stale_modules() {
    let arg_match = format!("sink_name={SINK_NAME}");
    let listing = match Command::new("pactl")
        .args(["list", "short", "modules"])
        .output()
    {
        Ok(o) if o.status.success() => o.stdout,
        _ => return,
    };
    for line in String::from_utf8_lossy(&listing).lines() {
        // Format: "<idx>\t<module-name>\t<args>"
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3
            && parts[1] == "module-null-sink"
            && parts[2].contains(&arg_match)
        {
            let _ = Command::new("pactl")
                .args(["unload-module", parts[0]])
                .output();
            info!(stale_module = parts[0], "cleaned up leftover mineshare_mic module");
        }
    }
}

/// Reinterpret an `&[f32]` as a little-endian byte slice. Safe on
/// every target Rust supports (all are LE for f32).
fn bytemuck_le_f32(samples: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and we only read; the resulting
    // slice has the same lifetime as the input.
    unsafe {
        std::slice::from_raw_parts(
            samples.as_ptr() as *const u8,
            std::mem::size_of_val(samples),
        )
    }
}
