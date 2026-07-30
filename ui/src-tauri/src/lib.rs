//! MineShare Tauri app library.
//!
//! M5 collapses the M0-era discovery-only prototype into the real
//! daemon runtime: the GUI process *is* the bridge. Headless
//! installs (M4) keep working with the standalone daemon binary;
//! the Tauri shell is for users who want a window.

mod state;
mod tray;

use mineshare_daemon::audio_status::AudioStatus;
use mineshare_daemon::latency::LatencySnapshot;
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
    mineshare_daemon::settings::set_audio_toggle(&stream, &direction, enabled)
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
struct DevicesSnapshot {
    outputs: Vec<mineshare_audio::DeviceInfo>,
    inputs: Vec<mineshare_audio::DeviceInfo>,
    /// Currently-selected output device name, or `None` if the
    /// runtime is following the OS default (Stage 8.4).
    selected_output: Option<String>,
    selected_input: Option<String>,
    selected_sysout_capture: Option<String>,
}

#[tauri::command]
fn set_input_lock(locked: bool) {
    mineshare_input::set_input_locked(locked);
}

/// GUI button equivalent of the Ctrl+Alt+K hotkey: cycles the
/// keyboard target through Auto → ForcePeer → ForceLocal → Auto.
#[tauri::command]
fn cycle_keyboard_target() {
    mineshare_input::cycle_keyboard_target();
}

#[tauri::command]
fn set_keyboard_target(target: mineshare_input::KeyboardTarget) {
    mineshare_input::set_keyboard_target(target);
}

/// GUI button equivalent of the Ctrl+Alt+G hotkey: toggles Game
/// Drive on this machine (drive the peer's cursor-locked game with
/// pure-relative mouse + keyboard, no cursor-crossing).
#[tauri::command]
fn toggle_game_drive() {
    mineshare_input::toggle_game_drive();
}

#[tauri::command]
fn get_mouse_rate_stats() -> mineshare_input::MouseRateStats {
    mineshare_input::mouse_rate_stats()
}

#[tauri::command]
fn get_latency() -> LatencySnapshot {
    mineshare_daemon::latency::snapshot()
}

// ----------------------- File transfer (Stage 12) -----------------------

/// `start_send` calls `tokio::spawn` internally, which panics with
/// "there is no reactor running" when invoked from a *sync* Tauri
/// command (those run on Tauri's blocking thread pool — outside any
/// runtime context). Marking this `async` makes Tauri schedule it on
/// `tauri::async_runtime` (which IS Tokio when the `tokio` feature
/// is enabled, the Tauri 2 default), so `tokio::spawn` works and
/// the file-send task is dispatched onto the same runtime the
/// embedded daemon runs on.
#[tauri::command]
async fn send_file(path: String) -> Result<u64, String> {
    mineshare_daemon::files::start_send(std::path::PathBuf::from(path))
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn get_transfers() -> Vec<mineshare_daemon::files::TransferSnapshot> {
    mineshare_daemon::files::snapshot()
}

#[tauri::command]
fn cancel_transfer(id: u64) {
    mineshare_daemon::files::user_cancel(id);
}

/// Open the Downloads/MineShare folder in the OS file manager so
/// the user can grab their just-received files in one click.
#[tauri::command]
fn open_downloads_dir(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = mineshare_daemon::files::download_dir();
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings() -> mineshare_daemon::settings::Settings {
    mineshare_daemon::settings::load()
}

#[tauri::command]
fn set_settings(
    settings: mineshare_daemon::settings::Settings,
) -> Result<mineshare_daemon::settings::Settings, String> {
    mineshare_daemon::settings::apply(settings).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_pairing_phase() -> mineshare_daemon::pairing::PairingPhase {
    mineshare_daemon::pairing::current_phase()
}

#[tauri::command]
fn submit_pin(pin: String) {
    mineshare_daemon::pairing::submit_pin(pin);
}

#[tauri::command]
fn list_trusted_peers() -> Vec<mineshare_daemon::trust::TrustedPeer> {
    mineshare_daemon::trust::list_trusted()
}

#[tauri::command]
fn revoke_trusted_peer(device_id: String) -> Result<(), String> {
    mineshare_daemon::trust::revoke(&device_id).map_err(|e| e.to_string())
}

/// Async + blocking-pool version: cpal device enumeration on Win
/// involves COM round-trips that can take 100–500 ms. If we ran
/// it on the Tauri command executor (the main async runtime), a
/// burst of GUI calls would back up other commands and contribute
/// to "Not Responding" pauses on slower laptops. The audio crate
/// also caches the result for 5 s so most calls return instantly.
#[tauri::command]
async fn list_audio_devices() -> DevicesSnapshot {
    tokio::task::spawn_blocking(|| DevicesSnapshot {
        outputs: mineshare_audio::list_output_devices(),
        inputs: mineshare_audio::list_input_devices(),
        selected_output: mineshare_audio::selected_output_device(),
        selected_input: mineshare_audio::selected_input_device(),
        selected_sysout_capture: mineshare_audio::selected_sysout_capture_device(),
    })
    .await
    .unwrap_or(DevicesSnapshot {
        outputs: vec![],
        inputs: vec![],
        selected_output: None,
        selected_input: None,
        selected_sysout_capture: None,
    })
}

/// Force a fresh enumeration on the next `list_audio_devices` —
/// used by the "↻ refresh" button so a hot-plugged device shows
/// up without waiting for the 5 s cache TTL.
#[tauri::command]
fn refresh_audio_devices() {
    mineshare_audio::invalidate_device_cache();
}

/// Pick the playback output device by name. Pass `null` (or omit
/// the field) to revert to the OS default. Takes effect within
/// ~200 ms, no daemon restart required (Stage 8.4).
#[tauri::command]
fn set_audio_output_device(name: Option<String>) -> Result<(), String> {
    mineshare_daemon::settings::set_audio_output_device(name).map_err(|e| e.to_string())
}

/// Mirror of [`set_audio_output_device`] for the mic capture path.
#[tauri::command]
fn set_audio_input_device(name: Option<String>) -> Result<(), String> {
    mineshare_daemon::settings::set_audio_input_device(name).map_err(|e| e.to_string())
}

/// Choose and persist the Windows endpoint captured as local system audio.
#[tauri::command]
fn set_sysout_capture_device(name: Option<String>) -> Result<(), String> {
    mineshare_daemon::settings::set_sysout_capture_device(name).map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn promote_taskbar_window_icon(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        ICON_BIG, ICON_SMALL, ICON_SMALL2, SendMessageW, WM_GETICON, WM_SETICON,
    };

    let hwnd = window.hwnd()?;
    // Tauri/wry supplies the small titlebar icon, but on some Windows 11
    // builds leaves WM_GETICON(ICON_BIG) empty. The taskbar then falls back
    // to the generic white-document glyph. Reuse the live small HICON for
    // BIG/SMALL2 so every shell surface resolves the same application mark.
    let mut icon = unsafe {
        SendMessageW(
            hwnd,
            WM_GETICON,
            Some(WPARAM(ICON_SMALL2 as usize)),
            None,
        )
    };
    if icon.0 == 0 {
        icon = unsafe {
            SendMessageW(
                hwnd,
                WM_GETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                None,
            )
        };
    }
    if icon.0 != 0 {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(icon.0)),
            );
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL2 as usize)),
                Some(LPARAM(icon.0)),
            );
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    info!("MineShare GUI starting");

    tauri::Builder::default()
        // MUST be the first plugin: its setup hook runs before our
        // `.setup()` below, so a duplicate launch is detected and
        // exits *before* we ever call `bootstrap_runtime()` — the
        // embedded daemon therefore never double-spawns. The callback
        // fires in the EXISTING instance to surface its window.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            use tauri::Manager;
            info!("second instance launched — focusing existing window");
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Set the live window icon explicitly. The bundle icon is enough
            // for Explorer and installers, but Windows can otherwise keep a
            // generic document icon for the running WebView taskbar button.
            use tauri::Manager;
            if let (Some(window), Some(icon)) = (
                app.get_webview_window("main"),
                app.default_window_icon().cloned(),
            ) {
                window.set_icon(icon)?;
                #[cfg(target_os = "windows")]
                promote_taskbar_window_icon(&window)?;
            }

            // Spawn the daemon here (not before the builder) so it
            // only ever starts in the surviving single instance.
            if let Err(e) = state::bootstrap_runtime() {
                eprintln!("daemon bootstrap failed: {e:#}");
            }
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
            refresh_audio_devices,
            set_audio_output_device,
            set_audio_input_device,
            set_sysout_capture_device,
            set_input_lock,
            cycle_keyboard_target,
            set_keyboard_target,
            toggle_game_drive,
            get_mouse_rate_stats,
            get_latency,
            send_file,
            get_transfers,
            cancel_transfer,
            open_downloads_dir,
            get_settings,
            set_settings,
            get_pairing_phase,
            submit_pin,
            list_trusted_peers,
            revoke_trusted_peer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
