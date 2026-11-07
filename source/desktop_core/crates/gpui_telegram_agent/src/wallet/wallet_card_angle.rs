//! Telegram Desktop 1:1 Parity Submodule: `WalletCardAngle`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_angle.cpp`, `Telegram/SourceFiles/wallet/wallet_card_angle.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const FOLLOW_JUMP: f64 = 0.5;
pub const FOLLOW_DURATION_MS: f64 = 100.0;
pub const FOLLOW_RAMP: f64 = 0.5;
pub const ANGLE_LIMIT: f64 = 16.0;
pub const ANGLE_STEP: f64 = 0.25;
pub const CENTER_SLACK: f64 = 1.0;

pub fn follow_ease(progress: f64, ramp: f64) -> f64 {
    if progress > 0.5 {
        return 1.0 - follow_ease(1.0 - progress, ramp);
    }
    let speed = 1.0 / (1.0 - ramp);
    if progress < ramp {
        speed * progress * progress / (2.0 * ramp)
    } else {
        speed * (progress - ramp / 2.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardAngle {
    pub angle_x: f64,
    pub angle_y: f64,
    pub target_x: f64,
    pub target_y: f64,
    pub current_jump: f64,
    pub progress: f64,
    pub is_tracking: bool,
}

impl Default for WalletCardAngle {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardAngle {
    pub fn new() -> Self {
        Self {
            angle_x: 0.0,
            angle_y: 0.0,
            target_x: 0.0,
            target_y: 0.0,
            current_jump: 0.0,
            progress: 1.0,
            is_tracking: false,
        }
    }

    pub fn set_pointer_pos(&mut self, px: f64, py: f64, card_w: f64, card_h: f64) {
        if card_w <= 0.0 || card_h <= 0.0 {
            return;
        }
        let half_w = card_w * 0.5;
        let half_h = card_h * 0.5;
        let dx = px - half_w;
        let dy = py - half_h;

        let slack_dx = if dx.abs() < CENTER_SLACK { 0.0 } else { dx };
        let slack_dy = if dy.abs() < CENTER_SLACK { 0.0 } else { dy };

        let norm_x = (slack_dx / half_w).clamp(-1.0, 1.0);
        let norm_y = (slack_dy / half_h).clamp(-1.0, 1.0);

        self.target_x = norm_y * ANGLE_LIMIT;
        self.target_y = -norm_x * ANGLE_LIMIT;
        self.is_tracking = true;

        if (self.angle_x - self.target_x).abs() > 0.1 || (self.angle_y - self.target_y).abs() > 0.1 {
            self.current_jump = FOLLOW_JUMP;
            self.progress = 0.0;
        }
    }

    pub fn release_pointer(&mut self) {
        self.target_x = 0.0;
        self.target_y = 0.0;
        self.is_tracking = false;
        self.progress = 0.0;
    }

    pub fn step(&mut self, dt_ms: f64) {
        if self.progress >= 1.0 {
            self.angle_x = self.target_x;
            self.angle_y = self.target_y;
            return;
        }
        self.progress = (self.progress + dt_ms / FOLLOW_DURATION_MS).min(1.0);
        let eased = follow_ease(self.progress, FOLLOW_RAMP);
        let factor = self.current_jump + (1.0 - self.current_jump) * eased;

        self.angle_x += (self.target_x - self.angle_x) * factor;
        self.angle_y += (self.target_y - self.angle_y) * factor;
    }

    pub fn quantized_angles(&self) -> (f64, f64) {
        let qx = (self.angle_x / ANGLE_STEP).round() * ANGLE_STEP;
        let qy = (self.angle_y / ANGLE_STEP).round() * ANGLE_STEP;
        (qx, qy)
    }
}
