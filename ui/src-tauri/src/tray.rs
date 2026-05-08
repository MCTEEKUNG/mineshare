//! System tray + close-to-tray wiring.
//!
//! Closing the MineShare window via the X button **hides** it to
//! the tray instead of exiting; the embedded daemon keeps running
//! and the bridge stays up. The user re-opens the window from the
//! tray menu's "Show MineShare" entry and tears everything down
//! via "Quit MineShare".

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WebviewWindow};

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show MineShare", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit MineShare", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("tray icon".into()))?;

    TrayIconBuilder::with_id("mineshare-tray")
        .icon(icon)
        .tooltip("MineShare")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Plain left-click on the tray icon == "Show MineShare".
            // Right-click brings up the menu (handled by the OS).
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}
