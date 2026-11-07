//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_instance.h:L1-L106`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_instance.cpp:L1-L271`
//! Parity Module: `media_streaming_instance` | Feature: `TCG-N52` (Streaming Consumer Instance Handle)

use super::media_streaming_common::{FrameWithInfo, PlaybackOptions, StreamingError};
use serde::{Deserialize, Serialize};

/// Ledger Target Symbol: `MediaStreamingInstanceStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingInstanceStreaming {
    pub instance_id: u64,
    pub document_id: u64,
    pub priority: i32,
    pub is_player_locked: bool,
    pub is_active: bool,
    pub is_paused: bool,
    pub speed: f64,
    pub volume: f64,
    pub frames_shown_count: usize,
    pub last_frame: Option<FrameWithInfo>,
    pub last_error: Option<StreamingError>,
}

impl Default for MediaStreamingInstanceStreaming {
    fn default() -> Self {
        Self {
            instance_id: 1,
            document_id: 0,
            priority: 1,
            is_player_locked: false,
            is_active: false,
            is_paused: false,
            speed: 1.0,
            volume: 1.0,
            frames_shown_count: 0,
            last_frame: None,
            last_error: None,
        }
    }
}

impl MediaStreamingInstanceStreaming {
    pub fn new(instance_id: u64, document_id: u64) -> Self {
        Self {
            instance_id,
            document_id,
            priority: 1,
            is_player_locked: false,
            is_active: false,
            is_paused: false,
            speed: 1.0,
            volume: 1.0,
            frames_shown_count: 0,
            last_frame: None,
            last_error: None,
        }
    }

    pub fn play(&mut self, options: PlaybackOptions) {
        let clean = options.sanitized();
        self.is_active = true;
        self.is_paused = false;
        self.speed = clean.speed;
        self.volume = clean.volume;
    }

    pub fn pause(&mut self) {
        self.is_paused = true;
    }

    pub fn resume(&mut self) {
        self.is_paused = false;
    }

    pub fn stop(&mut self) {
        self.is_active = false;
        self.is_paused = false;
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.5, 2.0);
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn lock_player(&mut self) {
        self.is_player_locked = true;
    }

    pub fn unlock_player(&mut self) {
        self.is_player_locked = false;
    }

    pub fn mark_frame_shown(&mut self) -> bool {
        self.frames_shown_count += 1;
        true
    }

    pub fn set_priority(&mut self, priority: i32) {
        self.priority = priority;
    }
}
