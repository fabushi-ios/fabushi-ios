//! Telegram Stories Media View Component
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_view.h:L38-L148`
//! - `Telegram/SourceFiles/media/stories/media_stories_view.cpp:L18-L194`

use serde::{Deserialize, Serialize};

/// Caption line clamp limits.
/// Upstream: `media_stories_view.h:L80-L81`
pub const K_COLLAPSED_CAPTION_LINES: usize = 2;
pub const K_MAX_SHOWN_CAPTION_LINES: usize = 4;

/// Content layout metrics for story canvas.
/// Upstream: `media_stories_view.h:L43-L49`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContentLayout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub fade: f32,
    pub scale: f32,
    pub radius: f32,
    pub header_outside: bool,
}

impl Default for ContentLayout {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 360.0,
            height: 640.0,
            fade: 0.0,
            scale: 1.0,
            radius: 8.0,
            header_outside: false,
        }
    }
}

/// Stories view wrapper.
/// Upstream: `media_stories_view.h:L83-L146`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesViewStories {
    pub story_id: i32,
    pub peer_id: u64,
    pub layout: ContentLayout,
    pub full_caption_shown: bool,
    pub is_repost: bool,
}

impl MediaStoriesViewStories {
    pub fn new(story_id: i32, peer_id: u64) -> Self {
        Self {
            story_id,
            peer_id,
            layout: ContentLayout::default(),
            full_caption_shown: false,
            is_repost: false,
        }
    }

    pub fn show_full_caption(&mut self) {
        self.full_caption_shown = true;
    }

    pub fn close_full_caption(&mut self) {
        self.full_caption_shown = false;
    }

    pub fn set_dimensions(&mut self, width: f32, height: f32) {
        self.layout.width = width;
        self.layout.height = height;
    }
}
