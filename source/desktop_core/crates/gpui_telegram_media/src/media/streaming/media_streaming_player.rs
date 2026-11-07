//! Upstream Sources:
//! - `Telegram/SourceFiles/media/streaming/media_streaming_player.h:L1-L225`
//! - `Telegram/SourceFiles/media/streaming/media_streaming_player.cpp:L1-L1044`
//! Parity Module: `media_streaming_player` | Feature: `TCG-N52` (Streaming Master Player Controller)

use super::media_streaming_common::{Information, PlaybackOptions, StreamingError, TrackState, K_TIME_UNKNOWN};
use serde::{Deserialize, Serialize};

/// `media_streaming_player.cpp:L22-L25`
pub const K_BUFFER_FOR_MS: i64 = 3000;
pub const K_LOAD_IN_ADVANCE_REMOTE_MS: i64 = 32000;
pub const K_LOAD_IN_ADVANCE_LOCAL_MS: i64 = 5000;
pub const K_MS_FREQUENCY: i64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerState {
    Uninitialized,
    Initializing,
    Playing,
    Buffering,
    Paused,
    Finished,
    Failed,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self::Uninitialized
    }
}

/// Ledger Target Symbol: `MediaStreamingPlayerStreaming`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaStreamingPlayerStreaming {
    pub is_remote: bool,
    pub state: PlayerState,
    pub options: PlaybackOptions,
    pub information: Information,
    pub is_full_in_cache: bool,
    pub looping_shift: i64,
    pub last_error: Option<StreamingError>,
    pub playback_start_ms: i64,
}

impl Default for MediaStreamingPlayerStreaming {
    fn default() -> Self {
        Self {
            is_remote: true,
            state: PlayerState::Uninitialized,
            options: PlaybackOptions::default(),
            information: Information::default(),
            is_full_in_cache: false,
            looping_shift: 0,
            last_error: None,
            playback_start_ms: 0,
        }
    }
}

impl MediaStreamingPlayerStreaming {
    pub fn new(is_remote: bool) -> Self {
        Self {
            is_remote,
            state: PlayerState::Uninitialized,
            options: PlaybackOptions::default(),
            information: Information::default(),
            is_full_in_cache: false,
            looping_shift: 0,
            last_error: None,
            playback_start_ms: 0,
        }
    }

    /// `media_streaming_player.cpp:L141-L142`
    pub fn load_in_advance_for(&self) -> i64 {
        if self.is_remote {
            K_LOAD_IN_ADVANCE_REMOTE_MS
        } else {
            K_LOAD_IN_ADVANCE_LOCAL_MS
        }
    }

    pub fn play(&mut self, options: PlaybackOptions) {
        self.options = options.sanitized();
        self.state = PlayerState::Buffering;
        self.playback_start_ms = chrono::Utc::now().timestamp_millis();
    }

    pub fn pause(&mut self) {
        if self.state == PlayerState::Playing || self.state == PlayerState::Buffering {
            self.state = PlayerState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == PlayerState::Paused {
            self.state = if self.is_buffered() {
                PlayerState::Playing
            } else {
                PlayerState::Buffering
            };
        }
    }

    pub fn stop(&mut self) {
        self.state = PlayerState::Finished;
    }

    /// `media_streaming_player.cpp:L134-L135`
    pub fn is_buffered(&self) -> bool {
        let video_ok = self.track_received_enough(&self.information.video.state, K_BUFFER_FOR_MS);
        let audio_ok = self.track_received_enough(&self.information.audio.state, K_BUFFER_FOR_MS);
        video_ok && audio_ok
    }

    fn track_received_enough(&self, state: &TrackState, amount: i64) -> bool {
        if state.duration == K_TIME_UNKNOWN || state.position == K_TIME_UNKNOWN {
            return true;
        }
        let needed = state.duration.min(state.position + amount);
        state.received_till >= needed
    }

    pub fn update_progress(&mut self, current_position: i64) {
        if self.state == PlayerState::Playing {
            self.information.video.state.position = current_position;
            self.information.audio.state.position = current_position;

            if !self.is_buffered() {
                self.state = PlayerState::Buffering;
            }

            let dur = self.information.video.state.duration;
            if dur != K_TIME_UNKNOWN && current_position >= dur {
                if self.options.loop_playback {
                    self.looping_shift += dur;
                    self.information.video.state.position = 0;
                    self.information.audio.state.position = 0;
                } else {
                    self.state = PlayerState::Finished;
                }
            }
        }
    }
}
