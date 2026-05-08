//! File + stdout logging for the daemon.
//!
//! Logs go to:
//!   1. stderr (always, ANSI-colored when TTY)
//!   2. `<config_dir>/MineShare/logs/daemon.YYYY-MM-DD` (rotating daily)
//!
//! Sync file writes — fine for a hobby daemon and means logs survive
//! `kill -9` / power loss without losing the in-memory queue.

use std::fs;
use std::io::Write;
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

    // Stage 6.3: install a panic hook that captures the panic
    // message, location, and full backtrace, drops a timestamped
    // file into `<config_dir>/MineShare/crashes/`, and *also*
    // emits the same payload through tracing so it lands in the
    // daily log. Idempotent — second `init()` call (from the
    // Tauri shell after the bin entry already set it) just keeps
    // the existing hook.
    install_panic_hook();
    Ok(())
}

fn install_panic_hook() {
    use std::sync::Once;
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        // Preserve any existing default hook (cargo / rust runtime)
        // so panics still print to stderr in a dev terminal.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let msg = info
                .payload()
                .downcast_ref::<&'static str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("(non-string panic payload)");
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "(unknown location)".to_string());
            let backtrace = std::backtrace::Backtrace::force_capture();
            let thread = std::thread::current()
                .name()
                .unwrap_or("(unnamed)")
                .to_string();

            tracing::error!(
                thread = %thread,
                location = %location,
                msg = %msg,
                "DAEMON PANIC — see crash file for full backtrace"
            );
            if let Err(e) = write_crash_file(&thread, &location, msg, &backtrace) {
                tracing::error!(error = %e, "failed to write crash file");
            }
            // Defer to the previous hook so dev-mode stderr output
            // still happens.
            prev(info);
        }));
    });
}

fn crash_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no OS config dir")?;
    let dir = base.join("MineShare").join("crashes");
    fs::create_dir_all(&dir).ok();
    Ok(dir)
}

fn write_crash_file(
    thread: &str,
    location: &str,
    msg: &str,
    backtrace: &std::backtrace::Backtrace,
) -> Result<()> {
    let dir = crash_dir()?;
    let ts = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Iso8601::DEFAULT)
        .unwrap_or_else(|_| "unknown-ts".into())
        .replace(':', "-"); // safe filename on Windows
    let path = dir.join(format!("crash-{ts}.txt"));
    let mut f = fs::File::create(&path)?;
    writeln!(f, "MineShare daemon panic")?;
    writeln!(f, "----------------------")?;
    writeln!(f, "version  : {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(f, "thread   : {}", thread)?;
    writeln!(f, "location : {}", location)?;
    writeln!(f, "message  : {}", msg)?;
    writeln!(f)?;
    writeln!(f, "{}", backtrace)?;
    Ok(())
}
