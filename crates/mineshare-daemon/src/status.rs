//! Global runtime-status snapshot exposed to the GUI shell (M5).
//!
//! The runtime updates these atomics whenever it transitions —
//! peer connect/disconnect, packet counts ticking up, cursor-mode
//! flips. The Tauri backend reads them via [`snapshot`] on a poll
//! timer and surfaces the values to the React frontend so the
//! status pill in the header updates without any IPC plumbing of
//! its own.
//!
//! All atomics are `Relaxed` — these are display-only counters,
//! we don't need any happens-before guarantees with the rest of
//! the runtime's hot path.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;

static PEER_CONNECTED: AtomicBool = AtomicBool::new(false);
static SENT_PKTS: AtomicU64 = AtomicU64::new(0);
static RECV_PKTS: AtomicU64 = AtomicU64::new(0);
static INJECTED: AtomicU64 = AtomicU64::new(0);
static AUDIO_RECV: AtomicU64 = AtomicU64::new(0);
static INJECT_ERRS: AtomicU64 = AtomicU64::new(0);
static DECRYPT_ERRS: AtomicU64 = AtomicU64::new(0);

static PEER_ADDR: Mutex<Option<String>> = Mutex::new(None);
static PEER_NAME: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Clone, Default, Serialize)]
pub struct StatusSnapshot {
    pub peer_connected: bool,
    pub peer_addr: Option<String>,
    pub peer_name: Option<String>,
    pub sent_pkts: u64,
    pub recv_pkts: u64,
    pub injected: u64,
    pub audio_recv: u64,
    pub inject_errs: u64,
    pub decrypt_errs: u64,
    /// True when the local capture has taken Remote control of
    /// the peer (we're driving them).
    pub local_in_remote: bool,
    /// True when the peer is driving us — we're a passive
    /// receiver, our cursor is grabbed.
    pub peer_in_remote: bool,
    /// True when game-mode lock is active — edge crossing /
    /// auto-handover are paused; only the Ctrl+Alt+R hotkey can
    /// move between machines.
    pub input_locked: bool,
}

pub fn snapshot() -> StatusSnapshot {
    StatusSnapshot {
        peer_connected: PEER_CONNECTED.load(Ordering::Relaxed),
        peer_addr: PEER_ADDR.lock().clone(),
        peer_name: PEER_NAME.lock().clone(),
        sent_pkts: SENT_PKTS.load(Ordering::Relaxed),
        recv_pkts: RECV_PKTS.load(Ordering::Relaxed),
        injected: INJECTED.load(Ordering::Relaxed),
        audio_recv: AUDIO_RECV.load(Ordering::Relaxed),
        inject_errs: INJECT_ERRS.load(Ordering::Relaxed),
        decrypt_errs: DECRYPT_ERRS.load(Ordering::Relaxed),
        local_in_remote: mineshare_input::local_in_remote(),
        peer_in_remote: mineshare_input::peer_in_remote(),
        input_locked: mineshare_input::is_input_locked(),
    }
}

// --- runtime-side setters --------------------------------------------

pub(crate) fn set_peer_connected(addr: String, name: Option<String>) {
    *PEER_ADDR.lock() = Some(addr);
    *PEER_NAME.lock() = name;
    PEER_CONNECTED.store(true, Ordering::Relaxed);
}

pub(crate) fn clear_peer_connected() {
    PEER_CONNECTED.store(false, Ordering::Relaxed);
    *PEER_ADDR.lock() = None;
    *PEER_NAME.lock() = None;
}

pub(crate) fn add_sent_pkts(n: u64) {
    SENT_PKTS.fetch_add(n, Ordering::Relaxed);
}
pub(crate) fn add_recv_pkts(n: u64) {
    RECV_PKTS.fetch_add(n, Ordering::Relaxed);
}
pub(crate) fn add_injected(n: u64) {
    INJECTED.fetch_add(n, Ordering::Relaxed);
}
pub(crate) fn add_audio_recv(n: u64) {
    AUDIO_RECV.fetch_add(n, Ordering::Relaxed);
}
pub(crate) fn add_inject_errs(n: u64) {
    INJECT_ERRS.fetch_add(n, Ordering::Relaxed);
}
pub(crate) fn add_decrypt_errs(n: u64) {
    DECRYPT_ERRS.fetch_add(n, Ordering::Relaxed);
}
