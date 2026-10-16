//! File transfer between peers.
//!
//! Files ride the existing encrypted TCP control channel — the
//! same Noise XX-derived AEAD that protects everything else, so
//! we get confidentiality + integrity for free without standing
//! up a second connection. Chunks are capped at 64 KB so other
//! ControlMsg traffic (input forwarding, layout pushes, latency
//! pings) interleaves smoothly even during multi-GB transfers.
//!
//! Lifecycle on each side:
//!
//!   sender   : `send_file(path)` →
//!              FileOffer →
//!              FileChunk × N →
//!              FileEnd (with sha256)
//!
//!   receiver : FileOffer → open `<download>/.<name>.partial` →
//!              FileChunk × N → write at offset →
//!              FileEnd → verify sha256 → atomic rename to final
//!
//! Both sides keep a `TransferState` indexed by a sender-generated
//! `transfer_id` so the GUI can render a live progress list and
//! cancel in flight. State lives behind a single Mutex; contention
//! is negligible at 64 KB chunk granularity.

use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Capped at 32 KB to fit comfortably inside Noise's hard
/// **65535-byte ciphertext limit per `write_message` call**
/// (snow library, derived from the Noise spec). The full
/// FileChunk frame is `bincode(ControlMsg::FileChunk { id: u64,
/// offset: u64, data: Vec<u8> })` plus a 16-byte AEAD tag plus
/// the 8-byte explicit nonce — at 64 KB data the plaintext
/// alone (~65557 bytes) already exceeded the Noise limit and
/// every `seal()` returned an error, killing the encrypted
/// control writer and surfacing as "control channel closed
/// mid-transfer" the moment the first chunk hit the wire.
///
/// 32 KB leaves ~32700 bytes of headroom — enough for any
/// future variant overhead — without measurably hurting
/// throughput (still ~3800 chunks/s at 1 Gbit, plenty for
/// real-world file transfers, and TCP coalesces them anyway).
pub const CHUNK_BYTES: usize = 32 * 1024;
pub const MAX_INCOMING_FILE_BYTES: u64 = 100 * 1024 * 1024 * 1024;
const MAX_FILE_NAME_BYTES: usize = 255;
const FILE_QUEUE_CAPACITY: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Bytes flowing OUT of this machine to the peer.
    Sending,
    /// Bytes flowing IN from the peer to this machine.
    Receiving,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Offer sent or received, transfer not yet streaming.
    Pending,
    /// Chunks actively flowing.
    Active,
    /// Stream finished; waiting on the integrity check.
    Verifying,
    /// All done, file at its final destination.
    Done,
    /// Aborted by the user on either side.
    Cancelled,
    /// Network error, sha256 mismatch, disk write fail, etc.
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferSnapshot {
    pub id: u64,
    pub direction: Direction,
    pub status: Status,
    pub name: String,
    pub size_bytes: u64,
    pub bytes_so_far: u64,
    /// Final on-disk path once `Done`. None for outgoing
    /// transfers (it's the path we read from) and for in-flight
    /// receives (still on the `.partial` temp file).
    pub final_path: Option<String>,
    /// Set when status is `Failed` so the GUI can show the user
    /// what went wrong without having to grep the log.
    pub error: Option<String>,
    pub seconds_elapsed: f32,
}

/// Internal mutable state. The temp file handle (for incoming) is
/// kept open for the duration so we don't pay open/close per
/// chunk. We don't hold the outgoing file handle here — the
/// sender task owns its own.
struct Transfer {
    id: u64,
    direction: Direction,
    status: Status,
    name: String,
    size_bytes: u64,
    bytes_so_far: u64,
    /// Where we land the file once `Done`. For outgoing this is
    /// just the source path the user dropped; for incoming it's
    /// the unique destination name we resolved at offer time.
    final_path: Option<PathBuf>,
    /// `<final_path>.partial` for incoming. `None` for outgoing.
    temp_path: Option<PathBuf>,
    /// Open handle for the in-progress receive. Dropped on
    /// `mark_done`/`mark_failed`/`cancel`.
    incoming_file: Option<File>,
    /// Running sha256 for receive integrity check.
    incoming_sha: Option<Sha256>,
    error: Option<String>,
    started_at: Instant,
}

static TRANSFERS: Mutex<Option<HashMap<u64, Transfer>>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut HashMap<u64, Transfer>) -> R) -> R {
    let mut guard = TRANSFERS.lock();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

pub fn next_id() -> u64 {
    // Independent across peers/restarts, while exactly representable by the
    // JavaScript UI (which sends this ID back when the user presses Cancel).
    loop {
        let id = rand::random::<u64>() & ((1u64 << 53) - 1);
        if id != 0 && !with_state(|m| m.contains_key(&id)) {
            return id;
        }
    }
}

/// User-visible "Downloads / MineShare" folder. Created on first
/// incoming transfer so we don't litter empty dirs everywhere.
pub fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("MineShare")
}

/// Strip path components and dangerous chars from a peer-supplied
/// filename. Defends against `..`, absolute paths, and embedded
/// nulls / separators that a malicious peer might use to write
/// outside the download dir.
pub fn sanitize_name(raw: &str) -> String {
    let basename = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed");
    let cleaned: String = basename
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_end_matches(['.', ' ']);
    let stem = cleaned
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        });
    if cleaned.is_empty() {
        "unnamed".to_string()
    } else if reserved {
        format!("_{cleaned}")
    } else {
        cleaned.to_string()
    }
}

/// Collision-resilient destination resolver — if `Downloads/
/// MineShare/foo.png` already exists, returns
/// `foo (1).png`, then `foo (2).png`, etc.
fn resolve_destination(dir: &Path, name: &str) -> PathBuf {
    let target = dir.join(name);
    if !target.exists() {
        return target;
    }
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    for n in 1..=999 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    let suffix = uuid::Uuid::new_v4();
    dir.join(format!("{stem}-{suffix}{ext}"))
}

#[cfg(target_os = "windows")]
fn publish_without_overwrite(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Same-directory rename, deliberately WITHOUT MOVEFILE_REPLACE_EXISTING.
    // Works on Windows download volumes that do not support hard links too.
    // SAFETY: both NUL-terminated paths remain alive throughout the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(target_os = "windows"))]
fn publish_without_overwrite(from: &Path, to: &Path) -> std::io::Result<()> {
    // ponytail: requires hard-link-capable downloads on Unix; use native
    // rename-noreplace if support for other Unix filesystems is required.
    std::fs::hard_link(from, to)
}

// ----------------------------------------------------------------------------
// Sending: state-machine helpers called by the per-transfer
// sender task in runtime.rs.
// ----------------------------------------------------------------------------

pub fn register_outgoing(id: u64, source_path: &Path, size_bytes: u64) {
    let name = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed")
        .to_string();
    let t = Transfer {
        id,
        direction: Direction::Sending,
        status: Status::Pending,
        name,
        size_bytes,
        bytes_so_far: 0,
        final_path: Some(source_path.to_path_buf()),
        temp_path: None,
        incoming_file: None,
        incoming_sha: None,
        error: None,
        started_at: Instant::now(),
    };
    with_state(|m| {
        m.insert(id, t);
    });
}

pub fn mark_active(id: u64) {
    with_state(|m| {
        if let Some(t) = m.get_mut(&id).filter(|t| t.status == Status::Pending) {
            t.status = Status::Active;
        }
    });
}

pub fn add_progress(id: u64, bytes: u64) {
    with_state(|m| {
        if let Some(t) = m.get_mut(&id) {
            t.bytes_so_far = t.bytes_so_far.saturating_add(bytes);
        }
    });
}

pub fn mark_done(id: u64) {
    with_state(|m| {
        if let Some(t) = m.get_mut(&id) {
            if matches!(t.status, Status::Cancelled | Status::Failed) {
                return;
            }
            t.status = Status::Done;
            t.bytes_so_far = t.size_bytes;
            // Drop the receive file handle so the OS flushes.
            t.incoming_file = None;
            t.incoming_sha = None;
        }
    });
}

pub fn mark_failed(id: u64, why: impl Into<String>) {
    let why = why.into();
    tracing::warn!(id, error = %why, "file transfer failed");
    with_state(|m| {
        if let Some(t) = m.get_mut(&id) {
            if matches!(t.status, Status::Done | Status::Cancelled | Status::Failed) {
                return;
            }
            t.status = Status::Failed;
            t.error = Some(why);
            t.incoming_file = None;
            t.incoming_sha = None;
            // Drop temp file (best-effort cleanup).
            if let Some(tp) = t.temp_path.take() {
                let _ = std::fs::remove_file(tp);
            }
        }
    });
}

pub fn fail_incoming(id: u64, why: impl Into<String>) {
    if with_state(|m| {
        m.get(&id)
            .is_some_and(|t| t.direction == Direction::Receiving)
    }) {
        mark_failed(id, why);
    }
}

pub fn mark_cancelled(id: u64) {
    with_state(|m| {
        if let Some(t) = m.get_mut(&id) {
            if matches!(t.status, Status::Done | Status::Cancelled | Status::Failed) {
                return;
            }
            t.status = Status::Cancelled;
            t.incoming_file = None;
            t.incoming_sha = None;
            if let Some(tp) = t.temp_path.take() {
                let _ = std::fs::remove_file(tp);
            }
        }
    });
}

pub fn is_cancelled(id: u64) -> bool {
    with_state(|m| {
        m.get(&id)
            .map(|t| matches!(t.status, Status::Cancelled | Status::Failed))
            .unwrap_or(true)
    })
}

/// Cancel an in-flight transfer from the user (GUI button). The
/// matching `ControlMsg::FileCancel` is sent by the runtime when
/// it next polls the state — there's no immediate signal needed
/// because chunk loops on the sender check `is_cancelled()` per
/// iteration.
pub fn user_cancel(id: u64) {
    mark_cancelled(id);
    if let Some(tx) = session_tx() {
        let _ = tx.try_send(crate::runtime::ControlMsg::FileCancel { id });
    }
}

pub fn snapshot() -> Vec<TransferSnapshot> {
    with_state(|m| {
        let mut v: Vec<_> = m
            .values()
            .map(|t| TransferSnapshot {
                id: t.id,
                direction: t.direction,
                status: t.status,
                name: t.name.clone(),
                size_bytes: t.size_bytes,
                bytes_so_far: t.bytes_so_far,
                final_path: t
                    .final_path
                    .as_ref()
                    .and_then(|p| p.to_str().map(|s| s.to_string())),
                error: t.error.clone(),
                seconds_elapsed: t.started_at.elapsed().as_secs_f32(),
            })
            .collect();
        // Newest first so the GUI shows the active transfer at top.
        v.sort_by(|a, b| a.seconds_elapsed.total_cmp(&b.seconds_elapsed));
        v
    })
}

// ----------------------------------------------------------------------------
// Receiving: called by the runtime reader on each ControlMsg::File*
// arrival.
// ----------------------------------------------------------------------------

/// Allocate the destination path and open the `.partial` file.
/// Duplicate IDs are rejected rather than replacing live or completed state.
pub async fn begin_incoming(id: u64, name: &str, size_bytes: u64) -> Result<()> {
    begin_incoming_at(id, name, size_bytes, &download_dir()).await
}

async fn begin_incoming_at(id: u64, name: &str, size_bytes: u64, dir: &Path) -> Result<()> {
    validate_offer(name, size_bytes)?;
    anyhow::ensure!(
        !with_state(|m| m.contains_key(&id)),
        "transfer id already exists"
    );
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create download dir {}", dir.display()))?;
    let safe_name = sanitize_name(name);
    let final_path = resolve_destination(dir, &safe_name);
    // Never derive the staging path from the destination name: two active
    // transfers with the same name must not truncate each other's bytes.
    let temp_path = dir.join(format!(".mineshare-{}.partial", uuid::Uuid::new_v4()));
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .await
        .with_context(|| format!("open temp file {}", temp_path.display()))?;
    let display_name = final_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&safe_name)
        .to_string();
    tracing::info!(id, name = %display_name, size_bytes, "incoming file");
    with_state(|m| {
        m.insert(
            id,
            Transfer {
                id,
                direction: Direction::Receiving,
                status: Status::Active,
                name: display_name,
                size_bytes,
                bytes_so_far: 0,
                final_path: Some(final_path),
                temp_path: Some(temp_path),
                incoming_file: Some(file),
                incoming_sha: Some(Sha256::new()),
                error: None,
                started_at: Instant::now(),
            },
        );
    });
    Ok(())
}

/// Append the next sequential chunk at `offset`. Rejecting gaps, overlaps, and
/// oversized chunks keeps the declared size and streaming digest trustworthy.
pub async fn write_chunk(id: u64, offset: u64, data: &[u8]) -> Result<()> {
    // Take ownership of the file handle + sha briefly so we can
    // do async IO without holding the parking_lot mutex across
    // an await.
    let (mut file, mut sha) = {
        let mut guard = TRANSFERS.lock();
        let m = guard.get_or_insert_with(HashMap::new);
        let t = m.get_mut(&id).context("unknown transfer id")?;
        anyhow::ensure!(
            t.direction == Direction::Receiving,
            "not an incoming transfer"
        );
        if !matches!(t.status, Status::Active) {
            bail!("transfer {id} not active");
        }
        validate_chunk(t.bytes_so_far, t.size_bytes, offset, data.len())?;
        let file = t
            .incoming_file
            .take()
            .context("transfer file handle gone")?;
        let sha = t.incoming_sha.take().context("transfer sha gone")?;
        (file, sha)
    };
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    file.write_all(data).await?;
    sha.update(data);
    let len = data.len() as u64;
    // Put them back.
    with_state(|m| {
        if let Some(t) = m.get_mut(&id).filter(|t| t.status == Status::Active) {
            t.incoming_file = Some(file);
            t.incoming_sha = Some(sha);
            t.bytes_so_far = offset + len;
        }
    });
    Ok(())
}

/// Finalize: verify sha256, atomic rename to final destination.
pub async fn finalize_incoming(id: u64, expected_sha: [u8; 32]) -> Result<()> {
    // Drain the file + sha + paths.
    let (file, sha, temp_path, final_path) = {
        let mut guard = TRANSFERS.lock();
        let m = guard.get_or_insert_with(HashMap::new);
        let t = m.get_mut(&id).context("unknown transfer id")?;
        anyhow::ensure!(
            t.direction == Direction::Receiving && t.status == Status::Active,
            "incoming transfer is not active"
        );
        anyhow::ensure!(
            t.bytes_so_far == t.size_bytes,
            "transfer {id} incomplete: received {} of {} bytes",
            t.bytes_so_far,
            t.size_bytes
        );
        t.status = Status::Verifying;
        let file = t.incoming_file.take().context("file handle gone")?;
        let sha = t.incoming_sha.take().context("sha gone")?;
        let temp_path = t.temp_path.clone().context("no temp path")?;
        let final_path = t.final_path.clone().context("no final path")?;
        (file, sha, temp_path, final_path)
    };
    // Flush + close the file before rename (Win complains about
    // renaming an open handle).
    let mut file = file;
    file.flush().await?;
    drop(file);

    let got: [u8; 32] = sha.finalize().into();
    if got != expected_sha {
        mark_failed(id, "sha256 mismatch");
        let _ = tokio::fs::remove_file(&temp_path).await;
        bail!("sha256 mismatch on transfer {id}");
    }
    // Commit under the state lock so Cancel cannot race publication. The
    // no-replace operation protects files that appeared since the offer.
    let final_path = with_state(|m| -> Result<PathBuf> {
        let t = m.get_mut(&id).context("unknown transfer id")?;
        anyhow::ensure!(
            t.status == Status::Verifying,
            "transfer cancelled during verification"
        );
        let dir = final_path.parent().context("destination has no parent")?;
        let name = final_path
            .file_name()
            .and_then(|n| n.to_str())
            .context("invalid destination")?;
        let mut destination = final_path.clone();
        loop {
            match publish_without_overwrite(&temp_path, &destination) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    destination = resolve_destination(dir, name);
                }
                Err(e) => return Err(e).context("publish incoming file without overwrite"),
            }
        }
        let _ = std::fs::remove_file(&temp_path);
        t.temp_path = None;
        t.final_path = Some(destination.clone());
        t.name = destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        t.status = Status::Done;
        Ok(destination)
    })?;
    tracing::info!(id, path = %final_path.display(), "incoming file complete");
    Ok(())
}

// ----------------------------------------------------------------------------
// Per-session control-channel sender — set by `run_peer_session`
// so the Tauri command's send-file task can push ControlMsg
// variants without owning the broadcast handle directly.
// ----------------------------------------------------------------------------

pub type ControlSender = tokio::sync::mpsc::Sender<crate::runtime::ControlMsg>;

pub const fn file_queue_capacity() -> usize {
    FILE_QUEUE_CAPACITY
}

static SESSION_TX: Mutex<Option<ControlSender>> = Mutex::new(None);

pub fn set_session_tx(tx: ControlSender) {
    *SESSION_TX.lock() = Some(tx);
}

pub fn clear_session_tx() {
    *SESSION_TX.lock() = None;
    // Also fail any in-flight transfers so the GUI doesn't keep
    // showing a stuck progress bar after the peer drops.
    let pending: Vec<u64> = with_state(|m| {
        m.values()
            .filter(|t| {
                matches!(
                    t.status,
                    Status::Pending | Status::Active | Status::Verifying
                )
            })
            .map(|t| t.id)
            .collect()
    });
    for id in pending {
        mark_failed(id, "session ended mid-transfer");
    }
}

pub fn session_tx() -> Option<ControlSender> {
    SESSION_TX.lock().clone()
}

// ----------------------------------------------------------------------------
// Outgoing send pipeline (called from the Tauri command).
// ----------------------------------------------------------------------------

/// Spawn a task that streams the file at `source_path` through
/// the active session's control channel. Returns the transfer
/// id immediately; progress + completion can be polled via
/// `snapshot()`.
pub fn start_send(source_path: PathBuf) -> Result<u64> {
    let tx = session_tx().context("no peer connected")?;
    if !source_path.is_file() {
        bail!("not a file: {}", source_path.display());
    }
    let metadata = std::fs::metadata(&source_path)
        .with_context(|| format!("stat {}", source_path.display()))?;
    let size_bytes = metadata.len();
    validate_offer(
        source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed"),
        size_bytes,
    )?;
    let id = next_id();
    register_outgoing(id, &source_path, size_bytes);

    tokio::spawn(async move {
        if let Err(e) = drive_send(id, source_path, size_bytes, tx.clone()).await {
            mark_failed(id, format!("{e:#}"));
            let _ = tx.send(crate::runtime::ControlMsg::FileCancel { id }).await;
        }
    });
    Ok(id)
}

pub const MAX_CLIPBOARD_FILES: usize = 64;
const MAX_CLIPBOARD_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Reuse the verified, non-overwriting transfer pipeline; clipboard references
/// follow FileEnd on the SAME FIFO, never before the bytes are present.
pub fn send_clipboard(paths: Vec<PathBuf>, runtime: &tokio::runtime::Handle) -> Result<()> {
    anyhow::ensure!(
        !paths.is_empty() && paths.len() <= MAX_CLIPBOARD_FILES,
        "clipboard file count out of range"
    );
    let tx = session_tx().context("no peer connected")?;
    let mut sources = Vec::with_capacity(paths.len());
    let mut total = 0u64;
    for path in paths {
        let metadata = std::fs::metadata(&path)?;
        anyhow::ensure!(metadata.is_file(), "clipboard folders are not supported");
        total = total
            .checked_add(metadata.len())
            .context("clipboard file sizes overflow")?;
        anyhow::ensure!(
            total <= MAX_CLIPBOARD_FILE_BYTES,
            "clipboard files exceed 256 MiB; use explicit file transfer"
        );
        validate_offer(
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unnamed"),
            metadata.len(),
        )?;
        sources.push((path, metadata.len()));
    }
    // This runs only on the dedicated clipboard OS thread, like send_image.
    // Serialize copies so a slow old batch cannot overwrite a newer clipboard.
    runtime.block_on(async move {
        let mut ids = Vec::with_capacity(sources.len());
        for (path, size) in sources {
            let id = next_id();
            register_outgoing(id, &path, size);
            if let Err(e) = drive_send(id, path, size, tx.clone()).await {
                mark_failed(id, format!("{e:#}"));
                let _ = tx.send(crate::runtime::ControlMsg::FileCancel { id }).await;
                return Err(e);
            }
            ids.push(id);
        }
        tx.send(crate::runtime::ControlMsg::ClipboardFiles(ids))
            .await
            .context("control channel closed before clipboard publication")
    })
}

pub fn received_clipboard_paths(ids: &[u64]) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(
        !ids.is_empty() && ids.len() <= MAX_CLIPBOARD_FILES,
        "clipboard file count out of range"
    );
    with_state(|transfers| {
        ids.iter()
            .map(|id| {
                let transfer = transfers
                    .get(id)
                    .context("unknown clipboard file transfer")?;
                anyhow::ensure!(
                    transfer.direction == Direction::Receiving && transfer.status == Status::Done,
                    "clipboard file is not a verified incoming transfer"
                );
                transfer
                    .final_path
                    .clone()
                    .context("clipboard file has no destination")
            })
            .collect()
    })
}

async fn drive_send(
    id: u64,
    source_path: PathBuf,
    size_bytes: u64,
    tx: ControlSender,
) -> Result<()> {
    use crate::runtime::ControlMsg as M;

    let name = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed")
        .to_string();

    // Stream chunks + compute sha256 in one pass.
    let mut file = File::open(&source_path)
        .await
        .with_context(|| format!("open source {}", source_path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut offset: u64 = 0;

    // Send the offer up front so the receiver has the file size
    // and can render the progress bar from byte 0.
    tx.send(M::FileOffer {
        id,
        name: name.clone(),
        size_bytes,
    })
    .await
    .map_err(|_| anyhow::anyhow!("control channel closed"))?;
    mark_active(id);

    loop {
        if is_cancelled(id) {
            let _ = tx.send(M::FileCancel { id }).await;
            bail!("cancelled by user");
        }
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let chunk = buf[..n].to_vec();
        anyhow::ensure!(
            offset + n as u64 <= size_bytes,
            "source file grew during transfer"
        );
        hasher.update(&chunk);
        tx.send(M::FileChunk {
            id,
            offset,
            data: chunk,
        })
        .await
        .map_err(|_| anyhow::anyhow!("control channel closed mid-transfer"))?;
        add_progress(id, n as u64);
        offset += n as u64;
    }
    let sha: [u8; 32] = hasher.finalize().into();
    anyhow::ensure!(offset == size_bytes, "source file shrank during transfer");
    anyhow::ensure!(!is_cancelled(id), "transfer cancelled before completion");
    tx.send(M::FileEnd { id, sha256: sha })
        .await
        .map_err(|_| anyhow::anyhow!("control channel closed at finalize"))?;
    mark_done(id);
    tracing::info!(id, name = %name, "outgoing file complete");
    Ok(())
}

fn validate_offer(name: &str, size_bytes: u64) -> Result<()> {
    anyhow::ensure!(!name.is_empty(), "incoming filename is empty");
    anyhow::ensure!(
        name.len() <= MAX_FILE_NAME_BYTES,
        "incoming filename exceeds {MAX_FILE_NAME_BYTES} bytes"
    );
    anyhow::ensure!(
        size_bytes <= MAX_INCOMING_FILE_BYTES,
        "incoming file exceeds {} byte safety limit",
        MAX_INCOMING_FILE_BYTES
    );
    Ok(())
}

fn validate_chunk(received: u64, declared: u64, offset: u64, len: usize) -> Result<()> {
    anyhow::ensure!(
        len <= CHUNK_BYTES,
        "incoming file chunk exceeds {CHUNK_BYTES} bytes"
    );
    anyhow::ensure!(offset == received, "non-sequential incoming file chunk");
    let end = offset
        .checked_add(len as u64)
        .context("incoming file chunk offset overflow")?;
    anyhow::ensure!(end <= declared, "incoming file chunk exceeds declared size");
    Ok(())
}

#[cfg(test)]
mod transfer_validation_tests {
    use super::*;

    #[test]
    fn accepts_sequential_chunks_within_declared_size() {
        assert!(validate_chunk(0, 64 * 1024, 0, CHUNK_BYTES).is_ok());
        assert!(
            validate_chunk(
                CHUNK_BYTES as u64,
                64 * 1024,
                CHUNK_BYTES as u64,
                CHUNK_BYTES
            )
            .is_ok()
        );
    }

    #[test]
    fn clipboard_never_publishes_unknown_or_outgoing_files() {
        assert!(received_clipboard_paths(&[]).is_err());
        assert!(received_clipboard_paths(&vec![1; MAX_CLIPBOARD_FILES + 1]).is_err());
        let id = next_id();
        assert!(received_clipboard_paths(&[id]).is_err());
        register_outgoing(id, Path::new("clipboard-test.png"), 4);
        mark_done(id);
        assert!(received_clipboard_paths(&[id]).is_err());
    }

    #[test]
    fn rejects_sparse_oversized_and_overflowing_chunks() {
        assert!(validate_chunk(0, 1_000_000, 500_000, 1).is_err());
        assert!(validate_chunk(0, 1_000_000, 0, CHUNK_BYTES + 1).is_err());
        assert!(validate_chunk(0, 10, 0, 11).is_err());
        assert!(validate_chunk(u64::MAX, u64::MAX, u64::MAX, 1).is_err());
    }

    #[test]
    fn rejects_unbounded_file_offers() {
        assert!(validate_offer("", 10).is_err());
        assert!(validate_offer("a", MAX_INCOMING_FILE_BYTES + 1).is_err());
        assert!(validate_offer(&"x".repeat(MAX_FILE_NAME_BYTES + 1), 10).is_err());
    }

    #[tokio::test]
    async fn downloads_never_overwrite_and_cancellation_is_terminal() {
        let dir =
            std::env::temp_dir().join(format!("mineshare-files-test-{}", uuid::Uuid::new_v4()));
        let a = next_id();
        let b = next_id();
        assert_ne!(a, b);
        assert!(a < 1 << 53 && b < 1 << 53, "UI IDs must round-trip exactly");
        begin_incoming_at(a, "same.txt", 3, &dir).await.unwrap();
        begin_incoming_at(b, "same.txt", 3, &dir).await.unwrap();
        assert_ne!(
            with_state(|m| m[&a].temp_path.clone()),
            with_state(|m| m[&b].temp_path.clone())
        );
        // Another program creates the destination after our offers.
        tokio::fs::write(dir.join("same.txt"), b"original")
            .await
            .unwrap();
        write_chunk(a, 0, b"one").await.unwrap();
        write_chunk(b, 0, b"two").await.unwrap();
        finalize_incoming(a, Sha256::digest(b"one").into())
            .await
            .unwrap();
        finalize_incoming(b, Sha256::digest(b"two").into())
            .await
            .unwrap();
        let pa = with_state(|m| m[&a].final_path.clone().unwrap());
        let pb = with_state(|m| m[&b].final_path.clone().unwrap());
        assert_eq!(
            received_clipboard_paths(&[b, a]).unwrap(),
            vec![pb.clone(), pa.clone()]
        );
        assert_ne!(pa, pb);
        assert_eq!(
            tokio::fs::read(dir.join("same.txt")).await.unwrap(),
            b"original"
        );
        assert_eq!(tokio::fs::read(pa).await.unwrap(), b"one");
        assert_eq!(tokio::fs::read(pb).await.unwrap(), b"two");
        mark_cancelled(a);
        mark_failed(a, "late error");
        assert_eq!(with_state(|m| m[&a].status), Status::Done);

        let c = next_id();
        begin_incoming_at(c, "cancelled.txt", 0, &dir)
            .await
            .unwrap();
        let temp = with_state(|m| m[&c].temp_path.clone().unwrap());
        mark_cancelled(c);
        assert!(!temp.exists());
        mark_active(c);
        mark_done(c);
        mark_failed(c, "late error");
        assert!(
            finalize_incoming(c, Sha256::digest([]).into())
                .await
                .is_err()
        );
        assert_eq!(with_state(|m| m[&c].status), Status::Cancelled);

        let d = next_id();
        register_outgoing(d, &dir.join("outgoing.txt"), 0);
        assert!(
            begin_incoming_at(d, "collision.txt", 0, &dir)
                .await
                .is_err()
        );
        fail_incoming(d, "ID collision");
        assert_eq!(with_state(|m| m[&d].status), Status::Pending);
        with_state(|m| {
            for id in [a, b, c, d] {
                m.remove(&id);
            }
        });
        tokio::fs::remove_dir_all(dir).await.unwrap();
    }

    #[test]
    fn names_cannot_address_windows_devices_or_alternate_streams() {
        assert_eq!(sanitize_name("report.txt:secret"), "report.txt_secret");
        assert_eq!(sanitize_name("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_name("LPT1"), "_LPT1");
        assert_eq!(sanitize_name(".. "), "unnamed");
        assert_eq!(sanitize_name("report\n?.txt"), "report__.txt");
    }
}
