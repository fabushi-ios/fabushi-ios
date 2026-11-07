//! Telegram Desktop 1:1 Parity Module: `VideoQualitySliderVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_quality_slider.cpp` (L1-L225)
//! - `Telegram/SourceFiles/editor/video/video_quality_slider.h` (L1-L57)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};
use super::video_editor_quality::{VideoQualityLevel, video_quality_label};

/// Upstream shift duration in milliseconds: `video_quality_slider.cpp:L18`
pub const K_SHIFT_DURATION_MS: u64 = 120;

/// Authentic representation of discrete video quality notch slider.
/// Upstream: `video_quality_slider.h:L16-L56` & `video_quality_slider.cpp:L26-L220`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoQualitySliderVideo {
    pub levels: Vec<VideoQualityLevel>,
    pub current_index: usize,
    pub width: f64,
    pub is_dragging: bool,
}

impl Default for VideoQualitySliderVideo {
    fn default() -> Self {
        Self::new(vec![
            VideoQualityLevel { shorter_side: 480, resolution: (854, 480) },
            VideoQualityLevel { shorter_side: 720, resolution: (1280, 720) },
            VideoQualityLevel { shorter_side: 0, resolution: (1920, 1080) },
        ])
    }
}

impl VideoQualitySliderVideo {
    pub fn new(levels: Vec<VideoQualityLevel>) -> Self {
        let count = levels.len();
        Self {
            levels,
            current_index: count.saturating_sub(1),
            width: 240.0,
            is_dragging: false,
        }
    }

    pub fn has_choice(&self) -> bool {
        self.levels.len() > 1
    }

    /// Computes X coordinate of a notch index along slider track.
    /// Upstream: `video_quality_slider.cpp:L39`
    pub fn dot_x(&self, index: usize) -> f64 {
        if self.levels.len() <= 1 {
            return self.width / 2.0;
        }
        let step = self.width / ((self.levels.len() - 1) as f64);
        (index as f64) * step
    }

    /// Finds closest notch index from mouse click X position.
    /// Upstream: `video_quality_slider.cpp:L40`
    pub fn index_at(&self, x: f64) -> usize {
        if self.levels.len() <= 1 {
            return 0;
        }
        let step = self.width / ((self.levels.len() - 1) as f64);
        let idx = (x / step).round() as usize;
        idx.min(self.levels.len() - 1)
    }

    pub fn set_index(&mut self, index: usize) -> Option<&VideoQualityLevel> {
        if index < self.levels.len() {
            self.current_index = index;
            Some(&self.levels[self.current_index])
        } else {
            None
        }
    }

    pub fn selected_label(&self) -> String {
        self.levels
            .get(self.current_index)
            .map(video_quality_label)
            .unwrap_or_else(|| "Original".to_string())
    }
}
