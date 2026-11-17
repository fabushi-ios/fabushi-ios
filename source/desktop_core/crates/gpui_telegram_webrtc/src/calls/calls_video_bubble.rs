//! Picture-in-Picture Draggable Video Bubble
//! Upstream: Telegram/SourceFiles/calls/calls_video_bubble.h:L20-L70, calls_video_bubble.cpp:L17-L100

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BubbleDragMode {
    None,
    SnapToCorners,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BubbleCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsVideoBubble {
    pub drag_mode: BubbleDragMode,
    pub active_corner: BubbleCorner,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub is_mirrored: bool,
    pub is_dragging: bool,
    pub is_visible: bool,
}

impl CallsVideoBubble {
    pub const DEFAULT_WIDTH: f32 = 160.0;
    pub const DEFAULT_HEIGHT: f32 = 110.0;
    pub const MARGIN: f32 = 16.0;

    pub fn new() -> Self {
        let mut s = Self {
            drag_mode: BubbleDragMode::SnapToCorners,
            active_corner: BubbleCorner::BottomRight,
            x: Self::MARGIN,
            y: Self::MARGIN,
            width: Self::DEFAULT_WIDTH,
            height: Self::DEFAULT_HEIGHT,
            is_mirrored: true,
            is_dragging: false,
            is_visible: false,
        };
        s.snap_to_corner(400.0, 300.0, BubbleCorner::BottomRight);
        s
    }

    pub fn snap_to_corner(&mut self, parent_w: f32, parent_h: f32, corner: BubbleCorner) {
        if !parent_w.is_finite() || !parent_h.is_finite() {
            return;
        }
        self.active_corner = corner;
        let max_x = (parent_w - self.width - Self::MARGIN).max(0.0);
        let max_y = (parent_h - self.height - Self::MARGIN).max(0.0);
        let min_x = Self::MARGIN.min(max_x);
        let min_y = Self::MARGIN.min(max_y);
        match corner {
            BubbleCorner::TopLeft => {
                self.x = min_x;
                self.y = min_y;
            }
            BubbleCorner::TopRight => {
                self.x = max_x;
                self.y = min_y;
            }
            BubbleCorner::BottomLeft => {
                self.x = min_x;
                self.y = max_y;
            }
            BubbleCorner::BottomRight => {
                self.x = max_x;
                self.y = max_y;
            }
        }
    }

    pub fn clamp_to_bounds(&mut self, parent_w: f32, parent_h: f32) {
        if !parent_w.is_finite() || !parent_h.is_finite() {
            return;
        }
        let max_x = (parent_w - self.width - Self::MARGIN).max(0.0);
        let max_y = (parent_h - self.height - Self::MARGIN).max(0.0);
        let min_x = Self::MARGIN.min(max_x);
        let min_y = Self::MARGIN.min(max_y);
        self.x = if self.x.is_finite() {
            self.x.clamp(min_x, max_x)
        } else {
            min_x
        };
        self.y = if self.y.is_finite() {
            self.y.clamp(min_y, max_y)
        } else {
            min_y
        };
    }

    pub fn start_drag(&mut self) {
        self.is_dragging = self.drag_mode != BubbleDragMode::None;
    }

    pub fn drag_to(&mut self, new_x: f32, new_y: f32, parent_w: f32, parent_h: f32) {
        if self.drag_mode == BubbleDragMode::None
            || !new_x.is_finite()
            || !new_y.is_finite()
            || !parent_w.is_finite()
            || !parent_h.is_finite()
        {
            return;
        }
        self.x = new_x;
        self.y = new_y;
        self.clamp_to_bounds(parent_w, parent_h);
    }

    pub fn end_drag(&mut self, parent_w: f32, parent_h: f32) {
        self.is_dragging = false;
        if !parent_w.is_finite() || !parent_h.is_finite() {
            return;
        }
        self.clamp_to_bounds(parent_w, parent_h);
        if self.drag_mode == BubbleDragMode::SnapToCorners {
            let max_x = (parent_w - self.width - Self::MARGIN).max(0.0);
            let max_y = (parent_h - self.height - Self::MARGIN).max(0.0);
            let min_x = Self::MARGIN.min(max_x);
            let min_y = Self::MARGIN.min(max_y);
            let center_x = self.x + self.width / 2.0;
            let center_y = self.y + self.height / 2.0;
            let corners = [
                (BubbleCorner::TopLeft, min_x, min_y),
                (BubbleCorner::TopRight, max_x, min_y),
                (BubbleCorner::BottomLeft, min_x, max_y),
                (BubbleCorner::BottomRight, max_x, max_y),
            ];
            let mut best_corner = BubbleCorner::BottomRight;
            let mut min_dist_sq = f32::MAX;
            for (corner, cx, cy) in corners {
                let ccx = cx + self.width / 2.0;
                let ccy = cy + self.height / 2.0;
                let dx = center_x - ccx;
                let dy = center_y - ccy;
                let dist_sq = dx * dx + dy * dy;
                if dist_sq < min_dist_sq {
                    min_dist_sq = dist_sq;
                    best_corner = corner;
                }
            }
            self.snap_to_corner(parent_w, parent_h, best_corner);
        }
    }

    pub fn set_mirrored(&mut self, mirrored: bool) {
        self.is_mirrored = mirrored;
    }
}

impl Default for CallsVideoBubble {
    fn default() -> Self {
        Self::new()
    }
}
