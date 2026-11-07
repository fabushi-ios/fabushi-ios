//! Telegram Desktop 1:1 Parity Module: `VideoEditorCommonVideo`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/video/video_editor_common.cpp` (L1-L116)
//! - `Telegram/SourceFiles/editor/video/video_editor_common.h` (L1-L65)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoModifications {
    pub crop_rect: Option<(u32, u32, u32, u32)>,
    pub rotation_angle: i32,
    pub flipped: bool,
    pub from_ms: u64,
    pub till_ms: u64,
    pub cover_ms: u64,
    pub quality: u32,
    pub is_gif: bool,
}

impl Default for VideoModifications {
    fn default() -> Self {
        Self {
            crop_rect: None,
            rotation_angle: 0,
            flipped: false,
            from_ms: 0,
            till_ms: 0,
            cover_ms: 0,
            quality: 0,
            is_gif: false,
        }
    }
}

/// Computes output video frame size considering crop and 90/270 degree rotation.
/// Upstream: `video_editor_common.cpp:L14-L27`
pub fn edited_frame_size(dimensions: (u32, u32), crop: Option<(u32, u32, u32, u32)>, angle: i32) -> (u32, u32) {
    let (w, h) = dimensions;
    if w == 0 || h == 0 {
        return (0, 0);
    }
    let mut result = if let Some((_, _, cw, ch)) = crop {
        (cw.min(w), ch.min(h))
    } else {
        (w, h)
    };
    if ((angle / 90).abs() % 2) != 0 {
        result = (result.1, result.0);
    }
    result
}

/// Determines whether modifications necessitate full video re-encoding.
/// Upstream: `video_editor_common.cpp:L29-L45`
pub fn video_edited(modifications: &VideoModifications, dimensions: (u32, u32), duration_ms: u64, has_audio: bool) -> bool {
    let audio_changed = modifications.is_gif == has_audio;
    let full_crop = (0, 0, dimensions.0, dimensions.1);
    let crop_changed = modifications.crop_rect.is_some() && modifications.crop_rect != Some(full_crop);

    modifications.quality > 0
        || audio_changed
        || modifications.rotation_angle != 0
        || modifications.flipped
        || crop_changed
        || modifications.from_ms > 0
        || (modifications.till_ms > 0 && modifications.till_ms < duration_ms)
}

/// Authentic representation of video modifications state.
/// Upstream: `video_editor_common.h:L15-L64`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VideoEditorCommonVideo {
    pub modifications: VideoModifications,
    pub source_dimensions: (u32, u32),
    pub source_duration_ms: u64,
    pub has_audio: bool,
}

impl VideoEditorCommonVideo {
    pub fn new(dimensions: (u32, u32), duration_ms: u64, has_audio: bool) -> Self {
        Self {
            modifications: VideoModifications {
                till_ms: duration_ms,
                ..Default::default()
            },
            source_dimensions: dimensions,
            source_duration_ms: duration_ms,
            has_audio,
        }
    }

    pub fn output_size(&self) -> (u32, u32) {
        edited_frame_size(
            self.source_dimensions,
            self.modifications.crop_rect,
            self.modifications.rotation_angle,
        )
    }

    pub fn is_reencode_required(&self) -> bool {
        video_edited(
            &self.modifications,
            self.source_dimensions,
            self.source_duration_ms,
            self.has_audio,
        )
    }
}
