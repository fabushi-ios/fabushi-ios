//! Telegram Desktop 1:1 Parity Submodule: `media_player_float.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Floating Video Note PiP Player)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_float.h:L35-L60`
//!           `Telegram/SourceFiles/media/player/media_player_float.cpp:L42-L85`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FloatPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FloatSize {
    pub width: i32,
    pub height: i32,
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaPlayerFloatPlayer {
    pub is_visible: bool,
    pub is_dragging: bool,
    pub position: FloatPoint,
    pub size: FloatSize,
    pub message_id: u64,
    pub is_round_video: bool,
}

impl MediaPlayerFloatPlayer {
    pub fn new() -> Self {
        Self {
            is_visible: false,
            is_dragging: false,
            position: FloatPoint { x: 20, y: 20 },
            size: FloatSize { width: 160, height: 160 },
            message_id: 0,
            is_round_video: true,
        }
    }

    pub fn show_video_note(&mut self, message_id: u64) {
        self.message_id = message_id;
        self.is_visible = true;
        self.is_round_video = true;
    }

    pub fn hide(&mut self) {
        self.is_visible = false;
        self.is_dragging = false;
    }

    pub fn drag_to(&mut self, x: i32, y: i32, bounds_width: i32, bounds_height: i32) {
        self.is_dragging = true;
        let max_x = (bounds_width - self.size.width).max(0);
        let max_y = (bounds_height - self.size.height).max(0);
        self.position.x = x.clamp(0, max_x);
        self.position.y = y.clamp(0, max_y);
    }

    pub fn finish_drag(&mut self) {
        self.is_dragging = false;
    }
}
