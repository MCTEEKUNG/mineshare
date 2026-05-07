//! Cursor routing FSM — independent of source FSM.

use crate::layout::MonitorId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorState {
    OnLocal(MonitorId),
    OnRemote(MonitorId),
}

#[derive(Debug, Default)]
pub struct RoutingFsm {
    state: Option<CursorState>,
}

impl RoutingFsm {
    pub fn new() -> Self {
        Self { state: None }
    }

    pub fn set(&mut self, s: CursorState) {
        self.state = Some(s);
    }

    pub fn current(&self) -> Option<CursorState> {
        self.state
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.state, Some(CursorState::OnRemote(_)))
    }
}
