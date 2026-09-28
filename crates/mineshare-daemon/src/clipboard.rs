//! Cross-platform clipboard text + image sync.
//!
//! M4 Slice 1: a daemon-lifetime watcher polls the local clipboard
//! every `POLL_MS` and pushes a copy onto an mpsc when the text
//! changes. The peer-session writer task reads those events and
//! sends them as `ControlMsg::ClipboardText` over the existing
//! encrypted TCP control channel — no new sockets, no new
//! handshakes.
//!
//! ## Echo guard
//!
//! A naive setup loops: A copies → A→B sends → B sets clipboard →
//! B's watcher sees a "change" → B→A sends → A sets → … forever.
//! We sidestep that by sharing the `LAST_TEXT` static between the
//! watcher and the inbound handler. When we set the clipboard from a
//! peer message we *also* update `LAST_TEXT` to the same value, so
//! the next watcher tick sees `current == last` and skips the send.
//!
//! ## Wayland note
//!
//! `arboard` on Linux uses the X11 Xclip protocol via Xwayland on
//! GNOME/KDE Wayland sessions. That works for the common case; pure
//! Wayland clipboard (where focused-app ownership matters) is a
//! known limitation we'll revisit if a user reports it.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

const POLL_MS: u64 = 400;
/// Cap forwarded clipboard size — pasting a 50 MB log into the
/// clipboard shouldn't accidentally flood the control channel.
// Leave room for ControlMsg/bincode framing and Noise's 16-byte tag inside
// its 65,535-byte ciphertext limit. A 64 KiB text used to kill the session.
const MAX_BYTES: usize = 60 * 1024;
pub const IMAGE_CHUNK_BYTES: usize = 32 * 1024;
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const CLIPBOARD_QUEUE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClipboardFingerprint {
    Text(String),
    Files(Vec<(PathBuf, u64, Option<std::time::SystemTime>)>),
    Image {
        width: usize,
        height: usize,
        sha256: [u8; 32],
    },
}

#[derive(Debug, Clone)]
pub enum ClipboardEvent {
    Text(String),
    ImageStart {
        id: u64,
        width: u32,
        height: u32,
        total_bytes: u64,
    },
    ImageChunk {
        id: u64,
        offset: u64,
        data: Vec<u8>,
    },
    ImageEnd {
        id: u64,
    },
}

static LAST_CLIPBOARD: Mutex<Option<ClipboardFingerprint>> = Mutex::new(None);
static WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

/// Channel to the single clipboard-owning thread. `apply_from_peer`
/// pushes inbound peer text here instead of touching `arboard`
/// itself.
///
/// **Why single-threaded.** `arboard` on Windows drives the OLE
/// clipboard (`OleInitialize` / `OleSetClipboard` / `OleGetClipboard`).
/// The old design polled `get_text` on the watcher thread while
/// `apply_from_peer` — running on an arbitrary tokio worker — built a
/// *fresh* `arboard::Clipboard` (another `OleInitialize`/`OleUninitialize`
/// pair) and called `set_text` concurrently. On a fresh peer session
/// both sides immediately exchange clipboard, so those two threads hit
/// the OLE clipboard at the same instant — and, inside the Tauri GUI
/// process, alongside WebView2's own clipboard use. That concurrent /
/// re-entrant OLE access corrupted the process heap
/// (`STATUS_HEAP_CORRUPTION` / 0xc0000374, faulting in ntdll), crashing
/// the GUI ~60–90% of the time on a simultaneous launch. Confirmed by
/// bisection: disabling clipboard sync took the crash rate to 0/12.
/// Funnelling every clipboard touch onto one thread removes the race.
enum ApplyEvent {
    Text(String),
    Image(arboard::ImageData<'static>),
    Files(Vec<PathBuf>),
}

static APPLY_TX: Mutex<Option<std::sync::mpsc::SyncSender<()>>> = Mutex::new(None);
static PENDING_APPLY: Mutex<Option<ApplyEvent>> = Mutex::new(None);
static INCOMING_IMAGE: Mutex<Option<IncomingImage>> = Mutex::new(None);
const APPLY_QUEUE_CAPACITY: usize = 1;

struct WatcherRunning;

impl Drop for WatcherRunning {
    fn drop(&mut self) {
        *APPLY_TX.lock() = None;
        WATCHER_RUNNING.store(false, Ordering::Release);
    }
}

struct IncomingImage {
    id: u64,
    width: usize,
    height: usize,
    total_bytes: usize,
    rgba: Vec<u8>,
}

impl IncomingImage {
    fn new(id: u64, width: u32, height: u32, total_bytes: u64) -> Result<Self> {
        let width = usize::try_from(width)?;
        let height = usize::try_from(height)?;
        let total_bytes = usize::try_from(total_bytes)?;
        validate_image_shape(width, height, total_bytes)?;
        Ok(Self {
            id,
            width,
            height,
            total_bytes,
            rgba: Vec::with_capacity(total_bytes),
        })
    }

    fn push(&mut self, id: u64, offset: u64, data: &[u8]) -> Result<()> {
        anyhow::ensure!(id == self.id, "clipboard image id changed mid-transfer");
        anyhow::ensure!(
            usize::try_from(offset)? == self.rgba.len(),
            "clipboard image chunk offset mismatch"
        );
        anyhow::ensure!(
            data.len() <= IMAGE_CHUNK_BYTES,
            "clipboard image chunk exceeds {IMAGE_CHUNK_BYTES} bytes"
        );
        anyhow::ensure!(
            self.rgba.len().saturating_add(data.len()) <= self.total_bytes,
            "clipboard image exceeds declared size"
        );
        self.rgba.extend_from_slice(data);
        Ok(())
    }

    fn finish(self, id: u64) -> Result<arboard::ImageData<'static>> {
        anyhow::ensure!(
            id == self.id,
            "clipboard image id changed before completion"
        );
        anyhow::ensure!(
            self.rgba.len() == self.total_bytes,
            "clipboard image ended at {} of {} bytes",
            self.rgba.len(),
            self.total_bytes
        );
        Ok(arboard::ImageData {
            width: self.width,
            height: self.height,
            bytes: Cow::Owned(self.rgba),
        })
    }
}

fn validate_image_shape(width: usize, height: usize, total_bytes: usize) -> Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0,
        "clipboard image has zero dimensions"
    );
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("clipboard image dimensions overflow"))?;
    anyhow::ensure!(
        total_bytes == expected,
        "clipboard image RGBA size mismatch: {total_bytes} != {expected}"
    );
    anyhow::ensure!(
        total_bytes <= MAX_IMAGE_BYTES,
        "clipboard image exceeds {} MB",
        MAX_IMAGE_BYTES / 1024 / 1024
    );
    Ok(())
}

fn image_fingerprint(image: &arboard::ImageData<'_>) -> ClipboardFingerprint {
    ClipboardFingerprint::Image {
        width: image.width,
        height: image.height,
        sha256: Sha256::digest(image.bytes.as_ref()).into(),
    }
}

fn send_image(tx: &Sender<ClipboardEvent>, image: arboard::ImageData<'static>) -> bool {
    let total_bytes = image.bytes.len();
    if validate_image_shape(image.width, image.height, total_bytes).is_err() {
        debug!(
            width = image.width,
            height = image.height,
            bytes = total_bytes,
            cap = MAX_IMAGE_BYTES,
            "skipping invalid or oversized clipboard image"
        );
        return true;
    }
    let Ok(width) = u32::try_from(image.width) else {
        return true;
    };
    let Ok(height) = u32::try_from(image.height) else {
        return true;
    };
    let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
    if tx
        .blocking_send(ClipboardEvent::ImageStart {
            id,
            width,
            height,
            total_bytes: total_bytes as u64,
        })
        .is_err()
    {
        return false;
    }
    for (index, chunk) in image.bytes.chunks(IMAGE_CHUNK_BYTES).enumerate() {
        if tx
            .blocking_send(ClipboardEvent::ImageChunk {
                id,
                offset: (index * IMAGE_CHUNK_BYTES) as u64,
                data: chunk.to_vec(),
            })
            .is_err()
        {
            return false;
        }
    }
    tx.blocking_send(ClipboardEvent::ImageEnd { id }).is_ok()
}

/// Spawn the clipboard watcher thread (idempotent). Only the first
/// call actually starts the OS-side thread; subsequent calls are
/// no-ops, so it's safe to invoke this from every peer session.
pub fn ensure_watcher(tx: Sender<ClipboardEvent>) {
    *INCOMING_IMAGE.lock() = None;
    // DIAGNOSTIC GATE (crash hunt): MINESHARE_NO_CLIPBOARD=1 disables
    // all clipboard sync, to test whether Win32 clipboard / OLE access
    // from the watcher thread + apply_from_peer racing with WebView2
    // is the STATUS_HEAP_CORRUPTION trigger.
    if std::env::var_os("MINESHARE_NO_CLIPBOARD").is_some() {
        warn!("clipboard sync disabled via MINESHARE_NO_CLIPBOARD");
        return;
    }
    if WATCHER_RUNNING.swap(true, Ordering::AcqRel) {
        // Already running. Replace the active sender so the new
        // peer session receives change notifications instead of the
        // dropped one.
        let _ = REPLACE_TX_GUARD.lock().replace(tx);
        return;
    }
    *REPLACE_TX_GUARD.lock() = Some(tx);

    // Channel for inbound peer clipboard payloads. The watcher thread
    // is the *only* thread that ever touches `arboard`; everything
    // else (apply_from_peer) hands work to it through here.
    let (apply_tx, apply_rx) = std::sync::mpsc::sync_channel::<()>(APPLY_QUEUE_CAPACITY);
    *APPLY_TX.lock() = Some(apply_tx);
    let runtime = tokio::runtime::Handle::current();

    let started = thread::Builder::new()
        .name("clipboard-watcher".into())
        .spawn(move || {
            let _running = WatcherRunning;
            let mut clipboard = match arboard::Clipboard::new() {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "clipboard init failed — text sync disabled");
                    return;
                }
            };
            info!(poll_ms = POLL_MS, "clipboard watcher started");

            loop {
                // Block up to POLL_MS for an inbound peer payload. A
                // payload wakes us immediately to apply it; a timeout
                // is the cue to poll the local clipboard for changes.
                // Either way, all `arboard` calls stay on this thread.
                match apply_rx.recv_timeout(Duration::from_millis(POLL_MS)) {
                    Ok(()) => {
                        let Some(event) = PENDING_APPLY.lock().take() else {
                            continue;
                        };
                        match event {
                            ApplyEvent::Files(paths) => {
                                let fingerprint = file_fingerprint(&paths);
                                match clipboard.set().file_list(&paths) {
                                    Ok(()) => {
                                        *LAST_CLIPBOARD.lock() = Some(fingerprint);
                                        info!(count = paths.len(), "applied peer clipboard files");
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "failed to apply peer clipboard files")
                                    }
                                }
                            }
                            ApplyEvent::Text(text) => {
                                let fingerprint = ClipboardFingerprint::Text(text.clone());
                                if let Err(e) = clipboard.set_text(text) {
                                    debug!(error = %e, "failed to apply peer clipboard text");
                                } else {
                                    *LAST_CLIPBOARD.lock() = Some(fingerprint);
                                    debug!("applied peer clipboard text (on watcher thread)");
                                }
                            }
                            ApplyEvent::Image(image) => {
                                let fingerprint = image_fingerprint(&image);
                                let width = image.width;
                                let height = image.height;
                                if let Err(e) = clipboard.set_image(image) {
                                    debug!(
                                        error = %e,
                                        "failed to apply peer clipboard image"
                                    );
                                } else {
                                    *LAST_CLIPBOARD.lock() = Some(fingerprint);
                                    info!(width, height, "applied peer clipboard image");
                                }
                            }
                        }
                        continue;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Fall through to the change-detection poll.
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        // The static holds the Sender for the process
                        // lifetime, so this is unreachable in practice;
                        // keep polling rather than killing the thread.
                    }
                }
                // Explorer's native file list must not degrade to filename text.
                if let Ok(paths) = clipboard.get().file_list()
                    && !paths.is_empty()
                {
                    let fingerprint = file_fingerprint(&paths);
                    if LAST_CLIPBOARD.lock().as_ref() != Some(&fingerprint) {
                        match crate::files::send_clipboard(paths, &runtime) {
                            Ok(()) => *LAST_CLIPBOARD.lock() = Some(fingerprint),
                            Err(e) => debug!(error = %e, "clipboard files not forwarded"),
                        }
                    }
                    continue;
                }
                // Rich content wins over alternate text (e.g. an image's name).
                match clipboard.get_image() {
                    Ok(image) => {
                        let fingerprint = image_fingerprint(&image);
                        if LAST_CLIPBOARD.lock().as_ref() != Some(&fingerprint) {
                            let tx = REPLACE_TX_GUARD.lock().clone();
                            if let Some(tx) = tx
                                && send_image(&tx, image)
                            {
                                *LAST_CLIPBOARD.lock() = Some(fingerprint);
                            }
                        }
                        continue;
                    }
                    Err(arboard::Error::ContentNotAvailable) => {}
                    Err(e) => {
                        debug!(error = %e, "clipboard image poll error — continuing");
                        continue;
                    }
                }
                let current_text = match clipboard.get_text() {
                    Ok(t) => Some(t),
                    Err(arboard::Error::ContentNotAvailable) => None,
                    Err(e) => {
                        // Transient errors (e.g. focus changes on
                        // Wayland) shouldn't kill the watcher.
                        debug!(error = %e, "clipboard poll error — continuing");
                        continue;
                    }
                };
                if let Some(current) = current_text {
                    if current.len() > MAX_BYTES {
                        debug!(
                            bytes = current.len(),
                            cap = MAX_BYTES,
                            "skipping oversized clipboard text"
                        );
                        continue;
                    }
                    let fingerprint = ClipboardFingerprint::Text(current.clone());
                    if LAST_CLIPBOARD.lock().as_ref() == Some(&fingerprint) {
                        continue;
                    }
                    let tx = REPLACE_TX_GUARD.lock().clone();
                    if let Some(tx) = tx {
                        if tx.blocking_send(ClipboardEvent::Text(current)).is_err() {
                            debug!("clipboard sink closed — waiting for next session");
                        } else {
                            *LAST_CLIPBOARD.lock() = Some(fingerprint);
                        }
                    }
                    continue;
                }
            }
        });
    if let Err(e) = started {
        drop(WatcherRunning);
        warn!(error = %e, "clipboard watcher failed to start; will retry next session");
    }
}

/// Apply a clipboard payload that arrived from the peer.
///
/// This does **not** touch `arboard` directly — doing so from the
/// tokio reader thread is what raced the watcher thread's OLE
/// clipboard access and corrupted the heap inside the GUI process
/// (see `APPLY_TX`). Instead we hand the text to the single
/// clipboard-owning watcher thread, which sets it (and updates
/// `LAST_TEXT` for the echo guard) on its own thread.
pub fn apply_from_peer(event: ClipboardEvent) -> Result<()> {
    if std::env::var_os("MINESHARE_NO_CLIPBOARD").is_some() {
        return Ok(());
    }
    // Assemble reliable TCP chunks before the bounded OS queue. A burst of
    // chunks must never lose its tail merely because the watcher is busy.
    let Some(event) = assemble_event(&mut INCOMING_IMAGE.lock(), event)? else {
        return Ok(());
    };
    enqueue_apply(event)
}

fn file_fingerprint(paths: &[PathBuf]) -> ClipboardFingerprint {
    ClipboardFingerprint::Files(
        paths
            .iter()
            .map(|path| {
                let metadata = std::fs::metadata(path).ok();
                (
                    path.clone(),
                    metadata.as_ref().map_or(0, |m| m.len()),
                    metadata.and_then(|m| m.modified().ok()),
                )
            })
            .collect(),
    )
}

pub fn apply_files_from_peer(paths: Vec<PathBuf>) -> Result<()> {
    if std::env::var_os("MINESHARE_NO_CLIPBOARD").is_some() {
        return Ok(());
    }
    *INCOMING_IMAGE.lock() = None;
    enqueue_apply(ApplyEvent::Files(paths))
}

fn enqueue_apply(event: ApplyEvent) -> Result<()> {
    let guard = APPLY_TX.lock();
    match guard.as_ref() {
        Some(tx) => {
            // Clipboard is a snapshot: retain only the newest COMPLETE payload.
            // A full wake-up queue cannot discard any image chunks.
            *PENDING_APPLY.lock() = Some(event);
            match tx.try_send(()) {
                Ok(()) | Err(std::sync::mpsc::TrySendError::Full(())) => {}
                Err(e) => return Err(anyhow::anyhow!("clipboard apply queue unavailable: {e}")),
            }
            debug!("queued peer clipboard event for watcher thread");
        }
        None => {
            // Watcher never started (init failed or clipboard disabled)
            // — drop silently; clipboard sync is best-effort.
            debug!("clipboard apply skipped — watcher not running");
        }
    }
    Ok(())
}

fn assemble_event(
    incoming: &mut Option<IncomingImage>,
    event: ClipboardEvent,
) -> Result<Option<ApplyEvent>> {
    match event {
        ClipboardEvent::Text(text) => {
            *incoming = None;
            Ok(Some(ApplyEvent::Text(text)))
        }
        ClipboardEvent::ImageStart {
            id,
            width,
            height,
            total_bytes,
        } => {
            *incoming = None;
            *incoming = Some(IncomingImage::new(id, width, height, total_bytes)?);
            Ok(None)
        }
        ClipboardEvent::ImageChunk { id, offset, data } => {
            let result = incoming
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("image chunk without start"))?
                .push(id, offset, &data);
            if result.is_err() {
                *incoming = None;
            }
            result?;
            Ok(None)
        }
        ClipboardEvent::ImageEnd { id } => {
            let image = incoming
                .take()
                .ok_or_else(|| anyhow::anyhow!("image end without start"))?
                .finish(id)?;
            Ok(Some(ApplyEvent::Image(image)))
        }
    }
}

/// Holds the currently-active mpsc sender so the watcher can hand
/// off new sessions without restarting the OS thread.
static REPLACE_TX_GUARD: Mutex<Option<Sender<ClipboardEvent>>> = Mutex::new(None);

pub const fn queue_capacity() -> usize {
    CLIPBOARD_QUEUE_CAPACITY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_burst_larger_than_old_queue_publishes_only_when_complete() {
        let size = 1024 * 768 * 4;
        let mut incoming = None;
        assert!(
            assemble_event(
                &mut incoming,
                ClipboardEvent::ImageStart {
                    id: 41,
                    width: 1024,
                    height: 768,
                    total_bytes: size as u64,
                }
            )
            .unwrap()
            .is_none()
        );
        for offset in (0..size).step_by(IMAGE_CHUNK_BYTES) {
            assert!(
                assemble_event(
                    &mut incoming,
                    ClipboardEvent::ImageChunk {
                        id: 41,
                        offset: offset as u64,
                        data: vec![123; IMAGE_CHUNK_BYTES.min(size - offset)],
                    }
                )
                .unwrap()
                .is_none()
            );
        }
        let Some(ApplyEvent::Image(image)) =
            assemble_event(&mut incoming, ClipboardEvent::ImageEnd { id: 41 }).unwrap()
        else {
            panic!("complete screenshot not published");
        };
        assert_eq!(image.bytes.len(), size);
        assert!(image.bytes.iter().all(|v| *v == 123));
        assert!(incoming.is_none());
    }

    #[test]
    fn invalid_chunk_and_replacement_cancel_previous_image() {
        let mut incoming = Some(IncomingImage::new(1, 1, 1, 4).unwrap());
        assert!(
            assemble_event(
                &mut incoming,
                ClipboardEvent::ImageChunk {
                    id: 2,
                    offset: 0,
                    data: vec![0; 4]
                }
            )
            .is_err()
        );
        assert!(incoming.is_none());
        assert!(assemble_event(&mut incoming, ClipboardEvent::ImageEnd { id: 1 }).is_err());
        incoming = Some(IncomingImage::new(1, 1, 1, 4).unwrap());
        assert!(matches!(
            assemble_event(&mut incoming, ClipboardEvent::Text("replacement".into())).unwrap(),
            Some(ApplyEvent::Text(_))
        ));
        assert!(incoming.is_none());
    }

    #[test]
    fn largest_clipboard_text_fits_noise_control_frame() {
        let msg = crate::runtime::ControlMsg::ClipboardText("x".repeat(MAX_BYTES));
        let payload = bincode::serde::encode_to_vec(msg, bincode::config::standard()).unwrap();
        assert!(payload.len() + 16 <= 65_535);
    }

    #[test]
    fn image_transfer_reassembles_rgba_exactly() {
        let rgba: Vec<u8> = (0..64).collect();
        let mut image = IncomingImage::new(7, 4, 4, rgba.len() as u64).unwrap();
        image.push(7, 0, &rgba[..31]).unwrap();
        image.push(7, 31, &rgba[31..]).unwrap();
        let image = image.finish(7).unwrap();
        assert_eq!(image.width, 4);
        assert_eq!(image.height, 4);
        assert_eq!(image.bytes.as_ref(), rgba);
    }

    #[test]
    fn image_transfer_rejects_invalid_shape_and_offsets() {
        assert!(IncomingImage::new(1, 4, 4, 63).is_err());
        assert!(IncomingImage::new(1, 0, 4, 0).is_err());
        assert!(IncomingImage::new(1, 8_192, 8_192, 268_435_456).is_err());

        let mut image = IncomingImage::new(1, 1, 1, 4).unwrap();
        assert!(image.push(1, 1, &[0, 0, 0, 0]).is_err());
    }

    #[test]
    fn image_sender_chunks_frames_below_control_limit() {
        let rgba = vec![0x7f; 128 * 128 * 4];
        let image = arboard::ImageData {
            width: 128,
            height: 128,
            bytes: Cow::Owned(rgba),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        assert!(send_image(&tx, image));

        assert!(matches!(
            rx.try_recv().unwrap(),
            ClipboardEvent::ImageStart {
                width: 128,
                height: 128,
                total_bytes: 65_536,
                ..
            }
        ));
        for expected_offset in [0, IMAGE_CHUNK_BYTES as u64] {
            match rx.try_recv().unwrap() {
                ClipboardEvent::ImageChunk { offset, data, .. } => {
                    assert_eq!(offset, expected_offset);
                    assert_eq!(data.len(), IMAGE_CHUNK_BYTES);
                }
                other => panic!("expected image chunk, got {other:?}"),
            }
        }
        assert!(matches!(
            rx.try_recv().unwrap(),
            ClipboardEvent::ImageEnd { .. }
        ));
    }
}
