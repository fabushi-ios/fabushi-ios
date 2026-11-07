//! Telegram Stories Stealth Mode Viewer Controls & Timer
//!
//! Upstream Source Reference:
//! - `Telegram/SourceFiles/media/stories/media_stories_stealth.h:L26-L45`
//! - `Telegram/SourceFiles/media/stories/media_stories_stealth.cpp:L37-L426`

use serde::{Deserialize, Serialize};

/// Duration of the "already active" toast notification in milliseconds.
/// Upstream: `media_stories_stealth.cpp:L39`
pub const K_ALREADY_TOAST_DURATION_MS: u64 = 4000;

/// Opacity of the stealth mode action button during cooldown.
/// Upstream: `media_stories_stealth.cpp:L40`
pub const K_COOLDOWN_BUTTON_LABEL_OPACITY: f32 = 0.5;

/// Formats the remaining time (in seconds) for stealth mode countdown.
/// Upstream: `media_stories_stealth.cpp:L410-L426`
pub fn time_left_text(left: i32) -> String {
    let left = left.max(0);
    let hours = left / 3600;
    let minutes = (left % 3600) / 60;
    let seconds = left % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}:{:02}", minutes, seconds)
    } else {
        format!("0:{:02}", seconds)
    }
}

/// Stealth mode operational state machine.
/// Upstream: `media_stories_stealth.cpp:L42-L47`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StealthModeState {
    pub enabled_till: i64,
    pub cooldown_till: i64,
    pub now: i64,
    pub premium: bool,
    pub has_callback: bool,
}

impl StealthModeState {
    pub fn is_active(&self) -> bool {
        self.enabled_till > self.now
    }

    pub fn is_cooldown(&self) -> bool {
        self.premium && (self.cooldown_till > self.now)
    }

    pub fn time_left(&self) -> i32 {
        (self.enabled_till - self.now).max(0) as i32
    }

    pub fn cooldown_left(&self) -> i32 {
        (self.cooldown_till - self.now).max(0) as i32
    }

    pub fn button_opacity(&self) -> f32 {
        if self.is_cooldown() {
            K_COOLDOWN_BUTTON_LABEL_OPACITY
        } else {
            1.0
        }
    }

    pub fn lock_visible(&self) -> bool {
        !self.premium
    }
}

/// Controller component for story stealth mode viewing.
/// Upstream: `media_stories_stealth.h:L26-L45`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaStoriesStealthStories {
    pub state: StealthModeState,
}

impl MediaStoriesStealthStories {
    pub fn new(state: StealthModeState) -> Self {
        Self { state }
    }

    pub fn update_now(&mut self, now: i64) {
        self.state.now = now;
    }

    pub fn activate(&mut self, duration_seconds: i64, cooldown_seconds: i64) -> bool {
        if !self.state.premium || self.state.is_cooldown() {
            return false;
        }
        self.state.enabled_till = self.state.now + duration_seconds;
        self.state.cooldown_till = self.state.now + cooldown_seconds;
        true
    }

    pub fn formatted_time_left(&self) -> String {
        time_left_text(self.state.time_left())
    }

    pub fn formatted_cooldown_left(&self) -> String {
        time_left_text(self.state.cooldown_left())
    }
}
