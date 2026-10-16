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
use std::io::Write;
use std::path::{Path, PathBuf};

static SETTINGS_IO: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub lock_effect: mineshare_input::LockEffect,
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
    #[serde(default)]
    pub audio_send_sysout: bool,
    #[serde(default)]
    pub audio_play_sysout: bool,
    #[serde(default)]
    pub audio_send_mic: bool,
    #[serde(default)]
    pub audio_play_mic: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsPatch {
    pub lock_effect: Option<mineshare_input::LockEffect>,
    pub mouse_sensitivity: Option<f32>,
    pub invert_scroll_y: Option<bool>,
    pub invert_scroll_x: Option<bool>,
    pub touchpad_scroll_speed: Option<f32>,
    pub auto_focus_on_take_control: Option<bool>,
    pub mouse_rate_hz: Option<u32>,
}

fn default_mouse_rate_hz() -> u32 {
    500
}

fn default_touchpad_scroll_speed() -> f32 {
    1.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            lock_effect: Default::default(),
            mouse_sensitivity: 1.0,
            invert_scroll_y: false,
            invert_scroll_x: false,
            touchpad_scroll_speed: default_touchpad_scroll_speed(),
            auto_focus_on_take_control: false,
            mouse_rate_hz: 500,
            audio_output_device: None,
            audio_input_device: None,
            sysout_capture_device: None,
            audio_send_sysout: false,
            audio_play_sysout: false,
            audio_send_mic: false,
            audio_play_mic: false,
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
            mouse_sensitivity: if self.mouse_sensitivity.is_finite() {
                self.mouse_sensitivity.clamp(0.25, 4.0)
            } else {
                1.0
            },
            touchpad_scroll_speed,
            lock_effect: self.lock_effect.clamped(),
            mouse_rate_hz: self.mouse_rate_hz.clamp(60, 1000),
            ..self
        }
    }
}

fn merge_patch(mut next: Settings, patch: SettingsPatch) -> Settings {
    if let Some(value) = patch.lock_effect {
        next.lock_effect = value;
    }
    if let Some(value) = patch.mouse_sensitivity {
        next.mouse_sensitivity = value;
    }
    if let Some(value) = patch.invert_scroll_y {
        next.invert_scroll_y = value;
    }
    if let Some(value) = patch.invert_scroll_x {
        next.invert_scroll_x = value;
    }
    if let Some(value) = patch.touchpad_scroll_speed {
        next.touchpad_scroll_speed = value;
    }
    if let Some(value) = patch.auto_focus_on_take_control {
        next.auto_focus_on_take_control = value;
    }
    if let Some(value) = patch.mouse_rate_hz {
        next.mouse_rate_hz = value;
    }
    next.clamped()
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

pub fn update(patch: SettingsPatch) -> Result<Settings> {
    let _io = SETTINGS_IO.lock();
    let merged = merge_patch(load(), patch);
    persist_then_apply(&merged, save, |settings| {
        push_to_input_layer(settings);
        push_to_audio_layer(settings);
    })?;
    Ok(merged)
}

fn save(s: &Settings) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create settings dir")?;
    }
    let bytes = serde_json::to_vec_pretty(s).context("serialize settings")?;
    atomic_write_file(&path, &bytes)?;
    Ok(())
}

pub(crate) fn atomic_write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("settings path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent).context("create settings parent")?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("settings file name is not valid UTF-8")?;
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));

    let result = (|| -> Result<()> {
        let mut temp = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("create temporary settings {}", temp_path.display()))?;
        temp.write_all(bytes)
            .with_context(|| format!("write temporary settings {}", temp_path.display()))?;
        temp.sync_all()
            .with_context(|| format!("flush temporary settings {}", temp_path.display()))?;
        drop(temp);
        atomic_replace(&temp_path, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(target_os = "windows")]
fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let moved = unsafe {
        MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("atomically replace {}", to.display()));
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn atomic_replace(from: &Path, to: &Path) -> Result<()> {
    std::fs::rename(from, to).with_context(|| format!("atomically replace {}", to.display()))?;
    if let Some(parent) = to.parent() {
        std::fs::File::open(parent)
            .and_then(|dir| dir.sync_all())
            .with_context(|| format!("flush settings directory {}", parent.display()))?;
    }
    Ok(())
}

/// Called by `runtime::run` once at startup so the input layer
/// reflects the persisted prefs from the very first event.
pub fn install_loaded() {
    install_loaded_serialized(&SETTINGS_IO, load, |settings| {
        push_to_input_layer(settings);
        push_to_audio_layer(settings);
    });
}

fn install_loaded_serialized(
    lock: &parking_lot::Mutex<()>,
    load_settings: impl FnOnce() -> Settings,
    apply: impl FnOnce(&Settings),
) {
    let _io = lock.lock();
    let settings = load_settings();
    apply(&settings);
}

fn push_to_input_layer(s: &Settings) {
    mineshare_input::set_lock_effect(s.lock_effect);
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

fn persist_then_apply(
    settings: &Settings,
    persist: impl FnOnce(&Settings) -> Result<()>,
    apply: impl FnOnce(&Settings),
) -> Result<()> {
    persist(settings)?;
    apply(settings);
    Ok(())
}

/// Persist and apply peer-audio playback routing without requiring callers to
/// understand or rewrite unrelated settings.
pub fn set_audio_output_device(name: Option<String>) -> Result<()> {
    let _io = SETTINGS_IO.lock();
    let mut s = load();
    s.audio_output_device = name;
    persist_then_apply(&s, save, push_to_audio_layer)
}

/// Persist and apply microphone capture routing.
pub fn set_audio_input_device(name: Option<String>) -> Result<()> {
    let _io = SETTINGS_IO.lock();
    let mut s = load();
    s.audio_input_device = name;
    persist_then_apply(&s, save, push_to_audio_layer)
}

/// Persist the system-audio capture endpoint. The running WASAPI capture
/// observes the version change and rebuilds its stream.
pub fn set_sysout_capture_device(name: Option<String>) -> Result<()> {
    let _io = SETTINGS_IO.lock();
    let mut s = load();
    s.sysout_capture_device = name;
    persist_then_apply(&s, save, push_to_audio_layer)
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
    persist_then_apply(&s, save, push_to_audio_layer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_mouse_sensitivity_never_reaches_input_scaling() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                Settings {
                    mouse_sensitivity: value,
                    ..Settings::default()
                }
                .clamped()
                .mouse_sensitivity,
                1.0
            );
        }
    }

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
        assert!(
            !Settings::default().auto_focus_on_take_control,
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
    fn lock_effect_patch_roundtrips_without_changing_input_or_audio() {
        let initial = Settings {
            mouse_sensitivity: 1.5,
            audio_play_sysout: true,
            ..Default::default()
        };
        let patch: SettingsPatch = serde_json::from_value(serde_json::json!({
            "lock_effect": { "rainbow": false, "color": [255, 30, 100], "animation": "pulse", "duration_ms": 9000, "thickness": 0 }
        })).unwrap();
        let result = merge_patch(initial, patch);
        assert_eq!(result.mouse_sensitivity, 1.5);
        assert!(result.audio_play_sysout);
        assert_eq!(result.lock_effect.duration_ms, 3000);
        assert_eq!(result.lock_effect.thickness, 2);
        let encoded = serde_json::to_value(&result).unwrap();
        let decoded: Settings = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded.lock_effect, result.lock_effect);
        let mut legacy = encoded;
        legacy.as_object_mut().unwrap().remove("lock_effect");
        assert_eq!(
            serde_json::from_value::<Settings>(legacy)
                .unwrap()
                .lock_effect,
            Default::default()
        );
        assert!(
            serde_json::from_value::<SettingsPatch>(
                serde_json::json!({ "lock_effect": { "color": [256,0,0] } })
            )
            .is_err()
        );
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
        assert!(!parsed.audio_send_sysout);
        assert!(!parsed.audio_play_sysout);
        assert!(!parsed.audio_send_mic);
        assert!(!parsed.audio_play_mic);
    }

    #[test]
    fn fresh_settings_fail_closed_for_every_audio_route() {
        let settings = Settings::default();
        assert!(!settings.audio_send_sysout);
        assert!(!settings.audio_play_sysout);
        assert!(!settings.audio_send_mic);
        assert!(!settings.audio_play_mic);
    }

    #[test]
    fn input_patch_preserves_concurrent_audio_routes() {
        let current = Settings {
            audio_send_sysout: true,
            audio_play_mic: true,
            ..Settings::default()
        };

        let merged = merge_patch(
            current,
            SettingsPatch {
                mouse_rate_hz: Some(250),
                ..SettingsPatch::default()
            },
        );

        assert_eq!(merged.mouse_rate_hz, 250);
        assert!(merged.audio_send_sysout);
        assert!(merged.audio_play_mic);
    }

    #[test]
    fn failed_audio_settings_write_never_changes_the_runtime_route() {
        let settings = Settings::default();
        let applied = std::sync::atomic::AtomicBool::new(false);
        let result = persist_then_apply(
            &settings,
            |_| anyhow::bail!("simulated disk failure"),
            |_| applied.store(true, std::sync::atomic::Ordering::Release),
        );
        assert!(result.is_err());
        assert!(!applied.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn startup_install_waits_for_in_flight_settings_update() {
        use std::sync::{Arc, Barrier, mpsc};
        use std::time::Duration;

        let lock = Arc::new(parking_lot::Mutex::new(()));
        let held = lock.lock();
        let started = Arc::new(Barrier::new(2));
        let (loaded_tx, loaded_rx) = mpsc::channel();
        let worker_lock = lock.clone();
        let worker_started = started.clone();
        let worker = std::thread::spawn(move || {
            worker_started.wait();
            install_loaded_serialized(
                worker_lock.as_ref(),
                || {
                    loaded_tx.send(()).unwrap();
                    Settings::default()
                },
                |_| {},
            );
        });

        started.wait();
        assert!(loaded_rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(held);
        loaded_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn settings_persistence_atomically_replaces_the_complete_file() {
        let dir =
            std::env::temp_dir().join(format!("mineshare-settings-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, b"old").unwrap();

        atomic_write_file(&path, b"new-complete-settings").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new-complete-settings");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
