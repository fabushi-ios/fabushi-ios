//! Telegram Desktop 1:1 Parity Submodule: `WalletCardViewMotion`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_view_motion.cpp`, `Telegram/SourceFiles/wallet/wallet_card_view_motion.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const LIFT_DURATION_MS: f64 = 100.0;
pub const SLIDE_DURATION_MS: f64 = 180.0;
pub const TURN_DURATION_MS: f64 = 370.0;
pub const TURN_PEAK_MS: f64 = 205.0;
pub const GLIDE_DURATION_MS: f64 = 205.0;
pub const DIM_DURATION_MS: f64 = 350.0;
pub const BLUR_DELAY_MS: f64 = 350.0;
pub const BLUR_DURATION_MS: f64 = 290.0;
pub const BUTTON_DELAY_MS: f64 = 300.0;
pub const BUTTON_DURATION_MS: f64 = 300.0;
pub const FALLBACK_DURATION_MS: f64 = 200.0;
pub const FALLBACK_BUTTON_DELAY_MS: f64 = 100.0;
pub const FALLBACK_BUTTON_DURATION_MS: f64 = 250.0;
pub const CLOSE_BLUR_DURATION_MS: f64 = 100.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardViewPose {
    pub center_x: f64,
    pub center_y: f64,
    pub angle_deg: f64,
    pub length_scale: f64,
    pub height_scale: f64,
    pub lean: f64,
    pub opacity: f64,
    pub is_at_rest: bool,
}

impl Default for CardViewPose {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_y: 0.0,
            angle_deg: 0.0,
            length_scale: 1.0,
            height_scale: 1.0,
            lean: 0.0,
            opacity: 1.0,
            is_at_rest: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardViewMotion {
    pub current_pose: CardViewPose,
    pub elapsed_ms: f64,
    pub is_opening: bool,
    pub is_closing: bool,
    pub backdrop_dim: f64,
    pub blur_radius: f64,
    pub buttons_opacity: f64,
}

impl Default for WalletCardViewMotion {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardViewMotion {
    pub fn new() -> Self {
        Self {
            current_pose: CardViewPose::default(),
            elapsed_ms: 0.0,
            is_opening: false,
            is_closing: false,
            backdrop_dim: 0.0,
            blur_radius: 0.0,
            buttons_opacity: 0.0,
        }
    }

    pub fn start_presentation(&mut self, origin_x: f64, origin_y: f64) {
        self.is_opening = true;
        self.is_closing = false;
        self.elapsed_ms = 0.0;
        self.current_pose = CardViewPose {
            center_x: origin_x,
            center_y: origin_y,
            angle_deg: 12.0,
            length_scale: 0.85,
            height_scale: 0.85,
            lean: 0.1,
            opacity: 0.0,
            is_at_rest: false,
        };
    }

    pub fn start_dismissal(&mut self) {
        self.is_opening = false;
        self.is_closing = true;
        self.elapsed_ms = 0.0;
    }

    pub fn step(&mut self, dt_ms: f64, target_cx: f64, target_cy: f64) {
        self.elapsed_ms += dt_ms;
        if self.is_opening {
            let t_slide = (self.elapsed_ms / SLIDE_DURATION_MS).min(1.0);
            let t_turn = (self.elapsed_ms / TURN_DURATION_MS).min(1.0);
            let t_dim = (self.elapsed_ms / DIM_DURATION_MS).min(1.0);

            self.current_pose.center_x += (target_cx - self.current_pose.center_x) * t_slide;
            self.current_pose.center_y += (target_cy - self.current_pose.center_y) * t_slide;
            self.current_pose.angle_deg = 12.0 * (1.0 - t_turn);
            self.current_pose.length_scale = 0.85 + 0.15 * t_slide;
            self.current_pose.height_scale = 0.85 + 0.15 * t_slide;
            self.current_pose.opacity = t_slide;
            self.backdrop_dim = 0.5 * t_dim;

            if self.elapsed_ms >= BLUR_DELAY_MS {
                let t_blur = ((self.elapsed_ms - BLUR_DELAY_MS) / BLUR_DURATION_MS).min(1.0);
                self.blur_radius = 16.0 * t_blur;
            }

            if self.elapsed_ms >= BUTTON_DELAY_MS {
                let t_btn = ((self.elapsed_ms - BUTTON_DELAY_MS) / BUTTON_DURATION_MS).min(1.0);
                self.buttons_opacity = t_btn;
            }

            if t_turn >= 1.0 && t_slide >= 1.0 {
                self.current_pose.is_at_rest = true;
                self.is_opening = false;
            }
        } else if self.is_closing {
            let t_close = (self.elapsed_ms / FALLBACK_DURATION_MS).min(1.0);
            self.current_pose.opacity = 1.0 - t_close;
            self.backdrop_dim = 0.5 * (1.0 - t_close);
            self.blur_radius = 16.0 * (1.0 - t_close);
            self.buttons_opacity = 1.0 - t_close;
            if t_close >= 1.0 {
                self.is_closing = false;
                self.current_pose.is_at_rest = true;
            }
        }
    }
}
