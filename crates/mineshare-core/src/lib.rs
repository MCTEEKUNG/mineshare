//! mineshare-core
//!
//! Layout, source/routing FSM, and clipboard primitives. Pure logic — no I/O.

pub mod device;
pub mod layout;
pub mod routing;
pub mod source_fsm;

pub use device::DeviceId;
pub use layout::{Monitor, MonitorId, PlacedMonitor, Rect, UnifiedLayout};
pub use routing::{CursorState, RoutingFsm};
pub use source_fsm::{SourceFsm, SourceState};
