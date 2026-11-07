//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_audio_track.h:L1-L96`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_audio_track.cpp:L1-L254`
//! Parity Module: `media_streaming_audio_track` | Feature: `TCG-N52` (Audio Track Decode & Mixer Feed)

use super::media_streaming_common::{K_TIME_UNKNOWN, PlaybackOptions, TrackState};
use serde::{Deserialize, Serialize};

/// Ledger Target Symbol: `MediaStreamingAudioTrackStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingAudioTrackStreaming {
    pub stream_index: i32,
    pub stream_time_base: (i32, i32),
    pub stream_duration: i64,
    pub frequency: u32,
    pub play_position: i64,
    pub started_position: i64,
    pub received_till: i64,
    pub speed: f64,
    pub volume: f64,
    pub is_paused: bool,
    pub is_waiting_for_data: bool,
    pub is_initialized: bool,
    pub read_till_end: bool,
    pub packets_processed: usize,
}

impl Default for MediaStreamingAudioTrackStreaming {
    fn default() -> Self {
        Self {
            stream_index: 0,
            stream_time_base: (1, 1000),
            stream_duration: 0,
            frequency: 48000,
            play_position: 0,
            started_position: K_TIME_UNKNOWN,
            received_till: 0,
            speed: 1.0,
            volume: 1.0,
            is_paused: false,
            is_waiting_for_data: false,
            is_initialized: false,
            read_till_end: false,
            packets_processed: 0,
        }
    }
}

impl MediaStreamingAudioTrackStreaming {
    pub fn new(options: &PlaybackOptions, duration: i64, frequency: u32) -> Self {
        Self {
            stream_index: 1,
            stream_time_base: (1, 1000),
            stream_duration: duration,
            frequency,
            play_position: options.position,
            started_position: options.position,
            received_till: options.position,
            speed: options.speed.clamp(0.5, 2.0),
            volume: options.volume.clamp(0.0, 1.0),
            is_paused: false,
            is_waiting_for_data: false,
            is_initialized: true,
            read_till_end: false,
            packets_processed: 0,
        }
    }

    /// `media_streaming_audio_track.cpp:L50-L66`
    pub fn process_packet(&mut self, pts: i64) {
        self.packets_processed += 1;
        self.received_till = pts.max(self.received_till);
        if self.stream_duration > 0 && self.received_till >= self.stream_duration {
            self.read_till_end = true;
        }
        self.is_waiting_for_data = false;
    }

    pub fn pause(&mut self) {
        self.is_paused = true;
    }

    pub fn resume(&mut self) {
        self.is_paused = false;
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.5, 2.0);
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn track_state(&self) -> TrackState {
        TrackState {
            position: self.play_position,
            received_till: self.received_till,
            duration: self.stream_duration,
        }
    }
}
