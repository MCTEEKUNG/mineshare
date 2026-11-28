use super::*;
use std::sync::LazyLock;

#[derive(Clone, Serialize)]
pub struct PeerSessionStatus {
    pub device_id: String,
    pub display_name: String,
    pub os: String,
    pub input_active: bool,
    pub send_audio: bool,
    pub receive_audio: bool,
    pub display_width: u32,
    pub display_height: u32,
    pub peer_side: crate::layout::PeerSide,
}

pub(super) struct PeerRoute {
    pub status: PeerSessionStatus,
    pub rev_tx: mpsc::UnboundedSender<mineshare_input::RemoteEvent>,
    pub control_tx: mpsc::UnboundedSender<ControlMsg>,
    pub side_tx: mpsc::UnboundedSender<crate::layout::PeerSide>,
    pub clip_tx: mpsc::Sender<crate::clipboard::ClipboardEvent>,
    pub file_tx: crate::files::ControlSender,
    pub inject: Arc<dyn InputInject>,
    pub staged_paste: bool,
    pub capabilities: DeviceCapabilities,
    pub touchpad_capabilities: u32,
    pub address: String,
    pub version: String,
    pub remote_receives_audio: bool,
    pub relay_tx: mpsc::Sender<(u64, mineshare_input::InputEvent)>,
}

struct InputRelay {
    source: String,
    target: String,
    epoch: u64,
    restore: (i32, i32),
    held: mineshare_input::ForwardedInputState,
}

#[derive(Default)]
struct Routes {
    peers: HashMap<String, PeerRoute>,
    active: Option<String>,
    relay: Option<InputRelay>,
    relay_epoch: u64,
    received_held: mineshare_input::ForwardedInputState,
}
static ROUTES: LazyLock<Mutex<Routes>> = LazyLock::new(|| Mutex::new(Routes::default()));

pub fn connected_peers() -> Vec<PeerSessionStatus> {
    let routes = ROUTES.lock();
    let mut result: Vec<_> = routes
        .peers
        .values()
        .map(|peer| {
            let mut status = peer.status.clone();
            status.input_active = routes.active.as_deref() == Some(&status.device_id);
            status.peer_side = crate::layout::side_for_peer(&status.device_id);
            status
        })
        .collect();
    result.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
            .then(a.device_id.cmp(&b.device_id))
    });
    result
}

pub(super) fn active_id() -> Option<String> {
    ROUTES.lock().active.clone()
}
pub(super) fn is_active(id: &str) -> bool {
    ROUTES.lock().active.as_deref() == Some(id)
}

pub fn select_input_peer(device_id: Option<&str>) -> Result<()> {
    let mut routes = ROUTES.lock();
    if let Some(id) = device_id {
        anyhow::ensure!(routes.peers.contains_key(id), "device is not connected");
    }
    if routes.active.as_deref() == device_id {
        return Ok(());
    }
    stop_relay(&mut routes, false);
    routes.received_held = Default::default();
    mineshare_input::force_local_exit_remote();
    mineshare_input::set_peer_in_remote(false);
    mineshare_input::set_game_drive(mineshare_input::GameDrive::Off);
    if let Some(old) = routes.active.as_ref().and_then(|id| routes.peers.get(id)) {
        let _ = old.control_tx.send(ControlMsg::ReleaseControl);
        let _ = old.control_tx.send(ControlMsg::ForceRelease);
        old.inject.release_all_held()?;
        for id in crate::files::cancel_active_session() {
            let _ = old.control_tx.send(ControlMsg::FileCancel { id });
        }
    }
    crate::clipboard::detach_peer();
    mineshare_input::clear_remote_event_sender();
    routes.active = device_id.map(str::to_owned);
    if let Some(peer) = device_id.and_then(|id| routes.peers.get(id)) {
        activate(peer);
    } else {
        crate::status::clear_peer_connected();
    }
    Ok(())
}

fn activate(peer: &PeerRoute) {
    mineshare_input::set_peer_screen(peer.status.display_width, peer.status.display_height);
    mineshare_input::set_peer_touchpad_capabilities(peer.touchpad_capabilities);
    crate::layout::activate_peer(&peer.status.device_id);
    crate::layout::set_propagate_sender(peer.side_tx.clone());
    mineshare_input::set_remote_event_sender(peer.rev_tx.clone());
    mineshare_input::reset_smart_decision();
    if peer.capabilities.files {
        crate::files::set_session_tx(peer.file_tx.clone());
    }
    if peer.capabilities.clipboard {
        crate::clipboard::ensure_watcher(
            peer.clip_tx.clone(),
            peer.file_tx.clone(),
            peer.inject.clone(),
            peer.staged_paste,
        );
    }
    crate::status::set_peer_connected(peer.address.clone(), Some(peer.status.display_name.clone()));
    crate::status::set_peer_version(Some(peer.version.clone()));
}

pub(super) fn register(peer: PeerRoute) {
    let _ = peer.control_tx.send(ControlMsg::AudioRoute {
        receive_system: peer.status.receive_audio,
    });
    let id = peer.status.device_id.clone();
    let select = {
        let mut routes = ROUTES.lock();
        let select = routes.active.is_none();
        routes.peers.insert(id.clone(), peer);
        select
    };
    if select {
        let _ = select_input_peer(Some(&id));
    }
}

pub(super) fn unregister(id: &str) -> bool {
    {
        let mut routes = ROUTES.lock();
        if routes
            .relay
            .as_ref()
            .is_some_and(|r| r.source == id || r.target == id)
        {
            stop_relay(&mut routes, true);
        }
    }
    let was_active = is_active(id);
    if was_active
        && ROUTES
            .lock()
            .peers
            .get(id)
            .is_some_and(|p| p.capabilities.files)
    {
        crate::files::clear_session_tx();
    }
    if was_active {
        let _ = select_input_peer(None);
    }
    ROUTES.lock().peers.remove(id);
    if was_active {
        let next = ROUTES.lock().peers.keys().min().cloned();
        if let Some(next) = next {
            let _ = select_input_peer(Some(&next));
        }
    }
    was_active
}

pub fn set_peer_audio_route(
    device_id: &str,
    send_system: bool,
    receive_system: bool,
) -> Result<()> {
    let mut routes = ROUTES.lock();
    let peer = routes
        .peers
        .get_mut(device_id)
        .context("device is not connected")?;
    anyhow::ensure!(
        !send_system || peer.capabilities.system_audio_receive,
        "device cannot receive system audio"
    );
    anyhow::ensure!(
        !receive_system || peer.capabilities.system_audio_send,
        "device cannot send system audio"
    );
    crate::settings::set_peer_audio_route(device_id, send_system, receive_system)?;
    peer.status.send_audio = send_system;
    peer.status.receive_audio = receive_system;
    let _ = peer
        .control_tx
        .send(ControlMsg::AudioRoute { receive_system });
    drop(routes);
    Ok(())
}

pub(super) fn sends_audio(id: &str, stream: mineshare_audio::StreamKind) -> bool {
    let routes = ROUTES.lock();
    match stream {
        mineshare_audio::StreamKind::SysOut => routes
            .peers
            .get(id)
            .is_some_and(|p| p.status.send_audio && p.remote_receives_audio),
        mineshare_audio::StreamKind::Mic => routes.active.as_deref() == Some(id),
    }
}
pub(super) fn receives_audio(id: &str) -> bool {
    ROUTES
        .lock()
        .peers
        .get(id)
        .is_some_and(|p| p.status.receive_audio)
}
pub(super) fn remote_receives(id: &str, receive: bool) {
    if let Some(peer) = ROUTES.lock().peers.get_mut(id) {
        peer.remote_receives_audio = receive;
    }
}
pub(super) fn geometry(id: &str, width: u32, height: u32) {
    if !(1..=32768).contains(&width) || !(1..=32768).contains(&height) {
        return;
    }
    let mut routes = ROUTES.lock();
    let active = routes.active.as_deref() == Some(id);
    if let Some(peer) = routes.peers.get_mut(id) {
        peer.status.display_width = width;
        peer.status.display_height = height;
        if active {
            mineshare_input::force_local_exit_remote();
            mineshare_input::set_peer_screen(width, height);
        }
    }
}

pub(super) fn select_edge(side: mineshare_input::PeerSide) -> bool {
    let id = {
        let routes = ROUTES.lock();
        routes
            .peers
            .values()
            .filter(|p| {
                p.capabilities.pointer
                    && crate::layout::map_side(crate::layout::side_for_peer(&p.status.device_id))
                        == side
            })
            .map(|p| p.status.device_id.clone())
            .min_by_key(|id| (routes.active.as_deref() != Some(id), id.clone()))
    };
    if let Some(id) = id {
        return select_input_peer(Some(&id)).is_ok();
    }
    false
}

pub fn push_peer_side(id: &str, side: crate::layout::PeerSide) {
    if let Some(peer) = ROUTES.lock().peers.get(id) {
        let _ = peer.side_tx.send(side);
    }
}

pub(super) fn inject_if_active(
    id: &str,
    event: mineshare_input::InputEvent,
    dispatch: impl FnOnce() -> bool,
) -> bool {
    let mut routes = ROUTES.lock();
    if routes.active.as_deref() != Some(id) {
        return true;
    }
    routes.received_held.apply_event(event);
    if routes.relay.is_some() {
        forward_relay(&mut routes, event);
        return true;
    }
    if !dispatch() {
        return false;
    }
    if !mineshare_input::peer_in_remote()
        || mineshare_input::is_game_receiving()
        || mineshare_input::remote_input_locked()
        || routes.received_held.any_held()
    {
        return true;
    }
    let mineshare_input::InputEvent::MouseMove { dx, dy } = event else {
        return true;
    };
    let Some(point) = routes
        .peers
        .get(id)
        .and_then(|p| p.inject.pointer_position())
    else {
        return true;
    };
    let Some(side) = outward_edge(point, dx, dy) else {
        return true;
    };
    if side == crate::layout::side_for_peer(id) {
        if let Some(source) = routes.peers.get(id) {
            let _ = source.control_tx.send(ControlMsg::ForceRelease);
        }
        mineshare_input::set_peer_in_remote(false);
        return true;
    }
    let target = routes
        .peers
        .values()
        .filter(|p| {
            p.status.device_id != id
                && p.capabilities.pointer
                && crate::layout::side_for_peer(&p.status.device_id) == side
        })
        .map(|p| p.status.device_id.clone())
        .min();
    if let Some(target) = target {
        begin_relay(&mut routes, id, target, point, side);
    }
    true
}

fn begin_relay(
    routes: &mut Routes,
    id: &str,
    target: String,
    point: mineshare_input::PointerPosition,
    side: crate::layout::PeerSide,
) {
    routes.relay_epoch = routes.relay_epoch.wrapping_add(1);
    let epoch = routes.relay_epoch;
    // Keep the middle desktop just inside the edge for the return trip.
    let restore = (
        point.x.clamp(point.left + 8, point.right - 8),
        point.y.clamp(point.top + 8, point.bottom - 8),
    );
    let peer = &routes.peers[&target];
    if peer
        .control_tx
        .send(ControlMsg::TakeControl { epoch })
        .is_ok()
    {
        routes.relay = Some(InputRelay {
            source: id.into(),
            target: target.clone(),
            epoch,
            restore,
            held: Default::default(),
        });
        info!(source = id, %target, ?side, "input relay entered");
    }
}

fn forward_relay(routes: &mut Routes, event: mineshare_input::InputEvent) {
    let Some(relay) = routes.relay.as_ref() else {
        return;
    };
    let packet = (relay.epoch, event);
    let target = relay.target.clone();
    if routes
        .peers
        .get(&target)
        .is_none_or(|p| p.relay_tx.try_send(packet).is_err())
    {
        stop_relay(routes, true);
        return;
    }
    routes.relay.as_mut().unwrap().held.apply_event(event);
}

fn outward_edge(
    p: mineshare_input::PointerPosition,
    dx: i32,
    dy: i32,
) -> Option<crate::layout::PeerSide> {
    use crate::layout::PeerSide::*;
    if dx < 0 && p.x <= p.left {
        Some(Left)
    } else if dx > 0 && p.x >= p.right {
        Some(Right)
    } else if dy < 0 && p.y <= p.top {
        Some(Top)
    } else if dy > 0 && p.y >= p.bottom {
        Some(Bottom)
    } else {
        None
    }
}

fn stop_relay(routes: &mut Routes, restore: bool) {
    let Some(relay) = routes.relay.take() else {
        return;
    };
    if let Some(peer) = routes.peers.get(&relay.target) {
        let _ = peer.control_tx.send(ControlMsg::ReleaseControl);
        let _ = peer.control_tx.send(ControlMsg::ForceRelease);
    }
    if restore && let Some(source) = routes.peers.get(&relay.source) {
        let _ = source.inject.warp_pointer(relay.restore.0, relay.restore.1);
        for event in mineshare_input::ForwardedInputState::default()
            .reconciliation_events(&routes.received_held)
        {
            let _ = source.inject.dispatch(event);
        }
    }
    info!(source = %relay.source, target = %relay.target, "input relay ended");
}

pub(super) fn return_relay(target: &str) -> bool {
    let mut routes = ROUTES.lock();
    if routes.relay.as_ref().is_none_or(|r| r.target != target) {
        return false;
    }
    stop_relay(&mut routes, true);
    true
}

pub(super) fn cancel_source_relay(source: &str) {
    let mut routes = ROUTES.lock();
    if routes.relay.as_ref().is_some_and(|r| r.source == source) {
        stop_relay(&mut routes, false);
    }
    if routes.active.as_deref() == Some(source) {
        routes.received_held = Default::default();
    }
}

pub(super) fn relay_matches(target: &str, epoch: u64) -> bool {
    ROUTES
        .lock()
        .relay
        .as_ref()
        .is_some_and(|r| r.target == target && r.epoch == epoch)
}

pub(super) fn relay_state(target: &str) -> Option<mineshare_input::ForwardedInputState> {
    ROUTES
        .lock()
        .relay
        .as_ref()
        .filter(|r| r.target == target)
        .map(|r| r.held)
}

pub(super) fn initial_audio_route(id: &str) -> (bool, bool) {
    if let Some(route) = crate::settings::load().peer_audio_routes.get(id) {
        return (route.send, route.receive);
    }
    let legacy = crate::trust::list_trusted().len() == 1;
    (legacy, legacy)
}

#[cfg(test)]
pub(super) fn send_test_control(id: &str, control: ControlMsg) {
    ROUTES
        .lock()
        .peers
        .get(id)
        .unwrap()
        .control_tx
        .send(control)
        .unwrap();
}

#[cfg(test)]
mod relay_tests {
    use super::*;
    use mineshare_input::{Button, InputEvent, KeyCode, PointerPosition};

    #[derive(Default)]
    struct Recorder {
        warps: Mutex<Vec<(i32, i32)>>,
        events: Mutex<Vec<InputEvent>>,
    }
    impl InputInject for Recorder {
        fn mouse_move_rel(&self, _: i32, _: i32) -> Result<()> {
            Ok(())
        }
        fn mouse_button(&self, _: Button, _: bool) -> Result<()> {
            Ok(())
        }
        fn key(&self, code: KeyCode, down: bool) -> Result<()> {
            self.events.lock().push(InputEvent::Key { code, down });
            Ok(())
        }
        fn scroll(&self, _: f32, _: f32) -> Result<()> {
            Ok(())
        }
        fn warp_pointer(&self, x: i32, y: i32) -> Result<()> {
            self.warps.lock().push((x, y));
            Ok(())
        }
    }
    fn peer(
        id: &str,
        inject: Arc<Recorder>,
    ) -> (
        PeerRoute,
        mpsc::UnboundedReceiver<ControlMsg>,
        mpsc::Receiver<(u64, InputEvent)>,
    ) {
        let (control_tx, control) = mpsc::unbounded_channel();
        let (relay_tx, relay) = mpsc::channel(2);
        (
            PeerRoute {
                status: PeerSessionStatus {
                    device_id: id.into(),
                    display_name: id.into(),
                    os: "test".into(),
                    input_active: false,
                    send_audio: false,
                    receive_audio: false,
                    display_width: 100,
                    display_height: 100,
                    peer_side: crate::layout::PeerSide::Bottom,
                },
                rev_tx: mpsc::unbounded_channel().0,
                control_tx,
                side_tx: mpsc::unbounded_channel().0,
                clip_tx: mpsc::channel(1).0,
                file_tx: mpsc::channel(1).0,
                inject,
                staged_paste: false,
                capabilities: DeviceCapabilities {
                    pointer: true,
                    ..Default::default()
                },
                touchpad_capabilities: 0,
                address: String::new(),
                version: String::new(),
                remote_receives_audio: false,
                relay_tx,
            },
            control,
            relay,
        )
    }
    fn bottom() -> PointerPosition {
        PointerPosition {
            x: 50,
            y: 99,
            left: 0,
            top: 0,
            right: 99,
            bottom: 99,
        }
    }

    #[test]
    fn received_mouse_can_enter_phone_and_return_without_releasing_ubuntu() {
        let inject = Arc::new(Recorder::default());
        let (source, mut upstream, _) = peer("ubuntu", inject.clone());
        let (phone, mut downstream, mut input) = peer("phone", inject.clone());
        let mut routes = Routes {
            active: Some("ubuntu".into()),
            ..Default::default()
        };
        routes.peers.insert("ubuntu".into(), source);
        routes.peers.insert("phone".into(), phone);
        assert_eq!(
            outward_edge(bottom(), 0, 10),
            Some(crate::layout::PeerSide::Bottom)
        );
        assert_eq!(outward_edge(bottom(), 0, -10), None);
        begin_relay(
            &mut routes,
            "ubuntu",
            "phone".into(),
            bottom(),
            crate::layout::PeerSide::Bottom,
        );
        assert!(matches!(
            downstream.try_recv(),
            Ok(ControlMsg::TakeControl { epoch: 1 })
        ));
        let movement = InputEvent::MouseMove { dx: 4, dy: 15 };
        forward_relay(&mut routes, movement);
        assert_eq!(input.try_recv().unwrap(), (1, movement));
        let key = InputEvent::Key {
            code: KeyCode(42),
            down: true,
        };
        routes.received_held.apply_event(key);
        forward_relay(&mut routes, key);
        assert_eq!(input.try_recv().unwrap(), (1, key));
        assert!(routes.relay.as_ref().unwrap().held.any_held());
        stop_relay(&mut routes, true);
        assert_eq!(routes.active.as_deref(), Some("ubuntu"));
        assert!(
            upstream.try_recv().is_err(),
            "return must preserve upstream ownership"
        );
        assert!(matches!(
            downstream.try_recv(),
            Ok(ControlMsg::ReleaseControl)
        ));
        assert!(matches!(
            downstream.try_recv(),
            Ok(ControlMsg::ForceRelease)
        ));
        assert_eq!(*inject.warps.lock(), vec![(50, 91)]);
        assert_eq!(*inject.events.lock(), vec![key]);
        begin_relay(
            &mut routes,
            "ubuntu",
            "phone".into(),
            bottom(),
            crate::layout::PeerSide::Bottom,
        );
        assert_eq!(
            routes.relay.as_ref().unwrap().epoch,
            2,
            "old queued packets cannot enter a new relay"
        );
    }

    #[test]
    fn disconnected_relay_returns_control_and_releases_destination() {
        let inject = Arc::new(Recorder::default());
        let (source, _, _) = peer("ubuntu", inject.clone());
        let (phone, mut control, input) = peer("phone", inject.clone());
        let mut routes = Routes::default();
        routes.peers.insert("ubuntu".into(), source);
        routes.peers.insert("phone".into(), phone);
        begin_relay(
            &mut routes,
            "ubuntu",
            "phone".into(),
            bottom(),
            crate::layout::PeerSide::Bottom,
        );
        control.try_recv().unwrap();
        drop(input);
        forward_relay(&mut routes, InputEvent::MouseMove { dx: 0, dy: 1 });
        assert!(routes.relay.is_none());
        assert!(matches!(control.try_recv(), Ok(ControlMsg::ReleaseControl)));
        assert_eq!(inject.warps.lock().len(), 1);
    }
}
