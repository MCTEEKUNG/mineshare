//! Persistent allowlist of peers we've completed PIN-pairing with.
//!
//! After a successful 6-digit PIN exchange we save the peer's
//! Noise XX static public key + their device_id / display_name.
//! On every subsequent handshake we look the peer's static up
//! here — if it's listed, we skip the PIN prompt entirely. New
//! peers (anyone else on the LAN that runs `mineshare-daemon`)
//! are blocked at the pairing step until their human enters the
//! PIN.
//!
//! The list lives in plaintext at
//! `<config_dir>/MineShare/trusted_peers.json`. The Noise XX
//! static public key is the actual security boundary — it's a
//! 32-byte X25519 pubkey baked into the peer's identity.json
//! (which is currently plaintext too; encrypting it via the OS
//! keyring is Stage 7.3 polish).

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedPeer {
    pub device_id: String,
    pub display_name: String,
    /// Hex-encoded Noise XX static public key (32 bytes → 64
    /// hex chars). The key is the source of truth for trust:
    /// matching device_id with a different pubkey means an
    /// imposter and we re-prompt for PIN.
    pub noise_static_hex: String,
    /// Unix timestamp of the successful pairing, for display
    /// only.
    pub paired_at: i64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TrustList {
    trusted: Vec<TrustedPeer>,
}

static CACHED: Mutex<Option<TrustList>> = Mutex::new(None);

fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no OS config dir")?;
    let dir = base.join("MineShare");
    fs::create_dir_all(&dir).ok();
    Ok(dir.join("trusted_peers.json"))
}

fn load_or_default() -> TrustList {
    config_path()
        .ok()
        .and_then(|p| fs::read(&p).ok())
        .and_then(|b| serde_json::from_slice::<TrustList>(&b).ok())
        .unwrap_or_default()
}

fn save(list: &TrustList) -> Result<()> {
    let path = config_path()?;
    fs::write(&path, serde_json::to_vec_pretty(list)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn ensure_loaded(g: &mut parking_lot::MutexGuard<Option<TrustList>>) {
    if g.is_none() {
        **g = Some(load_or_default());
    }
}

pub fn is_trusted(noise_static: &[u8]) -> bool {
    let hex = encode_hex(noise_static);
    let mut g = CACHED.lock();
    ensure_loaded(&mut g);
    g.as_ref()
        .unwrap()
        .trusted
        .iter()
        .any(|p| p.noise_static_hex == hex)
}

/// Insert (or replace) a trusted peer. De-duplicated by `device_id`
/// — a peer that re-issues their identity (lost the keypair, etc.)
/// gets their entry replaced with the new pubkey when they pair
/// again. Persisted immediately.
pub fn add_trusted(device_id: &str, display_name: &str, noise_static: &[u8]) -> Result<()> {
    let hex = encode_hex(noise_static);
    let mut g = CACHED.lock();
    ensure_loaded(&mut g);
    let list = g.as_mut().unwrap();
    list.trusted.retain(|p| p.device_id != device_id);
    list.trusted.push(TrustedPeer {
        device_id: device_id.to_string(),
        display_name: display_name.to_string(),
        noise_static_hex: hex,
        paired_at: now_unix(),
    });
    save(list)?;
    tracing::info!(device_id, display_name, "trusted peer recorded");
    Ok(())
}

pub fn list_trusted() -> Vec<TrustedPeer> {
    let mut g = CACHED.lock();
    ensure_loaded(&mut g);
    g.as_ref().unwrap().trusted.clone()
}

pub fn revoke(device_id: &str) -> Result<()> {
    let mut g = CACHED.lock();
    ensure_loaded(&mut g);
    let list = g.as_mut().unwrap();
    let before = list.trusted.len();
    list.trusted.retain(|p| p.device_id != device_id);
    if list.trusted.len() != before {
        save(list)?;
        tracing::info!(device_id, "trusted peer revoked");
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
