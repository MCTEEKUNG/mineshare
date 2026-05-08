//! MineShare Tauri app library.
//!
//! M5 collapses the M0-era discovery-only prototype into the real
//! daemon runtime: the GUI process *is* the bridge. Headless
//! installs (M4) keep working with the standalone daemon binary;
//! the Tauri shell is for users who want a window.

mod state;

use mineshare_daemon::audio_status::AudioStatus;
use mineshare_daemon::layout::LayoutConfig;
use mineshare_daemon::status::StatusSnapshot;
use tracing::info;

#[tauri::command]
fn get_status() -> StatusSnapshot {
    state::current_status()
}

#[tauri::command]
fn get_layout() -> LayoutConfig {
    mineshare_daemon::layout::current()
}

#[tauri::command]
fn set_layout(cfg: LayoutConfig) -> Result<(), String> {
    mineshare_daemon::layout::set(cfg).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_audio_status() -> AudioStatus {
    mineshare_daemon::audio_status::snapshot()
}

#[tauri::command]
fn set_audio_toggle(stream: String, direction: String, enabled: bool) -> Result<(), String> {
    use mineshare_daemon::audio_status as a;
    match (stream.as_str(), direction.as_str()) {
        ("sysout", "send") => a::set_send_sysout(enabled),
        ("sysout", "play") => a::set_play_sysout(enabled),
        ("mic", "send") => a::set_send_mic(enabled),
        ("mic", "play") => a::set_play_mic(enabled),
        _ => return Err(format!("unknown audio toggle: {stream}/{direction}")),
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(e) = state::bootstrap_runtime() {
        eprintln!("daemon bootstrap failed: {e:#}");
    }
    info!("MineShare GUI starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_layout,
            set_layout,
            get_audio_status,
            set_audio_toggle,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
