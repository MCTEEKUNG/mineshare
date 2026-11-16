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

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use rand::Rng;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PairingPhase {
    /// No pairing in progress (peer is trusted or no session).
    None,
    IncomingRequest {
        peer_name: String,
        peer_addr: String,
    },
    WaitingApproval {
        peer_name: String,
        peer_addr: String,
    },
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
    Trusted { peer_name: String },
    /// Mismatch / timeout / network error.
    Failed { reason: String },
}

struct State {
    phase: PairingPhase,
    attempt_id: u64,
    submitted_pin: Option<String>,
    response: Option<bool>,
    responded: bool,
    dismiss_at: Option<Instant>,
}
static STATE: Mutex<State> = Mutex::new(State {
    phase: PairingPhase::None,
    attempt_id: 0,
    submitted_pin: None,
    response: None,
    responded: false,
    dismiss_at: None,
});

#[derive(Clone, Serialize)]
pub struct PairingSnapshot {
    pub attempt_id: u64,
    #[serde(flatten)]
    pub phase: PairingPhase,
}

pub fn snapshot() -> PairingSnapshot {
    let mut state = STATE.lock();
    if state.dismiss_at.is_some_and(|at| at <= Instant::now()) {
        state.phase = PairingPhase::None;
        state.dismiss_at = None;
    }
    PairingSnapshot {
        attempt_id: state.attempt_id,
        phase: state.phase.clone(),
    }
}

pub fn current_phase() -> PairingPhase {
    snapshot().phase
}

pub fn set_phase(p: PairingPhase) {
    let mut state = STATE.lock();
    state.dismiss_at = matches!(
        p,
        PairingPhase::Trusted { .. } | PairingPhase::Failed { .. }
    )
    .then(|| Instant::now() + Duration::from_secs(3));
    state.phase = p;
}

/// Scoped to the runtime's exclusive session lifetime. Cancellation drops this
/// guard before a replacement can display its PIN or accept a submitted value.
pub(crate) struct Attempt;
impl Attempt {
    pub(crate) fn begin() -> Self {
        let mut state = STATE.lock();
        state.attempt_id += 1;
        state.phase = PairingPhase::None;
        state.submitted_pin = None;
        state.response = None;
        state.responded = false;
        state.dismiss_at = None;
        Self
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        let mut state = STATE.lock();
        state.submitted_pin = None;
        state.response = None;
        if matches!(
            state.phase,
            PairingPhase::AwaitingPin { .. }
                | PairingPhase::DisplayingPin { .. }
                | PairingPhase::Verifying
                | PairingPhase::IncomingRequest { .. }
                | PairingPhase::WaitingApproval { .. }
        ) {
            state.phase = PairingPhase::None;
        }
    }
}

/// Generate a random 6-digit PIN, formatted with leading zeros.
pub fn generate_pin() -> String {
    let n: u32 = rand::rng().random_range(0..1_000_000);
    format!("{n:06}")
}

/// Front-end calls this from the Tauri command when the user
/// clicks "Pair" with a typed-in PIN.
pub fn submit_pin(pin: String, attempt_id: u64) -> Result<(), String> {
    let mut state = STATE.lock();
    if state.attempt_id != attempt_id || !matches!(state.phase, PairingPhase::AwaitingPin { .. }) {
        return Err("this pairing attempt is no longer awaiting a PIN".into());
    }
    if pin.len() != 6 || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err("PIN must contain six digits".into());
    }
    state.submitted_pin = Some(pin);
    Ok(())
}

/// Daemon side drains the queued PIN once the user has clicked
/// pair. Returns `None` if nothing is queued yet — call from a
/// short-interval poll.
pub fn take_submitted_pin() -> Option<String> {
    STATE.lock().submitted_pin.take()
}

pub fn respond_to_request(accept: bool, attempt_id: u64) -> Result<(), String> {
    let mut state = STATE.lock();
    if state.attempt_id != attempt_id
        || !matches!(state.phase, PairingPhase::IncomingRequest { .. })
        || state.responded
    {
        return Err("this pairing request is no longer awaiting approval".into());
    }
    state.response = Some(accept);
    state.responded = true;
    if !accept {
        state.phase = PairingPhase::None;
    }
    Ok(())
}
pub(crate) fn take_response() -> Option<bool> {
    STATE.lock().response.take()
}

#[cfg(test)]
pub(crate) static TEST_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_attempt_clears_ui_and_queued_pin_and_rejects_stale_submissions() {
        let _serial = TEST_GUARD.blocking_lock();
        let first = Attempt::begin();
        set_phase(PairingPhase::AwaitingPin {
            peer_name: "A".into(),
            peer_addr: "A".into(),
        });
        let old_id = snapshot().attempt_id;
        assert!(submit_pin("12345x".into(), old_id).is_err());
        submit_pin("123456".into(), old_id).unwrap();
        drop(first);
        assert!(matches!(current_phase(), PairingPhase::None));
        let third = Attempt::begin();
        set_phase(PairingPhase::IncomingRequest {
            peer_name: "C".into(),
            peer_addr: "C".into(),
        });
        assert!(respond_to_request(true, old_id).is_err());
        respond_to_request(true, snapshot().attempt_id).unwrap();
        assert_eq!(take_response(), Some(true));
        assert!(respond_to_request(false, snapshot().attempt_id).is_err());
        drop(third);
        assert!(take_response().is_none());
        assert!(take_submitted_pin().is_none());
        let second = Attempt::begin();
        set_phase(PairingPhase::AwaitingPin {
            peer_name: "B".into(),
            peer_addr: "B".into(),
        });
        assert!(submit_pin("123456".into(), old_id).is_err());
        assert!(take_submitted_pin().is_none());
        set_phase(PairingPhase::Failed {
            reason: "peer disconnected".into(),
        });
        drop(second);
        STATE.lock().dismiss_at = Some(Instant::now());
        assert!(matches!(current_phase(), PairingPhase::None));
    }
}
