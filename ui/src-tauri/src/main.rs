// Prevents additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "windows")]
    {
        if let Some(code) = mineshare_input::windows::run_keyboard_worker_if_requested() {
            std::process::exit(code);
        }
        mineshare_input::windows::enable_gui_keyboard_worker();
    }
    mineshare_app_lib::run()
}
