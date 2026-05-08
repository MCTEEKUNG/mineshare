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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerSide {
    Left,
    Right,
    Top,
    Bottom,
}

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

/// Returns the in-memory current layout, lazily loading from disk
/// on first call. Subsequent calls hit the cached value.
pub fn current() -> LayoutConfig {
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
/// Layout page.
pub fn set(cfg: LayoutConfig) -> Result<()> {
    save(&cfg)?;
    mineshare_input::set_peer_side(map_side(cfg.peer_side));
    *CURRENT.lock() = Some(cfg);
    Ok(())
}

fn map_side(side: PeerSide) -> mineshare_input::PeerSide {
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
    match serde_json::from_slice::<LayoutConfig>(&bytes) {
        Ok(cfg) => cfg,
        Err(_) => LayoutConfig::default(),
    }
}

fn save(cfg: &LayoutConfig) -> Result<()> {
    let path = config_path()?;
    let json = serde_json::to_vec_pretty(cfg)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
