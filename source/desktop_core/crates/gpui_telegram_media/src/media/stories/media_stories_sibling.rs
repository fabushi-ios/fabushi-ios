//! Telegram Stories Neighboring Sibling Story Previews & Hover Perspective
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_sibling.h:L20-L80`
//! - `Telegram/SourceFiles/media/stories/media_stories_sibling.cpp:L28-L436`

use serde::{Deserialize, Serialize};

/// Fade transition duration for full-resolution thumbnail in milliseconds.
/// Upstream: `media_stories_sibling.cpp:L30`
pub const K_GOOD_FADE_DURATION_MS: u64 = 200;

/// Base background fade factor for sibling stories.
/// Upstream: `media_stories_sibling.cpp:L31`
pub const K_SIBLING_FADE: f32 = 0.5;

/// Background fade factor when hovering over sibling stories.
/// Upstream: `media_stories_sibling.cpp:L32`
pub const K_SIBLING_FADE_OVER: f32 = 0.4;

/// Base opacity of the peer name label on sibling cards.
/// Upstream: `media_stories_sibling.cpp:L33`
pub const K_SIBLING_NAME_OPACITY: f32 = 0.8;

/// Peer name label opacity on hover.
/// Upstream: `media_stories_sibling.cpp:L34`
pub const K_SIBLING_NAME_OPACITY_OVER: f32 = 1.0;

/// Additional scale factor applied on hover (perspective zoom up to 1.05x).
/// Upstream: `media_stories_sibling.cpp:L35`
pub const K_SIBLING_SCALE_OVER: f32 = 0.05;

/// Computed rendering properties for a sibling preview.
/// Upstream: `media_stories_sibling.cpp:L316-L333`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SiblingView {
    pub fade: f32,
    pub name_opacity: f32,
    pub scale: f32,
    pub is_ready: bool,
}

impl Default for SiblingView {
    fn default() -> Self {
        Self {
            fade: K_SIBLING_FADE,
            name_opacity: K_SIBLING_NAME_OPACITY,
            scale: 1.0,
            is_ready: false,
        }
    }
}

/// Sibling story model and view state calculator.
/// Upstream: `media_stories_sibling.h:L25-L79`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesSiblingStories {
    pub peer_id: u64,
    pub story_id: i32,
    pub user_name: String,
    pub has_good_thumbnail: bool,
    pub hover_progress: f32,
}

impl MediaStoriesSiblingStories {
    pub fn new(peer_id: u64, story_id: i32, user_name: String) -> Self {
        Self {
            peer_id,
            story_id,
            user_name,
            has_good_thumbnail: false,
            hover_progress: 0.0,
        }
    }

    pub fn set_hover(&mut self, over: f32) {
        self.hover_progress = over.clamp(0.0, 1.0);
    }

    /// Computes the visual styling of the sibling card based on hover state.
    /// Upstream: `media_stories_sibling.cpp:L316-L333`
    pub fn compute_view(&self) -> SiblingView {
        let over = self.hover_progress;
        let fade = K_SIBLING_FADE * (1.0 - over) + K_SIBLING_FADE_OVER * over;
        let name_opacity = K_SIBLING_NAME_OPACITY * (1.0 - over) + K_SIBLING_NAME_OPACITY_OVER * over;
        let scale = 1.0 + (over * K_SIBLING_SCALE_OVER);

        SiblingView {
            fade,
            name_opacity,
            scale,
            is_ready: self.has_good_thumbnail,
        }
    }
}
