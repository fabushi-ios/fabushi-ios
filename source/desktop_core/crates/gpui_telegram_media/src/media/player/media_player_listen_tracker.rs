//! Telegram Desktop 1:1 Parity Submodule: `media_player_listen_tracker.rs`
//! Subsystem: `media` | Feature: `TCG-N08` (Music Listen Metrics Tracker)
//! Upstream: `Telegram/SourceFiles/media/player/media_player_listen_tracker.h:L20-L38`
//!           `Telegram/SourceFiles/media/player/media_player_listen_tracker.cpp:L19-L57`

use serde::{Deserialize, Serialize};

/// Minimum seconds listened before reporting to API
/// Upstream: `media_player_listen_tracker.cpp:L19`
pub const REPORT_DURATION_SECONDS_MIN: i64 = 3;

/// Pause timeout before final report
/// Upstream: `media_player_listen_tracker.cpp:L54`
pub const PAUSE_TIMEOUT_MS: i64 = 60 * 1000;

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaPlayerListenTrackerPlayer {
    pub current_document_id: u64,
    pub listened_ms: i64,
    pub is_playing: bool,
    pub report_count: usize,
    pub last_reported_seconds: i64,
}

impl MediaPlayerListenTrackerPlayer {
    pub fn new() -> Self {
        Self {
            current_document_id: 0,
            listened_ms: 0,
            is_playing: false,
            report_count: 0,
            last_reported_seconds: 0,
        }
    }

    pub fn start(&mut self, document_id: u64) {
        if self.current_document_id != document_id {
            self.finalize();
            self.current_document_id = document_id;
            self.listened_ms = 0;
        }
        self.is_playing = true;
    }

    pub fn update_progress(&mut self, elapsed_ms: i64) {
        if self.is_playing && elapsed_ms > 0 {
            self.listened_ms += elapsed_ms;
        }
    }

    pub fn pause(&mut self) {
        self.is_playing = false;
    }

    pub fn should_report(&self) -> bool {
        let duration_secs = self.listened_ms / 1000;
        duration_secs >= REPORT_DURATION_SECONDS_MIN
    }

    pub fn finalize(&mut self) -> Option<i64> {
        let duration_secs = self.listened_ms / 1000;
        if self.should_report() {
            self.report_count += 1;
            self.last_reported_seconds = duration_secs;
            self.listened_ms = 0;
            self.is_playing = false;
            Some(duration_secs)
        } else {
            self.listened_ms = 0;
            self.is_playing = false;
            None
        }
    }
}
