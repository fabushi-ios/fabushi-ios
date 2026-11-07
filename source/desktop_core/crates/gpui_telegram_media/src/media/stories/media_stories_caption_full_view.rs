//! Telegram Stories Expanded Full Caption View
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_caption_full_view.h:L23-L54`
//! - `Telegram/SourceFiles/media/stories/media_stories_caption_full_view.cpp:L24-L226`

use serde::{Deserialize, Serialize};

/// State for full-screen expanded caption view overlay.
/// Upstream: `media_stories_caption_full_view.h:L27-L52`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CaptionFullViewState {
    pub is_open: bool,
    pub is_closing: bool,
    pub is_pulling: bool,
    pub is_down: bool,
    pub full_text: String,
    pub scroll_offset: f32,
}

impl CaptionFullViewState {
    pub fn new(text: String) -> Self {
        Self {
            is_open: true,
            is_closing: false,
            is_pulling: false,
            is_down: false,
            full_text: text,
            scroll_offset: 0.0,
        }
    }

    pub fn close(&mut self) {
        self.is_closing = true;
    }

    pub fn finish_closing(&mut self) {
        self.is_open = false;
        self.is_closing = false;
    }
}

/// Helper for calculating caption scroll container margins.
/// Upstream: `media_stories_caption_full_view.cpp:L43-L49`
pub fn compute_caption_top_skip(header_y: i32, header_height: i32, content_y: i32) -> i32 {
    (header_y + header_height - content_y).max(0)
}

/// Full caption view model.
/// Upstream: `media_stories_caption_full_view.h:L27-L52`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesCaptionFullViewStories {
    pub state: CaptionFullViewState,
    pub top_skip: i32,
}

impl MediaStoriesCaptionFullViewStories {
    pub fn new(text: String, header_y: i32, header_height: i32, content_y: i32) -> Self {
        let top_skip = compute_caption_top_skip(header_y, header_height, content_y);
        Self {
            state: CaptionFullViewState::new(text),
            top_skip,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.state.is_open
    }

    pub fn is_closing(&self) -> bool {
        self.state.is_closing
    }

    pub fn request_close(&mut self) {
        self.state.close();
    }
}
