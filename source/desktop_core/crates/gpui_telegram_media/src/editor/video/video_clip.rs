//! Telegram Desktop 1:1 Parity Module: `VideoClipVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_clip.cpp` (L1-L147)
//! - `Telegram/SourceFiles/editor/video/video_clip.h` (L1-L71)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoTrim {
    pub from: u64,
    pub till: u64,
}

impl Default for VideoTrim {
    fn default() -> Self {
        Self { from: 0, till: 0 }
    }
}

/// Authentic representation of video clip playback and trim state.
/// Upstream: `video_clip.h:L25-L69` & `video_clip.cpp:L16-L140`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoClipVideo {
    pub path: String,
    pub duration_ms: u64,
    pub has_audio: bool,
    pub trim: VideoTrim,
    pub volume: f64,
    pub sounding: bool,
    pub is_stopped: bool,
}

impl Default for VideoClipVideo {
    fn default() -> Self {
        Self::new("video.mp4".to_string(), 15000, true)
    }
}

impl VideoClipVideo {
    pub fn new(path: String, duration_ms: u64, has_audio: bool) -> Self {
        Self {
            path,
            duration_ms,
            has_audio,
            trim: VideoTrim { from: 0, till: duration_ms },
            volume: 1.0,
            sounding: has_audio,
            is_stopped: false,
        }
    }

    /// Clamps trim boundaries within valid video duration.
    /// Upstream: `video_clip.cpp:L54-L60`
    pub fn set_trim(&mut self, mut trim: VideoTrim) {
        let full = self.duration_ms;
        if full > 0 {
            trim.from = trim.from.min(full);
            trim.till = if trim.till > 0 {
                trim.till.clamp(trim.from, full)
            } else {
                0
            };
        }
        self.trim = trim;
    }

    /// Loop duration of trimmed segment.
    /// Upstream: `video_clip.cpp:L65-L67`
    pub fn loop_duration(&self) -> u64 {
        let till = if self.trim.till > 0 { self.trim.till } else { self.duration_ms };
        till.saturating_sub(self.trim.from)
    }

    pub fn set_volume(&mut self, volume: f64) {
        self.volume = volume.clamp(0.0, 1.0);
        self.sounding = self.volume > 0.0 && self.has_audio;
    }

    pub fn stop(&mut self) {
        self.is_stopped = true;
    }

    pub fn resume(&mut self) {
        self.is_stopped = false;
    }
}
