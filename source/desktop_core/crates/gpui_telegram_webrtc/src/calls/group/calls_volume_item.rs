//! Telegram Desktop 1:1 Parity Submodule: `CallsVolumeItemGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//! - `Telegram/SourceFiles/calls/group/calls_volume_item.h`
//! - `Telegram/SourceFiles/calls/group/calls_volume_item.cpp:L23-L150`

use serde::{Deserialize, Serialize};

/// Maximum participant volume percentage (200%)
/// calls_volume_item.cpp:L23
pub const K_MAX_VOLUME_PERCENT: u32 = 200;

/// Speaker audio energy threshold tiers: 5%, 25%, 75%
/// calls_volume_item.cpp:L25-L28: kSpeakerThreshold
pub const K_SPEAKER_THRESHOLDS: [f32; 3] = [
    10.0 / (K_MAX_VOLUME_PERCENT as f32), // 0.05 (5%)
    50.0 / (K_MAX_VOLUME_PERCENT as f32), // 0.25 (25%)
    150.0 / (K_MAX_VOLUME_PERCENT as f32), // 0.75 (75%)
];

/// Magnetic snap target points and snap tolerance windows
/// calls_volume_item.cpp:L30-L39: kVolumeStickedValues
pub const K_VOLUME_STICKIED_VALUES: [(f32, f32); 7] = [
    (25.0 / 200.0, 2.0 / 200.0),   // 25% (±1%)
    (50.0 / 200.0, 2.0 / 200.0),   // 50% (±1%)
    (75.0 / 200.0, 2.0 / 200.0),   // 75% (±1%)
    (100.0 / 200.0, 10.0 / 200.0), // 100% standard normal: wide magnetic snap (±5%)!
    (125.0 / 200.0, 2.0 / 200.0),  // 125% (±1%)
    (150.0 / 200.0, 2.0 / 200.0),  // 150% (±1%)
    (175.0 / 200.0, 2.0 / 200.0),  // 175% (±1%)
];

/// Apply upstream magnetic stickiness to raw slider position [0.0..1.0]
pub fn apply_magnetic_snap(raw_value: f32) -> f32 {
    let clamped = raw_value.clamp(0.0, 1.0);
    for &(target, radius) in &K_VOLUME_STICKIED_VALUES {
        if (clamped - target).abs() <= radius {
            return target;
        }
    }
    clamped
}

/// Compute speaker arc visual animation level (0..3)
pub fn speaker_arc_level(volume_ratio: f32) -> u8 {
    if volume_ratio < K_SPEAKER_THRESHOLDS[0] {
        0
    } else if volume_ratio < K_SPEAKER_THRESHOLDS[1] {
        1
    } else if volume_ratio < K_SPEAKER_THRESHOLDS[2] {
        2
    } else {
        3
    }
}

/// Participant Volume Slider Menu Item
/// Target Symbol: `CallsVolumeItemGroup`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallsVolumeItemGroup {
    pub volume_percent: u32,
    pub is_muted: bool,
    pub is_locally_muted: bool,
    pub can_mute: bool,
    pub participant_id: u64,
}

impl Default for CallsVolumeItemGroup {
    fn default() -> Self {
        Self {
            volume_percent: 100,
            is_muted: false,
            is_locally_muted: false,
            can_mute: true,
            participant_id: 0,
        }
    }
}

impl CallsVolumeItemGroup {
    pub fn new(participant_id: u64, start_volume_percent: u32, muted: bool) -> Self {
        Self {
            volume_percent: start_volume_percent.min(K_MAX_VOLUME_PERCENT),
            is_muted: muted,
            is_locally_muted: muted,
            can_mute: true,
            participant_id,
        }
    }

    /// Set slider position with magnetic snap [0.0..1.0] -> [0..200%]
    pub fn set_slider_ratio(&mut self, raw_ratio: f32) -> u32 {
        let snapped = apply_magnetic_snap(raw_ratio);
        let percent = (snapped * (K_MAX_VOLUME_PERCENT as f32)).round() as u32;
        self.volume_percent = percent.min(K_MAX_VOLUME_PERCENT);
        self.is_locally_muted = self.volume_percent == 0;
        self.volume_percent
    }

    pub fn slider_ratio(&self) -> f32 {
        if self.is_locally_muted {
            0.0
        } else {
            (self.volume_percent as f32) / (K_MAX_VOLUME_PERCENT as f32)
        }
    }

    pub fn speaker_arcs(&self) -> u8 {
        if self.is_locally_muted {
            0
        } else {
            speaker_arc_level(self.slider_ratio())
        }
    }

    pub fn toggle_local_mute(&mut self) -> bool {
        self.is_locally_muted = !self.is_locally_muted;
        self.is_locally_muted
    }
}
