//! User preferences that persist between sessions.
//!
//! Stage 10 — game-compatibility polish — introduces a small set
//! of knobs the user can dial in from the GUI: mouse-delta scale
//! (so a high-DPI machine driving a low-DPI peer can be sped up
//! or slowed down without changing OS settings), and scroll-wheel
//! inversion (cross-OS scroll direction is a notorious cross-OS
//! gripe that nobody agrees on).
//!
//! Persisted as JSON next to `layout.json`. Loaded once at daemon
//! startup and pushed into `mineshare-input` static state; live
//! edits go via `apply` which mutates both the on-disk file *and*
//! the input-layer atomics atomically (well, racily-but-fine, the
//! input layer just reads atomics on each event).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Multiplier applied to forwarded mouse deltas (Stage 10).
    /// 1.0 == identity. Capped at 0.25..=4.0 on the way in. Useful
    /// when the local + peer screens have wildly different DPIs:
    /// driving a 1080p Ubuntu from a 200%-scaled 2880×1800 Windows
    /// laptop feels too fast at 1.0, dialling down to ~0.6 makes
    /// the peer cursor feel like a native 1× mouse.
    pub mouse_sensitivity: f32,
    /// If true, flip the sign of vertical scroll deltas before
    /// they leave this machine. Use this when scrolling feels
    /// upside-down on the peer (Windows-vs-macOS-style "natural
    /// scroll" mismatch).
    pub invert_scroll_y: bool,
    /// Same idea for horizontal scroll — rarer but worth offering
    /// since trackpad horizontal scroll tends to feel inverted
    /// across the bridge too.
    pub invert_scroll_x: bool,
    /// Phase 2 keyboard-focus fix: when the peer drives our
    /// cursor across, fire a synthetic left-click in place right
    /// after the cursor warp so whatever window is under the
    /// cursor gets keyboard focus. Default OFF because the click
    /// really does click — buttons / drag handles get hit. Turn
    /// ON if you're on GNOME-Wayland (Ubuntu default) and your
    /// keystrokes from the peer keep vanishing into windows that
    /// don't have focus.
    #[serde(default)]
    pub auto_focus_on_take_control: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 1.0,
            invert_scroll_y: false,
            invert_scroll_x: false,
            auto_focus_on_take_control: false,
        }
    }
}

impl Settings {
    /// Clamp wild user input into a sane range. The slider in the
    /// GUI already enforces this; we re-clamp on load too in case
    /// somebody hand-edited the JSON to something nonsensical.
    pub fn clamped(self) -> Self {
        Self {
            mouse_sensitivity: self.mouse_sensitivity.clamp(0.25, 4.0),
            ..self
        }
    }
}

fn config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("no config dir resolved")?;
    Ok(dir.join("MineShare").join("settings.json"))
}

/// Load preferences from disk, falling back to defaults if the
/// file is missing or malformed (we don't want a busted edit to
/// brick the daemon — better to log and start clean).
pub fn load() -> Settings {
    match config_path().and_then(read_file) {
        Ok(s) => s.clamped(),
        Err(_) => Settings::default(),
    }
}

fn read_file(path: PathBuf) -> Result<Settings> {
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let s: Settings = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(s)
}

/// Apply a fresh settings struct: clamp it, push it into the
/// input-layer atomics so the next inject picks up the new values
/// immediately, and persist to disk.
pub fn apply(s: Settings) -> Result<Settings> {
    let clamped = s.clamped();
    push_to_input_layer(&clamped);
    save(&clamped)?;
    Ok(clamped)
}

fn save(s: &Settings) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create settings dir")?;
    }
    let bytes = serde_json::to_vec_pretty(s).context("serialize settings")?;
    std::fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Called by `runtime::run` once at startup so the input layer
/// reflects the persisted prefs from the very first event.
pub fn install_loaded() {
    let s = load();
    push_to_input_layer(&s);
}

fn push_to_input_layer(s: &Settings) {
    mineshare_input::set_mouse_sensitivity(s.mouse_sensitivity);
    mineshare_input::set_invert_scroll(s.invert_scroll_x, s.invert_scroll_y);
    mineshare_input::set_auto_focus_on_take_control(s.auto_focus_on_take_control);
}
