//! Telegram Stories Repost & Quote Attribution View
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_repost_view.h:L23-L71`
//! - `Telegram/SourceFiles/media/stories/media_stories_repost_view.cpp:L30-L298`

use serde::{Deserialize, Serialize};

/// Repost quote layout metrics.
/// Upstream: `media_stories_repost_view.cpp:L59-L78`
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RepostLayoutMetrics {
    pub padding_top: i32,
    pub padding_bottom: i32,
    pub semibold_font_height: i32,
    pub normal_font_height: i32,
}

impl Default for RepostLayoutMetrics {
    fn default() -> Self {
        Self {
            padding_top: 4,
            padding_bottom: 4,
            semibold_font_height: 16,
            normal_font_height: 14,
        }
    }
}

impl RepostLayoutMetrics {
    /// Full multi-line repost height with title and preview text.
    /// Upstream: `media_stories_repost_view.cpp:L59-L64`
    pub fn full_height(&self) -> i32 {
        self.padding_top + self.semibold_font_height + self.normal_font_height + self.padding_bottom
    }

    /// Compact single-line repost height (when caption/preview text is empty).
    /// Upstream: `media_stories_repost_view.cpp:L73-L78`
    pub fn simple_height(&self) -> i32 {
        self.full_height() - self.normal_font_height
    }
}

/// Repost source metadata.
/// Upstream: `media_stories_repost_view.h:L51-L66`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RepostSourceInfo {
    pub source_peer_id: u64,
    pub source_post_id: Option<i32>,
    pub source_name: String,
    pub text: String,
}

/// Repost view component for stories.
/// Upstream: `media_stories_repost_view.h:L28-L69`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesRepostViewStories {
    pub source: RepostSourceInfo,
    pub metrics: RepostLayoutMetrics,
}

impl MediaStoriesRepostViewStories {
    pub fn new(source: RepostSourceInfo) -> Self {
        Self {
            source,
            metrics: RepostLayoutMetrics::default(),
        }
    }

    pub fn is_simple(&self) -> bool {
        self.source.text.trim().is_empty()
    }

    pub fn height(&self) -> i32 {
        if self.is_simple() {
            self.metrics.simple_height()
        } else {
            self.metrics.full_height()
        }
    }

    pub fn quote_bg_rgba(&self) -> (u8, u8, u8, u8) {
        // Upstream: `media_stories_repost_view.cpp:L95` cache->bg = QColor(0, 0, 0, 64);
        (0, 0, 0, 64)
    }
}
