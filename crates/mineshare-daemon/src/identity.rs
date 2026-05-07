//! Persistent device identity: stable DeviceId + Noise XX static keypair.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use mineshare_core::DeviceId;
use mineshare_net::pairing;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    pub device_id: DeviceId,
    pub display_name: String,
    pub os: String,
    /// Noise XX static private key (32 bytes for X25519)
    pub noise_static_priv: Vec<u8>,
    pub noise_static_pub: Vec<u8>,
}

impl Identity {
    pub fn load_or_create() -> Result<Self> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        if path.exists() {
            let raw = fs::read_to_string(&path).context("read identity")?;
            let id: Self = serde_json::from_str(&raw).context("parse identity")?;
            info!(path = %path.display(), "loaded existing identity");
            Ok(id)
        } else {
            let kp = pairing::generate_static_key()?;
            let id = Self {
                device_id: DeviceId::new(),
                display_name: hostname::get()
                    .ok()
                    .and_then(|h| h.into_string().ok())
                    .unwrap_or_else(|| "mineshare".to_string()),
                os: detect_os(),
                noise_static_priv: kp.private,
                noise_static_pub: kp.public,
            };
            let raw = serde_json::to_string_pretty(&id)?;
            fs::write(&path, raw).context("write identity")?;
            info!(path = %path.display(), "created new identity");
            Ok(id)
        }
    }
}

fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no OS config dir")?;
    Ok(base.join("MineShare").join("identity.json"))
}

fn detect_os() -> String {
    if cfg!(target_os = "windows") {
        "windows".to_string()
    } else if cfg!(target_os = "linux") {
        "linux".to_string()
    } else if cfg!(target_os = "macos") {
        "macos".to_string()
    } else {
        std::env::consts::OS.to_string()
    }
}
