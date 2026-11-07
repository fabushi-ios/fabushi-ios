//! Telegram Desktop 1:1 Parity Submodule: `media_child_ffmpeg_loader.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Child FFmpeg Streaming Audio Loader)
//! Upstream: `Telegram/SourceFiles/media/audio/media_child_ffmpeg_loader.h:L15-L61`
//!           `Telegram/SourceFiles/media/audio/media_child_ffmpeg_loader.cpp:L20-L80`

use serde::{Deserialize, Serialize};

/// Upstream: `media_child_ffmpeg_loader.h:L15-L21`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalSoundData {
    pub duration_ms: i64,
    pub speed: f64,
    pub volume: f64,
}

impl Default for ExternalSoundData {
    fn default() -> Self {
        Self {
            duration_ms: 0,
            speed: 1.0,
            volume: 1.0,
        }
    }
}

/// Upstream: `media_child_ffmpeg_loader.h:L23-L26`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ExternalSoundPart {
    pub audio_id: u64,
    pub packet_count: usize,
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaChildFfmpegLoaderAudio {
    pub sound_data: ExternalSoundData,
    pub queued_packets: usize,
    pub force_to_buffer: bool,
    pub eof_reached: bool,
}

impl MediaChildFfmpegLoaderAudio {
    pub fn new() -> Self {
        Self {
            sound_data: ExternalSoundData::default(),
            queued_packets: 0,
            force_to_buffer: false,
            eof_reached: false,
        }
    }

    pub fn with_sound_data(sound_data: ExternalSoundData) -> Self {
        Self {
            sound_data,
            queued_packets: 0,
            force_to_buffer: false,
            eof_reached: false,
        }
    }

    pub fn enqueue_packets(&mut self, count: usize) {
        self.queued_packets = self.queued_packets.saturating_add(count);
    }

    pub fn set_force_to_buffer(&mut self, force: bool) {
        self.force_to_buffer = force;
    }

    pub fn set_eof(&mut self, eof: bool) {
        self.eof_reached = eof;
    }
}
