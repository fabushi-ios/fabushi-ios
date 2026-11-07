//! Telegram Desktop 1:1 Parity Submodule: `media_audio_loader.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Audio Player Loader Abstraction)
//! Upstream: `Telegram/SourceFiles/media/audio/media_audio_loader.h:L16-L84`
//!           `Telegram/SourceFiles/media/audio/media_audio_loader.cpp:L12-L90`

use serde::{Deserialize, Serialize};

/// Upstream: `media_audio_loader.h:L43-L49`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoaderReadError {
    Other,
    Retry,
    RetryNotQueued,
    Wait,
    EndOfFile,
}

/// Upstream: `media_audio_loader.h:L55-L60`
pub fn bytes_per_buffer(samples_frequency: i32, sample_size: i32) -> i32 {
    samples_frequency * sample_size
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaAudioLoaderAudio {
    pub file_path: String,
    pub data_size: usize,
    pub saved_samples: Vec<u8>,
    pub holds_saved_samples: bool,
    pub samples_frequency: i32,
    pub sample_size: i32,
}

impl MediaAudioLoaderAudio {
    pub fn new() -> Self {
        Self {
            file_path: String::new(),
            data_size: 0,
            saved_samples: Vec::new(),
            holds_saved_samples: false,
            samples_frequency: 48000,
            sample_size: 2, // 16-bit
        }
    }

    /// Upstream: `media_audio_loader.cpp:L28-L32`
    pub fn check(&self, path: &str, size: usize) -> bool {
        self.file_path == path && self.data_size == size
    }

    /// Upstream: `media_audio_loader.cpp:L34-L40`
    pub fn save_decoded_samples(&mut self, samples: Vec<u8>) {
        self.saved_samples = samples;
        self.holds_saved_samples = true;
    }

    /// Upstream: `media_audio_loader.cpp:L42-L49`
    pub fn take_saved_decoded_samples(&mut self) -> Vec<u8> {
        self.holds_saved_samples = false;
        std::mem::take(&mut self.saved_samples)
    }

    pub fn buffer_size(&self) -> i32 {
        bytes_per_buffer(self.samples_frequency, self.sample_size)
    }
}
