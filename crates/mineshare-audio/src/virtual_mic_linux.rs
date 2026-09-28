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
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::Mutex;
use tracing::{info, warn};

use crate::codec::OpusDecoder;
use crate::{
    AudioFrame, AudioPlayback, BackendState, BackendStatus, FRAME_SAMPLES_INTERLEAVED,
    PlaybackSession,
};

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
const PIPEWIRE_PCM_QUEUE_CAPACITY: usize = 8;
const PACAT_RETRY_BACKOFF: Duration = Duration::from_secs(1);

fn pacat_rebuild_due(now: Instant, next_attempt: Instant) -> bool {
    now >= next_attempt
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PipewireQueueResult {
    Queued,
    Dropped,
    Disconnected,
}

fn try_enqueue_pipewire<T>(tx: &std::sync::mpsc::SyncSender<T>, value: T) -> PipewireQueueResult {
    match tx.try_send(value) {
        Ok(()) => PipewireQueueResult::Queued,
        Err(std::sync::mpsc::TrySendError::Full(_)) => PipewireQueueResult::Dropped,
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => PipewireQueueResult::Disconnected,
    }
}

pub struct PipewireVirtualMic {
    /// Module index returned by `pactl load-module` — needed for
    /// the matching `pactl unload-module` on shutdown.
    module_index: Option<String>,
    writer_tx: Option<mpsc::SyncSender<QueuedPipewirePcm>>,
    writer: Option<thread::JoinHandle<()>>,
    pacat: Arc<Mutex<Option<Child>>>,
    session: Arc<Mutex<PipewireSessionState>>,
    stop: Arc<AtomicBool>,
    status: BackendStatus,
}

struct QueuedPipewirePcm {
    session: PlaybackSession,
    bytes: Vec<u8>,
}

struct PipewireSessionState {
    next_epoch: u64,
    current: Option<PlaybackSession>,
    decoder: Option<OpusDecoder>,
    scratch: Vec<f32>,
}

fn pipewire_session_is_current(current: &PlaybackSession, candidate: &PlaybackSession) -> bool {
    current.same_identity(candidate)
}

fn retire_pipewire_session(state: &mut PipewireSessionState, session: &PlaybackSession) -> bool {
    if !state
        .current
        .as_ref()
        .is_some_and(|current| pipewire_session_is_current(current, session))
    {
        return false;
    }
    state.current = None;
    state.decoder = None;
    true
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

        let (writer_tx, writer_rx) = mpsc::sync_channel(PIPEWIRE_PCM_QUEUE_CAPACITY);
        let pacat = Arc::new(Mutex::new(None));
        let session = Arc::new(Mutex::new(PipewireSessionState {
            next_epoch: 0,
            current: None,
            decoder: Some(OpusDecoder::new()?),
            scratch: vec![0f32; FRAME_SAMPLES_INTERLEAVED],
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let status = BackendStatus::new(BackendState::Idle);
        let writer = thread::Builder::new()
            .name("pipewire-virtual-mic-writer".into())
            .spawn({
                let pacat = pacat.clone();
                let session = session.clone();
                let stop = stop.clone();
                let status = status.clone();
                move || run_pacat_writer(writer_rx, pacat, session, stop, status)
            });
        let writer = match writer {
            Ok(writer) => writer,
            Err(error) => {
                let _ = Command::new("pactl")
                    .args(["unload-module", &module_index])
                    .status();
                return Err(error).context("spawn PipeWire virtual-mic writer");
            }
        };

        Ok(Self {
            module_index: Some(module_index),
            writer_tx: Some(writer_tx),
            writer: Some(writer),
            pacat,
            session,
            stop,
            status,
        })
    }
}

impl Drop for PipewireVirtualMic {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.writer_tx.take();
        if let Some(mut child) = self.pacat.lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        if let Some(idx) = self.module_index.take() {
            let _ = Command::new("pactl").args(["unload-module", &idx]).status();
            info!(module = %idx, "unloaded PipeWire null-sink");
        }
    }
}

impl AudioPlayback for PipewireVirtualMic {
    fn backend_status(&self) -> BackendStatus {
        self.status.clone()
    }

    fn begin_session(&self) -> PlaybackSession {
        let mut state = self.session.lock();
        state.next_epoch = state.next_epoch.wrapping_add(1);
        let session = PlaybackSession::new(state.next_epoch);
        state.current = Some(session.clone());
        state.decoder = None;
        session
    }

    fn end_session(&self, session: &PlaybackSession) {
        retire_pipewire_session(&mut self.session.lock(), session);
    }

    fn enqueue(&self, session: &PlaybackSession, frame: AudioFrame) -> Result<()> {
        let mut state = self.session.lock();
        if !state
            .current
            .as_ref()
            .is_some_and(|current| pipewire_session_is_current(current, session))
        {
            return Ok(());
        }
        if state.decoder.is_none() {
            state.decoder = Some(OpusDecoder::new()?);
        }
        let PipewireSessionState {
            decoder, scratch, ..
        } = &mut *state;
        let n = decoder
            .as_mut()
            .expect("session decoder initialized above")
            .decode(&frame.opus, scratch)?;
        let pcm = &scratch[..n];

        // Reinterpret f32 slice as little-endian bytes for pacat's
        // `--format=float32le`. f32 is LE on every platform we ship.
        let bytes = bytemuck_le_f32(pcm).to_vec();
        drop(state);

        let Some(writer_tx) = self.writer_tx.as_ref() else {
            self.status.set(BackendState::Degraded);
            anyhow::bail!("PipeWire virtual-mic writer is unavailable");
        };
        match try_enqueue_pipewire(
            writer_tx,
            QueuedPipewirePcm {
                session: session.clone(),
                bytes,
            },
        ) {
            PipewireQueueResult::Queued | PipewireQueueResult::Dropped => Ok(()),
            PipewireQueueResult::Disconnected => {
                self.status.set(BackendState::Degraded);
                anyhow::bail!("PipeWire virtual-mic writer terminated")
            }
        }
    }
}

fn spawn_pacat() -> Result<(Child, ChildStdin)> {
    let mut child = Command::new("pacat")
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
        .stderr(Stdio::null())
        .spawn()
        .context("spawn `pacat` for virtual-mic playback")?;
    let stdin = child.stdin.take().context("pacat stdin pipe missing")?;
    Ok((child, stdin))
}

fn stop_shared_pacat(pacat: &Mutex<Option<Child>>) {
    if let Some(mut child) = pacat.lock().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn run_pacat_writer(
    rx: mpsc::Receiver<QueuedPipewirePcm>,
    pacat: Arc<Mutex<Option<Child>>>,
    session: Arc<Mutex<PipewireSessionState>>,
    stop: Arc<AtomicBool>,
    status: BackendStatus,
) {
    let mut stdin: Option<ChildStdin> = None;
    let mut next_build_attempt = Instant::now();

    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let queued = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(queued) => queued,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if session.lock().current.is_none() && stdin.is_some() {
                    stdin = None;
                    stop_shared_pacat(&pacat);
                    status.set(BackendState::Idle);
                    info!("idle PipeWire virtual-mic writer released pacat");
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let is_current = session
            .lock()
            .current
            .as_ref()
            .is_some_and(|current| pipewire_session_is_current(current, &queued.session));
        if !is_current {
            continue;
        }

        if stdin.is_none() {
            if !pacat_rebuild_due(Instant::now(), next_build_attempt) {
                continue;
            }
            status.set(BackendState::Starting);
            match spawn_pacat() {
                Ok((mut child, next_stdin)) => {
                    if stop.load(Ordering::Acquire) {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    *pacat.lock() = Some(child);
                    stdin = Some(next_stdin);
                    status.set(BackendState::Active);
                    info!(sink = SINK_NAME, "pacat playback into virtual mic started");
                }
                Err(error) => {
                    status.set(BackendState::Degraded);
                    next_build_attempt = Instant::now() + PACAT_RETRY_BACKOFF;
                    warn!(%error, "pacat build failed — retrying after bounded backoff");
                    continue;
                }
            }
        }

        let still_current = session
            .lock()
            .current
            .as_ref()
            .is_some_and(|current| pipewire_session_is_current(current, &queued.session));
        if !still_current {
            continue;
        }
        if let Some(writer) = stdin.as_mut()
            && let Err(error) = writer.write_all(&queued.bytes)
        {
            warn!(%error, "pacat stdin write failed — rebuilding after bounded backoff");
            status.set(BackendState::Degraded);
            stdin = None;
            stop_shared_pacat(&pacat);
            next_build_attempt = Instant::now() + PACAT_RETRY_BACKOFF;
        }
    }

    drop(stdin);
    stop_shared_pacat(&pacat);
    status.set(BackendState::Stopped);
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
        if parts.len() >= 3 && parts[1] == "module-null-sink" && parts[2].contains(&arg_match) {
            let _ = Command::new("pactl")
                .args(["unload-module", parts[0]])
                .output();
            info!(
                stale_module = parts[0],
                "cleaned up leftover mineshare_mic module"
            );
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

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = assert!(PIPEWIRE_PCM_QUEUE_CAPACITY <= 8);

    #[test]
    fn pacat_rebuild_waits_for_bounded_backoff() {
        let now = std::time::Instant::now();
        assert!(!pacat_rebuild_due(now, now + PACAT_RETRY_BACKOFF));
        assert!(pacat_rebuild_due(
            now + PACAT_RETRY_BACKOFF,
            now + PACAT_RETRY_BACKOFF
        ));
    }

    #[test]
    fn pipewire_writer_queue_is_bounded_and_nonblocking() {
        let (tx, _rx) = std::sync::mpsc::sync_channel(1);
        assert_eq!(try_enqueue_pipewire(&tx, 1), PipewireQueueResult::Queued);
        assert_eq!(try_enqueue_pipewire(&tx, 2), PipewireQueueResult::Dropped);
    }

    #[test]
    fn equal_epoch_from_another_pipewire_backend_is_not_current() {
        let current = PlaybackSession::new(4);
        let foreign = PlaybackSession::new(4);

        assert!(!pipewire_session_is_current(&current, &foreign));
    }

    #[test]
    fn ending_current_pipewire_session_resets_decoder_only_once() {
        let session = PlaybackSession::new(8);
        let mut state = PipewireSessionState {
            next_epoch: 8,
            current: Some(session.clone()),
            decoder: Some(OpusDecoder::new().expect("create test decoder")),
            scratch: vec![0.0; FRAME_SAMPLES_INTERLEAVED],
        };

        assert!(retire_pipewire_session(&mut state, &session));
        assert!(state.current.is_none());
        assert!(state.decoder.is_none());
        assert!(!retire_pipewire_session(&mut state, &session));
        assert_eq!(state.next_epoch, 8);
    }
}
