//! Tauri-side application state.
//!
//! M5 wires the GUI process to the same `mineshare_daemon::runtime`
//! that powers the headless daemon binary — capture, audio,
//! clipboard, control channel, the lot. The Tauri `setup` callback
//! kicks off `runtime::run` on a dedicated Windows runtime and never awaits
//! it; that task lives for the program's lifetime. Tauri commands
//! read state through the shared globals exposed by
//! `mineshare_daemon::status`, so the React frontend just polls
//! `get_status` on a timer.

use anyhow::Result;
use mineshare_daemon::runtime;
use mineshare_daemon::status::{StatusSnapshot, snapshot};
use tracing::error;

/// Keep Windows transport scheduling independent of WebView command work.
/// The dedicated runtime remains alive for the embedded daemon's lifetime.
pub fn bootstrap_runtime() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let realtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mineshare-realtime")
            .on_thread_start(|| {
                let scheduled = mineshare_audio::register_realtime_thread();
                tracing::info!(scheduled, "realtime transport worker started");
            })
            .enable_all()
            .build()?;
        std::thread::Builder::new()
            .name("mineshare-daemon".into())
            .spawn(move || realtime.block_on(run_embedded()))?;
    }
    #[cfg(not(target_os = "windows"))]
    tauri::async_runtime::spawn(run_embedded());
    Ok(())
}

async fn run_embedded() {
    let opts = runtime::RunOpts {
        capture: std::env::var_os("MINESHARE_DISABLE_CAPTURE").is_none(),
        inject: true,
        audio: std::env::var_os("MINESHARE_DISABLE_AUDIO").is_none(),
    };
    if let Err(e) = runtime::run(opts).await {
        error!(error = %e, "embedded daemon runtime ended with error");
    }
}

/// Tauri command handler — returns a fresh status snapshot every call.
pub fn current_status() -> StatusSnapshot {
    snapshot()
}
