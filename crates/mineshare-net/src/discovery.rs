//! mDNS service discovery for MineShare peers.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use mineshare_core::DeviceId;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

pub const SERVICE_TYPE: &str = "_mineshare._tcp.local.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerAdvert {
    pub device_id: DeviceId,
    pub display_name: String,
    pub os: String,
    pub control_port: u16,
    pub addresses: Vec<IpAddr>,
}

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    PeerOnline(PeerAdvert),
    PeerOffline(DeviceId),
}

pub struct Discovery {
    daemon: ServiceDaemon,
    instance_name: String,
}

impl Discovery {
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("failed to start mDNS daemon")?;
        Ok(Self {
            daemon,
            instance_name: String::new(),
        })
    }

    /// Announce ourselves on the LAN. Re-callable to update info.
    pub fn announce(&mut self, advert: &PeerAdvert) -> Result<()> {
        let host = format!("{}.local.", short_id(&advert.device_id));
        let instance = format!("{}-{}", advert.display_name, short_id(&advert.device_id));
        self.instance_name = instance.clone();

        let mut props: HashMap<String, String> = HashMap::new();
        props.insert("device_id".into(), advert.device_id.to_string());
        props.insert("display_name".into(), advert.display_name.clone());
        props.insert("os".into(), advert.os.clone());

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance,
            &host,
            advert.addresses.as_slice(),
            advert.control_port,
            Some(props),
        )
        .context("invalid mDNS service info")?;

        self.daemon
            .register(info)
            .context("failed to register mDNS service")?;
        info!(instance, "announced mDNS service");
        Ok(())
    }

    /// Browse for peers. Sends events on the channel until the receiver is dropped.
    pub fn browse(&self, tx: mpsc::Sender<DiscoveryEvent>) -> Result<()> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .context("failed to start mDNS browse")?;

        let me = self.instance_name.clone();
        tokio::spawn(async move {
            loop {
                let evt = match tokio::task::spawn_blocking({
                    let receiver = receiver.clone();
                    move || receiver.recv_timeout(Duration::from_secs(3600))
                })
                .await
                {
                    Ok(Ok(e)) => e,
                    Ok(Err(_)) => continue, // timeout
                    Err(_) => break,
                };

                match evt {
                    ServiceEvent::ServiceResolved(info) => {
                        let fullname = info.get_fullname();
                        if fullname.contains(&me) {
                            debug!(fullname, "ignoring self advertisement");
                            continue;
                        }
                        match peer_from_info(&info) {
                            Ok(p) => {
                                let _ = tx.send(DiscoveryEvent::PeerOnline(p)).await;
                            }
                            Err(e) => warn!(error = %e, "skipping malformed advertisement"),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        if let Some(id) = parse_device_id_from_fullname(&fullname) {
                            let _ = tx.send(DiscoveryEvent::PeerOffline(id)).await;
                        }
                    }
                    _ => {}
                }
            }
        });
        Ok(())
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.shutdown();
    }
}

fn short_id(id: &DeviceId) -> String {
    id.to_string()
        .split('-')
        .next()
        .unwrap_or("unknown")
        .to_string()
}

fn peer_from_info(info: &mdns_sd::ServiceInfo) -> Result<PeerAdvert> {
    let props = info.get_properties();
    let device_id_str = props
        .get_property_val_str("device_id")
        .ok_or_else(|| anyhow::anyhow!("missing device_id"))?;
    let device_id = DeviceId(uuid::Uuid::parse_str(device_id_str)?);
    let display_name = props
        .get_property_val_str("display_name")
        .unwrap_or("(unknown)")
        .to_string();
    let os = props.get_property_val_str("os").unwrap_or("?").to_string();
    let control_port = info.get_port();
    let addresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();

    Ok(PeerAdvert {
        device_id,
        display_name,
        os,
        control_port,
        addresses,
    })
}

fn parse_device_id_from_fullname(_fullname: &str) -> Option<DeviceId> {
    // mDNS removed event provides only fullname; in real impl we maintain a map.
    None
}
