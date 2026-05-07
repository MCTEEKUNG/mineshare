use serde::{Deserialize, Serialize};

use crate::device::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonitorId {
    pub device: DeviceId,
    pub index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Monitor {
    pub id: MonitorId,
    pub name: String,
    pub native_size: (u32, u32),
    pub dpi_scale: f32,
    pub os_position: (i32, i32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedMonitor {
    pub monitor: MonitorId,
    pub rect: Rect,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UnifiedLayout {
    pub monitors: Vec<PlacedMonitor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

/// Bridge edge: cursor crossing this edge moves to another device's monitor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BridgeEdge {
    pub from: MonitorId,
    pub from_edge: Edge,
    pub to: MonitorId,
}

impl UnifiedLayout {
    /// Derive bridge edges between monitors of *different* devices that touch.
    /// Each adjacency yields two `BridgeEdge`s — one per crossing direction.
    pub fn bridge_edges(&self) -> Vec<BridgeEdge> {
        let mut edges = Vec::new();
        for a in &self.monitors {
            for b in &self.monitors {
                if a.monitor.device == b.monitor.device {
                    continue;
                }
                // a's right edge touches b's left edge
                if a.rect.right() == b.rect.x && rects_overlap_y(&a.rect, &b.rect) {
                    edges.push(BridgeEdge {
                        from: a.monitor,
                        from_edge: Edge::Right,
                        to: b.monitor,
                    });
                }
                // a's left edge touches b's right edge
                if a.rect.x == b.rect.right() && rects_overlap_y(&a.rect, &b.rect) {
                    edges.push(BridgeEdge {
                        from: a.monitor,
                        from_edge: Edge::Left,
                        to: b.monitor,
                    });
                }
                // a's bottom edge touches b's top edge
                if a.rect.bottom() == b.rect.y && rects_overlap_x(&a.rect, &b.rect) {
                    edges.push(BridgeEdge {
                        from: a.monitor,
                        from_edge: Edge::Bottom,
                        to: b.monitor,
                    });
                }
                // a's top edge touches b's bottom edge
                if a.rect.y == b.rect.bottom() && rects_overlap_x(&a.rect, &b.rect) {
                    edges.push(BridgeEdge {
                        from: a.monitor,
                        from_edge: Edge::Top,
                        to: b.monitor,
                    });
                }
            }
        }
        edges
    }
}

fn rects_overlap_y(a: &Rect, b: &Rect) -> bool {
    a.y < b.bottom() && b.y < a.bottom()
}

fn rects_overlap_x(a: &Rect, b: &Rect) -> bool {
    a.x < b.right() && b.x < a.right()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mid(d: DeviceId, i: u32) -> MonitorId {
        MonitorId {
            device: d,
            index: i,
        }
    }

    #[test]
    fn detects_horizontal_bridge() {
        let a = DeviceId::new();
        let b = DeviceId::new();
        let layout = UnifiedLayout {
            monitors: vec![
                PlacedMonitor {
                    monitor: mid(a, 0),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                    },
                },
                PlacedMonitor {
                    monitor: mid(b, 0),
                    rect: Rect {
                        x: 1920,
                        y: 0,
                        w: 1920,
                        h: 1080,
                    },
                },
            ],
        };
        let edges = layout.bridge_edges();
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().any(|e| e.from_edge == Edge::Right));
    }

    #[test]
    fn no_bridge_within_same_device() {
        let a = DeviceId::new();
        let layout = UnifiedLayout {
            monitors: vec![
                PlacedMonitor {
                    monitor: mid(a, 0),
                    rect: Rect {
                        x: 0,
                        y: 0,
                        w: 1920,
                        h: 1080,
                    },
                },
                PlacedMonitor {
                    monitor: mid(a, 1),
                    rect: Rect {
                        x: 1920,
                        y: 0,
                        w: 1920,
                        h: 1080,
                    },
                },
            ],
        };
        assert!(layout.bridge_edges().is_empty());
    }
}
