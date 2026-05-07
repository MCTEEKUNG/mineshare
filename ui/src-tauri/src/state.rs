//! M0 in-process app state: identity + discovery.
//!
//! Owns no input/audio yet. M4 will move this to a separate daemon process.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use mineshare_core::DeviceId;
use mineshare_ipc::IpcResponse;
use mineshare_net::{Discovery, DiscoveryEvent, PeerAdvert};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{debug, info};

type PeerMap = Arc<Mutex<HashMap<DeviceId, PeerAdvert>>>;

pub struct AppState {
    device_id: DeviceId,
    display_name: String,
    os: String,
    control_port: Mutex<Option<u16>>,
    peers: PeerMap,
}

impl AppState {
    pub fn bootstrap() -> Result<Self> {
        Ok(Self {
            device_id: DeviceId::new(),
            display_name: hostname_str(),
            os: detect_os(),
            control_port: Mutex::new(None),
            peers: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn status(&self) -> IpcResponse {
        IpcResponse::Status {
            device_id: self.device_id,
            display_name: self.display_name.clone(),
            os: self.os.clone(),
        }
    }

    pub fn peers(&self) -> Vec<PeerAdvert> {
        self.peers.lock().values().cloned().collect()
    }

    pub async fn start_discovery(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0))
            .await
            .context("bind control listener")?;
        let port = listener.local_addr()?.port();
        *self.control_port.lock() = Some(port);
        info!(port, "control listener bound (GUI in-process)");
        // listener not driven yet in M0 — daemon binary handles inbound handshakes
        drop(listener);

        let mut d = Discovery::new()?;
        let advert = PeerAdvert {
            device_id: self.device_id,
            display_name: self.display_name.clone(),
            os: self.os.clone(),
            control_port: port,
            addresses: detect_local_addresses(),
        };
        d.announce(&advert)?;

        let (tx, mut rx) = mpsc::channel::<DiscoveryEvent>(32);
        d.browse(tx)?;

        // Keep the Discovery handle alive for the program lifetime.
        std::mem::forget(d);

        let me = self.device_id;
        let peers = Arc::clone(&self.peers);
        tauri::async_runtime::spawn(async move {
            while let Some(evt) = rx.recv().await {
                match evt {
                    DiscoveryEvent::PeerOnline(p) => {
                        if p.device_id == me {
                            continue;
                        }
                        debug!(peer = %p.device_id, name = %p.display_name, "peer online");
                        peers.lock().insert(p.device_id, p);
                    }
                    DiscoveryEvent::PeerOffline(id) => {
                        peers.lock().remove(&id);
                    }
                }
            }
        });
        Ok(())
    }
}

fn hostname_str() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "mineshare".to_string())
}

fn detect_os() -> String {
    std::env::consts::OS.to_string()
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
