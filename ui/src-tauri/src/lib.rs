//! MineShare Tauri app library.
//!
//! For M0 the GUI process embeds discovery in-process so we can demonstrate
//! peer detection without a separate daemon binary. M4 will move this into
//! a real background service and have the UI talk to it over IPC.

mod state;

use mineshare_ipc::IpcResponse;
use mineshare_net::PeerAdvert;
use state::AppState;
use std::sync::Arc;
use tracing::info;

#[tauri::command]
async fn get_status(state: tauri::State<'_, Arc<AppState>>) -> Result<IpcResponse, String> {
    Ok(state.status())
}

#[tauri::command]
async fn list_peers(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<PeerAdvert>, String> {
    Ok(state.peers())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let app_state = Arc::new(AppState::bootstrap().expect("AppState bootstrap failed"));
    info!("MineShare GUI starting");

    let state_for_setup = app_state.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(app_state.clone())
        .setup(move |_app| {
            let state = state_for_setup.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = state.start_discovery().await {
                    tracing::error!(error = %e, "discovery failed to start");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_status, list_peers])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,mineshare=debug"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
