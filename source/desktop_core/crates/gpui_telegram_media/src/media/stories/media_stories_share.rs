//! Telegram Stories Share & Timestamp Deep-Link Controls
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_share.h:L20-L34`
//! - `Telegram/SourceFiles/media/stories/media_stories_share.cpp:L35-L319`

use serde::{Deserialize, Serialize};

/// Formats a video timestamp in seconds to standard time format ("h:mm:ss" or "m:ss").
/// Upstream: `media_stories_share.cpp:L237-L246`
pub fn format_share_at_time(seconds: i32) -> String {
    let seconds = seconds.max(0);
    let minutes = seconds / 60;
    let h = minutes / 60;
    let m = minutes % 60;
    let s = seconds % 60;

    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, s)
    } else {
        format!("{}:{:02}", m, s)
    }
}

/// Share dialog configuration and state.
/// Upstream: `media_stories_share.cpp:L36-L87`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShareStoryConfig {
    pub peer_id: u64,
    pub story_id: i32,
    pub has_direct_link: bool,
    pub is_call_story: bool,
    pub video_timestamp: Option<i32>,
}

impl ShareStoryConfig {
    pub fn can_copy_link(&self) -> bool {
        self.has_direct_link && !self.is_call_story
    }

    pub fn count_messages_for_comment(&self, comment: &str) -> usize {
        if self.is_call_story || comment.is_empty() {
            1
        } else {
            2
        }
    }

    pub fn formatted_timestamp(&self) -> Option<String> {
        self.video_timestamp.map(format_share_at_time)
    }
}

/// Story sharing controller component.
/// Upstream: `media_stories_share.h:L20-L34`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaStoriesShareStories {
    pub config: ShareStoryConfig,
}

impl MediaStoriesShareStories {
    pub fn new(config: ShareStoryConfig) -> Self {
        Self { config }
    }

    pub fn generate_story_link(&self, username: Option<&str>) -> Option<String> {
        if !self.config.has_direct_link {
            return None;
        }

        let base = match username {
            Some(u) => format!("https://t.me/{}/s/{}", u, self.config.story_id),
            None => format!("https://t.me/c/{}/s/{}", self.config.peer_id, self.config.story_id),
        };

        if let Some(ts) = self.config.video_timestamp {
            Some(format!("{}?t={}", base, ts))
        } else {
            Some(base)
        }
    }
}
