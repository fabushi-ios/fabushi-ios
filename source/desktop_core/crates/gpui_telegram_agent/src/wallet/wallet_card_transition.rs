//! Telegram Desktop 1:1 Parity Submodule: `WalletCardTransition`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_transition.cpp`, `Telegram/SourceFiles/wallet/wallet_card_transition.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const WAVE_DURATION_MS: f64 = 320.0;
pub const FADE_DURATION_MS: f64 = 460.0;
pub const RIM_INSIDE: f64 = 0.024;
pub const RIM_OUTSIDE: f64 = 0.016;
pub const RIM_ALPHA: f64 = 0.10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CardLookReveal {
    Instant,
    Wave,
    Fade,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardLookChange {
    pub reveal: CardLookReveal,
    pub origin_x: f64,
    pub origin_y: f64,
}

impl Default for CardLookChange {
    fn default() -> Self {
        Self {
            reveal: CardLookReveal::Instant,
            origin_x: 0.0,
            origin_y: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardTransition {
    pub change: CardLookChange,
    pub elapsed_ms: f64,
    pub progress: f64,
    pub is_active: bool,
}

impl Default for WalletCardTransition {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardTransition {
    pub fn new() -> Self {
        Self {
            change: CardLookChange::default(),
            elapsed_ms: 0.0,
            progress: 1.0,
            is_active: false,
        }
    }

    pub fn start(&mut self, reveal: CardLookReveal, origin_x: f64, origin_y: f64) {
        self.change = CardLookChange {
            reveal,
            origin_x,
            origin_y,
        };
        self.elapsed_ms = 0.0;
        self.progress = if reveal == CardLookReveal::Instant { 1.0 } else { 0.0 };
        self.is_active = reveal != CardLookReveal::Instant;
    }

    pub fn step(&mut self, dt_ms: f64) {
        if !self.is_active {
            return;
        }
        self.elapsed_ms += dt_ms;
        let total_duration = match self.change.reveal {
            CardLookReveal::Instant => 1.0,
            CardLookReveal::Wave => WAVE_DURATION_MS,
            CardLookReveal::Fade => FADE_DURATION_MS,
        };
        self.progress = (self.elapsed_ms / total_duration).min(1.0);
        if self.progress >= 1.0 {
            self.is_active = false;
        }
    }

    pub fn wave_radius(&self, max_hypot: f64) -> f64 {
        if self.change.reveal == CardLookReveal::Wave {
            self.progress * max_hypot
        } else {
            max_hypot
        }
    }

    pub fn rim_opacity(&self) -> f32 {
        if self.change.reveal == CardLookReveal::Wave && self.is_active {
            (RIM_ALPHA * (1.0 - self.progress)) as f32
        } else {
            0.0
        }
    }
}
