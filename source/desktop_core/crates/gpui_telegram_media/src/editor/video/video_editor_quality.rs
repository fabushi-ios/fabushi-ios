//! Telegram Desktop 1:1 Parity Module: `VideoEditorQualityVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_editor_quality.cpp` (L1-L46)
//! - `Telegram/SourceFiles/editor/video/video_editor_quality.h` (L1-L26)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream standard video resolution ladders: `video_editor_quality.cpp:L16`
pub const K_QUALITY_LEVELS: [u32; 4] = [360, 480, 720, 1080];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoQualityLevel {
    /// Target shorter side; zero represents preserving original resolution.
    pub shorter_side: u32,
    pub resolution: (u32, u32),
}

/// Downscales resolution keeping aspect ratio to match target shorter side.
pub fn downscaled_size(size: (u32, u32), target_shorter_side: u32) -> (u32, u32) {
    let (w, h) = size;
    let shorter = w.min(h);
    if shorter == 0 || target_shorter_side == 0 {
        return size;
    }
    let scale = (target_shorter_side as f64) / (shorter as f64);
    let mut nw = ((w as f64) * scale).round() as u32;
    let mut nh = ((h as f64) * scale).round() as u32;
    // Align to even dimensions for video codecs
    nw &= !1;
    nh &= !1;
    (nw.max(2), nh.max(2))
}

/// Computes available video quality rungs for edited video dimensions.
/// Upstream: `video_editor_quality.cpp:L20-L38`
pub fn compute_video_quality_levels(edited: (u32, u32)) -> Vec<VideoQualityLevel> {
    let mut result = Vec::new();
    let (w, h) = edited;
    if w == 0 || h == 0 {
        return result;
    }
    let shorter = w.min(h);
    for &side in &K_QUALITY_LEVELS {
        if side >= shorter {
            break;
        }
        let resolution = downscaled_size(edited, side);
        result.push(VideoQualityLevel {
            shorter_side: side,
            resolution,
        });
    }
    result.push(VideoQualityLevel {
        shorter_side: 0,
        resolution: edited,
    });
    result
}

/// Formats quality level label (`"720p"` or `"Original"`).
/// Upstream: `video_editor_quality.cpp:L40-L44`
pub fn video_quality_label(level: &VideoQualityLevel) -> String {
    if level.shorter_side > 0 {
        format!("{}p", level.shorter_side)
    } else {
        "Original".to_string()
    }
}

/// Authentic representation of video editor quality options.
/// Upstream: `video_editor_quality.h:L10-L26` & `video_editor_quality.cpp:L20-L45`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoEditorQualityVideo {
    pub levels: Vec<VideoQualityLevel>,
    pub selected_index: usize,
}

impl Default for VideoEditorQualityVideo {
    fn default() -> Self {
        Self::new((1920, 1080))
    }
}

impl VideoEditorQualityVideo {
    pub fn new(edited_resolution: (u32, u32)) -> Self {
        let levels = compute_video_quality_levels(edited_resolution);
        let selected_index = levels.len().saturating_sub(1);
        Self {
            levels,
            selected_index,
        }
    }

    pub fn selected_level(&self) -> Option<&VideoQualityLevel> {
        self.levels.get(self.selected_index)
    }

    pub fn set_shorter_side(&mut self, shorter_side: u32) {
        if let Some(idx) = self.levels.iter().position(|l| l.shorter_side == shorter_side) {
            self.selected_index = idx;
        }
    }
}
