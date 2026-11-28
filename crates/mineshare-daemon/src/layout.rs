//! User-configurable monitor layout — which side of the local
//! display is the peer "stuck to". The bridge's edge-detection
//! logic uses this to decide which edge of our screen triggers
//! entry into Remote mode and where to warp the cursor on
//! TakeControl.
//!
//! M5 Slice 2 ships the persistence + Tauri commands; the actual
//! input-side conditional logic lands in 2b. Until then changing
//! the side via the GUI updates the config file but the bridge
//! keeps using the hardcoded Win-left / Ubuntu-right convention.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

pub use mineshare_protocol::PeerSide;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutConfig {
    pub peer_side: PeerSide,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        // Matches the hardcoded assumption from M0–M4 (Win on the
        // left, Ubuntu on the right). The peer thus sits to the
        // right of *us* on Win, and to the left of *us* on Linux.
        // The default differs by OS so a fresh install matches the
        // typical dev rig without the user touching the GUI.
        Self {
            #[cfg(target_os = "windows")]
            peer_side: PeerSide::Right,
            #[cfg(target_os = "linux")]
            peer_side: PeerSide::Left,
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            peer_side: PeerSide::Right,
        }
    }
}

static CURRENT: Mutex<Option<LayoutConfig>> = Mutex::new(None);

/// Per-peer-session sender that the runtime writer task drains and
/// sends as `ControlMsg::SetPeerSide` over the encrypted control
/// channel. Set when a session comes up, cleared when it tears
/// down. We push the *opposite* of our local side here whenever
/// the user flips it via the GUI — the peer then applies that
/// opposite side via `apply_from_peer` so both ends agree on the
/// desk layout in one click.
static PROPAGATE_TX: Mutex<Option<UnboundedSender<PeerSide>>> = Mutex::new(None);

pub fn set_propagate_sender(tx: UnboundedSender<PeerSide>) {
    *PROPAGATE_TX.lock() = Some(tx);
}

pub fn clear_propagate_sender() {
    *PROPAGATE_TX.lock() = None;
}

/// Returns the in-memory current layout, lazily loading from disk
/// on first call. Subsequent calls hit the cached value.
pub fn current() -> LayoutConfig {
    let active = ACTIVE_PEER.lock().clone();
    if let Some(side) = active.and_then(|id| PEER_LAYOUT.lock().get(&id).copied()) {
        return LayoutConfig { peer_side: side };
    }
    legacy_current()
}
fn legacy_current() -> LayoutConfig {
    let mut guard = CURRENT.lock();
    if let Some(cfg) = guard.as_ref() {
        return cfg.clone();
    }
    let cfg = load_or_default();
    *guard = Some(cfg.clone());
    cfg
}

/// Replace the current layout, persisting to disk *and* pushing
/// the new side to `mineshare_input` so the bridge picks it up
/// immediately without a daemon restart. Called from the Tauri
/// `set_layout` command when the user drags-or-clicks in the
/// Layout page. ALSO propagates the mirror side to the peer over
/// the active control channel so both machines agree on the desk
/// arrangement after one click.
pub fn set(cfg: LayoutConfig) -> Result<()> {
    let active = ACTIVE_PEER.lock().clone();
    if let Some(id) = active {
        return set_for_peer(&id, cfg.peer_side);
    }
    apply_locally(&cfg, true)
}

/// Apply a layout received from the peer — same thing as `set` but
/// without echoing it back over the control channel. Avoids an
/// infinite ping-pong of SetPeerSide messages.
pub fn apply_from_peer(cfg: LayoutConfig) -> Result<()> {
    apply_locally(&cfg, false)
}

fn apply_locally(cfg: &LayoutConfig, propagate: bool) -> Result<()> {
    let mut current = CURRENT.lock();
    save(cfg)?;
    mineshare_input::set_peer_side(map_side(cfg.peer_side));
    *current = Some(cfg.clone());
    if propagate && let Some(tx) = PROPAGATE_TX.lock().as_ref() {
        let _ = tx.send(cfg.peer_side.opposite());
    }
    Ok(())
}

pub(crate) fn map_side(side: PeerSide) -> mineshare_input::PeerSide {
    match side {
        PeerSide::Left => mineshare_input::PeerSide::Left,
        PeerSide::Right => mineshare_input::PeerSide::Right,
        PeerSide::Top => mineshare_input::PeerSide::Top,
        PeerSide::Bottom => mineshare_input::PeerSide::Bottom,
    }
}

fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no OS config dir")?;
    let dir = base.join("MineShare");
    fs::create_dir_all(&dir).ok();
    Ok(dir.join("layout.json"))
}

fn load_or_default() -> LayoutConfig {
    let path = match config_path() {
        Ok(p) => p,
        Err(_) => return LayoutConfig::default(),
    };
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return LayoutConfig::default(),
    };
    serde_json::from_slice::<LayoutConfig>(&bytes).unwrap_or_default()
}

fn save(cfg: &LayoutConfig) -> Result<()> {
    let path = config_path()?;
    let json = serde_json::to_vec_pretty(cfg)?;
    crate::settings::atomic_write_file(&path, &json)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

static ACTIVE_PEER: Mutex<Option<String>> = Mutex::new(None);
static PEER_LAYOUT: std::sync::LazyLock<Mutex<std::collections::HashMap<String, PeerSide>>> =
    std::sync::LazyLock::new(|| {
        let map = config_path()
            .ok()
            .and_then(|p| fs::read(p.with_file_name("peer-layout.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Mutex::new(map)
    });
pub fn side_for_peer(device_id: &str) -> PeerSide {
    PEER_LAYOUT
        .lock()
        .get(device_id)
        .copied()
        .unwrap_or_else(|| legacy_current().peer_side)
}
pub(crate) fn activate_peer(device_id: &str) {
    let side = side_for_peer(device_id);
    *ACTIVE_PEER.lock() = Some(device_id.into());
    mineshare_input::set_peer_side(map_side(side));
}
pub fn set_for_peer(device_id: &str, side: PeerSide) -> Result<()> {
    anyhow::ensure!(
        uuid::Uuid::parse_str(device_id).is_ok(),
        "invalid device ID"
    );
    let mut map = PEER_LAYOUT.lock();
    let mut next = map.clone();
    next.insert(device_id.into(), side);
    crate::settings::atomic_write_file(
        &config_path()?.with_file_name("peer-layout.json"),
        &serde_json::to_vec_pretty(&next)?,
    )?;
    *map = next;
    drop(map);
    if ACTIVE_PEER.lock().as_deref() == Some(device_id) {
        mineshare_input::set_peer_side(map_side(side));
    }
    crate::runtime::push_peer_side(device_id, side.opposite());
    Ok(())
}
pub(crate) fn apply_for_peer(device_id: &str, side: PeerSide) -> Result<()> {
    let mut map = PEER_LAYOUT.lock();
    let mut next = map.clone();
    next.insert(device_id.into(), side);
    crate::settings::atomic_write_file(
        &config_path()?.with_file_name("peer-layout.json"),
        &serde_json::to_vec_pretty(&next)?,
    )?;
    *map = next;
    if ACTIVE_PEER.lock().as_deref() == Some(device_id) {
        mineshare_input::set_peer_side(map_side(side));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn existing_layout_api_follows_active_device_and_unsaved_devices_use_legacy_side() {
        let _serial = crate::pairing::TEST_LOCK.blocking_lock();
        let previous_active = ACTIVE_PEER.lock().take();
        let previous_map = std::mem::take(&mut *PEER_LAYOUT.lock());
        PEER_LAYOUT
            .lock()
            .insert("test-phone".into(), PeerSide::Bottom);
        *ACTIVE_PEER.lock() = Some("test-phone".into());
        assert_eq!(current().peer_side, PeerSide::Bottom);
        *ACTIVE_PEER.lock() = Some("test-unsaved".into());
        let legacy = legacy_current().peer_side;
        assert_eq!(current().peer_side, legacy);
        assert_eq!(side_for_peer("test-unsaved"), legacy);
        *PEER_LAYOUT.lock() = previous_map;
        *ACTIVE_PEER.lock() = previous_active;
    }
}
