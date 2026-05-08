//! Tauri-side application state.
//!
//! M5 wires the GUI process to the same `mineshare_daemon::runtime`
//! that powers the headless daemon binary — capture, audio,
//! clipboard, control channel, the lot. The Tauri `setup` callback
//! kicks off `runtime::run` on the async runtime and never awaits
//! it; that task lives for the program's lifetime. Tauri commands
//! read state through the shared globals exposed by
//! `mineshare_daemon::status`, so the React frontend just polls
//! `get_status` on a timer.

use anyhow::Result;
use mineshare_daemon::status::{StatusSnapshot, snapshot};
use mineshare_daemon::{logs, runtime};
use tracing::error;

/// Bootstraps logging and spawns the daemon runtime as a background
/// task. Returns immediately so the UI window opens without
/// blocking on the long-running runtime future.
pub fn bootstrap_runtime() -> Result<()> {
    // Logs go to the same `%APPDATA%\MineShare\logs\` (Win) /
    // `~/.config/MineShare/logs/` (Linux) directory the standalone
    // daemon writes to — same `logs::init` does the right thing on
    // both.
    let _ = logs::init();

    tauri::async_runtime::spawn(async move {
        let opts = runtime::RunOpts {
            capture: true,
            inject: true,
        };
        if let Err(e) = runtime::run(opts).await {
            error!(error = %e, "embedded daemon runtime ended with error");
        }
    });
    Ok(())
}

/// Tauri command handler — returns a fresh status snapshot every call.
pub fn current_status() -> StatusSnapshot {
    snapshot()
}
