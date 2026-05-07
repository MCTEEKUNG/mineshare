//! mineshare-audio
//!
//! 4-stream bridge: sysout × 2 + mic × 2. M0: trait surface only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamKind {
    SysOut,
    Mic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpusFrame {
    pub stream: StreamKind,
    pub source_id: [u8; 16],
    pub bytes: Vec<u8>,
}

pub trait AudioCapture: Send {
    fn start(&mut self) -> anyhow::Result<()>;
    fn next_frame(&mut self) -> Option<OpusFrame>;
}

pub trait AudioPlayback: Send {
    fn enqueue(&self, frame: OpusFrame);
}
