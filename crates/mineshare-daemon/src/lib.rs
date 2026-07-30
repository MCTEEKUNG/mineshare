//! MineShare daemon library.
//!
//! Most of the daemon lives here so both the standalone
//! `mineshare-daemon` binary (M4 install) and the upcoming Tauri
//! GUI shell (M5) can drive the same runtime without re-implementing
//! the input/audio/control plumbing.
//!
//! The `main.rs` next door is a thin clap entry point that calls
//! into [`runtime::run`] / [`collect::run`].

pub mod audio_status;
pub mod clipboard;
pub mod collect;
pub mod files;
pub mod identity;
pub mod latency;
pub mod layout;
pub mod logs;
pub mod pairing;
pub mod runtime;
pub mod runtime_owner;
pub mod settings;
pub mod status;
pub mod trust;

use std::sync::OnceLock;

/// Precise build identity: `"<semver> · <hash>[-dirty] · <date>"`,
/// e.g. `0.0.6 · 1a2b3c4 · 2026-05-27`. The hash/date come from `build.rs`
/// (git); the `-dirty` suffix marks a build made with uncommitted tracked
/// changes. Two machines built from the same commit produce the *same*
/// string — which is what lets the GUI/tray show "same build" vs the bare
/// semver that previously hid newer code under an unchanged version number.
pub fn build_id() -> String {
    format!(
        "{} · {}{} · {}",
        env!("CARGO_PKG_VERSION"),
        env!("MINESHARE_GIT_HASH"),
        env!("MINESHARE_GIT_DIRTY"),
        env!("MINESHARE_BUILD_DATE"),
    )
}

/// Compact identity for tray tooltips: `"<semver> · <hash>[-dirty]"`.
pub fn build_id_short() -> String {
    format!(
        "{} · {}{}",
        env!("CARGO_PKG_VERSION"),
        env!("MINESHARE_GIT_HASH"),
        env!("MINESHARE_GIT_DIRTY"),
    )
}

/// Process-exit hook registry. The daemon runtime registers a closure
/// (currently: broadcast the mDNS goodbye) that the GUI's tray "Quit"
/// path fires before `app.exit(0)` — a hard exit that would otherwise
/// skip all teardown and leave peers waiting out the mDNS cache TTL.
type ShutdownHook = Box<dyn Fn() + Send + Sync + 'static>;
static GOODBYE_HOOK: OnceLock<ShutdownHook> = OnceLock::new();

/// Register the graceful-exit hook. Called once by the runtime after it
/// announces over mDNS. Ignored if already set.
pub fn register_shutdown_hook(hook: ShutdownHook) {
    let _ = GOODBYE_HOOK.set(hook);
}

/// Fire the graceful-exit hook (mDNS goodbye, …). Safe to call from any
/// thread; a no-op if nothing was registered. Call before a hard
/// process exit so peers learn we're leaving immediately.
pub fn run_shutdown_hook() {
    if let Some(h) = GOODBYE_HOOK.get() {
        h();
    }
}
