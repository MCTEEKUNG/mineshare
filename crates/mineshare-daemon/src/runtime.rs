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
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bincode::config::standard;
use mineshare_audio::{AudioFrame, AudioPlayback};
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

/// Echo-loop guard: the peer's *sysout* coming out of our speakers
/// is what our own loopback re-captures and would forward back as a
/// new sysout frame — that's the feedback loop. Mic frames don't
/// loop the same way (they originate from a separate physical
/// device), so we deliberately track only peer-sysout arrivals
/// here. Mixing mic into the guard causes user-talk-while-music-
/// plays scenarios to stutter the music: every breath/keyboard
/// click captured by the peer's mic would briefly suppress our
/// sysout forwarding.
///
/// We *only* arm the guard on substantial Opus payloads — a silent
/// stream still sends ~3-byte comfort-noise frames at 50 fps, and
/// counting those would permanently suppress whichever side talks
/// last (a deadlock on idle). 12 bytes is well above DTX/CN size
/// while still triggering on quiet music.
const ECHO_GUARD_MS: u64 = 500;
const ECHO_TRIGGER_MIN_BYTES: usize = 12;
static LAST_PEER_SYSOUT_AT_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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
    /// Peer's clipboard changed — apply the text on this side. Bound
    /// to 64 KB at the watcher to keep oversized pastes from
    /// flooding the control channel; receivers should also sanity-
    /// check length before applying.
    ClipboardText(String),
}

/// Tagged UDP payload — input events and audio frames share the same
/// encrypted UDP socket, so the receiver needs to know which kind of
/// payload arrived. bincode prefixes the variant index automatically.
///
/// Wire format is positional; both daemons must run the same version.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireFrame {
    Input(InputEvent),
    Audio(AudioFrame),
}

#[derive(Debug, Clone, Copy)]
pub struct RunOpts {
    pub capture: bool,
    pub inject: bool,
}

pub async fn run(opts: RunOpts) -> Result<()> {
    logs::init()?;

    let identity = Identity::load_or_create().context("identity bootstrap failed")?;

    // Push the persisted layout to the input modules before any
    // capture starts so edge detection / boundary warp / virt_x
    // sign convention come up in the right orientation.
    let layout_cfg = crate::layout::current();
    let side = match layout_cfg.peer_side {
        crate::layout::PeerSide::Left => mineshare_input::PeerSide::Left,
        crate::layout::PeerSide::Right => mineshare_input::PeerSide::Right,
    };
    mineshare_input::set_peer_side(side);

    info!(
        device_id = %identity.device_id,
        name = %identity.display_name,
        os = %identity.os,
        capture = opts.capture,
        inject = opts.inject,
        peer_side = ?layout_cfg.peer_side,
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

    // Input + audio frames share the same broadcast channel so the
    // per-peer UDP-send task only has to subscribe once. WireFrame's
    // tagged variants tell the receiver which payload kind to inject.
    let (wire_bcast, _) = broadcast::channel::<WireFrame>(1024);
    let bcast_for_input_drain = wire_bcast.clone();
    tokio::spawn(async move {
        while let Some(ev) = cap_rx.recv().await {
            let _ = bcast_for_input_drain.send(WireFrame::Input(ev));
        }
        debug!("input capture pump terminated");
    });

    // --- Audio sysout capture (Win loopback / Linux PipeWire monitor) --
    let (audio_cap_tx, mut audio_cap_rx) = mpsc::unbounded_channel::<AudioFrame>();
    let audio_cap_started = if opts.capture {
        match mineshare_audio::make_sysout_capture() {
            Ok(mut cap) => match cap.start(audio_cap_tx.clone()) {
                Ok(()) => {
                    info!("audio sysout capture started");
                    Some(cap)
                }
                Err(e) => {
                    warn!(error = %e, "audio sysout capture failed to start");
                    None
                }
            },
            Err(e) => {
                info!(reason = %e, "audio sysout capture not available — skipping");
                None
            }
        }
    } else {
        info!("audio capture disabled via --no-capture");
        None
    };
    let _audio_cap_alive = audio_cap_started;

    // --- Mic capture (M3 Slice 3) --------------------------------------
    // Default input device on each platform — cpal handles WASAPI on
    // Windows and ALSA-via-PipeWire on Linux. Frames carry the
    // `StreamKind::Mic` tag so the receiver can route them to a
    // virtual mic device (PipeWire null-sink / VB-CABLE) rather than
    // mixing them with sysout into the speakers.
    let mic_cap_started = if opts.capture {
        match mineshare_audio::make_mic_capture() {
            Ok(mut cap) => match cap.start(audio_cap_tx) {
                Ok(()) => {
                    info!("mic capture started");
                    Some(cap)
                }
                Err(e) => {
                    warn!(error = %e, "mic capture failed to start");
                    None
                }
            },
            Err(e) => {
                info!(reason = %e, "mic capture not available — skipping");
                None
            }
        }
    } else {
        None
    };
    let _mic_cap_alive = mic_cap_started;

    let bcast_for_audio_drain = wire_bcast.clone();
    tokio::spawn(async move {
        while let Some(frame) = audio_cap_rx.recv().await {
            // Echo-loop guard ONLY applies to sysout: when the peer
            // is talking and we play it back, our own loopback
            // re-captures it and would feed a runaway loop. Mic
            // frames originate from a physically separate device
            // (the user's mic) and don't have this problem, so we
            // forward them unconditionally.
            if matches!(frame.stream, mineshare_audio::StreamKind::SysOut) {
                let last = LAST_PEER_SYSOUT_AT_MS.load(Ordering::Relaxed);
                if last != 0 && now_ms().saturating_sub(last) < ECHO_GUARD_MS {
                    continue;
                }
            }
            let _ = bcast_for_audio_drain.send(WireFrame::Audio(frame));
        }
        debug!("audio capture pump terminated");
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

    // --- Audio playback ---------------------------------------------------
    // Playback is built on both sides so either end can render the
    // peer's sysout. Slice 1 only uses it on the Linux side, but
    // running it everywhere is harmless if no audio frames arrive.
    let playback: Arc<dyn AudioPlayback> = if opts.inject {
        match mineshare_audio::make_playback() {
            Ok(p) => {
                info!("audio playback ready");
                Arc::from(p)
            }
            Err(e) => {
                warn!(error = %e, "audio playback unavailable — sysout from peer will be silent");
                Arc::new(NullPlayback)
            }
        }
    } else {
        info!("audio playback disabled via --no-inject");
        Arc::new(NullPlayback)
    };

    // --- Virtual mic playback (M3 Slice 3b) -----------------------------
    // The peer's Mic frames need their own sink so apps on this side
    // see a "MineShare Mic" input device. Linux gets a PipeWire
    // null-sink we create at startup; Windows uses VB-CABLE if the
    // user installed it. Either failure is non-fatal — the bridge
    // keeps working, mic frames just become silent on this side until
    // VB-CABLE is installed (or the daemon restarted).
    let mic_playback: Arc<dyn AudioPlayback> = if opts.inject {
        match mineshare_audio::make_virtual_mic_playback() {
            Ok(p) => {
                info!("virtual mic playback ready");
                Arc::from(p)
            }
            Err(e) => {
                warn!(reason = %e, "virtual mic playback unavailable — peer mic frames will be dropped");
                Arc::new(NullPlayback)
            }
        }
    } else {
        Arc::new(NullPlayback)
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
    let resp_playback = playback.clone();
    let resp_mic_playback = mic_playback.clone();
    let resp_bcast = wire_bcast.clone();
    tokio::spawn(async move {
        accept_loop(
            listener,
            resp_static,
            resp_inject,
            resp_playback,
            resp_mic_playback,
            resp_bcast,
        )
        .await;
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
                    let playback = playback.clone();
                    let mic_playback = mic_playback.clone();
                    let bcast = wire_bcast.clone();
                    let known_for_loop = known.clone();
                    let peer_id = peer.device_id;
                    // Reconnect loop: redial after each session ends as long
                    // as the peer is still in `known_peers` (mDNS hasn't
                    // observed it dropping). This is what keeps us
                    // reconnecting after the peer's daemon restarts.
                    tokio::spawn(async move {
                        loop {
                            let peer_now = {
                                let k = known_for_loop.lock();
                                match k.get(&peer_id) {
                                    Some(p) => p.clone(),
                                    None => {
                                        debug!(peer = %peer_id, "peer offline — exiting reconnect loop");
                                        return;
                                    }
                                }
                            };
                            match dial_and_run(
                                &peer_now,
                                &init_static,
                                inject.clone(),
                                playback.clone(),
                                mic_playback.clone(),
                                bcast.clone(),
                            )
                            .await
                            {
                                Ok(()) => info!(peer = %peer_id, "session ended — will reconnect"),
                                Err(e) => warn!(peer = %peer_id, error = %e, "session error — will reconnect"),
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
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
    playback: Arc<dyn AudioPlayback>,
    mic_playback: Arc<dyn AudioPlayback>,
    bcast: broadcast::Sender<WireFrame>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(%addr, "incoming connection");
                let key = static_priv.clone();
                let inject = inject.clone();
                let playback = playback.clone();
                let mic_playback = mic_playback.clone();
                let bcast = bcast.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_inbound(stream, &key, inject, playback, mic_playback, bcast).await
                    {
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
    playback: Arc<dyn AudioPlayback>,
    mic_playback: Arc<dyn AudioPlayback>,
    bcast: broadcast::Sender<WireFrame>,
) -> Result<()> {
    let peer_addr = stream.peer_addr()?;
    let mut resp = Responder::new(static_priv)?;
    let session = resp.handshake(&mut stream).await?;
    info!(
        %peer_addr,
        peer_pub = %hex_short(&session.remote_static),
        "inbound Noise XX handshake completed"
    );
    run_peer_session(stream, session, inject, playback, mic_playback, bcast).await
}

async fn dial_and_run(
    peer: &PeerAdvert,
    static_priv: &[u8],
    inject: Arc<dyn InputInject>,
    playback: Arc<dyn AudioPlayback>,
    mic_playback: Arc<dyn AudioPlayback>,
    bcast: broadcast::Sender<WireFrame>,
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
    run_peer_session(stream, session, inject, playback, mic_playback, bcast).await
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
    playback: Arc<dyn AudioPlayback>,
    mic_playback: Arc<dyn AudioPlayback>,
    bcast: broadcast::Sender<WireFrame>,
) -> Result<()> {
    let aead = EncryptedSession::from(session);
    let peer_addr = stream.peer_addr()?;
    crate::status::set_peer_connected(peer_addr.to_string(), None);

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

    // Writer task: drain both input-RemoteEvent and clipboard-text
    // changes, encode each as a `ControlMsg`, and send over the
    // encrypted TCP channel. We `select!` between the two channels
    // so neither side starves the other.
    let (rev_tx, mut rev_rx) =
        tokio::sync::mpsc::unbounded_channel::<mineshare_input::RemoteEvent>();
    mineshare_input::set_remote_event_sender(rev_tx);
    let (clip_tx, mut clip_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    crate::clipboard::ensure_watcher(clip_tx);
    let aead_writer = aead.clone_handle();
    let writer_handle = tokio::spawn(async move {
        loop {
            let msg = tokio::select! {
                ev = rev_rx.recv() => match ev {
                    Some(mineshare_input::RemoteEvent::Entered) => ControlMsg::TakeControl,
                    Some(mineshare_input::RemoteEvent::Exited) => ControlMsg::ReleaseControl,
                    Some(mineshare_input::RemoteEvent::RequestPeerExit) => ControlMsg::ForceRelease,
                    None => break,
                },
                clip = clip_rx.recv() => match clip {
                    Some(text) => ControlMsg::ClipboardText(text),
                    None => break,
                },
            };
            if let Err(e) = write_encrypted(&mut tcp_write, &aead_writer, &msg).await {
                warn!(error = %e, "control writer failed — peer probably disconnected");
                break;
            }
            debug!(?msg, "sent ControlMsg");
        }
    });

    // Reader task: peer's ControlMsg → coordination state updates.
    // Returns when the TCP control channel closes — the main loop
    // watches this handle in `select!` so it can shut the whole peer
    // session down (forward loop, UDP recv, stats) and let the caller's
    // reconnect loop run.
    let aead_reader = aead.clone_handle();
    let inject_for_reader = inject.clone();
    let reader_handle = tokio::spawn(async move {
        loop {
            match read_encrypted::<_, ControlMsg>(&mut tcp_read, &aead_reader).await {
                Ok(ControlMsg::TakeControl) => {
                    info!("peer took Remote control");
                    mineshare_input::set_peer_in_remote(true);
                    // Warp our cursor to the boundary edge so the peer's
                    // virt_x model lines up with the real cursor position
                    // — otherwise their exit threshold fires after a tiny
                    // rightward motion even though the cursor is mid-screen.
                    mineshare_input::on_peer_take_control(&*inject_for_reader);
                }
                Ok(ControlMsg::ReleaseControl) => {
                    info!("peer released Remote control");
                    mineshare_input::set_peer_in_remote(false);
                }
                Ok(ControlMsg::ForceRelease) => {
                    info!("peer asked us to release Remote");
                    mineshare_input::force_local_exit_remote();
                }
                Ok(ControlMsg::ClipboardText(text)) => {
                    if let Err(e) = crate::clipboard::apply_from_peer(&text) {
                        warn!(error = %e, "failed to apply peer clipboard");
                    }
                }
                Err(e) => {
                    debug!(error = %e, "control reader ended");
                    break;
                }
            }
        }
    });

    let udp = Arc::new(udp);
    let stats = Arc::new(SessionStats::default());

    // --- recv → inject / playback ---------------------------------------
    let aead_recv = aead.clone_handle();
    let udp_recv = udp.clone();
    let inject_recv = inject.clone();
    let playback_recv = playback.clone();
    let mic_playback_recv = mic_playback.clone();
    let stats_recv = stats.clone();
    let recv_handle = tokio::spawn(async move {
        // Buffer must fit the largest WireFrame: input events are
        // tiny but Opus frames + AEAD tag + bincode framing top out
        // around 1.5 KB. 4 KB leaves comfortable headroom.
        let mut buf = vec![0u8; 4096];
        loop {
            match udp_recv.recv_from(&mut buf).await {
                Ok((n, src)) if src == peer_udp => {
                    stats_recv.recv_pkts.fetch_add(1, Ordering::Relaxed);
                    stats_recv.recv_bytes.fetch_add(n as u64, Ordering::Relaxed);
                    match aead_recv.open(&buf[..n]) {
                        Ok(pt) => {
                            match bincode::serde::decode_from_slice::<WireFrame, _>(
                                &pt,
                                standard(),
                            ) {
                                Ok((WireFrame::Input(ev), _)) => {
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
                                Ok((WireFrame::Audio(frame), _)) => {
                                    // Arm the echo-loop guard ONLY on peer
                                    // sysout arrivals. Mic frames from the
                                    // peer don't cause sysout↔sysout
                                    // feedback — including them here makes
                                    // every keyboard click / breath the peer
                                    // makes briefly mute our outgoing sysout,
                                    // which the user perceives as stutter on
                                    // shared music or video. Filter out
                                    // Opus DTX comfort noise (≤ 3 bytes) so
                                    // the always-on silence stream doesn't
                                    // permanently suppress us.
                                    if matches!(
                                        frame.stream,
                                        mineshare_audio::StreamKind::SysOut
                                    ) && frame.opus.len() >= ECHO_TRIGGER_MIN_BYTES
                                    {
                                        LAST_PEER_SYSOUT_AT_MS
                                            .store(now_ms(), Ordering::Relaxed);
                                    }
                                    let n = stats_recv.audio_recv.load(Ordering::Relaxed);
                                    if n % 250 == 0 {
                                        tracing::info!(
                                            seq = frame.seq,
                                            stream = ?frame.stream,
                                            opus_bytes = frame.opus.len(),
                                            n,
                                            "sample audio frame"
                                        );
                                    }
                                    // Route by stream kind: SysOut → the
                                    // default output device (peer's
                                    // speakers); Mic → the virtual-mic
                                    // sink (PipeWire null-sink on Linux,
                                    // VB-CABLE on Windows) so apps see
                                    // it as an input device without
                                    // mixing into the speaker output.
                                    let target = match frame.stream {
                                        mineshare_audio::StreamKind::SysOut => &playback_recv,
                                        mineshare_audio::StreamKind::Mic => &mic_playback_recv,
                                    };
                                    if let Err(e) = target.enqueue(frame) {
                                        warn!(error = %e, "audio enqueue failed");
                                    } else {
                                        stats_recv.audio_recv.fetch_add(1, Ordering::Relaxed);
                                    }
                                }
                                Err(e) => warn!(error = %e, "wireframe decode failed"),
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
    let stats_handle = tokio::spawn(async move {
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
                    audio_recv = delta.audio_recv,
                    "1-Hz stats"
                );
            }
            prev = curr;
        }
    });

    // --- capture broadcast → send ---------------------------------------
    // We pin the reader handle so `select!` can poll it without consuming
    // it; when the TCP control channel closes the reader returns and we
    // break out, tearing down every other task in the session so the
    // caller's reconnect loop can redial.
    let mut sub = bcast.subscribe();
    tokio::pin!(reader_handle);
    let exit_reason = loop {
        tokio::select! {
            biased;
            _ = &mut reader_handle => {
                break "TCP control reader ended";
            }
            recv = sub.recv() => match recv {
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
                        break "UDP send error";
                    }
                    stats.sent_pkts.fetch_add(1, Ordering::Relaxed);
                    stats.sent_bytes.fetch_add(len as u64, Ordering::Relaxed);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "broadcast subscriber lagged — events dropped");
                }
                Err(broadcast::error::RecvError::Closed) => break "broadcast closed",
            },
        }
    };

    info!(reason = exit_reason, %peer_addr, "peer session ending");
    writer_handle.abort();
    recv_handle.abort();
    stats_handle.abort();
    // Reset cross-session coordination state so the next handshake
    // doesn't inherit a stale belief that the peer holds Remote.
    mineshare_input::set_peer_in_remote(false);
    mineshare_input::clear_remote_event_sender();
    crate::status::clear_peer_connected();
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
    audio_recv: AtomicU64,
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
    audio_recv: u64,
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
            audio_recv: self.audio_recv.load(Ordering::Relaxed),
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
            audio_recv: self.audio_recv.saturating_sub(prev.audio_recv),
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

/// No-op playback used when audio output isn't available (no default
/// device, or running headless). Drops every frame silently so the
/// daemon stays useful for input-only setups.
struct NullPlayback;

impl AudioPlayback for NullPlayback {
    fn enqueue(&self, _frame: AudioFrame) -> Result<()> {
        Ok(())
    }
}
