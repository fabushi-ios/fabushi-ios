//! Peer-Colored Dynamic Radial Background for Calls
//! Upstream: Telegram/SourceFiles/calls/calls_panel_background.h:L26-L75, calls_panel_background.cpp:L30-L120
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsPanelBackground {
    pub peer_id: u64,
    pub background_colors: Vec<u32>,
    pub custom_emoji_id: Option<i64>,
    pub center_x: f32,
    pub center_y: f32,
    pub radius: f32,
    pub dark_overlay_opacity: f32,
    pub darken_above_lightness: f32,
}

impl CallsPanelBackground {
    pub const K_DARK_OVERLAY_OPACITY: f32 = 0.14;
    pub const K_DARKEN_ABOVE_LIGHTNESS: f32 = 0.3;

    pub fn new(peer_id: u64, colors: Vec<u32>) -> Self {
        Self {
            peer_id,
            background_colors: colors,
            custom_emoji_id: None,
            center_x: 0.0,
            center_y: 0.0,
            radius: 0.0,
            dark_overlay_opacity: Self::K_DARK_OVERLAY_OPACITY,
            darken_above_lightness: Self::K_DARKEN_ABOVE_LIGHTNESS,
        }
    }

    pub fn update_geometry(&mut self, width: f32, height: f32, photo_top: f32, photo_size: f32) {
        self.center_x = width / 2.0;
        self.center_y = photo_top + photo_size / 2.0;

        let d1 = (self.center_x.powi(2) + self.center_y.powi(2)).sqrt();
        let d2 = ((width - self.center_x).powi(2) + (height - self.center_y).powi(2)).sqrt();
        self.radius = d1.max(d2);
    }

    pub fn set_custom_emoji(&mut self, emoji_id: i64) {
        self.custom_emoji_id = Some(emoji_id);
    }

    pub fn text_color_override(&self) -> u32 {
        if let Some(&first) = self.background_colors.first() {
            let r = ((first >> 16) & 0xFF) as f32 / 255.0;
            let g = ((first >> 8) & 0xFF) as f32 / 255.0;
            let b = (first & 0xFF) as f32 / 255.0;
            let lightness = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            if lightness > self.darken_above_lightness {
                0x000000
            } else {
                0xFFFFFF
            }
        } else {
            0xFFFFFF
        }
    }
}
