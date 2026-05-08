//! MineShare Tauri app library.
//!
//! M5 collapses the M0-era discovery-only prototype into the real
//! daemon runtime: the GUI process *is* the bridge. Headless
//! installs (M4) keep working with the standalone daemon binary;
//! the Tauri shell is for users who want a window.

mod state;
mod tray;

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

#[derive(serde::Serialize)]
struct DevicesSnapshot {
    outputs: Vec<mineshare_audio::DeviceInfo>,
    inputs: Vec<mineshare_audio::DeviceInfo>,
}

#[tauri::command]
fn set_input_lock(locked: bool) {
    mineshare_input::set_input_locked(locked);
}

#[tauri::command]
fn list_audio_devices() -> DevicesSnapshot {
    DevicesSnapshot {
        outputs: mineshare_audio::list_output_devices(),
        inputs: mineshare_audio::list_input_devices(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(e) = state::bootstrap_runtime() {
        eprintln!("daemon bootstrap failed: {e:#}");
    }
    info!("MineShare GUI starting");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            tray::install(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window should NOT quit the daemon —
            // hide to tray instead. The user can re-open from the
            // tray menu, and "Quit" there is the only path that
            // tears the bridge down.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_layout,
            set_layout,
            get_audio_status,
            set_audio_toggle,
            list_audio_devices,
            set_input_lock,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
