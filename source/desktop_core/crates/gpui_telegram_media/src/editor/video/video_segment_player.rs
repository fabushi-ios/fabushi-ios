//! Telegram Desktop 1:1 Parity Module: `VideoSegmentPlayerVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_segment_player.cpp` (L1-L259)
//! - `Telegram/SourceFiles/editor/video/video_segment_player.h` (L1-L98)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SegmentPlayerOptions {
    pub keep_alpha: bool,
    pub audio: bool,
    pub volume: f64,
}

impl Default for SegmentPlayerOptions {
    fn default() -> Self {
        Self {
            keep_alpha: false,
            audio: false,
            volume: 1.0,
        }
    }
}

/// Authentic representation of video segment looping player.
/// Upstream: `video_segment_player.h:L25-L97` & `video_segment_player.cpp:L32-L255`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoSegmentPlayerVideo {
    pub path: String,
    pub from_ms: u64,
    pub till_ms: u64,
    pub position_ms: u64,
    pub is_paused: bool,
    pub is_seeking: bool,
    pub volume: f64,
    pub sound_enabled: bool,
    pub is_ready: bool,
}

impl Default for VideoSegmentPlayerVideo {
    fn default() -> Self {
        Self::new("video.mp4".to_string(), 0, 10000, SegmentPlayerOptions::default())
    }
}

impl VideoSegmentPlayerVideo {
    pub fn new(path: String, from_ms: u64, till_ms: u64, options: SegmentPlayerOptions) -> Self {
        Self {
            path,
            from_ms,
            till_ms,
            position_ms: from_ms,
            is_paused: false,
            is_seeking: false,
            volume: options.volume.clamp(0.0, 1.0),
            sound_enabled: options.audio,
            is_ready: true,
        }
    }

    pub fn set_segment(&mut self, from_ms: u64, till_ms: u64) {
        self.from_ms = from_ms;
        self.till_ms = till_ms.max(from_ms);
        if self.position_ms < self.from_ms || self.position_ms >= self.till_ms {
            self.position_ms = self.from_ms;
        }
    }

    /// Advances playback position by delta milliseconds, looping at segment boundary.
    /// Upstream: `video_segment_player.cpp:L140-L160`
    pub fn tick(&mut self, delta_ms: u64) -> u64 {
        if self.is_paused || self.is_seeking || self.till_ms <= self.from_ms {
            return self.position_ms;
        }
        self.position_ms += delta_ms;
        if self.position_ms >= self.till_ms {
            self.position_ms = self.from_ms;
        }
        self.position_ms
    }

    pub fn restart(&mut self, position_ms: u64) {
        self.position_ms = position_ms.clamp(self.from_ms, self.till_ms);
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.is_paused = paused;
    }

    pub fn is_held(&self) -> bool {
        self.is_paused || self.is_seeking
    }
}
