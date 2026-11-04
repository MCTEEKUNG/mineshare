//! Cross-platform clipboard text + image sync.
//!
//! M4 Slice 1: a daemon-lifetime watcher polls the local clipboard
//! every `POLL_MS` (Windows checks its revision without opening unchanged
//! data) and pushes changed snapshots onto an mpsc. The writer reads events and
//! sends them as `ControlMsg::ClipboardText` over the existing
//! encrypted TCP control channel — no new sockets, no new
//! handshakes.
//!
//! ## Echo guard
//!
//! A naive setup loops: A copies → A→B sends → B sets clipboard →
//! B's watcher sees a "change" → B→A sends → A sets → … forever.
//! We sidestep that by sharing the `LAST_CLIPBOARD` static between the
//! watcher and the inbound handler. When we set the clipboard from a
//! peer message we *also* update `LAST_CLIPBOARD` to the same value, so
//! the next watcher tick sees `current == last` and skips the send.
//! Windows also remembers the revision of our own write: unchanged images
//! are never decoded again, and Ctrl+C of identical content is a new local
//! copy. Queued peer data cannot replace a newer local revision.
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
use std::time::{Duration, Instant};
use std::{sync::Arc, thread};

use anyhow::Result;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

// Windows can check a revision without opening (and locking) the clipboard.
// Only read/decode its contents when that revision changes.
const POLL_MS: u64 = if cfg!(target_os = "windows") { 50 } else { 400 };
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
    FileOffer(u64),
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
    Files(Vec<PathBuf>, Paste),
}

const OFFER_TTL: Duration = Duration::from_secs(120);

#[derive(Clone)]
struct Offer {
    id: u64,
    revision: Option<u32>,
    fingerprint: Option<ClipboardFingerprint>,
    expires: Instant,
}

struct Paste {
    offer: Offer,
    window: u64,
    interaction: u64,
    sent: bool,
    inject: Arc<dyn mineshare_input::InputInject>,
    tx: crate::files::ControlSender,
}

#[derive(Default)]
struct FileSession {
    connection: Option<(
        crate::files::ControlSender,
        Arc<dyn mineshare_input::InputInject>,
    )>,
    local: Option<Offer>,
    remote: Option<Offer>,
    paste: Option<Paste>,
    request: Option<(Offer, crate::files::ControlSender)>,
}

static FILE_SESSION: Mutex<FileSession> = Mutex::new(FileSession {
    connection: None,
    local: None,
    remote: None,
    paste: None,
    request: None,
});
static NEXT_FILE_OFFER: AtomicU64 = AtomicU64::new(1);
#[cfg(test)]
static TEST_PASTE_WINDOW: AtomicU64 = AtomicU64::new(0);

fn paste_window() -> Option<u64> {
    #[cfg(test)]
    {
        let id = TEST_PASTE_WINDOW.load(Ordering::Relaxed);
        if id != 0 {
            return Some(id);
        }
    }
    mineshare_input::paste_window()
}

pub fn clear_file_session() {
    *FILE_SESSION.lock() = FileSession::default();
}

fn wake_watcher() {
    if let Some(tx) = APPLY_TX.lock().as_ref() {
        let _ = tx.try_send(());
    }
}

pub fn offer_from_peer(id: u64) {
    let revision = clipboard_revision();
    let fingerprint = LAST_CLIPBOARD.lock().clone();
    let mut state = FILE_SESSION.lock();
    if state.connection.is_some() {
        state.remote = (id != 0).then(|| Offer {
            id,
            revision,
            fingerprint,
            expires: Instant::now() + OFFER_TTL,
        });
    }
}

fn request_local_paste() -> bool {
    let offer_id = {
        let mut state = FILE_SESSION.lock();
        let Some(offer) = state.remote.as_ref() else {
            return false;
        };
        if offer.revision != clipboard_revision() || Instant::now() >= offer.expires {
            state.remote = None;
            return false;
        }
        offer.id
    };
    let Some(window) = paste_window() else {
        return false;
    };
    let mut state = FILE_SESSION.lock();
    if !state
        .remote
        .as_ref()
        .is_some_and(|offer| offer.id == offer_id)
    {
        return false;
    }
    let Some((tx, inject)) = state.connection.clone() else {
        return false;
    };
    let offer = state.remote.take().unwrap();
    state.paste = Some(Paste {
        offer,
        window,
        interaction: mineshare_input::file_paste_interaction(),
        sent: false,
        inject,
        tx,
    });
    drop(state);
    wake_watcher();
    true
}

pub fn request_from_peer(id: u64) {
    let mut state = FILE_SESSION.lock();
    if state.local.as_ref().is_some_and(|offer| offer.id == id)
        && let Some((tx, _)) = state.connection.clone()
    {
        let offer = state.local.take().unwrap();
        state.request = Some((offer, tx));
        drop(state);
        wake_watcher();
    }
}

pub fn finish_file_paste(id: u64, ids: &[u64]) {
    let paste = {
        let mut state = FILE_SESSION.lock();
        if !state
            .paste
            .as_ref()
            .is_some_and(|p| p.offer.id == id && p.sent)
        {
            return;
        }
        state.paste.take().unwrap()
    };
    if ids.is_empty() || Instant::now() >= paste.offer.expires {
        return;
    }
    if let Err(error) = crate::files::received_clipboard_paths(ids).and_then(|paths| {
        enqueue_apply(PendingApply {
            received_revision: paste.offer.revision,
            event: ApplyEvent::Files(paths, paste),
        })
    }) {
        warn!(%error, "could not prepare requested file paste");
    }
}

fn clipboard_file_list(clipboard: &mut arboard::Clipboard) -> Result<Vec<PathBuf>> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        use std::io::Read;
        use wl_clipboard_rs::paste::{self, ClipboardType, MimeType, Seat};
        match paste::get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            MimeType::Specific("text/uri-list"),
        ) {
            Ok((pipe, _)) => {
                const MAX_URI_LIST: u64 = 1024 * 1024;
                let mut list = Vec::new();
                pipe.take(MAX_URI_LIST + 1).read_to_end(&mut list)?;
                anyhow::ensure!(
                    list.len() as u64 <= MAX_URI_LIST,
                    "file clipboard URI list too large"
                );
                return parse_file_uris(&list);
            }
            Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(Vec::new()),
            Err(_) => {} // Read the Xwayland selection on GNOME.
        }
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_some() {
        return x11_file_list();
    }
    Ok(clipboard.get().file_list()?)
}

#[cfg(target_os = "linux")]
fn x11_file_list() -> Result<Vec<PathBuf>> {
    thread_local! {
        static CLIPBOARD: std::cell::RefCell<Option<(x11_clipboard::Clipboard, u32)>> =
            const { std::cell::RefCell::new(None) };
    }
    CLIPBOARD.with(|state| {
        let mut state = state.borrow_mut();
        if state.is_none() {
            let clipboard = x11_clipboard::Clipboard::new()?;
            let uri_list = clipboard.getter.get_atom("text/uri-list")?;
            *state = Some((clipboard, uri_list));
        }
        let (clipboard, uri_list) = state.as_ref().unwrap();
        let atoms = &clipboard.getter.atoms;
        let list = clipboard.load(
            atoms.clipboard,
            *uri_list,
            atoms.property,
            Duration::from_millis(100),
        )?;
        parse_file_uris(&list)
    })
}

#[cfg(target_os = "linux")]
fn parse_file_uris(list: &[u8]) -> Result<Vec<PathBuf>> {
    use std::os::unix::ffi::OsStrExt;
    list.split(|&byte| byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty() && !line.starts_with(b"#"))
        .map(|line| {
            // Strip CRLF before decoding. A percent-encoded CR may be part of
            // a legitimate Unix filename, while a raw CR ends the URI line.
            let path = line
                .strip_prefix(b"file://")
                .filter(|path| path.starts_with(b"/"))
                .ok_or_else(|| anyhow::anyhow!("file clipboard URI is not a local path"))?;
            let bytes: Vec<u8> = percent_encoding::percent_decode(path).collect();
            anyhow::ensure!(!bytes.contains(&0), "file clipboard URI contains NUL");
            Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
        })
        .collect()
}

fn snapshot_fingerprint(
    clipboard: &mut arboard::Clipboard,
) -> Result<Option<ClipboardFingerprint>> {
    if let Ok(paths) = clipboard_file_list(clipboard)
        && !paths.is_empty()
    {
        return Ok(Some(file_fingerprint(&paths)));
    }
    match clipboard.get_image() {
        Ok(image) => return Ok(Some(image_fingerprint(&image))),
        Err(arboard::Error::ContentNotAvailable) => {}
        Err(error) => return Err(error.into()),
    }
    match clipboard.get_text() {
        Ok(text) => Ok(Some(ClipboardFingerprint::Text(text))),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn paste_is_current(paste: &Paste, clipboard: &mut arboard::Clipboard) -> bool {
    !paste.tx.is_closed()
        && Instant::now() < paste.offer.expires
        && paste.interaction == mineshare_input::file_paste_interaction()
        && paste.offer.revision == clipboard_revision()
        && snapshot_fingerprint(clipboard).is_ok_and(|fp| fp == paste.offer.fingerprint)
        && paste_window() == Some(paste.window)
}

fn service_file_requests(clipboard: &mut arboard::Clipboard, runtime: &tokio::runtime::Handle) {
    use crate::runtime::ControlMsg;
    let (request, paste) = {
        let mut state = FILE_SESSION.lock();
        let paste = if state.paste.as_ref().is_some_and(|p| !p.sent) {
            state.paste.take()
        } else {
            None
        };
        (state.request.take(), paste)
    };
    if let Some(mut paste) = paste {
        if paste_is_current(&paste, clipboard) {
            let mut state = FILE_SESSION.lock();
            // Publish pending state before the peer can reply. A new request
            // may supersede this one while the clipboard is being checked.
            if state.paste.is_none()
                && state
                    .connection
                    .as_ref()
                    .is_some_and(|(tx, _)| tx.same_channel(&paste.tx))
                && paste
                    .tx
                    .try_send(ControlMsg::ClipboardFilePaste(paste.offer.id))
                    .is_ok()
            {
                paste.sent = true;
                state.paste = Some(paste);
            }
        } else if !paste.tx.is_closed()
            && Instant::now() < paste.offer.expires
            && paste.interaction == mineshare_input::file_paste_interaction()
        {
            // A local copy can beat Linux polling. Deliver the original
            // shortcut without asking the peer when that happens.
            if let Err(error) = paste.inject.paste_in_window(paste.window) {
                warn!(%error, "could not restore local clipboard paste");
            }
        }
    }
    if let Some((offer, tx)) = request {
        let paths = if !tx.is_closed()
            && Instant::now() < offer.expires
            && offer.revision == clipboard_revision()
            && !files_excluded_from_monitoring()
        {
            clipboard_file_list(clipboard).ok().filter(|paths| {
                offer.fingerprint.as_ref() == Some(&file_fingerprint(paths))
                    && offer.revision == clipboard_revision()
            })
        } else {
            None
        };
        runtime.spawn(async move {
            if let Some(paths) = paths {
                match crate::files::send_clipboard(paths, offer.id, tx.clone()).await {
                    Ok(()) => return,
                    Err(error) => warn!(%error, "requested clipboard file transfer failed"),
                }
            }
            let _ = tx
                .send(ControlMsg::ClipboardFiles {
                    offer: offer.id,
                    ids: Vec::new(),
                })
                .await;
        });
    }
}

struct PendingApply {
    event: ApplyEvent,
    received_revision: Option<u32>,
}

/// Local clipboard writes take precedence over an older queued peer snapshot.
/// Our own writes are distinguished from user copies, so two consecutive peer
/// updates still work even if the second arrived while applying the first.
#[derive(Default)]
struct RevisionGuard {
    observed: Option<u32>,
    applied: Option<u32>,
    polled: bool,
}

impl RevisionGuard {
    fn new(revision: Option<u32>) -> Self {
        Self {
            observed: revision,
            ..Self::default()
        }
    }

    fn needs_poll(&self, revision: Option<u32>) -> bool {
        !self.polled || revision.is_none() || revision != self.observed
    }

    fn observe(&mut self, revision: Option<u32>) {
        self.observed = revision;
        self.polled = true;
    }

    fn applied(&mut self, revision: Option<u32>) {
        self.observe(revision);
        self.applied = revision;
    }

    fn may_apply(&self, received: Option<u32>, current: Option<u32>) -> bool {
        let Some(current) = current else {
            return true;
        };
        if self.local_revision_changed(Some(current)) {
            // A user copy happened since our last poll. Do not overwrite it
            // just because an inbound wake-up beat the change-detection tick.
            return false;
        }
        received.is_none_or(|received| received == current) || self.applied == Some(current)
    }

    fn local_revision_changed(&self, revision: Option<u32>) -> bool {
        revision.is_some() && self.observed.is_some() && revision != self.observed
    }
}

fn clipboard_revision() -> Option<u32> {
    #[cfg(target_os = "windows")]
    {
        // This does not OpenClipboard or read any user data. Zero means the
        // API is unavailable for this window station; retain basic polling.
        let revision =
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        (revision != 0).then_some(revision)
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

static APPLY_TX: Mutex<Option<std::sync::mpsc::SyncSender<()>>> = Mutex::new(None);
static PENDING_APPLY: Mutex<Option<PendingApply>> = Mutex::new(None);
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
    received_revision: Option<u32>,
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
            received_revision: clipboard_revision(),
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
pub fn ensure_watcher(
    tx: Sender<ClipboardEvent>,
    file_tx: crate::files::ControlSender,
    inject: Arc<dyn mineshare_input::InputInject>,
) {
    *INCOMING_IMAGE.lock() = None;
    // DIAGNOSTIC GATE (crash hunt): MINESHARE_NO_CLIPBOARD=1 disables
    // all clipboard sync, to test whether Win32 clipboard / OLE access
    // from the watcher thread + apply_from_peer racing with WebView2
    // is the STATUS_HEAP_CORRUPTION trigger.
    if std::env::var_os("MINESHARE_NO_CLIPBOARD").is_some() {
        warn!("clipboard sync disabled via MINESHARE_NO_CLIPBOARD");
        return;
    }
    *FILE_SESSION.lock() = FileSession {
        connection: Some((file_tx, inject)),
        ..FileSession::default()
    };
    mineshare_input::register_file_paste(request_local_paste);
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
            let mut revisions = RevisionGuard::new(clipboard_revision());

            loop {
                service_file_requests(&mut clipboard, &runtime);
                // Block up to POLL_MS for an inbound peer payload. A
                // payload wakes us immediately to apply it; a timeout
                // is the cue to poll the local clipboard for changes.
                // Either way, all `arboard` calls stay on this thread.
                match apply_rx.recv_timeout(Duration::from_millis(POLL_MS)) {
                    Ok(()) => {
                        let Some(pending) = PENDING_APPLY.lock().take() else {
                            continue;
                        };
                        if revisions.may_apply(pending.received_revision, clipboard_revision()) {
                            match pending.event {
                                ApplyEvent::Files(paths, paste) => {
                                    if !paste_is_current(&paste, &mut clipboard) {
                                        continue;
                                    }
                                    let fingerprint = file_fingerprint(&paths);
                                    let setter = clipboard.set();
                                    #[cfg(target_os = "windows")]
                                    let setter = {
                                        use arboard::SetExtWindows;
                                        setter.exclude_from_monitoring()
                                    };
                                    match setter.file_list(&paths) {
                                        Ok(()) => {
                                            *LAST_CLIPBOARD.lock() = Some(fingerprint);
                                            revisions.applied(clipboard_revision());
                                            info!(count = paths.len(), "applied peer clipboard files");
                                            if paste.interaction == mineshare_input::file_paste_interaction()
                                                && let Err(error) = paste.inject.paste_in_window(paste.window)
                                            {
                                                warn!(%error, "could not replay requested file paste");
                                            }
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
                                        revisions.applied(clipboard_revision());
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
                                        revisions.applied(clipboard_revision());
                                        info!(width, height, "applied peer clipboard image");
                                    }
                                }
                            }
                            continue;
                        }
                        debug!("discarded stale peer clipboard — preserving newer local copy");
                        // Poll and publish the local copy now, rather than
                        // letting a burst of stale inbound events starve it.
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
                let revision = clipboard_revision();
                if !revisions.needs_poll(revision) {
                    continue;
                }
                // Explorer's native file list must not degrade to filename text.
                if let Ok(paths) = clipboard_file_list(&mut clipboard)
                    && !paths.is_empty()
                {
                    let fingerprint = file_fingerprint(&paths);
                    if clipboard_revision() != revision {
                        continue;
                    }
                    if should_send_files(&revisions, revision, &fingerprint, LAST_CLIPBOARD.lock().as_ref(), files_excluded_from_monitoring())
                        && paths.len() <= crate::files::MAX_CLIPBOARD_FILES
                    {
                        let id = NEXT_FILE_OFFER.fetch_add(1, Ordering::Relaxed);
                        let tx = REPLACE_TX_GUARD.lock().clone();
                        let mut state = FILE_SESSION.lock();
                        state.remote = None;
                        state.paste = None;
                        state.local = state.connection.as_ref().map(|_| Offer {
                            id, revision, fingerprint: Some(fingerprint.clone()), expires: Instant::now() + OFFER_TTL,
                        });
                        drop(state);
                        if let Some(tx) = tx {
                            let _ = tx.blocking_send(ClipboardEvent::FileOffer(id));
                        }
                    }
                    *LAST_CLIPBOARD.lock() = Some(fingerprint);
                    revisions.observe(revision);
                    continue;
                }
                // Rich content wins over alternate text (e.g. an image's name).
                match clipboard.get_image() {
                    Ok(image) => {
                        let fingerprint = image_fingerprint(&image);
                        if clipboard_revision() != revision {
                            continue;
                        }
                        if revisions.local_revision_changed(revision)
                            || LAST_CLIPBOARD.lock().as_ref() != Some(&fingerprint) {
                            invalidate_file_offers();
                            let tx = REPLACE_TX_GUARD.lock().clone();
                            if let Some(tx) = tx
                                && send_image(&tx, image)
                            {
                                *LAST_CLIPBOARD.lock() = Some(fingerprint);
                                revisions.observe(revision);
                            }
                        } else {
                            revisions.observe(revision);
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
                if clipboard_revision() != revision {
                    continue;
                }
                if let Some(current) = current_text {
                    if current.len() > MAX_BYTES {
                        revisions.observe(revision);
                        debug!(
                            bytes = current.len(),
                            cap = MAX_BYTES,
                            "skipping oversized clipboard text"
                        );
                        continue;
                    }
                    let fingerprint = ClipboardFingerprint::Text(current.clone());
                    if !revisions.local_revision_changed(revision)
                        && LAST_CLIPBOARD.lock().as_ref() == Some(&fingerprint) {
                        revisions.observe(revision);
                        continue;
                    }
                    invalidate_file_offers();
                    let tx = REPLACE_TX_GUARD.lock().clone();
                    if let Some(tx) = tx {
                        if tx.blocking_send(ClipboardEvent::Text(current)).is_err() {
                            debug!("clipboard sink closed — waiting for next session");
                        } else {
                            *LAST_CLIPBOARD.lock() = Some(fingerprint);
                            revisions.observe(revision);
                        }
                    }
                    continue;
                }
                if revisions.local_revision_changed(revision) {
                    invalidate_file_offers();
                }
                revisions.observe(revision);
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

fn should_send_files(
    revisions: &RevisionGuard,
    revision: Option<u32>,
    fingerprint: &ClipboardFingerprint,
    last: Option<&ClipboardFingerprint>,
    excluded_from_monitoring: bool,
) -> bool {
    if excluded_from_monitoring
        || (!revisions.polled && !revisions.local_revision_changed(revision))
    {
        return false;
    }
    revisions.local_revision_changed(revision) || last != Some(fingerprint)
}

fn files_excluded_from_monitoring() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::DataExchange::{
            IsClipboardFormatAvailable, RegisterClipboardFormatW,
        };
        static FORMAT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        let format = *FORMAT.get_or_init(|| {
            let name: Vec<u16> = "ExcludeClipboardContentFromMonitorProcessing\0"
                .encode_utf16()
                .collect();
            unsafe { RegisterClipboardFormatW(name.as_ptr()) }
        });
        // Added atomically with CF_HDROP by arboard, retained across process
        // restarts, and cleared by the next genuine native Copy operation.
        format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

fn invalidate_file_offers() {
    let mut state = FILE_SESSION.lock();
    state.remote = None;
    state.paste = None;
    let revoke = state.local.take().is_some();
    drop(state);
    if revoke && let Some(tx) = REPLACE_TX_GUARD.lock().clone() {
        let _ = tx.blocking_send(ClipboardEvent::FileOffer(0));
    }
}

fn enqueue_apply(event: PendingApply) -> Result<()> {
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
) -> Result<Option<PendingApply>> {
    match event {
        ClipboardEvent::FileOffer(_) => Ok(None),
        ClipboardEvent::Text(text) => {
            *incoming = None;
            Ok(Some(PendingApply {
                event: ApplyEvent::Text(text),
                received_revision: clipboard_revision(),
            }))
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
                .ok_or_else(|| anyhow::anyhow!("image end without start"))?;
            let received_revision = image.received_revision;
            Ok(Some(PendingApply {
                event: ApplyEvent::Image(image.finish(id)?),
                received_revision,
            }))
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

    #[cfg(target_os = "linux")]
    #[test]
    fn wayland_uri_lists_keep_localized_paths_and_strip_only_line_endings() {
        assert_eq!(
            parse_file_uris(b"# Files\r\nfile:///tmp/%E0%B9%84%E0%B8%9F%E0%B8%A5%E0%B9%8C%20a.png\r\nfile:///tmp/encoded%0D\r\n").unwrap(),
            [PathBuf::from("/tmp/ไฟล์ a.png"), PathBuf::from("/tmp/encoded\r")],
        );
        assert!(parse_file_uris(b"file://other-host/tmp/a.png\r\n").is_err());
        assert!(parse_file_uris(b"file:///tmp/encoded%00\n").is_err());
    }

    #[cfg(target_os = "windows")]
    fn isolate_native_clipboard() {
        use windows_sys::Win32::System::StationsAndDesktops::*;
        let desktop: Vec<u16> = "Default\0".encode_utf16().collect();
        unsafe {
            let station = CreateWindowStationW(std::ptr::null(), 0, 0x000f_037f, std::ptr::null());
            assert!(!station.is_null(), "private window station unavailable");
            assert_ne!(SetProcessWindowStation(station), 0);
            let desktop = CreateDesktopW(
                desktop.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0x000f_01ff,
                std::ptr::null(),
            );
            assert!(!desktop.is_null(), "private desktop unavailable");
            assert_ne!(SetThreadDesktop(desktop), 0);
        }
    }

    #[test]
    #[ignore = "run alone with MINESHARE_TEST_CLIPBOARD=1 and a private X display on Linux"]
    fn native_file_copy_transfers_only_on_paste_and_preserves_newer_local_copy() -> Result<()> {
        use crate::runtime::ControlMsg;
        use mineshare_input::{Button, InputInject, KeyCode};

        assert_eq!(
            std::env::var("MINESHARE_TEST_CLIPBOARD").as_deref(),
            Ok("1")
        );
        #[cfg(target_os = "windows")]
        isolate_native_clipboard();
        #[cfg(target_os = "linux")]
        assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());

        #[derive(Default)]
        struct Recorder(AtomicU64);
        impl InputInject for Recorder {
            fn mouse_move_rel(&self, _: i32, _: i32) -> Result<()> {
                unreachable!()
            }
            fn mouse_button(&self, _: Button, _: bool) -> Result<()> {
                unreachable!()
            }
            fn key(&self, _: KeyCode, _: bool) -> Result<()> {
                unreachable!()
            }
            fn scroll(&self, _: f32, _: f32) -> Result<()> {
                unreachable!()
            }
            fn paste_in_window(&self, window: u64) -> Result<()> {
                self.0.store(window, Ordering::Release);
                Ok(())
            }
        }

        let dir =
            std::env::temp_dir().join(format!("mineshare-clipboard-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir)?;
        let pdf = dir.join("fixture.pdf");
        let png = dir.join("fixture.png");
        std::fs::write(&pdf, b"%PDF fixture")?;
        std::fs::write(&png, b"PNG fixture")?;
        let mut clipboard = arboard::Clipboard::new()?;
        clipboard.set_text("local baseline")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let _entered = runtime.enter();
        let (events, mut clipboard_events) = tokio::sync::mpsc::channel(queue_capacity());
        let (wire, mut messages) = tokio::sync::mpsc::channel(crate::files::file_queue_capacity());
        let recorder = Arc::new(Recorder::default());
        ensure_watcher(events, wire, recorder.clone());

        runtime.block_on(async {
            let wait = Duration::from_secs(6);
            assert!(matches!(
                tokio::time::timeout(wait, clipboard_events.recv()).await?,
                Some(ClipboardEvent::Text(_))
            ));
            clipboard.set().file_list(&[&pdf, &png])?;
            let Some(ClipboardEvent::FileOffer(id)) =
                tokio::time::timeout(wait, clipboard_events.recv()).await?
            else {
                anyhow::bail!("copy did not publish a token");
            };
            assert!(
                tokio::time::timeout(Duration::from_millis(POLL_MS * 2), messages.recv())
                    .await
                    .is_err()
            );
            assert_eq!(clipboard.get().file_list()?, [pdf.clone(), png.clone()]);
            request_from_peer(id);
            let mut bytes = Vec::new();
            loop {
                match tokio::time::timeout(wait, messages.recv()).await?.unwrap() {
                    ControlMsg::FileOffer { .. } | ControlMsg::FileEnd { .. } => {}
                    ControlMsg::FileChunk { data, .. } => bytes.extend_from_slice(&data),
                    ControlMsg::ClipboardFiles { offer, ids } => {
                        assert_eq!(offer, id);
                        assert_eq!(ids.len(), 2);
                        break;
                    }
                    other => anyhow::bail!("unexpected file transfer message: {other:?}"),
                }
            }
            assert_eq!(bytes, b"%PDF fixturePNG fixture");
            request_from_peer(id);
            assert!(
                tokio::time::timeout(Duration::from_millis(POLL_MS * 2), messages.recv())
                    .await
                    .is_err()
            );

            clipboard.set_text("destination clipboard")?;
            assert!(matches!(
                tokio::time::timeout(wait, clipboard_events.recv()).await?,
                Some(ClipboardEvent::Text(_))
            ));
            let received_id = u64::MAX - 1;
            let contents = b"requested PDF";
            crate::files::begin_incoming_at(
                received_id,
                "received.pdf",
                contents.len() as u64,
                &dir,
            )
            .await?;
            crate::files::write_chunk(received_id, 0, contents).await?;
            crate::files::finalize_incoming(received_id, Sha256::digest(contents).into()).await?;
            TEST_PASTE_WINDOW.store(123, Ordering::Relaxed);
            offer_from_peer(100);
            assert!(request_local_paste());
            assert!(matches!(
                tokio::time::timeout(wait, messages.recv()).await?,
                Some(ControlMsg::ClipboardFilePaste(100))
            ));
            finish_file_paste(100, &[received_id]);
            tokio::time::timeout(wait, async {
                while recorder.0.load(Ordering::Acquire) != 123 {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await?;
            assert_eq!(clipboard.get().file_list()?, [dir.join("received.pdf")]);

            offer_from_peer(101);
            assert!(request_local_paste());
            assert!(matches!(
                tokio::time::timeout(wait, messages.recv()).await?,
                Some(ControlMsg::ClipboardFilePaste(101))
            ));
            TEST_PASTE_WINDOW.store(124, Ordering::Relaxed);
            finish_file_paste(101, &[received_id]);
            tokio::time::sleep(Duration::from_millis(POLL_MS * 2)).await;
            assert_eq!(recorder.0.load(Ordering::Acquire), 123);

            offer_from_peer(102);
            clipboard.set_text("a newer local copy")?;
            let _ = request_local_paste(); // Linux detects the change on its watcher thread.
            assert!(
                tokio::time::timeout(Duration::from_millis(POLL_MS * 2), messages.recv())
                    .await
                    .is_err()
            );
            assert_eq!(clipboard.get_text()?, "a newer local copy");
            clear_file_session();
            Ok::<_, anyhow::Error>(())
        })?;
        std::fs::remove_file(pdf)?;
        std::fs::remove_file(png)?;
        std::fs::remove_file(dir.join("received.pdf"))?;
        std::fs::remove_dir(dir)?;
        Ok(())
    }

    #[test]
    fn file_clipboard_restart_never_replays_six_existing_files() {
        let paths: Vec<_> = (0..6)
            .map(|i| PathBuf::from(format!("received-{i}.txt")))
            .collect();
        let fingerprint = file_fingerprint(&paths);
        for revision in [Some(100), None] {
            // Repeated process restarts lose LAST_CLIPBOARD but retain native
            // file clipboard. None means Linux/content-polling fallback.
            for _restart in 0..4 {
                let guard = RevisionGuard::new(revision);
                assert!(!should_send_files(
                    &guard,
                    revision,
                    &fingerprint,
                    None,
                    false
                ));
            }
        }
    }

    #[test]
    fn peer_file_clipboard_marker_survives_revision_changes_and_restart() {
        let fingerprint = file_fingerprint(&[PathBuf::from("received.txt")]);
        let mut guard = RevisionGuard::new(Some(100));
        guard.applied(Some(100));
        assert!(!should_send_files(
            &guard,
            Some(101),
            &fingerprint,
            Some(&fingerprint),
            true
        ));
        assert!(!should_send_files(
            &RevisionGuard::new(Some(101)),
            Some(101),
            &fingerprint,
            None,
            true
        ));
    }

    #[test]
    fn file_baseline_and_failed_send_do_not_retry_but_fresh_copy_still_works() {
        let fingerprint = file_fingerprint(&[PathBuf::from("example.txt")]);
        for initial in [Some(100), None] {
            let mut guard = RevisionGuard::new(initial);
            assert!(!should_send_files(
                &guard,
                initial,
                &fingerprint,
                None,
                false
            ));
            guard.observe(initial);
            assert!(!should_send_files(
                &guard,
                initial,
                &fingerprint,
                Some(&fingerprint),
                false
            ));
            // A real copy after startup must still be forwarded. On Linux
            // a new file fingerprint, rather than a native revision, detects it.
            let fresh = file_fingerprint(&[PathBuf::from("fresh.txt")]);
            let revision = initial.map(|r| r + 1);
            assert!(should_send_files(
                &guard,
                revision,
                &fresh,
                Some(&fingerprint),
                false
            ));
            // Production records the attempted snapshot before side effects;
            // model an error half way through a multi-file batch here.
            guard.observe(revision);
            for _poll in 0..20 {
                assert!(!should_send_files(
                    &guard,
                    revision,
                    &fresh,
                    Some(&fresh),
                    false
                ));
            }
            if let Some(r) = revision {
                assert!(
                    should_send_files(&guard, Some(r + 1), &fresh, Some(&fresh), false),
                    "explicit recopy is allowed"
                );
            }
        }
    }

    #[test]
    fn local_copy_before_poll_cannot_be_overwritten_by_peer_wakeup() {
        let mut guard = RevisionGuard::new(Some(100));
        guard.applied(Some(100)); // old laptop text currently on desktop
        assert!(!guard.may_apply(Some(101), Some(101))); // Ctrl+C beat our tick
        assert!(guard.needs_poll(Some(101)));
        guard.observe(Some(101)); // publish desktop copy
        assert!(!guard.may_apply(Some(100), Some(101))); // late old laptop data
        assert!(guard.may_apply(Some(101), Some(101))); // truly new peer update
    }

    #[test]
    fn queued_peer_updates_are_not_mistaken_for_local_copies() {
        let mut guard = RevisionGuard::new(Some(100));
        assert!(guard.may_apply(Some(100), Some(100)));
        guard.applied(Some(102));
        assert!(guard.may_apply(Some(100), Some(102)));
        assert!(!guard.needs_poll(Some(102))); // no decode/hash/echo of own write
    }

    #[test]
    fn copying_identical_text_again_is_a_new_local_copy() {
        let mut guard = RevisionGuard::new(Some(100));
        guard.applied(Some(100));
        assert!(!guard.local_revision_changed(Some(100)));
        assert!(guard.local_revision_changed(Some(101)));
        guard.observe(Some(101));
        assert!(!guard.local_revision_changed(Some(101)));
    }

    #[test]
    fn unchanged_windows_clipboard_is_not_reopened_each_tick() {
        let mut guard = RevisionGuard::new(Some(100));
        assert!(guard.needs_poll(Some(100)));
        guard.observe(Some(100));
        assert!(!guard.needs_poll(Some(100)));
        assert!(guard.needs_poll(Some(101)));
        // A failed read/send must leave the revision eligible for retry.
        assert!(guard.needs_poll(Some(101)));
    }

    #[test]
    fn revision_wrap_and_missing_native_api_keep_sync_working() {
        let mut guard = RevisionGuard::new(Some(u32::MAX));
        guard.observe(Some(u32::MAX));
        assert!(!guard.may_apply(Some(u32::MAX), Some(1)));
        assert!(guard.needs_poll(Some(1)));
        guard.observe(Some(1));
        assert!(guard.may_apply(Some(1), Some(1)));
        let mut fallback = RevisionGuard::default();
        fallback.observe(None);
        assert!(fallback.needs_poll(None));
        assert!(fallback.may_apply(None, None));
    }

    #[test]
    fn delayed_image_keeps_begin_revision_and_cannot_replace_new_text() {
        let mut incoming = Some(IncomingImage::new(9, 1, 1, 4).unwrap());
        incoming.as_mut().unwrap().received_revision = Some(100);
        assemble_event(
            &mut incoming,
            ClipboardEvent::ImageChunk {
                id: 9,
                offset: 0,
                data: vec![0; 4],
            },
        )
        .unwrap();
        let pending = assemble_event(&mut incoming, ClipboardEvent::ImageEnd { id: 9 })
            .unwrap()
            .unwrap();
        assert_eq!(pending.received_revision, Some(100));
        let mut guard = RevisionGuard::new(Some(100));
        guard.observe(Some(101));
        assert!(!guard.may_apply(pending.received_revision, Some(101)));
    }

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
        let Some(PendingApply {
            event: ApplyEvent::Image(image),
            ..
        }) = assemble_event(&mut incoming, ClipboardEvent::ImageEnd { id: 41 }).unwrap()
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
            Some(PendingApply {
                event: ApplyEvent::Text(_),
                ..
            })
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
