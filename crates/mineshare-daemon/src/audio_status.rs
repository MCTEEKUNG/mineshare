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
/// monitor) over the bridge. Off → the backend releases capture
/// resources and emits no frames.
pub static SEND_SYSOUT: AtomicBool = AtomicBool::new(false);
/// Render peer sysout frames into the local default audio output.
/// Off → frames never reach the playback backend.
pub static PLAY_SYSOUT: AtomicBool = AtomicBool::new(false);
/// Forward locally-captured mic frames.
pub static SEND_MIC: AtomicBool = AtomicBool::new(false);
/// Render peer mic frames into the virtual-mic sink.
pub static PLAY_MIC: AtomicBool = AtomicBool::new(false);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectiveAudioState {
    Disabled,
    Idle,
    Starting,
    Active,
    Degraded,
    Stopped,
}

fn effective_state(desired: bool, backend: mineshare_audio::BackendState) -> EffectiveAudioState {
    if !desired {
        return EffectiveAudioState::Disabled;
    }
    match backend {
        mineshare_audio::BackendState::Idle => EffectiveAudioState::Idle,
        mineshare_audio::BackendState::Starting => EffectiveAudioState::Starting,
        mineshare_audio::BackendState::Active => EffectiveAudioState::Active,
        mineshare_audio::BackendState::Degraded => EffectiveAudioState::Degraded,
        mineshare_audio::BackendState::Stopped => EffectiveAudioState::Stopped,
    }
}

#[derive(Clone)]
struct BackendStatuses {
    send_sysout: mineshare_audio::BackendStatus,
    play_sysout: mineshare_audio::BackendStatus,
    send_mic: mineshare_audio::BackendStatus,
    play_mic: mineshare_audio::BackendStatus,
}

static BACKEND_STATUSES: Mutex<Option<BackendStatuses>> = Mutex::new(None);

pub fn install_backend_statuses(
    send_sysout: mineshare_audio::BackendStatus,
    play_sysout: mineshare_audio::BackendStatus,
    send_mic: mineshare_audio::BackendStatus,
    play_mic: mineshare_audio::BackendStatus,
) {
    *BACKEND_STATUSES.lock() = Some(BackendStatuses {
        send_sysout,
        play_sysout,
        send_mic,
        play_mic,
    });
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
    pub send_sysout_state: EffectiveAudioState,
    pub play_sysout_state: EffectiveAudioState,
    pub send_mic_state: EffectiveAudioState,
    pub play_mic_state: EffectiveAudioState,
    pub virtual_mic: VirtualMicBackend,
    pub os: &'static str,
}

pub fn snapshot() -> AudioStatus {
    let send_sysout = SEND_SYSOUT.load(Ordering::Relaxed);
    let play_sysout = PLAY_SYSOUT.load(Ordering::Relaxed);
    let send_mic = SEND_MIC.load(Ordering::Relaxed);
    let play_mic = PLAY_MIC.load(Ordering::Relaxed);
    let backend = BACKEND_STATUSES.lock().clone();
    let state = |desired: bool, status: Option<&mineshare_audio::BackendStatus>| {
        effective_state(
            desired,
            status
                .map(mineshare_audio::BackendStatus::get)
                .unwrap_or(mineshare_audio::BackendState::Degraded),
        )
    };
    AudioStatus {
        send_sysout,
        play_sysout,
        send_mic,
        play_mic,
        send_sysout_state: state(send_sysout, backend.as_ref().map(|s| &s.send_sysout)),
        play_sysout_state: state(play_sysout, backend.as_ref().map(|s| &s.play_sysout)),
        send_mic_state: state(send_mic, backend.as_ref().map(|s| &s.send_mic)),
        play_mic_state: state(play_mic, backend.as_ref().map(|s| &s.play_mic)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_route_reports_disabled_instead_of_backend_state() {
        assert_eq!(
            effective_state(false, mineshare_audio::BackendState::Degraded),
            EffectiveAudioState::Disabled
        );
    }
}
