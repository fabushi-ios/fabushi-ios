//! Telegram Desktop 1:1 Parity Module: `VideoTimelineVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_timeline.cpp` (L1-L287)
//! - `Telegram/SourceFiles/editor/video/video_timeline.h` (L1-L92)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream maximum frames per thumbnail strip: `video_timeline.cpp:L18`
pub const K_MAX_FRAMES: usize = 24;

/// Upstream maximum cached frame sets: `video_timeline.cpp:L19`
pub const K_MAX_CACHED_FRAME_SETS: usize = 8;

/// Upstream cover dot animation duration: `video_timeline.cpp:L20`
pub const K_DOT_DURATION_MS: u64 = 500;

/// Upstream reload debounce delay: `video_timeline.cpp:L21`
pub const K_RELOAD_DELAY_MS: u64 = 150;

/// Upstream frame width tolerance: `video_timeline.cpp:L23`
pub const K_FRAME_WIDTH_TOLERANCE: f64 = 0.25;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoThumbnailSlot {
    pub timestamp_ms: u64,
    pub index: usize,
}

/// Authentic representation of interactive video thumbnail timeline.
/// Upstream: `video_timeline.h:L53-L92` & `video_timeline.cpp:L74-L280`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoTimelineVideo {
    pub duration_ms: u64,
    pub from_ms: u64,
    pub till_ms: u64,
    pub cover_ms: u64,
    pub max_duration_ms: u64,
    pub min_duration_ms: u64,
    pub width: f64,
    pub frame_slots: Vec<VideoThumbnailSlot>,
    pub cached_sets_count: usize,
}

impl Default for VideoTimelineVideo {
    fn default() -> Self {
        Self::new(30000, 320.0)
    }
}

impl VideoTimelineVideo {
    pub fn new(duration_ms: u64, width: f64) -> Self {
        let mut tl = Self {
            duration_ms,
            from_ms: 0,
            till_ms: duration_ms,
            cover_ms: 0,
            max_duration_ms: duration_ms,
            min_duration_ms: 1000,
            width: width.max(100.0),
            frame_slots: Vec::with_capacity(K_MAX_FRAMES),
            cached_sets_count: 1,
        };
        tl.generate_slots();
        tl
    }

    /// Generates up to 24 equidistant thumbnail sample timestamps.
    /// Upstream: `video_timeline.cpp:L18`, `L74-L89`
    pub fn generate_slots(&mut self) {
        self.frame_slots.clear();
        let count = K_MAX_FRAMES;
        if self.duration_ms == 0 {
            return;
        }
        let step = (self.duration_ms as f64) / (count as f64);
        for i in 0..count {
            let ts = ((i as f64) * step).round() as u64;
            self.frame_slots.push(VideoThumbnailSlot {
                timestamp_ms: ts.min(self.duration_ms),
                index: i,
            });
        }
    }

    /// Computes X coordinate along the timeline for a given timestamp.
    /// Upstream: `video_timeline.cpp:L98-L100`
    pub fn x_at(&self, timestamp_ms: u64) -> f64 {
        if self.duration_ms == 0 {
            return 0.0;
        }
        let ratio = (timestamp_ms as f64) / (self.duration_ms as f64);
        ratio.clamp(0.0, 1.0) * self.width
    }

    pub fn set_trim(&mut self, from_ms: u64, till_ms: u64) {
        self.from_ms = from_ms.min(self.duration_ms);
        self.till_ms = till_ms.clamp(self.from_ms + self.min_duration_ms, self.duration_ms);
    }

    pub fn set_cover(&mut self, cover_ms: u64) {
        self.cover_ms = cover_ms.clamp(self.from_ms, self.till_ms);
    }
}
