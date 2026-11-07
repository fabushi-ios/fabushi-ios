//! Telegram Desktop 1:1 Parity Submodule: `media_player_volume_controller.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Volume Controller & Wheel Step Handling)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_volume_controller.h:L27-L45`
//!           `Telegram/SourceFiles/media/player/media_player_volume_controller.cpp:L28-L82`

use serde::{Deserialize, Serialize};

pub const VOLUME_STEP_WHEEL: f64 = 0.05;

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaPlayerVolumeControllerPlayer {
    pub volume: f64,
    pub remembered_volume: f64,
    pub is_vertical: bool,
    pub is_muted: bool,
}

impl MediaPlayerVolumeControllerPlayer {
    pub fn new() -> Self {
        Self {
            volume: 1.0,
            remembered_volume: 1.0,
            is_vertical: false,
            is_muted: false,
        }
    }

    pub fn set_volume(&mut self, volume: f64) {
        let clamped = volume.clamp(0.0, 1.0);
        self.volume = clamped;
        if clamped > 0.0 {
            self.remembered_volume = clamped;
            self.is_muted = false;
        } else {
            self.is_muted = true;
        }
    }

    pub fn toggle_mute(&mut self) {
        if self.is_muted || self.volume == 0.0 {
            self.volume = if self.remembered_volume > 0.0 {
                self.remembered_volume
            } else {
                0.5
            };
            self.is_muted = false;
        } else {
            self.remembered_volume = self.volume;
            self.volume = 0.0;
            self.is_muted = true;
        }
    }

    pub fn handle_wheel_delta(&mut self, delta_units: i32) {
        let change = (delta_units as f64) * VOLUME_STEP_WHEEL;
        self.set_volume(self.volume + change);
    }

    pub fn set_is_vertical(&mut self, vertical: bool) {
        self.is_vertical = vertical;
    }
}
