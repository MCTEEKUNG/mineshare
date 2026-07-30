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

static SETTINGS_IO: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

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
    /// Capture-side multiplier for two-finger touchpad translation and
    /// legacy scroll-wheel deltas. Native pinch geometry and 3/4-finger
    /// actions are deliberately left unchanged.
    #[serde(default = "default_touchpad_scroll_speed")]
    pub touchpad_scroll_speed: f32,
    /// Focus the window under the remote cursor immediately before the first
    /// remote key-down after a handoff. This avoids clicking the boundary at
    /// take-control time and makes keyboard focus follow the user's intent.
    #[serde(default)]
    pub auto_focus_on_take_control: bool,
    /// Target mouse forward/inject rate in Hz. Drives the runtime
    /// flush interval in `mineshare-input` (both capture-forward on
    /// Windows and inject-coalesce on Linux). Clamped 60..=1000.
    #[serde(default = "default_mouse_rate_hz")]
    pub mouse_rate_hz: u32,
    /// Device used to render peer system audio. `None` follows the OS default.
    #[serde(default)]
    pub audio_output_device: Option<String>,
    /// Device used to capture the local microphone.
    #[serde(default)]
    pub audio_input_device: Option<String>,
    /// Windows render endpoint whose mixer is captured through WASAPI
    /// loopback. Kept separate from peer playback to avoid accidental
    /// feedback routing.
    #[serde(default)]
    pub sysout_capture_device: Option<String>,
    /// Persisted audio direction policy. Keeping these in settings makes a
    /// one-way sysout route survive restarts instead of reopening a feedback
    /// path every time the app launches.
    #[serde(default = "default_true")]
    pub audio_send_sysout: bool,
    #[serde(default = "default_true")]
    pub audio_play_sysout: bool,
    #[serde(default = "default_true")]
    pub audio_send_mic: bool,
    #[serde(default = "default_true")]
    pub audio_play_mic: bool,
}

fn default_mouse_rate_hz() -> u32 {
    500
}

fn default_touchpad_scroll_speed() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 1.0,
            invert_scroll_y: false,
            invert_scroll_x: false,
            touchpad_scroll_speed: default_touchpad_scroll_speed(),
            auto_focus_on_take_control: false,
            mouse_rate_hz: 500,
            audio_output_device: None,
            audio_input_device: None,
            sysout_capture_device: None,
            audio_send_sysout: true,
            audio_play_sysout: true,
            audio_send_mic: true,
            audio_play_mic: true,
        }
    }
}

impl Settings {
    /// Clamp wild user input into a sane range. The slider in the
    /// GUI already enforces this; we re-clamp on load too in case
    /// somebody hand-edited the JSON to something nonsensical.
    pub fn clamped(self) -> Self {
        let touchpad_scroll_speed = if self.touchpad_scroll_speed.is_finite() {
            self.touchpad_scroll_speed.clamp(0.25, 4.0)
        } else {
            default_touchpad_scroll_speed()
        };
        Self {
            mouse_sensitivity: self.mouse_sensitivity.clamp(0.25, 4.0),
            touchpad_scroll_speed,
            mouse_rate_hz: self.mouse_rate_hz.clamp(60, 1000),
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
    let s: Settings =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    Ok(s)
}

/// Apply a fresh settings struct: clamp it, push it into the
/// input-layer atomics so the next inject picks up the new values
/// immediately, and persist to disk.
pub fn apply(s: Settings) -> Result<Settings> {
    let _io = SETTINGS_IO.lock();
    let clamped = s.clamped();
    push_to_input_layer(&clamped);
    push_to_audio_layer(&clamped);
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
    push_to_audio_layer(&s);
}

fn push_to_input_layer(s: &Settings) {
    mineshare_input::set_mouse_sensitivity(s.mouse_sensitivity);
    mineshare_input::set_touchpad_scroll_speed(s.touchpad_scroll_speed);
    mineshare_input::set_invert_scroll(s.invert_scroll_x, s.invert_scroll_y);
    mineshare_input::set_auto_focus_on_take_control(s.auto_focus_on_take_control);
    mineshare_input::set_mouse_rate_hz(s.mouse_rate_hz);
}

fn push_to_audio_layer(s: &Settings) {
    mineshare_audio::set_output_device(s.audio_output_device.clone());
    mineshare_audio::set_input_device(s.audio_input_device.clone());
    mineshare_audio::set_sysout_capture_device(s.sysout_capture_device.clone());
    crate::audio_status::set_send_sysout(s.audio_send_sysout);
    crate::audio_status::set_play_sysout(s.audio_play_sysout);
    crate::audio_status::set_send_mic(s.audio_send_mic);
    crate::audio_status::set_play_mic(s.audio_play_mic);
}

/// Persist and apply peer-audio playback routing without requiring callers to
/// understand or rewrite unrelated settings.
pub fn set_audio_output_device(name: Option<String>) -> Result<()> {
    let _io = SETTINGS_IO.lock();
    let mut s = load();
    s.audio_output_device = name;
    push_to_audio_layer(&s);
    save(&s)
}

/// Persist and apply microphone capture routing.
pub fn set_audio_input_device(name: Option<String>) -> Result<()> {
    let _io = SETTINGS_IO.lock();
    let mut s = load();
    s.audio_input_device = name;
    push_to_audio_layer(&s);
    save(&s)
}

/// Persist the system-audio capture endpoint. The running WASAPI capture
/// observes the version change and rebuilds its stream.
pub fn set_sysout_capture_device(name: Option<String>) -> Result<()> {
    let _io = SETTINGS_IO.lock();
    let mut s = load();
    s.sysout_capture_device = name;
    push_to_audio_layer(&s);
    save(&s)
}

/// Persist and apply one audio direction toggle.
pub fn set_audio_toggle(stream: &str, direction: &str, enabled: bool) -> Result<()> {
    let _io = SETTINGS_IO.lock();
    let mut s = load();
    match (stream, direction) {
        ("sysout", "send") => s.audio_send_sysout = enabled,
        ("sysout", "play") => s.audio_play_sysout = enabled,
        ("mic", "send") => s.audio_send_mic = enabled,
        ("mic", "play") => s.audio_play_mic = enabled,
        _ => anyhow::bail!("unknown audio toggle: {stream}/{direction}"),
    }
    push_to_audio_layer(&s);
    save(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_rate_clamps_into_range() {
        let lo = Settings {
            mouse_rate_hz: 5,
            ..Settings::default()
        }
        .clamped();
        let hi = Settings {
            mouse_rate_hz: 9000,
            ..Settings::default()
        }
        .clamped();
        assert_eq!(lo.mouse_rate_hz, 60);
        assert_eq!(hi.mouse_rate_hz, 1000);
        assert_eq!(Settings::default().mouse_rate_hz, 500);
        assert_eq!(
            Settings::default().auto_focus_on_take_control,
            false,
            "keyboard handoff must never synthesize a click by default"
        );
    }

    #[test]
    fn touchpad_scroll_speed_defaults_and_clamps() {
        let lo = Settings {
            touchpad_scroll_speed: 0.01,
            ..Settings::default()
        }
        .clamped();
        let hi = Settings {
            touchpad_scroll_speed: 99.0,
            ..Settings::default()
        }
        .clamped();
        let invalid = Settings {
            touchpad_scroll_speed: f32::NAN,
            ..Settings::default()
        }
        .clamped();
        assert_eq!(lo.touchpad_scroll_speed, 0.25);
        assert_eq!(hi.touchpad_scroll_speed, 4.0);
        assert_eq!(invalid.touchpad_scroll_speed, 1.0);
        assert_eq!(Settings::default().touchpad_scroll_speed, 1.0);
    }

    #[test]
    fn legacy_settings_without_audio_routing_remain_compatible() {
        let legacy = r#"{
            "mouse_sensitivity": 1.0,
            "invert_scroll_y": false,
            "invert_scroll_x": false,
            "auto_focus_on_take_control": false,
            "mouse_rate_hz": 500
        }"#;
        let parsed: Settings = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.audio_output_device, None);
        assert_eq!(parsed.audio_input_device, None);
        assert_eq!(parsed.sysout_capture_device, None);
        assert_eq!(parsed.touchpad_scroll_speed, 1.0);
        assert!(parsed.audio_send_sysout);
        assert!(parsed.audio_play_sysout);
        assert!(parsed.audio_send_mic);
        assert!(parsed.audio_play_mic);
    }
}
