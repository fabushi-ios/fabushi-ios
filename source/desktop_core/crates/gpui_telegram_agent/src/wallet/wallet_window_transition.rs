//! Wallet Window Transition & Fluid Animation Mechanics
//! Upstream: `Telegram/SourceFiles/wallet/wallet_window_transition.cpp`, `wallet_window_transition.h`
//! Subsystem: `wallet`, Feature ID: `CAP-06`

use serde::{Deserialize, Serialize};

pub const WINDOW_WAVE_DURATION_MS: u64 = 640;
pub const SNAP_EPSILON: f32 = 1e-3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionDirection {
    Opening,
    Closing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletWindowTransition {
    pub progress: f32,
    pub direction: TransitionDirection,
    pub duration_ms: u64,
    pub is_animating: bool,
}

impl Default for WalletWindowTransition {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletWindowTransition {
    pub fn new() -> Self {
        Self {
            progress: 0.0,
            direction: TransitionDirection::Opening,
            duration_ms: WINDOW_WAVE_DURATION_MS,
            is_animating: false,
        }
    }

    pub fn start_reveal(&mut self) {
        self.direction = TransitionDirection::Opening;
        self.progress = 0.0;
        self.is_animating = true;
    }

    pub fn start_hide(&mut self) {
        self.direction = TransitionDirection::Closing;
        self.progress = 1.0;
        self.is_animating = true;
    }

    pub fn tick(&mut self, delta_ms: u64) -> bool {
        if !self.is_animating {
            return false;
        }

        let step = (delta_ms as f32) / (self.duration_ms as f32);
        match self.direction {
            TransitionDirection::Opening => {
                self.progress = (self.progress + step).min(1.0);
                if (1.0 - self.progress) < SNAP_EPSILON {
                    self.progress = 1.0;
                    self.is_animating = false;
                }
            }
            TransitionDirection::Closing => {
                self.progress = (self.progress - step).max(0.0);
                if self.progress < SNAP_EPSILON {
                    self.progress = 0.0;
                    self.is_animating = false;
                }
            }
        }
        self.is_animating
    }

    pub fn eased_alpha(&self) -> f32 {
        // Cubic ease out: 1 - (1 - t)^3
        let inv = 1.0 - self.progress;
        1.0 - inv * inv * inv
    }
}
