//! Telegram Stories Media Viewer Delegate
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_delegate.h:L28-L64`
//! - `Telegram/SourceFiles/media/stories/media_stories_delegate.cpp:L1-L10`

use serde::{Deserialize, Serialize};

/// Reason for moving to another story.
/// Upstream: `media_stories_delegate.h:L30-L33`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum JumpReason {
    #[default]
    Finished,
    User,
}

/// Sibling navigation direction.
/// Upstream: `media_stories_delegate.h:L35-L38`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SiblingType {
    Left,
    Right,
}

/// Delegate state for story media viewer actions and callbacks.
/// Upstream: `media_stories_delegate.h:L40-L62`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStoriesDelegateStories {
    pub paused: bool,
    pub volume: f64,
    pub top_notch_skip: i32,
    pub sibling_over_left: f64,
    pub sibling_over_right: f64,
    pub layer_shown: bool,
}

impl Default for MediaStoriesDelegateStories {
    fn default() -> Self {
        Self {
            paused: false,
            volume: 1.0,
            top_notch_skip: 0,
            sibling_over_left: 0.0,
            sibling_over_right: 0.0,
            layer_shown: false,
        }
    }
}

impl MediaStoriesDelegateStories {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn toggle_paused(&mut self) -> bool {
        self.paused = !self.paused;
        self.paused
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn sibling_over(&self, sibling: SiblingType) -> f64 {
        match sibling {
            SiblingType::Left => self.sibling_over_left,
            SiblingType::Right => self.sibling_over_right,
        }
    }

    pub fn set_sibling_over(&mut self, sibling: SiblingType, over: f64) {
        let clamped = over.clamp(0.0, 1.0);
        match sibling {
            SiblingType::Left => self.sibling_over_left = clamped,
            SiblingType::Right => self.sibling_over_right = clamped,
        }
    }
}
