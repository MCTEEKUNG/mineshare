//! Daemon runtime — what `mineshare-daemon run` (default) does.
//!
//! After mDNS discovery + Noise XX handshake, two peers exchange UDP port
//! numbers over the encrypted TCP control channel and then forward captured
//! mouse/keyboard events over UDP. M1 forwards *all* captured events
//! continuously; M2 will gate forwarding via the source/routing FSMs.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use bincode::config::standard;
use mineshare_core::DeviceId;
use mineshare_input::{InputEvent, InputInject, make_capture, make_inject};
use mineshare_net::{
    Discovery, DiscoveryEvent, EncryptedSession, Initiator, NoiseSession, PeerAdvert, Responder,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use crate::identity::Identity;
use crate::logs;

const DEFAULT_CONTROL_PORT: u16 = 0; // 0 = OS-assigned

/// One-shot message exchanged on the encrypted TCP control channel after
/// the Noise handshake. Carries the peer-side UDP port (so we know where
/// to send the input/audio streams) plus the peer's primary screen
/// geometry in physical pixels — used by both sides to clamp `virt_x`
/// against the real peer width instead of hardcoded constants.
///
/// Wire format is positional (bincode), so adding fields is a breaking
/// protocol change. Both daemons must run the same version.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PortAnnounce {
    udp_port: u16,
    daemon_version: String,
    screen_w: u32,
    screen_h: u32,
}

/// Streamed messages on the encrypted TCP control channel after the
/// initial PortAnnounce exchange. Used to coordinate which peer holds
/// Remote mode at any given moment so the local capture can refuse to
/// also enter Remote (which would have both ends fighting for the
/// cursor).
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ControlMsg {
    TakeControl,
    ReleaseControl,
    /// Peer is requesting that *we* leave Remote (their hotkey was
    /// pressed while we held Remote). We should call
    /// `force_local_exit_remote()` and then send `ReleaseControl`
    /// from the resulting `Exited` event.
    ForceRelease,
}

#[derive(Debug, Clone, Copy)]
pub struct RunOpts {
    pub capture: bool,
    pub inject: bool,
}

pub async fn run(opts: RunOpts) -> Result<()> {
    logs::init()?;

    let identity = Identity::load_or_create().context("identity bootstrap failed")?;
    info!(
        device_id = %identity.device_id,
        name = %identity.display_name,
        os = %identity.os,
        capture = opts.capture,
        inject = opts.inject,
        "MineShare daemon starting"
    );

    // --- Input capture ----------------------------------------------------
    // Capture starts in passive mode immediately. Events fan out to every
    // connected peer via a broadcast channel.
    let (cap_tx, mut cap_rx) = mpsc::unbounded_channel::<InputEvent>();
    let cap_started = if opts.capture {
        match make_capture() {
            Ok(mut cap) => match cap.start(cap_tx) {
                Ok(()) => {
                    info!("input capture started (passive)");
                    Some(cap)
                }
                Err(e) => {
                    warn!(error = %e, "input capture failed to start — will run inject-only");
                    None
                }
            },
            Err(e) => {
                warn!(error = %e, "no input capture available on this platform");
                None
            }
        }
    } else {
        info!("capture disabled via --no-capture");
        None
    };
    // keep alive
    let _cap_alive = cap_started;

    let (event_bcast, _) = broadcast::channel::<InputEvent>(1024);
    let bcast_for_drain = event_bcast.clone();
    tokio::spawn(async move {
        while let Some(ev) = cap_rx.recv().await {
            let _ = bcast_for_drain.send(ev);
        }
        debug!("capture pump terminated");
    });

    // --- Input injection --------------------------------------------------
    let inject: Arc<dyn InputInject> = if opts.inject {
        match make_inject() {
            Ok(boxed) => {
                info!("input inject ready");
                Arc::from(boxed)
            }
            Err(e) => {
                warn!(error = %e, "no input inject available — running capture-only");
                Arc::new(NullInject)
            }
        }
    } else {
        info!("inject disabled via --no-inject");
        Arc::new(NullInject)
    };

    // --- Control listener -------------------------------------------------
    let listener = TcpListener::bind(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        DEFAULT_CONTROL_PORT,
    ))
    .await
    .context("failed to bind control listener")?;
    let local_port = listener.local_addr()?.port();
    info!(port = local_port, "control listener bound");

    let known_peers: Arc<Mutex<HashMap<DeviceId, PeerAdvert>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let resp_static = identity.noise_static_priv.clone();
    let resp_inject = inject.clone();
    let resp_bcast = event_bcast.clone();
    tokio::spawn(async move {
        accept_loop(listener, resp_static, resp_inject, resp_bcast).await;
    });

    // --- mDNS announce + browse ------------------------------------------
    let mut discovery = Discovery::new()?;
    let advert = PeerAdvert {
        device_id: identity.device_id,
        display_name: identity.display_name.clone(),
        os: identity.os.clone(),
        control_port: local_port,
        addresses: detect_local_addresses(),
    };
    discovery.announce(&advert)?;

    let (tx, mut rx) = mpsc::channel::<DiscoveryEvent>(32);
    discovery.browse(tx)?;

    let init_static = identity.noise_static_priv.clone();
    let known = known_peers.clone();
    let local_id = identity.device_id;

    while let Some(evt) = rx.recv().await {
        match evt {
            DiscoveryEvent::PeerOnline(peer) => {
                if peer.device_id == local_id {
                    debug!("ignoring own advert");
                    continue;
                }
                let already_known = {
                    let mut k = known.lock();
                    let new = !k.contains_key(&peer.device_id);
                    k.insert(peer.device_id, peer.clone());
                    !new
                };
                if already_known {
                    continue;
                }
                info!(
                    peer = %peer.device_id,
                    name = %peer.display_name,
                    os = %peer.os,
                    addrs = ?peer.addresses,
                    port = peer.control_port,
                    "peer discovered"
                );

                if local_id.0 < peer.device_id.0 {
                    let init_static = init_static.clone();
                    let inject = inject.clone();
                    let bcast = event_bcast.clone();
                    tokio::spawn(async move {
                        if let Err(e) = dial_and_run(&peer, &init_static, inject, bcast).await {
                            warn!(peer = %peer.device_id, error = %e, "outbound peer session ended");
                        }
                    });
                } else {
                    debug!(peer = %peer.device_id, "deferring handshake — peer will initiate");
                }
            }
            DiscoveryEvent::PeerOffline(id) => {
                known.lock().remove(&id);
                info!(peer = %id, "peer offline");
            }
        }
    }

    Ok(())
}

async fn accept_loop(
    listener: TcpListener,
    static_priv: Vec<u8>,
    inject: Arc<dyn InputInject>,
    bcast: broadcast::Sender<InputEvent>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(%addr, "incoming connection");
                let key = static_priv.clone();
                let inject = inject.clone();
                let bcast = bcast.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_inbound(stream, &key, inject, bcast).await {
                        warn!(%addr, error = %e, "inbound peer session ended");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "accept error");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

async fn handle_inbound(
    mut stream: TcpStream,
    static_priv: &[u8],
    inject: Arc<dyn InputInject>,
    bcast: broadcast::Sender<InputEvent>,
) -> Result<()> {
    let peer_addr = stream.peer_addr()?;
    let mut resp = Responder::new(static_priv)?;
    let session = resp.handshake(&mut stream).await?;
    info!(
        %peer_addr,
        peer_pub = %hex_short(&session.remote_static),
        "inbound Noise XX handshake completed"
    );
    run_peer_session(stream, session, inject, bcast).await
}

async fn dial_and_run(
    peer: &PeerAdvert,
    static_priv: &[u8],
    inject: Arc<dyn InputInject>,
    bcast: broadcast::Sender<InputEvent>,
) -> Result<()> {
    let addr = peer
        .addresses
        .iter()
        .copied()
        .next()
        .context("peer has no addresses")?;
    let sock = SocketAddr::new(addr, peer.control_port);
    debug!(%sock, "dialing peer");
    let mut stream = TcpStream::connect(sock).await?;
    let mut init = Initiator::new(static_priv)?;
    let session = init.handshake(&mut stream).await?;
    info!(
        peer = %peer.device_id,
        peer_pub = %hex_short(&session.remote_static),
        "outbound Noise XX handshake completed"
    );
    run_peer_session(stream, session, inject, bcast).await
}

/// Drives one peer connection after the Noise handshake.
///
/// 1. Bind a UDP socket and announce its port over the encrypted TCP channel.
/// 2. Receive the peer's UDP port.
/// 3. Spawn UDP-recv → decrypt → inject loop.
/// 4. Spawn capture-bcast-recv → encrypt → UDP-send loop.
async fn run_peer_session(
    mut stream: TcpStream,
    session: NoiseSession,
    inject: Arc<dyn InputInject>,
    bcast: broadcast::Sender<InputEvent>,
) -> Result<()> {
    let aead = EncryptedSession::from(session);
    let peer_addr = stream.peer_addr()?;

    let udp = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)).await?;
    let local_udp_port = udp.local_addr()?.port();
    debug!(local_udp_port, "local UDP socket bound for peer");

    let (local_w, local_h) = mineshare_input::local_screen_geometry();
    let announce = PortAnnounce {
        udp_port: local_udp_port,
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        screen_w: local_w,
        screen_h: local_h,
    };
    write_encrypted(&mut stream, &aead, &announce).await?;
    let peer_announce: PortAnnounce = read_encrypted(&mut stream, &aead).await?;
    let peer_udp = SocketAddr::new(peer_addr.ip(), peer_announce.udp_port);
    mineshare_input::set_peer_screen(peer_announce.screen_w, peer_announce.screen_h);
    info!(
        %peer_addr,
        local_udp = local_udp_port,
        peer_udp = %peer_udp,
        peer_ver = %peer_announce.daemon_version,
        local_screen = ?(local_w, local_h),
        peer_screen = ?(peer_announce.screen_w, peer_announce.screen_h),
        "input UDP channel established"
    );

    // Split the TCP stream so we can run a writer task (forwarding
    // RemoteEvent → ControlMsg) and a reader task (receiving the peer's
    // ControlMsg → set_peer_in_remote) in parallel after PortAnnounce.
    let (mut tcp_read, mut tcp_write) = stream.into_split();

    // Writer task: capture-side RemoteEvent → ControlMsg over TCP.
    let (rev_tx, mut rev_rx) =
        tokio::sync::mpsc::unbounded_channel::<mineshare_input::RemoteEvent>();
    mineshare_input::set_remote_event_sender(rev_tx);
    let aead_writer = aead.clone_handle();
    tokio::spawn(async move {
        while let Some(ev) = rev_rx.recv().await {
            let msg = match ev {
                mineshare_input::RemoteEvent::Entered => ControlMsg::TakeControl,
                mineshare_input::RemoteEvent::Exited => ControlMsg::ReleaseControl,
                mineshare_input::RemoteEvent::RequestPeerExit => ControlMsg::ForceRelease,
            };
            if let Err(e) = write_encrypted(&mut tcp_write, &aead_writer, &msg).await {
                warn!(error = %e, "control writer failed — peer probably disconnected");
                break;
            }
            debug!(?msg, "sent ControlMsg");
        }
    });

    // Reader task: peer's ControlMsg → coordination state updates.
    let aead_reader = aead.clone_handle();
    tokio::spawn(async move {
        loop {
            match read_encrypted::<_, ControlMsg>(&mut tcp_read, &aead_reader).await {
                Ok(ControlMsg::TakeControl) => {
                    info!("peer took Remote control");
                    mineshare_input::set_peer_in_remote(true);
                }
                Ok(ControlMsg::ReleaseControl) => {
                    info!("peer released Remote control");
                    mineshare_input::set_peer_in_remote(false);
                }
                Ok(ControlMsg::ForceRelease) => {
                    info!("peer asked us to release Remote");
                    mineshare_input::force_local_exit_remote();
                }
                Err(e) => {
                    debug!(error = %e, "control reader ended");
                    break;
                }
            }
        }
        // Connection lost — clear stale state so a future reconnection
        // doesn't start with the wrong belief.
        mineshare_input::set_peer_in_remote(false);
        mineshare_input::clear_remote_event_sender();
    });

    let udp = Arc::new(udp);
    let stats = Arc::new(SessionStats::default());

    // --- recv → inject ----------------------------------------------------
    let aead_recv = aead.clone_handle();
    let udp_recv = udp.clone();
    let inject_recv = inject.clone();
    let stats_recv = stats.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            match udp_recv.recv_from(&mut buf).await {
                Ok((n, src)) if src == peer_udp => {
                    stats_recv.recv_pkts.fetch_add(1, Ordering::Relaxed);
                    stats_recv.recv_bytes.fetch_add(n as u64, Ordering::Relaxed);
                    match aead_recv.open(&buf[..n]) {
                        Ok(pt) => {
                            match bincode::serde::decode_from_slice::<InputEvent, _>(
                                &pt,
                                standard(),
                            ) {
                                Ok((ev, _)) => {
                                    // Log every 200th event so we can spot-check
                                    // what's actually arriving without spamming.
                                    let n = stats_recv.injected.load(Ordering::Relaxed);
                                    if n % 200 == 0 {
                                        tracing::info!(?ev, n, "sample inject event");
                                    }
                                    if let Err(e) = inject_recv.dispatch(ev) {
                                        warn!(error = %e, "inject failed");
                                        stats_recv.inject_errs.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        stats_recv.injected.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => warn!(error = %e, "decode failed"),
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "decrypt failed");
                            stats_recv.decrypt_errs.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
                Ok((_, src)) => debug!(%src, "UDP from unexpected source — ignoring"),
                Err(e) => {
                    warn!(error = %e, "UDP recv error");
                    break;
                }
            }
        }
    });

    // --- 1-Hz stats logger ----------------------------------------------
    let stats_tick = stats.clone();
    let peer_label = peer_addr.to_string();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut prev = StatsSnapshot::default();
        loop {
            interval.tick().await;
            let curr = stats_tick.snapshot();
            let delta = curr.delta(&prev);
            if delta.sent_pkts != 0 || delta.recv_pkts != 0 {
                info!(
                    peer = %peer_label,
                    sent_pkts = delta.sent_pkts,
                    sent_bytes = delta.sent_bytes,
                    recv_pkts = delta.recv_pkts,
                    recv_bytes = delta.recv_bytes,
                    injected = delta.injected,
                    inject_errs = delta.inject_errs,
                    decrypt_errs = delta.decrypt_errs,
                    "1-Hz stats"
                );
            }
            prev = curr;
        }
    });

    // --- capture broadcast → send ---------------------------------------
    let mut sub = bcast.subscribe();
    loop {
        match sub.recv().await {
            Ok(ev) => {
                let pt = match bincode::serde::encode_to_vec(ev, standard()) {
                    Ok(b) => b,
                    Err(e) => {
                        warn!(error = %e, "encode failed");
                        continue;
                    }
                };
                let ct = match aead.seal(&pt) {
                    Ok(b) => b,
                    Err(e) => {
                        warn!(error = %e, "encrypt failed");
                        continue;
                    }
                };
                let len = ct.len();
                if let Err(e) = udp.send_to(&ct, peer_udp).await {
                    warn!(error = %e, "UDP send failed — ending session");
                    break;
                }
                stats.sent_pkts.fetch_add(1, Ordering::Relaxed);
                stats.sent_bytes.fetch_add(len as u64, Ordering::Relaxed);
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "broadcast subscriber lagged — events dropped");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    Ok(())
}

async fn write_encrypted<W, T>(stream: &mut W, aead: &EncryptedSession, msg: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let pt = bincode::serde::encode_to_vec(msg, standard())?;
    let ct = aead.seal(&pt)?;
    let len = u32::try_from(ct.len()).context("frame too large")?;
    stream.write_all(&len.to_be_bytes()).await?;
    stream.write_all(&ct).await?;
    Ok(())
}

async fn read_encrypted<R, T>(stream: &mut R, aead: &EncryptedSession) -> Result<T>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut hdr = [0u8; 4];
    stream.read_exact(&mut hdr).await?;
    let len = u32::from_be_bytes(hdr) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    let pt = aead.open(&buf)?;
    let (val, _) = bincode::serde::decode_from_slice::<T, _>(&pt, standard())?;
    Ok(val)
}

fn hex_short(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

fn detect_local_addresses() -> Vec<IpAddr> {
    use std::net::UdpSocket;
    let mut addrs = Vec::new();
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0")
        && s.connect("8.8.8.8:80").is_ok()
        && let Ok(local) = s.local_addr()
    {
        addrs.push(local.ip());
    }
    if addrs.is_empty() {
        addrs.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    addrs
}

#[derive(Default)]
struct SessionStats {
    sent_pkts: AtomicU64,
    sent_bytes: AtomicU64,
    recv_pkts: AtomicU64,
    recv_bytes: AtomicU64,
    injected: AtomicU64,
    inject_errs: AtomicU64,
    decrypt_errs: AtomicU64,
}

#[derive(Default, Clone, Copy)]
struct StatsSnapshot {
    sent_pkts: u64,
    sent_bytes: u64,
    recv_pkts: u64,
    recv_bytes: u64,
    injected: u64,
    inject_errs: u64,
    decrypt_errs: u64,
}

impl SessionStats {
    fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            sent_pkts: self.sent_pkts.load(Ordering::Relaxed),
            sent_bytes: self.sent_bytes.load(Ordering::Relaxed),
            recv_pkts: self.recv_pkts.load(Ordering::Relaxed),
            recv_bytes: self.recv_bytes.load(Ordering::Relaxed),
            injected: self.injected.load(Ordering::Relaxed),
            inject_errs: self.inject_errs.load(Ordering::Relaxed),
            decrypt_errs: self.decrypt_errs.load(Ordering::Relaxed),
        }
    }
}

impl StatsSnapshot {
    fn delta(&self, prev: &StatsSnapshot) -> StatsSnapshot {
        StatsSnapshot {
            sent_pkts: self.sent_pkts.saturating_sub(prev.sent_pkts),
            sent_bytes: self.sent_bytes.saturating_sub(prev.sent_bytes),
            recv_pkts: self.recv_pkts.saturating_sub(prev.recv_pkts),
            recv_bytes: self.recv_bytes.saturating_sub(prev.recv_bytes),
            injected: self.injected.saturating_sub(prev.injected),
            inject_errs: self.inject_errs.saturating_sub(prev.inject_errs),
            decrypt_errs: self.decrypt_errs.saturating_sub(prev.decrypt_errs),
        }
    }
}

/// No-op inject used as a fallback when the platform implementation can't
/// initialise (e.g. no GUI session, or `/dev/uinput` permission denied).
struct NullInject;

impl InputInject for NullInject {
    fn mouse_move_rel(&self, _dx: i32, _dy: i32) -> Result<()> {
        Ok(())
    }
    fn mouse_button(&self, _btn: mineshare_input::Button, _down: bool) -> Result<()> {
        Ok(())
    }
    fn key(&self, _code: mineshare_input::KeyCode, _down: bool) -> Result<()> {
        Ok(())
    }
    fn scroll(&self, _dx: f32, _dy: f32) -> Result<()> {
        Ok(())
    }
}
