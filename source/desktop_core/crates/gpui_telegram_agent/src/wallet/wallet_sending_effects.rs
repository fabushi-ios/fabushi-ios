//! Authentic TON Wallet Sending Glare & Clock Effects Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_sending_effects.cpp, Telegram/SourceFiles/wallet/wallet_sending_effects.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletSendingEffects {
    pub glare_progress: f64,
    pub clock_angle_degrees: f64,
    pub is_active: bool,
}

impl WalletSendingEffects {
    pub fn new() -> Self {
        Self {
            glare_progress: 0.0,
            clock_angle_degrees: 0.0,
            is_active: false,
        }
    }

    pub fn start(&mut self) {
        self.glare_progress = 0.0;
        self.clock_angle_degrees = 0.0;
        self.is_active = true;
    }

    pub fn tick(&mut self, dt_ms: u64) -> bool {
        if !self.is_active {
            return false;
        }
        // Glare cycle runs over 1200ms
        self.glare_progress = (self.glare_progress + (dt_ms as f64 / 1200.0)).fract();
        // Clock rotation 360 deg over 1000ms
        self.clock_angle_degrees = (self.clock_angle_degrees + (dt_ms as f64 * 360.0 / 1000.0)).rem_euclid(360.0);
        true
    }

    pub fn stop(&mut self) {
        self.is_active = false;
    }

    /// Computes glare band opacity gradient at relative position x in [0.0, 1.0]
    pub fn compute_glare_intensity(&self, x: f64) -> f64 {
        let center = self.glare_progress;
        let dist = (x - center).abs();
        if dist < 0.25 {
            (1.0 - dist / 0.25).powi(2)
        } else {
            0.0
        }
    }
}

impl Default for WalletSendingEffects {
    fn default() -> Self {
        Self::new()
    }
}
