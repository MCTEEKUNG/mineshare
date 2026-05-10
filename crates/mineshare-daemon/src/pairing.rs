//! In-memory pairing-state machine surfaced to the GUI.
//!
//! When a peer hits our daemon for the first time (or after their
//! Noise static key rotated), the runtime puts the local
//! `PairingPhase` into `AwaitingPin` (we're the dialer; user has
//! to type the PIN they read off the peer's screen) or
//! `DisplayingPin` (we accepted; user reads the PIN to the
//! human at the other end).
//!
//! The Tauri front-end polls `current_phase()` once a second; the
//! Status tab pops up a pairing card matching the phase. The
//! human enters the PIN, GUI calls `submit_pin`, the runtime
//! drains the value via `take_submitted_pin`, sends it on the
//! encrypted control channel, and we either advance to the
//! trusted state or surface `Failed`.

use parking_lot::Mutex;
use rand::Rng;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PairingPhase {
    /// No pairing in progress (peer is trusted or no session).
    None,
    /// We're the dialer — the user needs to read the PIN off the
    /// other machine's screen and type it on this one.
    AwaitingPin {
        peer_name: String,
        peer_addr: String,
    },
    /// We're the acceptor — show the PIN so the human can read
    /// it to whoever's typing on the dialer side.
    DisplayingPin {
        pin: String,
        peer_name: String,
        peer_addr: String,
    },
    /// PIN sent / received; waiting for the verification
    /// round-trip.
    Verifying,
    /// Successful — peer is now in the trust list. The card
    /// stays up briefly so the user gets visible feedback before
    /// the regular Status view returns.
    Trusted {
        peer_name: String,
    },
    /// Mismatch / timeout / network error.
    Failed {
        reason: String,
    },
}

static PHASE: Mutex<PairingPhase> = Mutex::new(PairingPhase::None);
static SUBMITTED_PIN: Mutex<Option<String>> = Mutex::new(None);

pub fn current_phase() -> PairingPhase {
    let p = PHASE.lock().clone();
    tracing::trace!(phase = ?p, "GUI polled current_phase");
    p
}

pub fn set_phase(p: PairingPhase) {
    tracing::info!(phase = ?p, "pairing phase changed");
    *PHASE.lock() = p;
}

/// Generate a random 6-digit PIN, formatted with leading zeros.
pub fn generate_pin() -> String {
    let n: u32 = rand::rng().random_range(0..1_000_000);
    format!("{n:06}")
}

/// Front-end calls this from the Tauri command when the user
/// clicks "Pair" with a typed-in PIN.
pub fn submit_pin(pin: String) {
    *SUBMITTED_PIN.lock() = Some(pin);
}

/// Daemon side drains the queued PIN once the user has clicked
/// pair. Returns `None` if nothing is queued yet — call from a
/// short-interval poll.
pub fn take_submitted_pin() -> Option<String> {
    SUBMITTED_PIN.lock().take()
}
