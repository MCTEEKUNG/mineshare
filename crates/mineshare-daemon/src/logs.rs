//! File + stdout logging for the daemon.
//!
//! Logs go to:
//!   1. stderr (always, ANSI-colored when TTY)
//!   2. `<config_dir>/MineShare/logs/daemon.YYYY-MM-DD` (rotating daily)
//!
//! File writes use a bounded, lossy background queue so diagnostics cannot
//! stall latency-sensitive input and audio paths.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const LOG_RETENTION_DAYS: u64 = 14;
const LOG_TOTAL_LIMIT_BYTES: u64 = 512 * 1024 * 1024;
const LOG_QUEUE_LINES: usize = 8_192;

static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

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
    prune_old_logs(&dir);
    let appender = tracing_appender::rolling::daily(&dir, "daemon");
    let (file_writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .buffered_lines_limit(LOG_QUEUE_LINES)
        .lossy(true)
        .thread_name("mineshare-log-writer")
        .finish(appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let already_set = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_target(true))
        .with(
            fmt::layer()
                .with_writer(file_writer)
                .with_target(true)
                .with_ansi(false),
        )
        .try_init()
        .is_err();

    if !already_set {
        // Keep the worker alive for the process lifetime. Dropping the guard
        // stops the background writer and would silently discard later logs.
        let _ = LOG_GUARD.set(guard);
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

/// Keep diagnostics useful without allowing a long-running tray process to
/// consume unbounded disk space. Files are deleted oldest-first, first by age
/// and then until the aggregate size is below the cap.
fn prune_old_logs(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let cutoff =
        SystemTime::now().checked_sub(Duration::from_secs(LOG_RETENTION_DAYS * 24 * 60 * 60));
    let mut logs = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("daemon.") || !path.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if cutoff.is_some_and(|limit| modified < limit) {
            let _ = fs::remove_file(&path);
            continue;
        }
        logs.push((modified, meta.len(), path));
    }

    logs.sort_by_key(|(modified, _, _)| *modified);
    let mut total: u64 = logs.iter().map(|(_, len, _)| *len).sum();
    for (_, len, path) in logs {
        if total <= LOG_TOTAL_LIMIT_BYTES {
            break;
        }
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
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
