//! Telegram Desktop 1:1 Parity Module: `VideoEditorLayerVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_editor_layer.cpp` (L1-L318)
//! - `Telegram/SourceFiles/editor/video/video_editor_layer.h` (L1-L68)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream profile video square side: `video_editor_layer.h:L28`
pub const K_PROFILE_VIDEO_SIDE: u32 = 800;

/// Upstream profile video maximum duration in milliseconds: `video_editor_layer.h:L29`
pub const K_PROFILE_VIDEO_MAX_DURATION_MS: u64 = 9600;

/// Upstream profile video minimum duration in milliseconds: `video_editor_layer.h:L30`
pub const K_PROFILE_VIDEO_MIN_DURATION_MS: u64 = 1000;

/// Upstream profile video frame rate: `video_editor_layer.h:L31`
pub const K_PROFILE_VIDEO_FPS: f64 = 30.0;

/// Upstream maximum dimension aspect ratio: `video_editor_layer.cpp:L30`
pub const K_MAX_SIDE_RATIO: u32 = 10;

/// Validates that video dimensions do not exceed 10:1 ratio.
/// Upstream: `video_editor_layer.cpp:L32-L36`
pub fn acceptable_dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= height.saturating_mul(K_MAX_SIDE_RATIO)
        && height <= width.saturating_mul(K_MAX_SIDE_RATIO)
}

/// Profile video encoding configuration.
/// Upstream: `video_editor_layer.cpp:L40-L58`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileVideoConfig {
    pub exact_size: (u32, u32),
    pub max_duration_ms: u64,
    pub min_duration_ms: u64,
    pub fps_limit: f64,
    pub remove_audio: bool,
    pub transcode_always: bool,
}

impl Default for ProfileVideoConfig {
    fn default() -> Self {
        Self {
            exact_size: (K_PROFILE_VIDEO_SIDE, K_PROFILE_VIDEO_SIDE),
            max_duration_ms: K_PROFILE_VIDEO_MAX_DURATION_MS,
            min_duration_ms: K_PROFILE_VIDEO_MIN_DURATION_MS,
            fps_limit: K_PROFILE_VIDEO_FPS,
            remove_audio: true,
            transcode_always: true,
        }
    }
}

/// Authentic representation of profile avatar video editor layer.
/// Upstream: `video_editor_layer.h:L40-L68` & `video_editor_layer.cpp:L40-L315`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoEditorLayerVideo {
    pub is_profile_mode: bool,
    pub config: ProfileVideoConfig,
    pub is_layer_visible: bool,
}

impl VideoEditorLayerVideo {
    pub fn new(is_profile: bool) -> Self {
        Self {
            is_profile_mode: is_profile,
            config: ProfileVideoConfig::default(),
            is_layer_visible: false,
        }
    }

    pub fn validate_source(&self, width: u32, height: u32) -> bool {
        acceptable_dimensions(width, height)
    }
}
