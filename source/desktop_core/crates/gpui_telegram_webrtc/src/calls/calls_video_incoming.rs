//! Remote Video Incoming Surface Renderer
//! Upstream: Telegram/SourceFiles/calls/calls_video_incoming.h:L19-L44, calls_video_incoming.cpp:L27-L140
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoRendererType {
    Software,
    OpenGL,
    RhiMetal,
    RhiVulkan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsVideoIncoming {
    pub renderer_type: VideoRendererType,
    pub frame_width: u32,
    pub frame_height: u32,
    pub bottom_shadow_alpha_max: u8,
    pub has_frame: bool,
    pub is_paused: bool,
    pub rotation_degrees: u16,
}

impl CallsVideoIncoming {
    pub const K_BOTTOM_SHADOW_ALPHA_MAX: u8 = 74;

    pub fn new(renderer_type: VideoRendererType) -> Self {
        Self {
            renderer_type,
            frame_width: 0,
            frame_height: 0,
            bottom_shadow_alpha_max: Self::K_BOTTOM_SHADOW_ALPHA_MAX,
            has_frame: false,
            is_paused: false,
            rotation_degrees: 0,
        }
    }

    pub fn update_frame_dimensions(&mut self, width: u32, height: u32, rotation: u16) {
        self.frame_width = width;
        self.frame_height = height;
        self.rotation_degrees = rotation;
        self.has_frame = width > 0 && height > 0;
    }

    pub fn aspect_ratio(&self) -> f32 {
        if self.frame_height == 0 {
            16.0 / 9.0
        } else {
            self.frame_width as f32 / self.frame_height as f32
        }
    }
}
