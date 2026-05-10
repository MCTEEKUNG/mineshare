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
use std::sync::atomic::{AtomicU64, Ordering};
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
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn with_state<R>(f: impl FnOnce(&mut HashMap<u64, Transfer>) -> R) -> R {
    let mut guard = TRANSFERS.lock();
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
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
            if c == '/' || c == '\\' || c == '\0' || (c.is_control() && c != '\n') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "unnamed".to_string()
    } else {
        cleaned
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
    // Worst case: timestamp-suffix to guarantee uniqueness.
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("{stem}-{suffix}{ext}"))
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
        if let Some(t) = m.get_mut(&id) {
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
            t.status = Status::Failed;
            t.error = Some(why);
            // Drop temp file (best-effort cleanup).
            if let Some(tp) = t.temp_path.take() {
                let _ = std::fs::remove_file(tp);
            }
            t.incoming_file = None;
            t.incoming_sha = None;
        }
    });
}

pub fn mark_cancelled(id: u64) {
    with_state(|m| {
        if let Some(t) = m.get_mut(&id) {
            t.status = Status::Cancelled;
            if let Some(tp) = t.temp_path.take() {
                let _ = std::fs::remove_file(tp);
            }
            t.incoming_file = None;
            t.incoming_sha = None;
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
                final_path: t.final_path.as_ref().and_then(|p| p.to_str().map(|s| s.to_string())),
                error: t.error.clone(),
                seconds_elapsed: t.started_at.elapsed().as_secs_f32(),
            })
            .collect();
        // Newest first so the GUI shows the active transfer at top.
        v.sort_by(|a, b| b.id.cmp(&a.id));
        v
    })
}

// ----------------------------------------------------------------------------
// Receiving: called by the runtime reader on each ControlMsg::File*
// arrival.
// ----------------------------------------------------------------------------

/// Allocate the destination path and open the `.partial` file.
/// Idempotent — re-offering the same id is a no-op.
pub async fn begin_incoming(id: u64, name: &str, size_bytes: u64) -> Result<()> {
    if with_state(|m| m.contains_key(&id)) {
        return Ok(());
    }
    let dir = download_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create download dir {}", dir.display()))?;
    let safe_name = sanitize_name(name);
    let final_path = resolve_destination(&dir, &safe_name);
    let temp_path = final_path.with_file_name(format!(
        ".{}.partial",
        final_path.file_name().and_then(|s| s.to_str()).unwrap_or("transfer")
    ));
    let file = File::create(&temp_path)
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

/// Append a chunk at `offset`. Out-of-order chunks are tolerated
/// via seek (file_chunk variant carries explicit offset for
/// future support, even though current sender streams strictly
/// sequentially).
pub async fn write_chunk(id: u64, offset: u64, data: &[u8]) -> Result<()> {
    // Take ownership of the file handle + sha briefly so we can
    // do async IO without holding the parking_lot mutex across
    // an await.
    let (mut file, mut sha) = {
        let mut guard = TRANSFERS.lock();
        let m = guard.get_or_insert_with(HashMap::new);
        let t = m.get_mut(&id).context("unknown transfer id")?;
        if !matches!(t.status, Status::Active) {
            bail!("transfer {id} not active");
        }
        let file = t.incoming_file.take().context("transfer file handle gone")?;
        let sha = t.incoming_sha.take().context("transfer sha gone")?;
        (file, sha)
    };
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    file.write_all(data).await?;
    sha.update(data);
    let len = data.len() as u64;
    // Put them back.
    with_state(|m| {
        if let Some(t) = m.get_mut(&id) {
            t.incoming_file = Some(file);
            t.incoming_sha = Some(sha);
            t.bytes_so_far = t.bytes_so_far.saturating_add(len);
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
    tokio::fs::rename(&temp_path, &final_path)
        .await
        .with_context(|| format!("rename {} → {}", temp_path.display(), final_path.display()))?;
    mark_done(id);
    tracing::info!(id, path = %final_path.display(), "incoming file complete");
    Ok(())
}

// ----------------------------------------------------------------------------
// Per-session control-channel sender — set by `run_peer_session`
// so the Tauri command's send-file task can push ControlMsg
// variants without owning the broadcast handle directly.
// ----------------------------------------------------------------------------

pub type ControlSender =
    tokio::sync::mpsc::UnboundedSender<crate::runtime::ControlMsg>;

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
            .filter(|t| matches!(t.status, Status::Pending | Status::Active | Status::Verifying))
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
    let id = next_id();
    register_outgoing(id, &source_path, size_bytes);

    tokio::spawn(async move {
        if let Err(e) = drive_send(id, source_path, size_bytes, tx).await {
            mark_failed(id, format!("{e:#}"));
        }
    });
    Ok(id)
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
    .map_err(|_| anyhow::anyhow!("control channel closed"))?;
    mark_active(id);

    loop {
        if is_cancelled(id) {
            let _ = tx.send(M::FileCancel { id });
            bail!("cancelled by user");
        }
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let chunk = buf[..n].to_vec();
        hasher.update(&chunk);
        tx.send(M::FileChunk {
            id,
            offset,
            data: chunk,
        })
        .map_err(|_| anyhow::anyhow!("control channel closed mid-transfer"))?;
        add_progress(id, n as u64);
        offset += n as u64;
    }
    let sha: [u8; 32] = hasher.finalize().into();
    tx.send(M::FileEnd { id, sha256: sha })
        .map_err(|_| anyhow::anyhow!("control channel closed at finalize"))?;
    mark_done(id);
    tracing::info!(id, name = %name, "outgoing file complete");
    Ok(())
}
