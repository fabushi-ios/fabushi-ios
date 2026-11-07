//! Telegram Desktop 1:1 Parity Submodule: `MediaViewPlaybackSponsoredView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_playback_sponsored.h`
//! - `Telegram/SourceFiles/media/view/media_view_playback_sponsored.cpp:L37-L38, L61-L100`

use serde::{Deserialize, Serialize};

pub const K_START_DELAY_MIN_MS: i64 = 1000;
pub const K_DURATION_MIN_MS: i64 = 5000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SponsoredState {
    #[default]
    Pending,
    Showing,
    AllowClose,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SponsoredAdData {
    pub title: String,
    pub text: String,
    pub sponsor_url: String,
    pub about_url: Option<String>,
}

/// Primary target symbol for `media_view_playback_sponsored.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewPlaybackSponsoredView {
    pub state: SponsoredState,
    pub ad_data: Option<SponsoredAdData>,
    pub elapsed_ms: i64,
    pub is_paused: bool,
}

impl Default for MediaViewPlaybackSponsoredView {
    fn default() -> Self {
        Self {
            state: SponsoredState::Pending,
            ad_data: None,
            elapsed_ms: 0,
            is_paused: false,
        }
    }
}

impl MediaViewPlaybackSponsoredView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ad(ad: SponsoredAdData) -> Self {
        Self {
            state: SponsoredState::Pending,
            ad_data: Some(ad),
            elapsed_ms: 0,
            is_paused: false,
        }
    }

    /// Advances playback timer
    /// Upstream: `media_view_playback_sponsored.cpp:L61-L100`
    pub fn tick(&mut self, delta_ms: i64) {
        if self.is_paused || self.state == SponsoredState::Dismissed {
            return;
        }

        self.elapsed_ms += delta_ms;

        if self.state == SponsoredState::Pending && self.elapsed_ms >= K_START_DELAY_MIN_MS {
            self.state = SponsoredState::Showing;
        }

        if self.state == SponsoredState::Showing && self.elapsed_ms >= (K_START_DELAY_MIN_MS + K_DURATION_MIN_MS) {
            self.state = SponsoredState::AllowClose;
        }
    }

    /// Remaining seconds before close is enabled
    pub fn countdown_seconds(&self) -> i32 {
        let total_required = K_START_DELAY_MIN_MS + K_DURATION_MIN_MS;
        let remaining = total_required - self.elapsed_ms;
        if remaining <= 0 {
            0
        } else {
            ((remaining + 999) / 1000) as i32
        }
    }

    /// Closes the sponsored ad card if allowed
    pub fn dismiss(&mut self) -> bool {
        if self.state == SponsoredState::AllowClose {
            self.state = SponsoredState::Dismissed;
            true
        } else {
            false
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.is_paused = paused;
    }
}
