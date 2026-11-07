//! Telegram Desktop 1:1 Parity Submodule: `WalletCardFlight`
//! Subsystem: `wallet` | Feature ID: `CAP-06`
//! Upstream Sources: `Telegram/SourceFiles/wallet/wallet_card_flight.cpp`, `Telegram/SourceFiles/wallet/wallet_card_flight.h`

#![allow(dead_code, unused_imports, unused_variables)]

use serde::{Deserialize, Serialize};
use gpui_telegram_core::error::Result;

pub const TRAVEL_LAYER_PROPERTY: &str = "walletTravelLayer";
pub const FLIGHT_DURATION_MS: f64 = 250.0;
pub const BEND: f64 = 0.45;
pub const BEND_STEPS: usize = 16;
pub const WIDEST_ANGLE: f64 = 45.0;
pub const QUARTER_TURN: f64 = 90.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FlightPose {
    pub center_x: f64,
    pub center_y: f64,
    pub side: f64,
    pub rotation: f64,
}

pub fn flight_ease_out(progress: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    1.0 - (1.0 - p) * (1.0 - p)
}

pub fn compute_flight_pose(from: &FlightPose, to: &FlightPose, t: f64, bend: f64) -> FlightPose {
    let eased = flight_ease_out(t);
    let linear_x = from.center_x + (to.center_x - from.center_x) * eased;
    let linear_y = from.center_y + (to.center_y - from.center_y) * eased;
    let span = (to.center_x - from.center_x).hypot(to.center_y - from.center_y);
    let parabolic_deflection = -4.0 * bend * eased * (1.0 - eased) * span;

    let side = from.side + (to.side - from.side) * eased;
    let rotation = from.rotation + (to.rotation - from.rotation) * eased;

    FlightPose {
        center_x: linear_x,
        center_y: linear_y + parabolic_deflection,
        side,
        rotation,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCardFlight {
    pub origin: FlightPose,
    pub target: FlightPose,
    pub current: FlightPose,
    pub progress: f64,
    pub is_animating: bool,
    pub elapsed_ms: f64,
}

impl Default for WalletCardFlight {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletCardFlight {
    pub fn new() -> Self {
        Self {
            origin: FlightPose::default(),
            target: FlightPose::default(),
            current: FlightPose::default(),
            progress: 1.0,
            is_animating: false,
            elapsed_ms: 0.0,
        }
    }

    pub fn start(&mut self, origin: FlightPose, target: FlightPose) {
        self.origin = origin.clone();
        self.target = target;
        self.current = origin;
        self.progress = 0.0;
        self.elapsed_ms = 0.0;
        self.is_animating = true;
    }

    pub fn step(&mut self, dt_ms: f64) -> &FlightPose {
        if !self.is_animating {
            return &self.current;
        }
        self.elapsed_ms += dt_ms;
        self.progress = (self.elapsed_ms / FLIGHT_DURATION_MS).min(1.0);
        self.current = compute_flight_pose(&self.origin, &self.target, self.progress, BEND);
        if self.progress >= 1.0 {
            self.is_animating = false;
        }
        &self.current
    }

    pub fn is_finished(&self) -> bool {
        !self.is_animating && self.progress >= 1.0
    }
}
