//! Telegram Desktop 1:1 Parity Submodule: `MediaViewPlaybackProgressView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_playback_progress.h`
//! - `Telegram/SourceFiles/media/view/media_view_playback_progress.cpp:L16, L29-L87, L104-L129`

use serde::{Deserialize, Serialize};

pub const K_PLAYBACK_ANIMATION_DURATION_MS: i64 = 200;

/// Smooth playback progress tracker
/// Upstream: `media_view_playback_progress.h:L19-L59`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewPlaybackProgressView {
    pub value: f64,
    pub available_till: f64,
    pub position: i64,
    pub length: i64,
    pub is_playing: bool,
    pub in_loading_state: bool,
}

impl Default for MediaViewPlaybackProgressView {
    fn default() -> Self {
        Self {
            value: 0.0,
            available_till: 0.0,
            position: 0,
            length: 0,
            is_playing: false,
            in_loading_state: false,
        }
    }
}

impl MediaViewPlaybackProgressView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates playback and buffer availability states
    /// Upstream: `media_view_playback_progress.cpp:L29-L87`
    pub fn update_state(
        &mut self,
        position: i64,
        length: i64,
        frequency: i64,
        received_till: i64,
        loaded_till_percent: f64,
    ) {
        self.is_playing = position > 0 && position < length;
        let received = if length > 0 && received_till > position {
            received_till
        } else {
            -1
        };
        let loaded = if loaded_till_percent > 0.0 && length > 0 {
            (loaded_till_percent * length as f64).floor() as i64
        } else {
            -1
        };
        let available = if length > 0 && loaded > position {
            received.max(loaded)
        } else {
            received
        };

        self.in_loading_state = false;

        let progress = if length > 0 {
            (position as f64 / length as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let available_progress = if available > position && length > 0 {
            (available as f64 / length as f64).clamp(0.0, 1.0)
        } else {
            progress
        };

        // Forward extrapolation for smooth animation
        // Upstream: `media_view_playback_progress.cpp:L65-L68`
        let animated_position = position + (frequency * K_PLAYBACK_ANIMATION_DURATION_MS / 1000);
        let animated_progress = if length > 0 {
            (animated_position as f64 / length as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };

        if length > 0 && animated_progress > self.value && position >= self.position {
            self.value = animated_progress;
        } else {
            self.value = progress;
        }

        self.available_till = available_progress;
        self.position = position;
        self.length = length;
    }

    /// Direct progress setter
    pub fn set_value(&mut self, value: f64) {
        self.value = value.clamp(0.0, 1.0);
    }
}
