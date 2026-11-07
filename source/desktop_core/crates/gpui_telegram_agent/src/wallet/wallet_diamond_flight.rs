//! Authentic TON Wallet Diamond Flight Particle Animation Implementation
//! Source: Telegram/SourceFiles/wallet/wallet_diamond_flight.cpp, Telegram/SourceFiles/wallet/wallet_diamond_flight.h
//! Subsystem: wallet | Feature ID: CAP-06

use serde::{Deserialize, Serialize};

pub const K_FLIGHT_RISE: f64 = 0.06;
pub const K_APEX_LATEST: f64 = 0.4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlightCoordinates {
    pub x: f64,
    pub y: f64,
    pub alpha: f64,
    pub scale: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletDiamondFlight {
    pub from_x: f64,
    pub from_y: f64,
    pub target_x: f64,
    pub target_y: f64,
    pub progress: f64,
    pub duration_ms: u64,
    pub is_finished: bool,
}

impl WalletDiamondFlight {
    pub fn new(from: (f64, f64), target: (f64, f64), duration_ms: u64) -> Self {
        Self {
            from_x: from.0,
            from_y: from.1,
            target_x: target.0,
            target_y: target.1,
            progress: 0.0,
            duration_ms: duration_ms.max(1),
            is_finished: false,
        }
    }

    /// Calculates current position along parabolic arc
    pub fn sample(&self, t: f64) -> FlightCoordinates {
        let clamped_t = t.clamp(0.0, 1.0);
        
        // Linear interpolation for X
        let x = self.from_x + (self.target_x - self.from_x) * clamped_t;
        
        // Parabolic arc for Y with apex rise
        let distance_y = self.target_y - self.from_y;
        let arc_height = (self.target_x - self.from_x).abs().max(100.0) * K_FLIGHT_RISE * 10.0;
        let arc = 4.0 * arc_height * clamped_t * (1.0 - clamped_t);
        let y = self.from_y + distance_y * clamped_t - arc;

        // Alpha fade-in / fade-out
        let alpha = if clamped_t < 0.15 {
            clamped_t / 0.15
        } else if clamped_t > 0.85 {
            (1.0 - clamped_t) / 0.15
        } else {
            1.0
        };

        // Pulsing scale
        let scale = 0.6 + 0.4 * (std::f64::consts::PI * clamped_t).sin();

        FlightCoordinates { x, y, alpha, scale }
    }

    pub fn tick(&mut self, dt_ms: u64) -> bool {
        if self.is_finished {
            return false;
        }
        let step = (dt_ms as f64) / (self.duration_ms as f64);
        self.progress = (self.progress + step).min(1.0);
        if self.progress >= 1.0 {
            self.is_finished = true;
        }
        !self.is_finished
    }
}

impl Default for WalletDiamondFlight {
    fn default() -> Self {
        Self::new((0.0, 0.0), (0.0, 0.0), 600)
    }
}
