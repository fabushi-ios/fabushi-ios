//! Calls Userpic Avatar with Mute Overlay
//! Upstream: Telegram/SourceFiles/calls/calls_userpic.h:L23-L65, calls_userpic.cpp:L28-L80
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsUserpic {
    pub peer_id: u64,
    pub x: i32,
    pub y: i32,
    pub size: i32,
    pub mute_pos_x: i32,
    pub mute_pos_y: i32,
    pub mute_size: i32,
    pub mute_stroke: i32,
    pub is_muted: bool,
    pub has_custom_photo: bool,
}

impl CallsUserpic {
    pub const DEFAULT_PHOTO_SIZE: i32 = 160;
    pub const PREVIEW_PHOTO_SIZE: i32 = 100;
    pub const DEFAULT_MUTE_SIZE: i32 = 36;
    pub const DEFAULT_MUTE_STROKE: i32 = 3;

    pub fn new(peer_id: u64) -> Self {
        Self {
            peer_id,
            x: 0,
            y: 21,
            size: Self::DEFAULT_PHOTO_SIZE,
            mute_pos_x: 140,
            mute_pos_y: 135,
            mute_size: Self::DEFAULT_MUTE_SIZE,
            mute_stroke: Self::DEFAULT_MUTE_STROKE,
            is_muted: false,
            has_custom_photo: false,
        }
    }

    pub fn set_geometry(&mut self, x: i32, y: i32, size: i32) {
        self.x = x;
        self.y = y;
        self.size = size;
    }

    pub fn set_mute_layout(&mut self, x: i32, y: i32, size: i32, stroke: i32) {
        self.mute_pos_x = x;
        self.mute_pos_y = y;
        self.mute_size = size;
        self.mute_stroke = stroke;
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.is_muted = muted;
    }
}
