//! Telegram Desktop 1:1 Parity Submodule: `media_audio_capture_common.rs`
//! Subsystem: `media` | Feature: `TCG-N06` (Audio Capture Common Structures)
//! Upstream: `Telegram/SourceFiles/media/audio/media_audio_capture_common.h:L1-L20`

use serde::{Deserialize, Serialize};

/// Upstream: `media_audio_capture_common.h:L12-L17`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CaptureResult {
    pub bytes: Vec<u8>,
    pub waveform: Vec<u8>,
    pub duration_ms: i64,
    pub is_video: bool,
}

/// Primary Ledger Target Symbol
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaAudioCaptureCommonAudio {
    pub result: CaptureResult,
}

impl MediaAudioCaptureCommonAudio {
    pub fn new() -> Self {
        Self {
            result: CaptureResult::default(),
        }
    }

    pub fn with_result(bytes: Vec<u8>, waveform: Vec<u8>, duration_ms: i64, is_video: bool) -> Self {
        Self {
            result: CaptureResult {
                bytes,
                waveform,
                duration_ms,
                is_video,
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.result.bytes.is_empty()
    }

    pub fn duration_seconds(&self) -> f64 {
        self.result.duration_ms as f64 / 1000.0
    }
}
