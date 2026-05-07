//! mineshare-input
//!
//! Cross-platform input capture and injection. M0: trait surface only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ScanCode(pub u32);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { btn: Button, down: bool },
    Key { scan: ScanCode, down: bool },
    Scroll { dx: f32, dy: f32 },
}

pub trait InputCapture: Send {
    fn start(&mut self) -> anyhow::Result<()>;
    fn poll(&mut self) -> Vec<InputEvent>;
    fn set_grab(&mut self, grab: bool);
}

pub trait InputInject: Send {
    fn mouse_move_rel(&self, dx: i32, dy: i32);
    fn mouse_button(&self, btn: Button, down: bool);
    fn key(&self, scan: ScanCode, down: bool);
    fn scroll(&self, dx: f32, dy: f32);
}
