//! One owner for input/audio routing; new connections require a targeted request.
use anyhow::{Result, bail, ensure};
use parking_lot::Mutex;
use std::sync::Arc;

use std::time::{Duration, Instant};
use tokio::sync::watch;

pub(crate) struct Sessions {
    state: Mutex<State>,
}
struct State {
    active: Vec<Active>,
    paused: bool,
    outgoing: Option<OutgoingRequest>,
    next_request: u64,
    lifetimes: Vec<(String, Arc<tokio::sync::Mutex<()>>)>,
}
#[derive(Clone)]
pub(crate) struct OutgoingRequest {
    pub id: u64,
    pub peer_id: String,
    until: Instant,
}
#[derive(Clone, serde::Serialize)]
pub struct ConnectionControl {
    pub paused: bool,
    pub pairing_enabled: bool,
    pub requested_peer_id: Option<String>,
    pub connected_peer_id: Option<String>,
    pub connected_peer_ids: Vec<String>,
    pub active_input_peer_id: Option<String>,
    /// Display-only metadata for the saved automatic reconnection target.
    pub selected_peer: Option<SelectedPeer>,
}
#[derive(Clone, serde::Serialize)]
pub struct SelectedPeer {
    pub device_id: String,
    pub display_name: String,
    pub os: Option<String>,
}
struct Active {
    peer: Vec<u8>,
    peer_id: String,
    preferred: bool,
    token: Arc<()>,
    cancel: watch::Sender<bool>,
    pairing: bool,
}

impl Sessions {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(State {
                active: Vec::new(),
                paused: false,
                outgoing: None,
                next_request: 0,
                lifetimes: Vec::new(),
            }),
        }
    }
    pub(crate) fn control(&self) -> ConnectionControl {
        let state = self.state.lock();
        let outgoing = state.outgoing.as_ref().filter(|r| r.until > Instant::now());
        ConnectionControl {
            paused: state.paused,
            pairing_enabled: outgoing.is_some()
                || state
                    .active
                    .iter()
                    .any(|a| a.pairing && !*a.cancel.borrow()),
            requested_peer_id: outgoing.map(|r| r.peer_id.clone()),
            connected_peer_id: state
                .active
                .iter()
                .find(|a| !a.pairing && !*a.cancel.borrow())
                .map(|a| a.peer_id.clone()),
            connected_peer_ids: state
                .active
                .iter()
                .filter(|a| !a.pairing && !*a.cancel.borrow())
                .map(|a| a.peer_id.clone())
                .collect(),
            active_input_peer_id: None,
            selected_peer: None,
        }
    }
    pub(crate) fn set_paused(&self, paused: bool) {
        let mut state = self.state.lock();
        state.paused = paused;
        if paused {
            state.outgoing = None;
            for active in &state.active {
                active.cancel.send_replace(true);
            }
        } else {
            crate::runtime::kick_reconnect();
        }
    }
    /// The old blanket admission switch is intentionally no longer available.
    pub(crate) fn set_pairing_enabled(&self, enabled: bool) -> Result<()> {
        ensure!(
            !enabled,
            "choose a computer before sending a pairing request"
        );
        let mut state = self.state.lock();
        state.outgoing = None;
        for active in state.active.iter().filter(|a| a.pairing) {
            active.cancel.send_replace(true);
        }
        Ok(())
    }
    pub(crate) fn request(&self, peer_id: &str) -> Result<OutgoingRequest> {
        let mut state = self.state.lock();
        ensure!(!state.paused, "resume connections before sending a request");
        ensure!(
            state
                .outgoing
                .as_ref()
                .is_none_or(|r| r.until <= Instant::now()),
            "a pairing request is already pending"
        );
        ensure!(
            !state.active.iter().any(|a| a.pairing),
            "respond to the current pairing request first"
        );
        state.next_request += 1;
        let request = OutgoingRequest {
            id: state.next_request,
            peer_id: peer_id.into(),
            until: Instant::now() + Duration::from_secs(90),
        };
        state.outgoing = Some(request.clone());
        crate::runtime::kick_reconnect();
        Ok(request)
    }
    pub(crate) fn outgoing_for(&self, peer_id: &str) -> Option<OutgoingRequest> {
        let state = self.state.lock();
        state
            .outgoing
            .as_ref()
            .filter(|r| !state.paused && r.peer_id == peer_id && r.until > Instant::now())
            .cloned()
    }
    pub(crate) fn request_is_current(&self, request: &OutgoingRequest) -> bool {
        self.outgoing_for(&request.peer_id)
            .is_some_and(|r| r.id == request.id)
    }
    pub(crate) fn finish_request(&self, id: u64) -> bool {
        let mut state = self.state.lock();
        if let Some(request) = state.outgoing.as_ref()
            && request.id == id
            && state.active.iter().any(|active| {
                active.pairing && active.peer_id == request.peer_id && !*active.cancel.borrow()
            })
        {
            return false;
        }
        if state.outgoing.as_ref().is_some_and(|r| r.id == id) {
            state.outgoing = None;
            true
        } else {
            false
        }
    }
    pub(crate) fn can_dial(&self, peer_id: &str, selected_trusted: bool) -> bool {
        let state = self.state.lock();
        if state.paused || state.active.iter().any(|a| a.peer_id == peer_id) {
            return false;
        }
        match state.outgoing.as_ref().filter(|r| r.until > Instant::now()) {
            Some(request) => request.peer_id == peer_id || selected_trusted,
            None => selected_trusted,
        }
    }
    pub(crate) fn claim(
        &self,
        peer: &[u8],
        peer_id: &str,
        preferred: bool,
        trusted: bool,
        pairing: bool,
        outgoing: Option<&OutgoingRequest>,
    ) -> Result<Claim<'_>> {
        let mut state = self.state.lock();
        ensure!(!state.paused, "connections paused");
        ensure!(
            trusted || pairing,
            "new connections require a pairing request"
        );
        if let Some(request) = outgoing {
            ensure!(
                state
                    .outgoing
                    .as_ref()
                    .is_some_and(|current| current.id == request.id
                        && current.peer_id == peer_id
                        && current.until > Instant::now()),
                "pairing request was cancelled or expired"
            );
        }
        if let Some(request) = state.outgoing.as_ref().filter(|r| r.until > Instant::now())
            && pairing
        {
            ensure!(
                request.peer_id == peer_id,
                "a request to another computer is pending"
            );
        }
        ensure!(
            !pairing
                || !state
                    .active
                    .iter()
                    .any(|a| a.pairing && a.peer_id != peer_id),
            "another pairing request is pending"
        );
        if let Some(old) = state.active.iter().find(|a| a.peer_id == peer_id) {
            ensure!(!*old.cancel.borrow(), "previous session is stopping");
            if old.peer != peer || old.peer_id != peer_id || old.preferred || !preferred {
                bail!("another connection owns the bridge");
            }
            old.cancel.send_replace(true);
        }
        let (cancel, cancelled) = watch::channel(false);
        let token = Arc::new(());
        state.active.retain(|a| a.peer_id != peer_id);
        state.active.push(Active {
            peer: peer.to_vec(),
            peer_id: peer_id.into(),
            preferred,
            token: token.clone(),
            cancel,
            pairing,
        });
        Ok(Claim {
            sessions: self,
            token,
            cancelled,
        })
    }
}
pub(crate) struct Claim<'a> {
    sessions: &'a Sessions,
    token: Arc<()>,
    cancelled: watch::Receiver<bool>,
}
impl Claim<'_> {
    pub(crate) fn is_current(&self) -> bool {
        self.sessions
            .state
            .lock()
            .active
            .iter()
            .any(|active| Arc::ptr_eq(&active.token, &self.token) && !*active.cancel.borrow())
    }
    pub(crate) fn mark_trusted(&self) {
        let mut state = self.sessions.state.lock();
        if let Some(active) = state
            .active
            .iter_mut()
            .find(|a| Arc::ptr_eq(&a.token, &self.token))
        {
            active.pairing = false;
            state.outgoing = None;
        }
    }
    pub(crate) async fn cancelled(&mut self) {
        if !*self.cancelled.borrow() {
            let _ = self.cancelled.changed().await;
        }
    }
}
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.sessions
            .state
            .lock()
            .active
            .retain(|a| !Arc::ptr_eq(&a.token, &self.token));
    }
}
impl Sessions {
    pub(crate) fn cancel_peer(&self, peer_id: &str) {
        let mut state = self.state.lock();
        for active in state.active.iter().filter(|a| a.peer_id == peer_id) {
            active.cancel.send_replace(true);
        }
        if state
            .outgoing
            .as_ref()
            .is_some_and(|r| r.peer_id == peer_id)
        {
            state.outgoing = None;
        }
    }
    pub(crate) fn lifetime(&self, peer_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut state = self.state.lock();
        if let Some((_, lifetime)) = state.lifetimes.iter().find(|(id, _)| id == peer_id) {
            return lifetime.clone();
        }
        let lifetime = Arc::new(tokio::sync::Mutex::new(()));
        state.lifetimes.push((peer_id.into(), lifetime.clone()));
        lifetime
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_selected_device_can_dial_and_bystanders_cannot_steal_the_request() {
        let s = Sessions::new();
        assert!(!s.can_dial("nearby", false));
        assert!(s.set_pairing_enabled(true).is_err());
        let request = s.request("chosen").unwrap();
        assert!(s.can_dial("chosen", false));
        assert!(s.can_dial("bystander", true));
        assert!(s.claim(b"third", "third", true, true, true, None).is_err());
        let owner = s
            .claim(b"chosen", "chosen", true, false, true, None)
            .unwrap();
        assert!(s.claim(b"third", "third", true, true, true, None).is_err());
        assert!(s.request("another").is_err());
        owner.mark_trusted();
        assert!(!s.control().pairing_enabled);
        assert!(!s.request_is_current(&request));
    }
    #[tokio::test]
    async fn pause_and_cancel_stop_pending_requests_but_cancelling_does_not_drop_a_connected_peer()
    {
        let s = Sessions::new();
        let request = s.request("peer").unwrap();
        let mut owner = s.claim(b"peer", "peer", true, false, true, None).unwrap();
        s.set_pairing_enabled(false).unwrap();
        tokio::time::timeout(Duration::from_millis(50), owner.cancelled())
            .await
            .unwrap();
        assert!(!s.request_is_current(&request));
        drop(owner);
        let mut trusted = s.claim(b"peer", "peer", true, true, false, None).unwrap();
        s.set_pairing_enabled(false).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), trusted.cancelled())
                .await
                .is_err()
        );
        s.set_paused(true);
        tokio::time::timeout(Duration::from_millis(50), trusted.cancelled())
            .await
            .unwrap();
        assert!(s.request("new").is_err());
        assert!(s.claim(b"peer", "peer", true, true, false, None).is_err());
        s.set_paused(false);
        assert!(s.claim(b"peer", "peer", true, true, false, None).is_err());
        drop(trusted);
        assert!(s.can_dial("peer", true));
    }
    #[test]
    fn expired_or_cancelled_attempt_cannot_finish_a_new_request_to_the_same_device() {
        let s = Sessions::new();
        let old = s.request("peer").unwrap();
        s.state.lock().outgoing.as_mut().unwrap().until = Instant::now() - Duration::from_secs(1);
        assert!(!s.request_is_current(&old));
        let new = s.request("peer").unwrap();
        assert!(!s.finish_request(old.id));
        assert!(s.request_is_current(&new));
        assert!(
            s.claim(b"peer", "peer", true, false, true, Some(&old))
                .is_err()
        );
        assert!(
            s.claim(b"peer", "peer", true, false, true, Some(&new))
                .is_ok()
        );
    }
    #[test]
    fn incoming_pairing_is_serialized_while_trusted_peers_continue() {
        let s = Sessions::new();
        let pending = s.claim(b"a", "a", true, false, true, None).unwrap();
        assert!(s.claim(b"b", "b", true, false, true, None).is_err());
        let connected = s.claim(b"c", "c", true, true, false, None).unwrap();
        assert!(connected.is_current());
        pending.mark_trusted();
        assert!(s.claim(b"b", "b", true, false, true, None).is_ok());
    }
    #[tokio::test]
    async fn three_peers_coexist_and_replacing_one_cannot_remove_another() {
        let s = Sessions::new();
        let one = s.claim(b"a", "a", true, true, false, None).unwrap();
        let two = s.claim(b"b", "b", false, true, false, None).unwrap();
        let three = s.claim(b"c", "c", true, true, false, None).unwrap();
        assert_eq!(s.control().connected_peer_ids.len(), 3);
        let replacement = s.claim(b"b", "b", true, true, false, None).unwrap();
        assert!(!two.is_current());
        drop(two);
        assert!(one.is_current() && three.is_current() && replacement.is_current());
        s.request("d").unwrap();
        assert!(one.is_current() && three.is_current() && replacement.is_current());
        drop(one);
        assert_eq!(s.control().connected_peer_ids.len(), 2);
    }
}
