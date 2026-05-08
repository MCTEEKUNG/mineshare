//! File + stdout logging for the daemon.
//!
//! Logs go to:
//!   1. stderr (always, ANSI-colored when TTY)
//!   2. `<config_dir>/MineShare/logs/daemon.YYYY-MM-DD` (rotating daily)
//!
//! Sync file writes — fine for a hobby daemon and means logs survive
//! `kill -9` / power loss without losing the in-memory queue.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Returns the directory log files are written to.
pub fn log_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no OS config dir")?;
    let dir = base.join("MineShare").join("logs");
    fs::create_dir_all(&dir).ok();
    Ok(dir)
}

/// Initialise tracing with stderr + rotating-daily file output.
///
/// Idempotent: callable from both `mineshare-daemon` (binary) and
/// `mineshare-app` (Tauri shell). The Tauri shell calls
/// `bootstrap_runtime` which calls this *and then* spawns
/// `runtime::run` which also calls this — without `try_init` the
/// second call panics with `SetGlobalDefaultError`.
pub fn init() -> Result<()> {
    let dir = log_dir()?;
    let appender = tracing_appender::rolling::daily(&dir, "daemon");

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,mineshare=debug"));

    let already_set = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(true))
        .with(
            fmt::layer()
                .with_writer(appender)
                .with_target(true)
                .with_ansi(false),
        )
        .try_init()
        .is_err();

    if !already_set {
        tracing::info!(log_dir = %dir.display(), "log file appender ready");
    }
    Ok(())
}
