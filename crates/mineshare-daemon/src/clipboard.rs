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

/// Spawn the clipboard watcher thread (idempotent). Only the first
/// call actually starts the OS-side thread; subsequent calls are
/// no-ops, so it's safe to invoke this from every peer session.
pub fn ensure_watcher(tx: UnboundedSender<String>) {
    if WATCHER_RUNNING.swap(true, Ordering::AcqRel) {
        // Already running. Replace the active sender so the new
        // peer session receives change notifications instead of the
        // dropped one.
        let _ = REPLACE_TX_GUARD.lock().replace(tx);
        return;
    }
    *REPLACE_TX_GUARD.lock() = Some(tx);

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
                thread::sleep(Duration::from_millis(POLL_MS));
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

/// Apply a clipboard payload that arrived from the peer. Updates
/// `LAST_TEXT` first so the watcher's next poll sees the value as
/// "already known" and doesn't echo it back.
pub fn apply_from_peer(text: &str) -> Result<()> {
    *LAST_TEXT.lock() = text.to_string();
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(text.to_string())?;
    info!(len = text.len(), "applied peer clipboard text");
    Ok(())
}

/// Holds the currently-active mpsc sender so the watcher can hand
/// off new sessions without restarting the OS thread.
static REPLACE_TX_GUARD: Mutex<Option<UnboundedSender<String>>> = Mutex::new(None);
