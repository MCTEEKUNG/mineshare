//! mineshare-net
//!
//! Discovery (mDNS), pairing (Noise XX), transport, and wire protocol.

pub mod discovery;
pub mod pairing;
pub mod proto;

pub use discovery::{Discovery, DiscoveryEvent, PeerAdvert, SERVICE_TYPE};
pub use pairing::{Initiator, NOISE_PARAMS, NoiseSession, Responder};
pub use proto::{ControlMsg, FRAME_VERSION, Frame};
