//! Wire protocol for the control channel (M0 minimal subset).

use mineshare_core::DeviceId;
use serde::{Deserialize, Serialize};

pub const FRAME_VERSION: u8 = 1;

/// Control-channel messages. Transport: TCP after Noise XX.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlMsg {
    Hello {
        device: DeviceId,
        os: String,
        display_name: String,
    },
    Heartbeat {
        ts_ns: u64,
    },
    Bye,
}

/// Length-prefixed frame on the wire (after Noise transport encryption).
#[derive(Debug, Clone)]
pub struct Frame {
    pub bytes: Vec<u8>,
}
