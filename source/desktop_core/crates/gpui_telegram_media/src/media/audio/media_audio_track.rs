//! Telegram Desktop 1:1 Parity Submodule: `media_audio_track.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Standalone Audio Track & Peak Sampler)
//! Upstream: `Telegram/SourceFiles/media/audio/media_audio_track.h:L23-L60`
//!           `Telegram/SourceFiles/media/audio/media_audio_track.cpp:L23-L26, L53-L80`

use serde::{Deserialize, Serialize};

/// Upstream: `media_audio_track.cpp:L23-L25`
pub const MAX_AUDIO_FILE_SIZE: usize = 10 * 1024 * 1024; // 10MB
pub const DETACH_DEVICE_TIMEOUT_MS: i64 = 500;
pub const TRACK_UPDATE_TIMEOUT_MS: i64 = 100;

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MediaAudioTrackAudio {
    pub is_active: bool,
    pub is_looping: bool,
    pub is_failed: bool,
    pub length_ms: i64,
    pub volume: f64,
    pub peak_duration_ms: i64,
    pub peaks: Vec<u16>,
}

impl MediaAudioTrackAudio {
    pub fn new() -> Self {
        Self {
            is_active: false,
            is_looping: false,
            is_failed: false,
            length_ms: 0,
            volume: 1.0,
            peak_duration_ms: 0,
            peaks: Vec::new(),
        }
    }

    pub fn play_once(&mut self, volume_override: Option<f64>) {
        self.is_active = true;
        self.is_looping = false;
        if let Some(vol) = volume_override {
            self.volume = vol.clamp(0.0, 1.0);
        }
    }

    pub fn play_in_loop(&mut self, volume_override: Option<f64>) {
        self.is_active = true;
        self.is_looping = true;
        if let Some(vol) = volume_override {
            self.volume = vol.clamp(0.0, 1.0);
        }
    }

    pub fn stop(&mut self) {
        self.is_active = false;
        self.is_looping = false;
    }

    pub fn sample_peak_each(&mut self, peak_duration_ms: i64) {
        self.peak_duration_ms = peak_duration_ms;
    }

    pub fn get_peak_value(&self, when_ms: i64) -> f64 {
        if self.peak_duration_ms <= 0 || self.peaks.is_empty() {
            return 0.0;
        }
        let index = (when_ms / self.peak_duration_ms) as usize;
        if index < self.peaks.len() {
            self.peaks[index] as f64 / 32767.0
        } else {
            0.0
        }
    }
}
