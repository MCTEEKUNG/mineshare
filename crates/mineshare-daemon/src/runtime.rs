//! Daemon runtime — what `mineshare-daemon run` (default) does.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use mineshare_core::DeviceId;
use mineshare_net::{Discovery, DiscoveryEvent, Initiator, PeerAdvert, Responder};
use parking_lot::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::identity::Identity;
use crate::logs;

const DEFAULT_CONTROL_PORT: u16 = 0; // 0 = OS-assigned

pub async fn run() -> Result<()> {
    logs::init()?;

    let identity = Identity::load_or_create().context("identity bootstrap failed")?;
    info!(device_id = %identity.device_id, name = %identity.display_name, os = %identity.os, "MineShare daemon starting");

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
    tokio::spawn(async move {
        accept_loop(listener, resp_static).await;
    });

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

                // Tie-break: only the device with smaller id initiates,
                // to avoid both sides connecting simultaneously.
                if local_id.0 < peer.device_id.0 {
                    let init_static = init_static.clone();
                    tokio::spawn(async move {
                        if let Err(e) = try_handshake(&peer, &init_static).await {
                            warn!(peer = %peer.device_id, error = %e, "handshake to peer failed");
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

async fn accept_loop(listener: TcpListener, static_priv: Vec<u8>) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(%addr, "incoming connection");
                let key = static_priv.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_incoming(stream, &key).await {
                        warn!(%addr, error = %e, "incoming handshake failed");
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

async fn handle_incoming(mut stream: TcpStream, static_priv: &[u8]) -> Result<()> {
    let peer_addr = stream.peer_addr()?;
    let mut resp = Responder::new(static_priv)?;
    let peer_pub = resp.handshake(&mut stream).await?;
    info!(
        %peer_addr,
        peer_pub = %hex_short(&peer_pub),
        "inbound Noise XX handshake completed"
    );
    Ok(())
}

async fn try_handshake(peer: &PeerAdvert, static_priv: &[u8]) -> Result<()> {
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
    let peer_pub = init.handshake(&mut stream).await?;
    info!(
        peer = %peer.device_id,
        peer_pub = %hex_short(&peer_pub),
        "outbound Noise XX handshake completed"
    );
    Ok(())
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
