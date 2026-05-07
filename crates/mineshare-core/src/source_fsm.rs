//! Auto-detect hardware-source FSM (Model B).
//!
//! Each daemon runs this. When local hardware input is observed, the daemon
//! claims the `hardware-source` role; if a peer claims first (and we are
//! currently `Idle`), we accept their claim.

use std::time::{Duration, Instant};

use crate::device::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    Idle,
    LocalSource,
    RemoteSource(DeviceId),
}

#[derive(Debug, Clone, Copy)]
pub enum SourceInput {
    LocalHwEvent,
    PeerClaim { device: DeviceId, ts_ns: u64 },
    PeerRelease { device: DeviceId },
    PeerDisconnect { device: DeviceId },
    Tick,
}

#[derive(Debug, Clone, Copy)]
pub enum SourceAction {
    None,
    BroadcastClaim { ts_ns: u64 },
    BroadcastRelease,
}

pub struct SourceFsm {
    pub local_id: DeviceId,
    state: SourceState,
    last_local_event_at: Option<Instant>,
    last_remote_event_at: Option<Instant>,
    /// hysteresis to avoid flapping during rapid alternation
    hysteresis: Duration,
    /// timeout to drop stale source role
    inactivity: Duration,
}

impl SourceFsm {
    pub fn new(local_id: DeviceId) -> Self {
        Self {
            local_id,
            state: SourceState::Idle,
            last_local_event_at: None,
            last_remote_event_at: None,
            hysteresis: Duration::from_millis(200),
            inactivity: Duration::from_secs(2),
        }
    }

    pub fn state(&self) -> SourceState {
        self.state
    }

    pub fn step(&mut self, input: SourceInput, now: Instant, ts_ns: u64) -> SourceAction {
        match input {
            SourceInput::LocalHwEvent => {
                self.last_local_event_at = Some(now);
                match self.state {
                    SourceState::LocalSource => SourceAction::None,
                    SourceState::Idle => {
                        self.state = SourceState::LocalSource;
                        SourceAction::BroadcastClaim { ts_ns }
                    }
                    SourceState::RemoteSource(_) => {
                        // only steal if remote has been quiet beyond hysteresis
                        if self
                            .last_remote_event_at
                            .map(|t| now.duration_since(t) > self.hysteresis)
                            .unwrap_or(true)
                        {
                            self.state = SourceState::LocalSource;
                            SourceAction::BroadcastClaim { ts_ns }
                        } else {
                            SourceAction::None
                        }
                    }
                }
            }
            SourceInput::PeerClaim {
                device,
                ts_ns: peer_ts,
            } => {
                self.last_remote_event_at = Some(now);
                match self.state {
                    SourceState::LocalSource => {
                        // tie-break: smaller ts wins; if equal, smaller hash wins
                        let local_ts = ts_ns;
                        let peer_wins = peer_ts < local_ts
                            || (peer_ts == local_ts && hash_id(&device) < hash_id(&self.local_id));
                        if peer_wins
                            && self
                                .last_local_event_at
                                .map(|t| now.duration_since(t) > self.hysteresis)
                                .unwrap_or(true)
                        {
                            self.state = SourceState::RemoteSource(device);
                            SourceAction::BroadcastRelease
                        } else {
                            SourceAction::None
                        }
                    }
                    SourceState::Idle => {
                        self.state = SourceState::RemoteSource(device);
                        SourceAction::None
                    }
                    SourceState::RemoteSource(_) => {
                        self.state = SourceState::RemoteSource(device);
                        SourceAction::None
                    }
                }
            }
            SourceInput::PeerRelease { device } => {
                if matches!(self.state, SourceState::RemoteSource(d) if d == device) {
                    self.state = SourceState::Idle;
                }
                SourceAction::None
            }
            SourceInput::PeerDisconnect { device } => {
                if matches!(self.state, SourceState::RemoteSource(d) if d == device) {
                    self.state = SourceState::Idle;
                }
                SourceAction::None
            }
            SourceInput::Tick => {
                // drop local source role if inactive
                if let SourceState::LocalSource = self.state {
                    let stale = self
                        .last_local_event_at
                        .map(|t| now.duration_since(t) > self.inactivity)
                        .unwrap_or(true);
                    if stale {
                        self.state = SourceState::Idle;
                        return SourceAction::BroadcastRelease;
                    }
                }
                SourceAction::None
            }
        }
    }
}

fn hash_id(id: &DeviceId) -> u128 {
    id.0.as_u128()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_event_claims_source_from_idle() {
        let me = DeviceId::new();
        let mut fsm = SourceFsm::new(me);
        let now = Instant::now();
        let action = fsm.step(SourceInput::LocalHwEvent, now, 100);
        assert_eq!(fsm.state(), SourceState::LocalSource);
        assert!(matches!(
            action,
            SourceAction::BroadcastClaim { ts_ns: 100 }
        ));
    }

    #[test]
    fn idle_accepts_peer_claim() {
        let me = DeviceId::new();
        let peer = DeviceId::new();
        let mut fsm = SourceFsm::new(me);
        let now = Instant::now();
        fsm.step(
            SourceInput::PeerClaim {
                device: peer,
                ts_ns: 10,
            },
            now,
            0,
        );
        assert_eq!(fsm.state(), SourceState::RemoteSource(peer));
    }

    #[test]
    fn earlier_ts_wins_tiebreak() {
        let me = DeviceId::new();
        let peer = DeviceId::new();
        let mut fsm = SourceFsm::new(me);
        let now = Instant::now();
        fsm.step(SourceInput::LocalHwEvent, now, 50);
        // hysteresis hasn't expired -> peer claim with later ts is rejected
        fsm.step(
            SourceInput::PeerClaim {
                device: peer,
                ts_ns: 60,
            },
            now,
            50,
        );
        assert_eq!(fsm.state(), SourceState::LocalSource);
    }
}
