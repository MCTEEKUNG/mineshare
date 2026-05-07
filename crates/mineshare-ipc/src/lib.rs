//! mineshare-ipc
//!
//! GUI <-> daemon protocol over JSON line-delimited messages.
//! M0: simple request/response shape; transport switched later (Tauri command vs socket).

use mineshare_core::DeviceId;
use mineshare_net::PeerAdvert;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcRequest {
    GetStatus,
    ListPeers,
    Pair { device_id: DeviceId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcResponse {
    Status {
        device_id: DeviceId,
        display_name: String,
        os: String,
    },
    Peers {
        peers: Vec<PeerAdvert>,
    },
    Ok,
    Error {
        message: String,
    },
}
