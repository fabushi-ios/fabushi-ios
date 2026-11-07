//! Telegram Desktop 1:1 Parity Submodule: `WalletCardFloat`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_float.cpp`, `Telegram/SourceFiles/wallet/wallet_card_float.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const SLIDE_DURATION_MS: f64 = 150.0;
pub const SLIDE_TIME_CONSTANT_MS: f64 = 48.0;
pub const LINGER_MS: f64 = 200.0;
pub const OUT_DURATION_MS: f64 = 60.0;
pub const OUT_TIME_CONSTANT_MS: f64 = 15.0;
pub const ENTRY_SCALE: f64 = 0.9;
pub const ENTRY_ROTATION: f64 = 2.0;
pub const ENTRY_LIFT: f64 = 0.175;
pub const OUT_LIFT: f64 = 0.226;
pub const OUT_SCALE: f64 = 0.944;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FloatPhase {
    Entering,
    Floating,
    Exiting,
    Resting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardFloat {
    pub phase: FloatPhase,
    pub scale: f64,
    pub rotation_deg: f64,
    pub lift_y: f64,
    pub elapsed_ms: f64,
}

impl Default for WalletCardFloat {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardFloat {
    pub fn new() -> Self {
        Self {
            phase: FloatPhase::Resting,
            scale: 1.0,
            rotation_deg: 0.0,
            lift_y: 0.0,
            elapsed_ms: 0.0,
        }
    }

    pub fn enter(&mut self) {
        self.phase = FloatPhase::Entering;
        self.elapsed_ms = 0.0;
        self.scale = ENTRY_SCALE;
        self.rotation_deg = ENTRY_ROTATION;
        self.lift_y = ENTRY_LIFT;
    }

    pub fn exit(&mut self) {
        self.phase = FloatPhase::Exiting;
        self.elapsed_ms = 0.0;
    }

    pub fn step(&mut self, dt_ms: f64) {
        self.elapsed_ms += dt_ms;
        match self.phase {
            FloatPhase::Resting => {
                self.scale = 1.0;
                self.rotation_deg = 0.0;
                self.lift_y = 0.0;
            }
            FloatPhase::Entering => {
                let t = (self.elapsed_ms / SLIDE_DURATION_MS).min(1.0);
                self.scale = ENTRY_SCALE + (1.0 - ENTRY_SCALE) * t;
                self.rotation_deg = ENTRY_ROTATION * (1.0 - t);
                self.lift_y = ENTRY_LIFT * (1.0 - t);
                if t >= 1.0 {
                    self.phase = FloatPhase::Floating;
                    self.elapsed_ms = 0.0;
                }
            }
            FloatPhase::Floating => {
                let oscillation_freq = 0.002;
                self.lift_y = (self.elapsed_ms * oscillation_freq).sin() * 4.0;
                self.rotation_deg = (self.elapsed_ms * oscillation_freq * 0.5).sin() * 0.4;
                self.scale = 1.0;
            }
            FloatPhase::Exiting => {
                let t = (self.elapsed_ms / OUT_DURATION_MS).min(1.0);
                self.scale = 1.0 + (OUT_SCALE - 1.0) * t;
                self.lift_y = OUT_LIFT * t;
                if t >= 1.0 {
                    self.phase = FloatPhase::Resting;
                    self.scale = 1.0;
                    self.lift_y = 0.0;
                    self.rotation_deg = 0.0;
                }
            }
        }
    }
}
