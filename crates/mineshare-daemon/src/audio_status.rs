//! Audio plane status + runtime toggles for the GUI's Audio tab.
//!
//! Each direction of each stream gets a `pub static AtomicBool`
//! that the runtime's pump tasks check on every frame; flipping
//! them via the Tauri commands takes effect on the next frame
//! (≤ 20 ms) without a daemon restart.
//!
//! `Status` snapshots are read by the GUI on a 1 s poll; they
//! also surface the platform-specific virtual-mic backend state
//! (PipeWire null-sink loaded / VB-CABLE detected / unavailable)
//! so the user can tell *why* their mic isn't appearing in
//! Discord without trawling the daemon log.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::Serialize;

/// Forward locally-captured sysout (WASAPI loopback / PipeWire
/// monitor) over the bridge. Off → we keep capturing for stats
/// but drop the frames before they hit the broadcast.
pub static SEND_SYSOUT: AtomicBool = AtomicBool::new(true);
/// Render peer sysout frames into the local default audio output.
/// Off → frames are decoded for stats but the playback ring is
/// skipped (silent).
pub static PLAY_SYSOUT: AtomicBool = AtomicBool::new(true);
/// Forward locally-captured mic frames.
pub static SEND_MIC: AtomicBool = AtomicBool::new(true);
/// Render peer mic frames into the virtual-mic sink.
pub static PLAY_MIC: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VirtualMicBackend {
    /// Linux: `pactl load-module module-null-sink` succeeded;
    /// apps see "MineShare-Mic" as a PipeWire monitor source.
    Pipewire,
    /// Windows: VB-CABLE Input device was found by cpal at
    /// startup; peer mic frames render into it and apps pick
    /// "CABLE Output" as their mic.
    VbCable,
    /// VB-CABLE not installed (Win) or pactl/pipewire-pulse
    /// missing (Linux). Mic frames keep flowing on the wire so
    /// the bridge isn't broken — they just don't audibly
    /// surface anywhere on this side.
    Unavailable,
}

static VIRTUAL_MIC_BACKEND: Mutex<VirtualMicBackend> = Mutex::new(VirtualMicBackend::Unavailable);

pub fn set_virtual_mic_backend(b: VirtualMicBackend) {
    *VIRTUAL_MIC_BACKEND.lock() = b;
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct AudioStatus {
    pub send_sysout: bool,
    pub play_sysout: bool,
    pub send_mic: bool,
    pub play_mic: bool,
    pub virtual_mic: VirtualMicBackend,
    pub os: &'static str,
}

pub fn snapshot() -> AudioStatus {
    AudioStatus {
        send_sysout: SEND_SYSOUT.load(Ordering::Relaxed),
        play_sysout: PLAY_SYSOUT.load(Ordering::Relaxed),
        send_mic: SEND_MIC.load(Ordering::Relaxed),
        play_mic: PLAY_MIC.load(Ordering::Relaxed),
        virtual_mic: *VIRTUAL_MIC_BACKEND.lock(),
        os: std::env::consts::OS,
    }
}

pub fn set_send_sysout(v: bool) {
    SEND_SYSOUT.store(v, Ordering::Relaxed);
    tracing::info!(enabled = v, "audio toggle: send sysout");
}
pub fn set_play_sysout(v: bool) {
    PLAY_SYSOUT.store(v, Ordering::Relaxed);
    tracing::info!(enabled = v, "audio toggle: play sysout");
}
pub fn set_send_mic(v: bool) {
    SEND_MIC.store(v, Ordering::Relaxed);
    tracing::info!(enabled = v, "audio toggle: send mic");
}
pub fn set_play_mic(v: bool) {
    PLAY_MIC.store(v, Ordering::Relaxed);
    tracing::info!(enabled = v, "audio toggle: play mic");
}
