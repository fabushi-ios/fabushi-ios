//! Telegram Stories Reply Bar & Comments Input Area
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_reply.h:L58-L188`
//! - `Telegram/SourceFiles/media/stories/media_stories_reply.cpp:L62-L1038`

use serde::{Deserialize, Serialize};

/// Type of story reply entry point.
/// Upstream: `media_stories_reply.h:L71-L75`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReplyAreaType {
    #[default]
    Reply,
    Comment,
    VideoStreamComment,
}

/// Reply bar context data.
/// Upstream: `media_stories_reply.h:L62-L69`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReplyAreaData {
    pub peer_id: u64,
    pub story_id: i32,
    pub is_video_stream: bool,
    pub stealth_time_left: i32,
    pub stars_per_message: i32,
}

/// Dynamic placeholder generator for reply field.
/// Upstream: `media_stories_reply.cpp:L65-L100`
pub fn determine_reply_placeholder(
    reply_type: ReplyAreaType,
    stealth_time_left: i32,
    stars_per_message: i32,
) -> String {
    if stealth_time_left > 0 {
        format!("Viewed anonymously ({} left)", stealth_time_left)
    } else if reply_type == ReplyAreaType::VideoStreamComment {
        if stars_per_message > 0 {
            format!("Comment ({} stars)", stars_per_message)
        } else {
            "Comment in video stream...".to_string()
        }
    } else if stars_per_message > 0 {
        format!("Send message ({} stars)", stars_per_message)
    } else if reply_type == ReplyAreaType::Comment {
        "Comment on story...".to_string()
    } else {
        "Send a message...".to_string()
    }
}

/// Reply area controller.
/// Upstream: `media_stories_reply.h:L77-L186`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesReplyStories {
    pub data: ReplyAreaData,
    pub reply_type: ReplyAreaType,
    pub is_focused: bool,
    pub current_text: String,
}

impl MediaStoriesReplyStories {
    pub fn new(data: ReplyAreaData, reply_type: ReplyAreaType) -> Self {
        Self {
            data,
            reply_type,
            is_focused: false,
            current_text: String::new(),
        }
    }

    pub fn set_text(&mut self, text: String) {
        self.current_text = text;
    }

    pub fn has_send_text(&self) -> bool {
        !self.current_text.trim().is_empty()
    }

    pub fn placeholder(&self) -> String {
        determine_reply_placeholder(
            self.reply_type,
            self.data.stealth_time_left,
            self.data.stars_per_message,
        )
    }
}
