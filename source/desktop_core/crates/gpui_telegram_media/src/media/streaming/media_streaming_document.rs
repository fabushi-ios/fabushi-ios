//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_document.h:L1-L99`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_document.cpp:L1-L369`
//! Parity Module: `media_streaming_document` | Feature: `TCG-N52` (Streaming Document & Adaptive Quality Switcher)

use super::media_streaming_common::{Information, PlaybackOptions, SpeedEstimate, K_DURATION_UNAVAILABLE, K_TIME_UNKNOWN};
use serde::{Deserialize, Serialize};

pub const K_WAITING_FAST_DURATION_MS: i64 = 200;
pub const K_WAITING_SHOW_DURATION_MS: i64 = 500;
pub const K_WAITING_SHOW_DELAY_MS: i64 = 500;
pub const K_GOOD_THUMB_QUALITY: u32 = 87;
pub const K_SWITCH_QUALITY_UP_PRELOADED_THRESHOLD_MS: i64 = 4000;
pub const K_SWITCH_QUALITY_UP_SPEED_MULTIPLIER: f64 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityDescriptor {
    pub size_in_bytes: u32,
    pub height: u32,
}

/// Ledger Target Symbol: `MediaStreamingDocumentStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingDocumentStreaming {
    pub document_id: u64,
    pub current_file_size: u32,
    pub current_speed: f64,
    pub information: Information,
    pub other_qualities: Vec<QualityDescriptor>,
    pub last_speed_estimate: SpeedEstimate,
    pub is_waiting: bool,
    pub waiting_opacity: f64,
    pub active_instances_count: usize,
    pub requested_quality_height: Option<u32>,
}

impl Default for MediaStreamingDocumentStreaming {
    fn default() -> Self {
        Self {
            document_id: 0,
            current_file_size: 0,
            current_speed: 1.0,
            information: Information::default(),
            other_qualities: Vec::new(),
            last_speed_estimate: SpeedEstimate::default(),
            is_waiting: false,
            waiting_opacity: 0.0,
            active_instances_count: 0,
            requested_quality_height: None,
        }
    }
}

impl MediaStreamingDocumentStreaming {
    pub fn new(document_id: u64, initial_file_size: u32) -> Self {
        Self {
            document_id,
            current_file_size: initial_file_size,
            current_speed: 1.0,
            information: Information::default(),
            other_qualities: Vec::new(),
            last_speed_estimate: SpeedEstimate::default(),
            is_waiting: false,
            waiting_opacity: 0.0,
            active_instances_count: 0,
            requested_quality_height: None,
        }
    }

    pub fn play(&mut self, options: &PlaybackOptions) {
        self.current_speed = options.speed;
        self.information.video.state.position = options.position;
        self.information.audio.state.position = options.position;
        self.set_waiting(true);
    }

    pub fn set_waiting(&mut self, waiting: bool) {
        self.is_waiting = waiting;
        self.waiting_opacity = if waiting { 1.0 } else { 0.0 };
        if waiting {
            let _ = self.check_switch_to_lower_quality();
        }
    }

    /// `media_streaming_document.cpp:L189-L222`
    pub fn check_switch_to_higher_quality(&mut self) -> bool {
        let video_state = &self.information.video.state;
        let duration = video_state.duration;
        let position = video_state.position;
        let received_till = video_state.received_till;

        if self.other_qualities.is_empty()
            || duration == K_TIME_UNKNOWN
            || duration == K_DURATION_UNAVAILABLE
            || position == K_TIME_UNKNOWN
            || received_till == K_TIME_UNKNOWN
            || self.last_speed_estimate.bytes_per_second == 0
            || self.last_speed_estimate.unreliable
            || received_till < duration.min(position + K_SWITCH_QUALITY_UP_PRELOADED_THRESHOLD_MS)
        {
            return false;
        }

        let duration_secs = duration as f64 / 1000.0;
        if duration_secs <= 0.0 {
            return false;
        }

        let multiplier = self.current_speed * K_SWITCH_QUALITY_UP_SPEED_MULTIPLIER;
        let mut target = None;

        for descriptor in &self.other_qualities {
            let per_second = descriptor.size_in_bytes as f64 / duration_secs;
            if descriptor.size_in_bytes > self.current_file_size
                && self.last_speed_estimate.bytes_per_second as f64 >= per_second * multiplier
            {
                target = Some(descriptor.height);
            }
        }

        if let Some(height) = target {
            self.requested_quality_height = Some(height);
            true
        } else {
            false
        }
    }

    /// `media_streaming_document.cpp:L224-L245`
    pub fn check_switch_to_lower_quality(&mut self) -> bool {
        if self.other_qualities.is_empty()
            || !self.is_waiting
            || self.last_speed_estimate.bytes_per_second == 0
        {
            return false;
        }

        let mut target = None;
        let mut best_size = 0;

        for descriptor in &self.other_qualities {
            if descriptor.size_in_bytes < self.current_file_size
                && descriptor.size_in_bytes > best_size
            {
                best_size = descriptor.size_in_bytes;
                target = Some(descriptor.height);
            }
        }

        if let Some(height) = target {
            self.requested_quality_height = Some(height);
            true
        } else {
            false
        }
    }
}
