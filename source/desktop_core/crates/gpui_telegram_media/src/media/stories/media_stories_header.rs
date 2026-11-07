//! Telegram Stories Header Bar & Privacy Controls
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_header.h:L28-L114`
//! - `Telegram/SourceFiles/media/stories/media_stories_header.cpp:L38-L976`

use serde::{Deserialize, Serialize};

/// Opacities for header UI text and controls.
/// Upstream: `media_stories_header.cpp:L40-L44`
pub const K_NAME_OPACITY: f32 = 1.0;
pub const K_DATE_OPACITY: f32 = 0.8;
pub const K_CONTROL_OPACITY: f32 = 0.65;
pub const K_CONTROL_OPACITY_OVER: f32 = 1.0;
pub const K_CONTROL_OPACITY_DISABLED: f32 = 0.45;

/// Volume dropdown hide debounce timeouts in milliseconds.
/// Upstream: `media_stories_header.cpp:L45-L46`
pub const K_VOLUME_HIDE_TIMEOUT_SHORT_MS: u64 = 20;
pub const K_VOLUME_HIDE_TIMEOUT_LONG_MS: u64 = 200;

/// Story privacy levels.
/// Upstream: `media_stories_header.cpp:L79-L97`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum StoryPrivacyLevel {
    #[default]
    Public,
    CloseFriends,
    Contacts,
    SelectedContacts,
}

/// Header bar display data.
/// Upstream: `media_stories_header.h:L33-L49`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HeaderData {
    pub peer_id: u64,
    pub peer_name: String,
    pub date: i64,
    pub full_index: usize,
    pub full_count: usize,
    pub privacy: StoryPrivacyLevel,
    pub edited: bool,
    pub is_video: bool,
    pub is_video_stream: bool,
    pub is_silent: bool,
}

/// Story header component model.
/// Upstream: `media_stories_header.h:L51-L112`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesHeaderStories {
    pub data: HeaderData,
    pub is_paused: bool,
    pub volume: f64,
    pub is_muted: bool,
}

impl MediaStoriesHeaderStories {
    pub fn new(data: HeaderData) -> Self {
        Self {
            data,
            is_paused: false,
            volume: 1.0,
            is_muted: false,
        }
    }

    pub fn toggle_pause(&mut self) -> bool {
        self.is_paused = !self.is_paused;
        self.is_paused
    }

    pub fn toggle_mute(&mut self) -> bool {
        self.is_muted = !self.is_muted;
        self.is_muted
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
        self.is_muted = self.volume <= 0.0;
    }

    pub fn has_privacy_badge(&self) -> bool {
        self.data.privacy != StoryPrivacyLevel::Public
    }
}
