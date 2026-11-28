//! Shared desktop/Android control-v5 wire types. Variant order is protocol ABI.
use anyhow::Result;
use mineshare_core::DeviceId;
use mineshare_input::{ForwardedInputState, InputEvent};
use serde::{Deserialize, Serialize};
pub const CONTROL_PROTOCOL: &str = "control-v5";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerSide {
    Left,
    Right,
    Top,
    Bottom,
}
impl PeerSide {
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LockVisual {
    pub locked: bool,
    pub effect: mineshare_input::LockEffect,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceCapabilities {
    pub os: String,
    pub pointer: bool,
    pub keyboard: bool,
    pub text_input: bool,
    pub system_audio_send: bool,
    pub system_audio_receive: bool,
    pub clipboard: bool,
    pub files: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamKind {
    SysOut,
    Mic,
}

/// One Opus-encoded 20 ms frame on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFrame {
    pub stream: StreamKind,
    /// Monotonically increasing per-stream counter — used by the jitter
    /// buffer to detect drops/reordering. Wraps at u32 max (≈ 24 days
    /// at 50 fps).
    pub seq: u32,
    /// Raw Opus payload. Decoder yields 48 kHz / 2-channel PCM.
    pub opus: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortAnnounce {
    pub udp_port: u16,
    pub daemon_version: String,
    pub screen_w: u32,
    pub screen_h: u32,
    pub touchpad_capabilities: u32,
    pub capabilities: DeviceCapabilities,
}

pub const CONNECTION_MAGIC: [u8; 8] = *b"MSPAIR05";
// The legacy announce field is a bitset. Use a non-touchpad bit so older
// peers can still connect without seeing the new clipboard-only message.
pub const STAGED_PASTE_CAPABILITY: u32 = 1 << 31;
#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionHello {
    pub magic: [u8; 8],
    pub device_id: DeviceId,
    pub display_name: String,
    pub trusts_peer: bool,
    pub requested: bool,
}
impl ConnectionHello {
    pub fn validate_for(&self, expected: Option<DeviceId>) -> Result<()> {
        anyhow::ensure!(
            self.magic == CONNECTION_MAGIC,
            "incompatible pairing protocol: update MineShare on both computers"
        );
        anyhow::ensure!(
            !self.display_name.is_empty()
                && self.display_name.len() <= 256
                && !self.display_name.chars().any(char::is_control),
            "invalid computer name"
        );
        if let Some(expected) = expected {
            anyhow::ensure!(
                self.device_id == expected,
                "the responding computer is not the selected device"
            );
        }
        Ok(())
    }
}

/// Streamed messages on the encrypted TCP control channel after the
/// initial PortAnnounce exchange. Used to coordinate which peer holds
/// Remote mode at any given moment so the local capture can refuse to
/// also enter Remote (which would have both ends fighting for the
/// cursor).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMsg {
    TakeControl {
        epoch: u64,
    },
    ReleaseControl,
    /// Peer is requesting that *we* leave Remote (their hotkey was
    /// pressed while we held Remote). We should call
    /// `force_local_exit_remote()` and then send `ReleaseControl`
    /// from the resulting `Exited` event.
    ForceRelease,
    /// Peer's clipboard changed — apply the text on this side. Bound
    /// to 64 KB at the watcher to keep oversized pastes from
    /// flooding the control channel; receivers should also sanity-
    /// check length before applying.
    ClipboardText(String),
    ClipboardImageStart {
        id: u64,
        width: u32,
        height: u32,
        total_bytes: u64,
    },
    ClipboardImageChunk {
        id: u64,
        offset: u64,
        data: Vec<u8>,
    },
    ClipboardImageEnd {
        id: u64,
    },
    /// Peer changed their layout — they want OUR `peer_side` to
    /// be the value they sent. The sender flips left↔right /
    /// top↔bottom on their side before sending, so receivers can
    /// apply the value verbatim.
    SetPeerSide(PeerSide),
    /// Stage 7 PIN-pairing: dialer sends the user-entered PIN
    /// plus its own identity so the acceptor can both verify
    /// and persist the trust entry. Used exactly once at the
    /// start of a session against an untrusted peer.
    PinAttempt {
        pin: String,
        device_id: String,
        display_name: String,
    },
    /// Acceptor's verdict on the PinAttempt + its own identity.
    /// On `ok=true` the dialer adds the acceptor to its trust
    /// list and proceeds with the session; on `ok=false` both
    /// sides tear the connection down.
    PinAck {
        ok: bool,
        device_id: String,
        display_name: String,
    },
    /// Stage 8.5 RTT heartbeat. The sender stamps its own current
    /// monotonic-ish ms in `ts_ms`, the peer echoes it verbatim
    /// as a [`ControlMsg::Pong`], the original sender measures
    /// `now - ts_ms` to learn round-trip time. Carrying our own
    /// timestamp on both legs avoids any need for clock sync
    /// between peers.
    Ping {
        ts_us: u64,
        lock_visual: Option<LockVisual>,
    },
    Pong {
        ts_us: u64,
    },
    /// Activity beacon driving the Smart keyboard target. Each side
    /// periodically advertises `last_input_ago_ms` — ms since its
    /// most recent activity (a deliberate mouse drag OR a click),
    /// capped at 60 s, or `None` if it has been idle all session.
    ///
    /// The receiver back-dates its single `PEER_ACTIVITY_AT` by the
    /// reported age, so the most-recent-activity race compares both
    /// sides on the receiver's own clock with no clock sync.
    ///
    /// Wire format is positional (bincode), so changing these fields
    /// is a breaking protocol change. Both daemons must upgrade in
    /// lockstep.
    ActivityBeacon {
        last_input_ago_ms: Option<u32>,
    },
    /// Stage 12 file transfer — declares an incoming file. The
    /// receiver allocates a `.partial` temp file under
    /// `<download>/MineShare/` and prepares to write chunks.
    /// Auto-accept: peer is already trusted, so we don't prompt.
    FileOffer {
        id: u64,
        name: String,
        size_bytes: u64,
    },
    /// One slice of bytes belonging to transfer `id`. `offset` is
    /// included so future out-of-order / parallel-chunk variants
    /// don't break the wire format. Current sender streams
    /// strictly sequentially.
    FileChunk {
        id: u64,
        offset: u64,
        data: Vec<u8>,
    },
    /// Sender finished — receiver verifies sha256 against the
    /// running hash, atomically renames the `.partial` to the
    /// final destination on match, deletes + fails on mismatch.
    FileEnd {
        id: u64,
        sha256: [u8; 32],
    },
    /// Either side aborted the transfer.
    FileCancel {
        id: u64,
    },
    /// Toggle the peer into / out of Game Drive "Receiving" — it should
    /// inject our pure-relative input and suppress its own cursor-crossing.
    GameDrive {
        active: bool,
        epoch: u64,
    },
    /// IDs of verified FileEnd transfers to publish as a native file clipboard.
    ClipboardFiles {
        offer: u64,
        ids: Vec<u64>,
    },
    InputLockRequest(mineshare_input::LockRequest),
    InputLockReply(mineshare_input::LockReply),
    PairingDecision {
        accepted: bool,
        reason: String,
    },
    ClipboardFileOffer(u64),
    ClipboardFilePaste(u64),
    ClipboardFileTransfer {
        offer: u64,
        id: u64,
        name: String,
        size_bytes: u64,
    },
    TextInput(String),
    DisplayGeometry {
        width: u32,
        height: u32,
    },
    AudioRoute {
        receive_system: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireFrame {
    Input {
        seq: u64,
        event: InputEvent,
    },
    InputState {
        seq: u64,
        after_input_seq: u64,
        state: ForwardedInputState,
    },
    Audio(AudioFrame),
}
