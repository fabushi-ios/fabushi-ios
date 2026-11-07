//! Telegram Desktop 1:1 Parity Submodule: `MediaViewVideoStreamView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_video_stream.h`
//! - `Telegram/SourceFiles/media/view/media_view_video_stream.cpp:L64-L100`

use serde::{Deserialize, Serialize};

/// Paid reaction donor information for live streams
/// Upstream: `media_view_video_stream.h:L45-L46`, `media_view_video_stream.cpp:L64-L86`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReactionTopPaid {
    pub peer_id: u64,
    pub count: u32,
    pub is_my: bool,
}

/// Primary target symbol for `media_view_video_stream.rs`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaViewVideoStreamView {
    pub call_link_slug: String,
    pub join_message_id: u64,
    pub volume: f64,
    pub comments_shown: bool,
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub donors: Vec<MessageReactionTopPaid>,
    pub is_active: bool,
}

impl Default for MediaViewVideoStreamView {
    fn default() -> Self {
        Self {
            call_link_slug: String::new(),
            join_message_id: 0,
            volume: 1.0,
            comments_shown: true,
            width: 1280,
            height: 720,
            x: 0,
            y: 0,
            donors: Vec::new(),
            is_active: false,
        }
    }
}

impl MediaViewVideoStreamView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_call(slug: impl Into<String>, join_id: u64) -> Self {
        Self {
            call_link_slug: slug.into(),
            join_message_id: join_id,
            is_active: true,
            ..Self::default()
        }
    }

    pub fn set_geometry(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.x = x;
        self.y = y;
        self.width = width;
        self.height = height;
    }

    pub fn toggle_comments(&mut self) -> bool {
        self.comments_shown = !self.comments_shown;
        self.comments_shown
    }

    /// Sets top star donors list
    /// Upstream: `media_view_video_stream.cpp:L64-L86`
    pub fn update_donors(&mut self, mut donors: Vec<MessageReactionTopPaid>, max_shown: usize) {
        donors.sort_by(|a, b| b.count.cmp(&a.count));
        donors.truncate(max_shown);
        self.donors = donors;
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }
}
