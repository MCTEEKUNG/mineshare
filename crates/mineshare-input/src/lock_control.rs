//! A lock belongs to the controlled desktop, not the keyboard's USB host.
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockRequest {
    pub epoch: u64,
    pub request: u64,
    pub locked: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockReply {
    pub epoch: u64,
    pub request: Option<u64>,
    pub locked: bool,
}

#[derive(Default)]
struct Route {
    epoch: u64,
    serial: u64,
    pending: Option<u64>,
    peer_epoch: Option<u64>,
    remote_locked: bool,
}
impl Route {
    fn begin(&mut self) -> u64 {
        self.epoch = self.epoch.wrapping_add(1);
        self.pending = None;
        self.remote_locked = false;
        self.epoch
    }
    fn request(&mut self) -> LockRequest {
        self.serial = self.serial.wrapping_add(1);
        self.pending = Some(self.serial);
        self.remote_locked = !self.remote_locked;
        LockRequest {
            epoch: self.epoch,
            request: self.serial,
            locked: self.remote_locked,
        }
    }
    fn reply(&mut self, reply: LockReply) -> bool {
        // Request zero is the initial handoff snapshot, not a user command.
        let stale = match reply.request {
            Some(0) => self.pending.is_some(),
            Some(id) => self.pending != Some(id),
            None => false,
        };
        if reply.epoch != self.epoch || stale {
            return false;
        }
        self.pending = None;
        self.remote_locked = reply.locked;
        true
    }
}
static ROUTE: Mutex<Route> = Mutex::new(Route {
    epoch: 0,
    serial: 0,
    pending: None,
    peer_epoch: None,
    remote_locked: false,
});
static REMOTE_LOCKED: AtomicBool = AtomicBool::new(false);

pub fn remote_input_locked() -> bool {
    REMOTE_LOCKED.load(Ordering::Acquire)
}

pub(crate) fn begin_local_control() -> u64 {
    let epoch = ROUTE.lock().begin();
    REMOTE_LOCKED.store(false, Ordering::Release);
    epoch
}

/// Publish ownership and its control message as one critical section. A keyboard
/// hook must not enqueue a lock request ahead of the mouse's TakeControl.
pub(crate) fn publish_local_control(game_drive: bool, publish: impl FnOnce()) {
    let mut route = ROUTE.lock();
    let epoch = route.begin();
    REMOTE_LOCKED.store(false, Ordering::Release);
    publish();
    super::fire_remote_event(if game_drive {
        super::RemoteEvent::GameDriveStart { epoch }
    } else {
        super::RemoteEvent::Entered { epoch }
    });
}
pub(crate) fn end_local_control() {
    begin_local_control(); // invalidate replies from the previous handoff
}

pub fn note_peer_control(epoch: u64) {
    let mut route = ROUTE.lock();
    route.peer_epoch = Some(epoch);
    // A desktop already manually locked stays locked when a peer enters it.
    super::fire_remote_event(super::RemoteEvent::InputLockReply(LockReply {
        epoch,
        request: Some(0),
        locked: super::is_input_locked(),
    }));
}

pub fn end_peer_control() {
    let mut route = ROUTE.lock();
    let owned = route.peer_epoch.take().is_some();
    if owned {
        publish_local(false, false, &route);
    }
}

/// Explicit GUI/tray actions address this desktop. Hotkeys use the focus router.
pub fn set_input_locked(locked: bool) {
    apply_local(locked, true);
}

fn apply_local(locked: bool, notify: bool) {
    if locked && super::local_in_remote() {
        super::force_local_exit_remote();
    }
    publish_local(locked, notify, &ROUTE.lock());
}

fn publish_local(locked: bool, notify: bool, route: &Route) {
    let previous = super::INPUT_LOCKED.swap(locked, Ordering::AcqRel);
    if previous != locked {
        tracing::info!(locked, "input lock toggled on controlled desktop");
        #[cfg(target_os = "windows")]
        super::windows::lock_feedback::show(locked);
    }
    if notify {
        let epoch = route.peer_epoch;
        if let Some(epoch) = epoch {
            super::fire_remote_event(super::RemoteEvent::InputLockReply(LockReply {
                epoch,
                request: None,
                locked,
            }));
        }
    }
}

pub fn toggle_focused_input_lock() {
    // Do not let a forced keyboard-only target move the mouse lock elsewhere.
    let to_peer = super::peer_session_ready()
        && (super::is_game_driving()
            || super::cursor_focus_is_peer(super::local_in_remote(), super::peer_in_remote())
                .unwrap_or(false));
    if to_peer {
        let request = {
            let mut route = ROUTE.lock();
            let request = route.request();
            REMOTE_LOCKED.store(request.locked, Ordering::Release);
            request
        };
        tracing::info!(
            epoch = request.epoch,
            request = request.request,
            locked = request.locked,
            "requesting lock on peer desktop"
        );
        super::fire_remote_event(super::RemoteEvent::InputLockRequest(request));
        // Teardown may have raced the hotkey; never pin a disconnected cursor.
        if !super::peer_session_ready() {
            reset();
        }
    } else {
        set_input_locked(!super::is_input_locked());
    }
}

pub fn receive_lock_request(request: LockRequest) -> LockReply {
    let route = ROUTE.lock();
    let current = route.peer_epoch == Some(request.epoch)
        && request.request != 0
        && !super::local_in_remote()
        && (super::peer_in_remote() || super::is_game_receiving());
    if current {
        publish_local(request.locked, false, &route);
    } else {
        tracing::debug!(
            epoch = request.epoch,
            "ignored lock request for retired cursor owner"
        );
    }
    LockReply {
        epoch: request.epoch,
        request: Some(request.request),
        locked: current && super::is_input_locked(),
    }
}

pub(crate) fn reclaim_from_peer_hardware() -> bool {
    let mut route = ROUTE.lock();
    if super::is_input_locked() {
        return false;
    }
    let changed = super::PEER_IN_REMOTE
        .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
        .is_ok();
    if changed {
        route.peer_epoch = None;
    }
    changed
}

pub fn receive_lock_reply(reply: LockReply) {
    let mut route = ROUTE.lock();
    if route.reply(reply) {
        REMOTE_LOCKED.store(route.remote_locked, Ordering::Release);
    }
}

pub fn reset() {
    {
        let mut route = ROUTE.lock();
        route.begin();
        route.peer_epoch = None;
    }
    REMOTE_LOCKED.store(false, Ordering::Release);
    apply_local(false, false);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_and_keyboard_share_cursor_owner_and_locked_edges_never_exit() {
        assert_eq!(crate::cursor_focus_is_peer(true, false), Some(true));
        assert_eq!(crate::cursor_focus_is_peer(false, true), Some(false));
        assert_eq!(crate::cursor_focus_is_peer(true, true), Some(true));
        assert_eq!(crate::cursor_focus_is_peer(false, false), None);
        for depth in [-10000, -100, -1, 0, 1, 3000] {
            assert!(crate::clamp_remote_depth(depth, 1919, 100, true) >= 0);
        }
        assert_eq!(crate::clamp_remote_depth(-101, 1919, 100, false), -100);
    }
    #[test]
    fn initial_snapshot_cannot_cancel_a_new_hotkey() {
        let mut route = Route::default();
        route.begin();
        let request = route.request();
        assert!(!route.reply(LockReply {
            epoch: request.epoch,
            request: Some(0),
            locked: false
        }));
        assert!(route.remote_locked);
        assert!(route.reply(LockReply {
            epoch: request.epoch,
            request: Some(request.request),
            locked: true
        }));
    }
    #[test]
    fn rapid_toggles_ignore_old_ack_and_old_handoff() {
        let mut route = Route::default();
        route.begin();
        let first = route.request();
        let second = route.request();
        assert!(first.locked);
        assert!(!second.locked);
        assert!(!route.reply(LockReply {
            epoch: first.epoch,
            request: Some(first.request),
            locked: true
        }));
        assert!(!route.remote_locked);
        assert!(route.reply(LockReply {
            epoch: second.epoch,
            request: Some(second.request),
            locked: false
        }));
        route.begin();
        assert!(!route.reply(LockReply {
            epoch: first.epoch,
            request: None,
            locked: true
        }));
        assert!(!route.remote_locked);
    }
    #[test]
    fn target_keyboard_can_unlock_and_retire_pending_ack() {
        let mut route = Route::default();
        route.begin();
        let request = route.request();
        assert!(route.reply(LockReply {
            epoch: request.epoch,
            request: None,
            locked: false
        }));
        assert!(!route.reply(LockReply {
            epoch: request.epoch,
            request: Some(request.request),
            locked: true
        }));
        assert!(!route.remote_locked);
    }
}
