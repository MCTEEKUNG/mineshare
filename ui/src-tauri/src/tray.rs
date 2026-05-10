//! System tray + close-to-tray wiring.
//!
//! Closing the MineShare window via the X button **hides** it to
//! the tray instead of exiting; the embedded daemon keeps running
//! and the bridge stays up. The user re-opens the window from the
//! tray menu's "Show MineShare" entry and tears everything down
//! via "Quit MineShare".
//!
//! Stage 8.2 enriches the menu with a live status header (peer
//! name / address) and quick toggles for the most common runtime
//! controls — game-mode lock and the four audio send/receive
//! switches — so users can flip the bridge between machines
//! without opening the window. A background task polls the daemon
//! state once a second and updates the menu items via Tauri's
//! `set_checked` / `set_text` APIs, so the tray reflects whatever
//! changed inside the window or via hotkey.

use mineshare_daemon::audio_status as audio;
use mineshare_daemon::status::snapshot;
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

/// Shared handles to the menu items that need live updates from
/// the polling task.
struct LiveMenu {
    status: MenuItem<Wry>,
    game_lock: CheckMenuItem<Wry>,
    send_sysout: CheckMenuItem<Wry>,
    play_sysout: CheckMenuItem<Wry>,
    send_mic: CheckMenuItem<Wry>,
    play_mic: CheckMenuItem<Wry>,
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    // ---- Status header ------------------------------------------------
    // Disabled menu item — acts as a label that we mutate from the
    // poll task to show "no peer", "paired with X", or
    // "daemon offline".
    let status = MenuItem::with_id(app, "status", "starting…", false, None::<&str>)?;

    // ---- Game-mode toggle --------------------------------------------
    let game_lock = CheckMenuItem::with_id(
        app,
        "game_lock",
        "Game mode (lock input)",
        true,
        false,
        None::<&str>,
    )?;

    // ---- Audio submenu -----------------------------------------------
    let send_sysout = CheckMenuItem::with_id(
        app,
        "send_sysout",
        "Send system sound",
        true,
        true,
        None::<&str>,
    )?;
    let play_sysout = CheckMenuItem::with_id(
        app,
        "play_sysout",
        "Receive system sound",
        true,
        true,
        None::<&str>,
    )?;
    let send_mic =
        CheckMenuItem::with_id(app, "send_mic", "Send microphone", true, true, None::<&str>)?;
    let play_mic = CheckMenuItem::with_id(
        app,
        "play_mic",
        "Receive microphone",
        true,
        true,
        None::<&str>,
    )?;
    let audio_menu = Submenu::with_items(
        app,
        "Audio",
        true,
        &[&send_sysout, &play_sysout, &send_mic, &play_mic],
    )?;

    // ---- Window controls ---------------------------------------------
    let show = MenuItem::with_id(app, "show", "Show MineShare", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit MineShare", true, None::<&str>)?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &sep1,
            &game_lock,
            &audio_menu,
            &sep2,
            &show,
            &sep3,
            &quit,
        ],
    )?;

    let live = Arc::new(LiveMenu {
        status,
        game_lock,
        send_sysout,
        play_sysout,
        send_mic,
        play_mic,
    });

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
            "game_lock" => {
                let now = mineshare_input::is_input_locked();
                mineshare_input::set_input_locked(!now);
            }
            "send_sysout" => audio::set_send_sysout(!audio::snapshot().send_sysout),
            "play_sysout" => audio::set_play_sysout(!audio::snapshot().play_sysout),
            "send_mic" => audio::set_send_mic(!audio::snapshot().send_mic),
            "play_mic" => audio::set_play_mic(!audio::snapshot().play_mic),
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

    // ---- Background poller -------------------------------------------
    // Refresh tray state every 2 s so checkmarks track whatever
    // the user did in the window (or via hotkey) without us having
    // to plumb explicit change events. 1 Hz felt nice but every
    // `set_text` / `set_checked` call marshals to the tao event
    // loop that also drives the WebView — too-frequent updates
    // contributed to "Not Responding" stalls on slower laptops.
    // 2 s is fast enough to feel live for tray inspection.
    let live_for_task = Arc::clone(&live);
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            refresh(&live_for_task);
        }
    });

    Ok(())
}

fn refresh(live: &LiveMenu) {
    let s = snapshot();
    let header = if !s.peer_connected {
        "no peer".to_string()
    } else if let Some(name) = s.peer_name.as_deref() {
        format!("paired with {name}")
    } else if let Some(addr) = s.peer_addr.as_deref() {
        format!("paired with {addr}")
    } else {
        "paired".to_string()
    };
    let _ = live.status.set_text(header);

    let _ = live.game_lock.set_checked(s.input_locked);

    let a = audio::snapshot();
    let _ = live.send_sysout.set_checked(a.send_sysout);
    let _ = live.play_sysout.set_checked(a.play_sysout);
    let _ = live.send_mic.set_checked(a.send_mic);
    let _ = live.play_mic.set_checked(a.play_mic);
}

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
