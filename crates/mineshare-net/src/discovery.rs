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
    /// Fully-qualified service name we registered (`<instance>.<type>`),
    /// needed to unregister (broadcast an mDNS goodbye) on shutdown.
    fullname: String,
}

impl Discovery {
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("failed to start mDNS daemon")?;
        Ok(Self {
            daemon,
            instance_name: String::new(),
            fullname: String::new(),
        })
    }

    /// Announce ourselves on the LAN. Re-callable to update info.
    pub fn announce(&mut self, advert: &PeerAdvert) -> Result<()> {
        let host = format!("{}.local.", short_id(&advert.device_id));
        let instance = format!("{}-{}", advert.display_name, short_id(&advert.device_id));
        self.instance_name = instance.clone();
        self.fullname = format!("{instance}.{SERVICE_TYPE}");

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
    ///
    /// Deduplication: on a multi-NIC host mdns_sd fires one `ServiceResolved`
    /// per receiving interface for the same remote service. We suppress the
    /// duplicates so `runtime.rs` never sees the same peer arrive twice and
    /// accidentally spawns two concurrent reconnect-loop tasks.
    ///
    /// PeerOffline: mdns_sd's `ServiceRemoved` only provides the fullname, not
    /// the device_id. We maintain a `fullname → DeviceId` map built from every
    /// `ServiceResolved` event so we can dispatch a proper `PeerOffline` when
    /// the last fullname entry for a device disappears.
    pub fn browse(&self, tx: mpsc::Sender<DiscoveryEvent>) -> Result<()> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .context("failed to start mDNS browse")?;

        let me = self.instance_name.clone();
        tokio::spawn(async move {
            // fullname → DeviceId  (populated on ServiceResolved)
            let mut fullname_to_id: HashMap<String, DeviceId> = HashMap::new();
            // DeviceId → last advert we forwarded as PeerOnline. We key on
            // the *endpoint* (control_port + address set), not mere
            // presence, so a peer that restarts with a new ephemeral port
            // or a new DHCP IP — same instance name — re-fires PeerOnline
            // carrying the NEW endpoint instead of being swallowed as a
            // duplicate. mdns-sd already re-emits ServiceResolved on an
            // SRV/A change; the old presence-only HashSet was discarding
            // it, which is what stranded the still-running peer on a dead
            // address and blocked auto-reconnect.
            let mut last_advert: HashMap<DeviceId, PeerAdvert> = HashMap::new();

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
                        let fullname = info.get_fullname().to_string();
                        if fullname.contains(&me) {
                            debug!(fullname, "ignoring self advertisement");
                            continue;
                        }
                        match peer_from_info(&info) {
                            Ok(p) => {
                                // Record fullname → id so ServiceRemoved can look it up.
                                fullname_to_id.insert(fullname.clone(), p.device_id);

                                // Forward when the endpoint is new OR changed.
                                // Suppress only byte-identical re-resolves (the
                                // genuine multi-NIC duplicate the old code targeted).
                                let changed = match last_advert.get(&p.device_id) {
                                    None => true,
                                    Some(prev) => !same_endpoint(prev, &p),
                                };
                                if changed {
                                    last_advert.insert(p.device_id, p.clone());
                                    let _ = tx.send(DiscoveryEvent::PeerOnline(p)).await;
                                } else {
                                    debug!(
                                        fullname,
                                        device_id = %p.device_id,
                                        "duplicate ServiceResolved — unchanged endpoint, skipping"
                                    );
                                }
                            }
                            Err(e) => warn!(error = %e, "skipping malformed advertisement"),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        if fullname.contains(&me) {
                            continue;
                        }
                        if let Some(id) = fullname_to_id.remove(&fullname) {
                            // Only dispatch PeerOffline when no other fullnames
                            // still reference this device (handles the case where
                            // the same service was resolved via multiple interfaces).
                            let still_active = fullname_to_id.values().any(|v| *v == id);
                            if !still_active {
                                last_advert.remove(&id);
                                let _ = tx.send(DiscoveryEvent::PeerOffline(id)).await;
                            }
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

    /// Build a self-contained closure that broadcasts the mDNS goodbye
    /// (TTL-0 unregister) for our registered service. `ServiceDaemon` is
    /// cheaply cloneable (Arc-backed), so the closure can be stashed in a
    /// global and fired from a different thread at process exit — e.g. the
    /// Tauri tray "Quit" handler before `app.exit(0)`, which otherwise
    /// hard-exits without unwinding the daemon runtime. The brief sleep
    /// lets the multicast actually leave the NIC before the process dies.
    ///
    /// Without this, a closing peer sends no goodbye and the other side
    /// only learns it left when the mDNS cache TTL expires (tens of
    /// seconds), keeping a stale advert that blocks reconnection.
    pub fn goodbye_fn(&self) -> impl Fn() + Send + Sync + 'static {
        let daemon = self.daemon.clone();
        let fullname = self.fullname.clone();
        move || {
            if fullname.is_empty() {
                return;
            }
            if let Ok(rx) = daemon.unregister(&fullname) {
                // Wait (bounded) for the unregister to be processed so the
                // goodbye packet is emitted before we return.
                let _ = rx.recv_timeout(Duration::from_millis(300));
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }
}

/// True when two adverts point at the same reachable endpoint — same
/// control port and the same set of addresses (order-independent).
/// `mdns-sd` can briefly union the old + new A records during an IP
/// change, so comparing as sorted sets avoids spurious "changed" churn
/// from address reordering while still detecting a real IP add/remove.
fn same_endpoint(a: &PeerAdvert, b: &PeerAdvert) -> bool {
    if a.control_port != b.control_port {
        return false;
    }
    if a.addresses.len() != b.addresses.len() {
        return false;
    }
    let mut x: Vec<IpAddr> = a.addresses.clone();
    let mut y: Vec<IpAddr> = b.addresses.clone();
    x.sort();
    y.sort();
    x == y
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

#[cfg(test)]
mod tests {
    use super::*;

    fn advert(port: u16, addrs: &[&str]) -> PeerAdvert {
        PeerAdvert {
            device_id: DeviceId(uuid::Uuid::nil()),
            display_name: "test".into(),
            os: "test".into(),
            control_port: port,
            addresses: addrs.iter().map(|a| a.parse().unwrap()).collect(),
        }
    }

    #[test]
    fn identical_endpoint_is_same() {
        // Byte-identical (the genuine multi-NIC duplicate) → suppress.
        assert!(same_endpoint(&advert(5000, &["192.168.1.5"]), &advert(5000, &["192.168.1.5"])));
    }

    #[test]
    fn address_order_does_not_matter() {
        // Same set, different order → still the same endpoint.
        assert!(same_endpoint(
            &advert(5000, &["192.168.1.5", "10.0.0.2"]),
            &advert(5000, &["10.0.0.2", "192.168.1.5"]),
        ));
    }

    #[test]
    fn changed_port_is_different() {
        // Ephemeral control port rotated on restart → must re-fire.
        assert!(!same_endpoint(&advert(5000, &["192.168.1.5"]), &advert(6001, &["192.168.1.5"])));
    }

    #[test]
    fn changed_ip_is_different() {
        // DHCP reassigned the host → must re-fire.
        assert!(!same_endpoint(&advert(5000, &["192.168.1.5"]), &advert(5000, &["192.168.1.9"])));
    }

    #[test]
    fn added_ip_during_dhcp_change_is_different() {
        // mdns-sd transiently unions old+new A records — the set grew,
        // so we treat it as changed and forward the (multi-address)
        // advert; dial_and_run then tries both and the live one wins.
        assert!(!same_endpoint(
            &advert(5000, &["192.168.1.5"]),
            &advert(5000, &["192.168.1.5", "192.168.1.9"]),
        ));
    }
}

