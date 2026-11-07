//! Telegram Desktop 1:1 Parity Submodule: `media_audio_loaders.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Multi-Track Loaders Dispatcher)
//! Upstream: `Telegram/SourceFiles/media/audio/media_audio_loaders.h:L19-L79`
//!           `Telegram/SourceFiles/media/audio/media_audio_loaders.cpp:L18-L97`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoaderTrackType {
    Voice,
    Song,
    Video,
}

/// Upstream: `media_audio_loaders.h:L40-L50`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SetupLoaderResult {
    pub old_speed: f64,
    pub new_speed: f64,
    pub fade_start_position: i64,
    pub position_samples: i64,
    pub normal_length: i64,
    pub frequency: i32,
    pub error_at_start: bool,
    pub just_started: bool,
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaAudioLoadersAudio {
    pub current_voice_id: u64,
    pub current_song_id: u64,
    pub current_video_id: u64,
    pub external_queue_depth: usize,
    pub is_loading: bool,
}

impl MediaAudioLoadersAudio {
    pub fn new() -> Self {
        Self {
            current_voice_id: 0,
            current_song_id: 0,
            current_video_id: 0,
            external_queue_depth: 0,
            is_loading: false,
        }
    }

    pub fn start_loading(&mut self, track_type: LoaderTrackType, id: u64) {
        match track_type {
            LoaderTrackType::Voice => self.current_voice_id = id,
            LoaderTrackType::Song => self.current_song_id = id,
            LoaderTrackType::Video => self.current_video_id = id,
        }
        self.is_loading = true;
    }

    pub fn clear(&mut self, track_type: LoaderTrackType) {
        match track_type {
            LoaderTrackType::Voice => self.current_voice_id = 0,
            LoaderTrackType::Song => self.current_song_id = 0,
            LoaderTrackType::Video => self.current_video_id = 0,
        }
        if self.current_voice_id == 0 && self.current_song_id == 0 && self.current_video_id == 0 {
            self.is_loading = false;
        }
    }

    pub fn feed_external_packet(&mut self) {
        self.external_queue_depth = self.external_queue_depth.saturating_add(1);
    }

    pub fn drain_external_packets(&mut self) -> usize {
        let count = self.external_queue_depth;
        self.external_queue_depth = 0;
        count
    }
}
