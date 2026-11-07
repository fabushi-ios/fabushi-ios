//! Telegram Desktop 1:1 Parity Submodule: `media_player_dropdown.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Speed Menu Dropdown & Slider Math)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_dropdown.h:L33-L60`
//!           `Telegram/SourceFiles/media/player/media_player_dropdown.cpp:L28-L53`

use serde::{Deserialize, Serialize};

/// Upstream: `media_player_dropdown.cpp:L28-L33`
pub const SPEED_DEBOUNCE_TIMEOUT_MS: i64 = 1000;
pub const SPEED_BOOST_HOLD_DELAY_MS: i64 = 200;
pub const SPEED_BOOST_VALUE: f64 = 2.0;
pub const SPEED_BOOST_THROTTLE_MS: i64 = 200;
pub const SPEED_BOOST_SETTLE_MS: i64 = 120;

pub const SPEED_MIN: f64 = 0.5;
pub const SPEED_MAX: f64 = 2.5;

/// Upstream: `media_player_dropdown.cpp:L43-L52`
pub const SPEED_STICKED_VALUES: [(f64, f64); 7] = [
    (0.8, 0.05),
    (1.0, 0.05),
    (1.2, 0.05),
    (1.5, 0.05),
    (1.7, 0.05),
    (2.0, 0.05),
    (2.2, 0.05),
];

/// Upstream: `media_player_dropdown.cpp:L34-L36`
pub fn speed_to_slider_value(speed: f64) -> f64 {
    ((speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN)).clamp(0.0, 1.0)
}

/// Upstream: `media_player_dropdown.cpp:L38-L41`
pub fn slider_value_to_speed(value: f64) -> f64 {
    let speed = value.clamp(0.0, 1.0) * (SPEED_MAX - SPEED_MIN) + SPEED_MIN;
    let rounded = (speed * 10.0).round() / 10.0;
    // Check snapping
    for &(stick, threshold) in &SPEED_STICKED_VALUES {
        if (rounded - stick).abs() <= threshold {
            return stick;
        }
    }
    rounded
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaPlayerDropdownPlayer {
    pub is_visible: bool,
    pub current_speed: f64,
    pub slider_value: f64,
    pub is_boosted: bool,
}

impl MediaPlayerDropdownPlayer {
    pub fn new() -> Self {
        Self {
            is_visible: false,
            current_speed: 1.0,
            slider_value: speed_to_slider_value(1.0),
            is_boosted: false,
        }
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.current_speed = speed.clamp(SPEED_MIN, SPEED_MAX);
        self.slider_value = speed_to_slider_value(self.current_speed);
    }

    pub fn set_slider_value(&mut self, value: f64) {
        self.slider_value = value.clamp(0.0, 1.0);
        self.current_speed = slider_value_to_speed(self.slider_value);
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.is_visible = visible;
    }

    pub fn set_boosted(&mut self, boosted: bool) {
        self.is_boosted = boosted;
    }
}
