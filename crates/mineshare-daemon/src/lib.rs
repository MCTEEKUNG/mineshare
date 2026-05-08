//! MineShare daemon library.
//!
//! Most of the daemon lives here so both the standalone
//! `mineshare-daemon` binary (M4 install) and the upcoming Tauri
//! GUI shell (M5) can drive the same runtime without re-implementing
//! the input/audio/control plumbing.
//!
//! The `main.rs` next door is a thin clap entry point that calls
//! into [`runtime::run`] / [`collect::run`].

pub mod clipboard;
pub mod collect;
pub mod identity;
pub mod layout;
pub mod logs;
pub mod runtime;
pub mod status;
