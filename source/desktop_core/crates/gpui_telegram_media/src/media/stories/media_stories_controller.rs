//! Telegram Stories Playback Controller & Gesture FSM
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_controller.h:L70-L406`
//! - `Telegram/SourceFiles/media/stories/media_stories_controller.cpp:L68-L2139`

use serde::{Deserialize, Serialize};

/// Photo story playback duration and progress polling interval in milliseconds.
/// Upstream: `media_stories_controller.cpp:L71-L72`
pub const K_PHOTO_PROGRESS_INTERVAL_MS: u64 = 100;
pub const K_PHOTO_DURATION_MS: u64 = 5000;

/// Visual layout multipliers.
/// Upstream: `media_stories_controller.cpp:L73-L78`
pub const K_FULL_CONTENT_FADE: f32 = 0.6;
pub const K_SIBLING_MULTIPLIER_DEFAULT: f32 = 0.448;
pub const K_SIBLING_MULTIPLIER_MAX: f32 = 0.72;
pub const K_SIBLING_OUTSIDE_PART: f32 = 0.24;
pub const K_SIBLING_USERPIC_SIZE: f32 = 0.3;
pub const K_INNER_HEIGHT_MULTIPLIER: f32 = 1.6;

/// Preload batch size limits.
/// Upstream: `media_stories_controller.cpp:L79-L82`
pub const K_PRELOAD_PEERS_COUNT: usize = 3;
pub const K_PRELOAD_STORIES_COUNT: usize = 5;
pub const K_PRELOAD_NEXT_MEDIA_COUNT: usize = 3;
pub const K_PRELOAD_PREVIOUS_MEDIA_COUNT: usize = 1;

/// Auto-mark-as-read threshold in seconds.
/// Upstream: `media_stories_controller.cpp:L83`
pub const K_MARK_AS_READ_AFTER_SECONDS: f32 = 0.2;

/// Playback pause state.
/// Upstream: `media_stories_controller.h:L97-L102`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PauseState {
    #[default]
    Playing,
    Paused,
    Inactive,
    InactivePaused,
}

/// Photo playback progress tracker with pause offset compensation.
/// Upstream: `media_stories_controller.cpp:L185-L235`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoPlayback {
    pub started: i64,
    pub paused_at: Option<i64>,
    pub duration_ms: u64,
}

impl PhotoPlayback {
    pub fn new(now_ms: i64, duration_ms: u64) -> Self {
        Self {
            started: now_ms,
            paused_at: None,
            duration_ms,
        }
    }

    pub fn toggle_paused(&mut self, paused: bool, now_ms: i64) {
        if paused {
            if self.paused_at.is_none() {
                self.paused_at = Some(now_ms);
            }
        } else if let Some(p) = self.paused_at.take() {
            let paused_duration = (now_ms - p).max(0);
            self.started += paused_duration;
        }
    }

    pub fn elapsed(&self, now_ms: i64) -> u64 {
        let reference = self.paused_at.unwrap_or(now_ms);
        (reference - self.started).max(0) as u64
    }

    pub fn progress(&self, now_ms: i64) -> f32 {
        if self.duration_ms == 0 {
            return 1.0;
        }
        (self.elapsed(now_ms) as f32 / self.duration_ms as f32).clamp(0.0, 1.0)
    }

    pub fn is_finished(&self, now_ms: i64) -> bool {
        self.elapsed(now_ms) >= self.duration_ms
    }
}

/// Stories media viewer controller.
/// Upstream: `media_stories_controller.h:L129-L215`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStoriesControllerStories {
    pub current_index: usize,
    pub total_stories: usize,
    pub peer_id: u64,
    pub pause_state: PauseState,
    pub playback: PhotoPlayback,
    pub marked_read: bool,
    pub has_left_sibling: bool,
    pub has_right_sibling: bool,
}

impl MediaStoriesControllerStories {
    pub fn new(peer_id: u64, total_stories: usize, current_index: usize, now_ms: i64) -> Self {
        let total = total_stories.max(1);
        let index = current_index.min(total - 1);
        Self {
            current_index: index,
            total_stories: total,
            peer_id,
            pause_state: PauseState::Playing,
            playback: PhotoPlayback::new(now_ms, K_PHOTO_DURATION_MS),
            marked_read: false,
            has_left_sibling: false,
            has_right_sibling: false,
        }
    }

    pub fn toggle_paused(&mut self, paused: bool, now_ms: i64) {
        self.pause_state = if paused {
            PauseState::Paused
        } else {
            PauseState::Playing
        };
        self.playback.toggle_paused(paused, now_ms);
    }

    pub fn content_pressed(&mut self, pressed: bool, now_ms: i64) {
        // Upstream: `media_stories_controller.cpp:L1478-L1486`
        self.toggle_paused(pressed, now_ms);
    }

    pub fn subjump_available(&self, delta: i32) -> bool {
        let target = self.current_index as i32 + delta;
        if target < 0 {
            self.has_left_sibling
        } else if target >= self.total_stories as i32 {
            self.has_right_sibling
        } else {
            true
        }
    }

    pub fn subjump_for(&mut self, delta: i32, now_ms: i64) -> bool {
        if delta > 0 {
            self.marked_read = true;
        }

        let target = self.current_index as i32 + delta;
        if target < 0 {
            if self.has_left_sibling {
                self.jump_for(-1)
            } else {
                self.current_index = 0;
                self.playback = PhotoPlayback::new(now_ms, K_PHOTO_DURATION_MS);
                true
            }
        } else if target >= self.total_stories as i32 {
            if self.has_right_sibling {
                self.jump_for(1)
            } else {
                false
            }
        } else {
            self.current_index = target as usize;
            self.playback = PhotoPlayback::new(now_ms, K_PHOTO_DURATION_MS);
            true
        }
    }

    pub fn jump_for(&mut self, delta: i32) -> bool {
        if delta < 0 && self.has_left_sibling {
            true
        } else if delta > 0 && self.has_right_sibling {
            self.marked_read = true;
            true
        } else {
            false
        }
    }
}
