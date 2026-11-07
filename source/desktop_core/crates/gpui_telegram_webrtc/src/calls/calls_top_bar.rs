//! Floating Top Bar with Call Duration & Audio Waves
//! Upstream: Telegram/SourceFiles/calls/calls_top_bar.h:L41-L113, calls_top_bar.cpp:L44-L105
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarState {
    Connecting,
    Active,
    Muted,
    ForceMuted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallsTopBar {
    pub call_id: i64,
    pub bar_state: BarState,
    pub duration_seconds: u32,
    pub is_muted: bool,
    pub audio_level: f32,
    pub minor_blob_alpha: f32,
    pub users_count: usize,
}

impl CallsTopBar {
    pub const K_UPDATE_DEBUG_TIMEOUT_MS: u64 = 500;
    pub const K_MINOR_BLOB_ALPHA: f32 = 76.0 / 255.0;
    pub const K_HIDE_BLOBS_DURATION_MS: u64 = 500;
    pub const K_BLOB_LEVEL_DURATION_MS: u64 = 250;
    pub const K_BLOB_UPDATE_INTERVAL_MS: u64 = 100;

    pub fn new(call_id: i64) -> Self {
        Self {
            call_id,
            bar_state: BarState::Connecting,
            duration_seconds: 0,
            is_muted: false,
            audio_level: 0.0,
            minor_blob_alpha: Self::K_MINOR_BLOB_ALPHA,
            users_count: 2,
        }
    }

    pub fn tick_duration(&mut self) -> u32 {
        self.duration_seconds = self.duration_seconds.saturating_add(1);
        self.duration_seconds
    }

    pub fn format_duration(&self) -> String {
        let hours = self.duration_seconds / 3600;
        let minutes = (self.duration_seconds % 3600) / 60;
        let seconds = self.duration_seconds % 60;
        if hours > 0 {
            format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
        } else {
            format!("{:02}:{:02}", minutes, seconds)
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.is_muted = muted;
        self.bar_state = if muted {
            BarState::Muted
        } else {
            BarState::Active
        };
    }

    pub fn set_audio_level(&mut self, level: f32) {
        self.audio_level = level.clamp(0.0, 1.0);
    }
}
