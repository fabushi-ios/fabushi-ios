//! 5-Star Call Rating & Post-Call Feedback
//! Upstream: Telegram/SourceFiles/calls/calls_rate_call.h:L25-L115, calls_rate_call.cpp:L20-L120
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsRateCall {
    pub call_id: i64,
    pub rating: i32,
    pub comment: String,
    pub is_submitted: bool,
    pub burst_count: usize,
    pub dark_opacity: f32,
    pub show_duration_ms: u64,
}

impl CallsRateCall {
    pub const K_STARS_COUNT: usize = 5;
    pub const K_BURST_MIN_RATING: i32 = 4;
    pub const K_SHOW_DURATION_MS: u64 = 250;
    pub const K_STAR_SHOW_DELAY_MS: u64 = 16;
    pub const K_SELECT_DURATION_MS: u64 = 250;
    pub const K_DARK_OPACITY: f32 = 0.14;
    pub const K_PATH_SIZE: f32 = 96.0;
    pub const K_PATH_STROKE: f32 = 6.0;
    pub const K_PATH_MITER_LIMIT: f32 = 4.0;
    pub const K_MAX_BURSTS: usize = 5;

    pub fn new(call_id: i64) -> Self {
        Self {
            call_id,
            rating: 0,
            comment: String::new(),
            is_submitted: false,
            burst_count: 0,
            dark_opacity: Self::K_DARK_OPACITY,
            show_duration_ms: Self::K_SHOW_DURATION_MS,
        }
    }

    pub fn set_rating(&mut self, stars: i32) -> bool {
        self.rating = stars.clamp(1, Self::K_STARS_COUNT as i32);
        let triggered_burst = self.rating >= Self::K_BURST_MIN_RATING;
        if triggered_burst {
            self.burst_count = (self.burst_count + 1).min(Self::K_MAX_BURSTS);
        }
        triggered_burst
    }

    pub fn submit(&mut self, comment: impl Into<String>) {
        self.comment = comment.into();
        self.is_submitted = true;
    }
}
