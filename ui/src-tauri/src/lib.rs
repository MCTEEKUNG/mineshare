//! MineShare Tauri app library.
//!
//! M5 collapses the M0-era discovery-only prototype into the real
//! daemon runtime: the GUI process *is* the bridge. Headless
//! installs (M4) keep working with the standalone daemon binary;
//! the Tauri shell is for users who want a window.

mod state;

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
            set_layout
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
