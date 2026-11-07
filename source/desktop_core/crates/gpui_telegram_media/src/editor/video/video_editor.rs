//! Telegram Desktop 1:1 Parity Module: `VideoEditorVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_editor.cpp` (L1-L999)
//! - `Telegram/SourceFiles/editor/video/video_editor.h` (L1-L145)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};
use super::video_editor_common::VideoModifications;
use super::video_editor_quality::VideoEditorQualityVideo;

/// Upstream bubble minimum visibility duration: `video_editor.cpp:L39`
pub const K_BUBBLE_MIN_VISIBLE_MS: u64 = 600;

/// Upstream bubble animation duration: `video_editor.cpp:L41`
pub const K_BUBBLE_DURATION_MS: u64 = 500;

/// Upstream frame capture animation duration: `video_editor.cpp:L43`
pub const K_CAPTURE_DURATION_MS: u64 = 420;

/// Upstream maximum GIF export size in bytes (10MB): `video_editor.cpp:L45`
pub const K_MAX_GIF_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Authentic representation of full video editor interactive widget.
/// Upstream: `video_editor.h:L43-L144` & `video_editor.cpp:L55-L990`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoEditorVideo {
    pub path: String,
    pub dimensions: (u32, u32),
    pub duration_ms: u64,
    pub source_size_bytes: u64,
    pub modifications: VideoModifications,
    pub quality: VideoEditorQualityVideo,
    pub is_playing: bool,
    pub is_gif: bool,
}

impl Default for VideoEditorVideo {
    fn default() -> Self {
        Self::new("video.mp4".to_string(), (1920, 1080), 12000, 5 * 1024 * 1024)
    }
}

impl VideoEditorVideo {
    pub fn new(path: String, dimensions: (u32, u32), duration_ms: u64, size_bytes: u64) -> Self {
        let quality = VideoEditorQualityVideo::new(dimensions);
        let modifications = VideoModifications {
            till_ms: duration_ms,
            ..Default::default()
        };
        Self {
            path,
            dimensions,
            duration_ms,
            source_size_bytes: size_bytes,
            modifications,
            quality,
            is_playing: false,
            is_gif: false,
        }
    }

    pub fn toggle_gif(&mut self) -> bool {
        self.is_gif = !self.is_gif;
        self.modifications.is_gif = self.is_gif;
        self.is_gif
    }

    pub fn toggle_play(&mut self) -> bool {
        self.is_playing = !self.is_playing;
        self.is_playing
    }

    /// Estimates output video file size in bytes based on duration and resolution.
    /// Upstream: `video_editor.cpp:L69-L71`
    pub fn estimate_output_size(&self) -> u64 {
        let trim_duration = self.modifications.till_ms.saturating_sub(self.modifications.from_ms);
        if self.duration_ms == 0 || trim_duration == 0 {
            return 0;
        }
        let selected_side = self.quality.selected_level().map(|l| l.shorter_side).unwrap_or(0);
        let scale_factor = if selected_side > 0 {
            let orig_shorter = self.dimensions.0.min(self.dimensions.1).max(1);
            let r = (selected_side as f64) / (orig_shorter as f64);
            (r * r).min(1.0)
        } else {
            1.0
        };
        let duration_factor = (trim_duration as f64) / (self.duration_ms as f64);
        let estimated = (self.source_size_bytes as f64) * duration_factor * scale_factor;
        estimated.round() as u64
    }

    pub fn collect(&self) -> VideoModifications {
        let mut mods = self.modifications.clone();
        if let Some(level) = self.quality.selected_level() {
            mods.quality = level.shorter_side;
        }
        mods
    }
}
