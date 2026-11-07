//! Picture-in-Picture Draggable Video Bubble
//! Upstream: Telegram/SourceFiles/calls/calls_video_bubble.h:L20-L70, calls_video_bubble.cpp:L17-L100
#![allow(dead_code)]

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
        Self {
            drag_mode: BubbleDragMode::SnapToCorners,
            active_corner: BubbleCorner::BottomRight,
            x: 0.0,
            y: 0.0,
            width: Self::DEFAULT_WIDTH,
            height: Self::DEFAULT_HEIGHT,
            is_mirrored: true,
            is_dragging: false,
            is_visible: false,
        }
    }

    pub fn snap_to_corner(&mut self, parent_w: f32, parent_h: f32, corner: BubbleCorner) {
        self.active_corner = corner;
        match corner {
            BubbleCorner::TopLeft => {
                self.x = Self::MARGIN;
                self.y = Self::MARGIN;
            }
            BubbleCorner::TopRight => {
                self.x = parent_w - self.width - Self::MARGIN;
                self.y = Self::MARGIN;
            }
            BubbleCorner::BottomLeft => {
                self.x = Self::MARGIN;
                self.y = parent_h - self.height - Self::MARGIN;
            }
            BubbleCorner::BottomRight => {
                self.x = parent_w - self.width - Self::MARGIN;
                self.y = parent_h - self.height - Self::MARGIN;
            }
        }
    }

    pub fn set_mirrored(&mut self, mirrored: bool) {
        self.is_mirrored = mirrored;
    }
}
