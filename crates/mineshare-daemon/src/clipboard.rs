//! Cross-platform clipboard text sync.
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use parking_lot::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};

const POLL_MS: u64 = 400;
/// Cap forwarded clipboard size — pasting a 50 MB log into the
/// clipboard shouldn't accidentally flood the control channel.
const MAX_BYTES: usize = 64 * 1024;

static LAST_TEXT: Mutex<String> = Mutex::new(String::new());
static WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);

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
static APPLY_TX: Mutex<Option<std::sync::mpsc::Sender<String>>> = Mutex::new(None);

/// Spawn the clipboard watcher thread (idempotent). Only the first
/// call actually starts the OS-side thread; subsequent calls are
/// no-ops, so it's safe to invoke this from every peer session.
pub fn ensure_watcher(tx: UnboundedSender<String>) {
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
    let (apply_tx, apply_rx) = std::sync::mpsc::channel::<String>();
    *APPLY_TX.lock() = Some(apply_tx);

    thread::Builder::new()
        .name("clipboard-watcher".into())
        .spawn(move || {
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
                    Ok(text) => {
                        // Update LAST_TEXT first so our own next poll
                        // sees this value as already-known and doesn't
                        // echo it back to the peer.
                        *LAST_TEXT.lock() = text.clone();
                        if let Err(e) = clipboard.set_text(text) {
                            debug!(error = %e, "failed to apply peer clipboard text");
                        } else {
                            debug!("applied peer clipboard text (on watcher thread)");
                        }
                        // Drain any further queued payloads cheaply on
                        // the next loop iterations; go poll afterwards.
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
                let current = match clipboard.get_text() {
                    Ok(t) => t,
                    Err(arboard::Error::ContentNotAvailable) => continue,
                    Err(e) => {
                        // Transient errors (e.g. focus changes on
                        // Wayland) shouldn't kill the watcher.
                        debug!(error = %e, "clipboard poll error — continuing");
                        continue;
                    }
                };
                if current.len() > MAX_BYTES {
                    debug!(
                        bytes = current.len(),
                        cap = MAX_BYTES,
                        "skipping oversized clipboard"
                    );
                    continue;
                }
                let mut last = LAST_TEXT.lock();
                if *last == current {
                    continue;
                }
                *last = current.clone();
                drop(last);

                let guard = REPLACE_TX_GUARD.lock();
                if let Some(tx) = guard.as_ref()
                    && tx.send(current.clone()).is_err()
                {
                    debug!("clipboard sink closed — waiting for next session");
                }
            }
        })
        .expect("spawn clipboard-watcher thread");
}

/// Apply a clipboard payload that arrived from the peer.
///
/// This does **not** touch `arboard` directly — doing so from the
/// tokio reader thread is what raced the watcher thread's OLE
/// clipboard access and corrupted the heap inside the GUI process
/// (see `APPLY_TX`). Instead we hand the text to the single
/// clipboard-owning watcher thread, which sets it (and updates
/// `LAST_TEXT` for the echo guard) on its own thread.
pub fn apply_from_peer(text: &str) -> Result<()> {
    if std::env::var_os("MINESHARE_NO_CLIPBOARD").is_some() {
        return Ok(());
    }
    let guard = APPLY_TX.lock();
    match guard.as_ref() {
        Some(tx) => {
            // Unbounded std channel; send only fails if the watcher
            // thread is gone, which doesn't happen in practice.
            let _ = tx.send(text.to_string());
            debug!(len = text.len(), "queued peer clipboard text for watcher thread");
        }
        None => {
            // Watcher never started (init failed or clipboard disabled)
            // — drop silently; clipboard sync is best-effort.
            debug!("clipboard apply skipped — watcher not running");
        }
    }
    Ok(())
}

/// Holds the currently-active mpsc sender so the watcher can hand
/// off new sessions without restarting the OS thread.
static REPLACE_TX_GUARD: Mutex<Option<UnboundedSender<String>>> = Mutex::new(None);
