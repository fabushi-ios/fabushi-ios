//! Telegram Desktop 1:1 Parity Submodule: `PhotoEditorCommon`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/photo_editor_common.h:L1-L100`, `Telegram/SourceFiles/editor/photo_editor_common.cpp:L1-L309`

use serde::{Deserialize, Serialize};

/// Audio track representation for photo/video overlay audio.
/// Upstream: `photo_editor_common.h:L17-L36`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AudioTrack {
    pub path: String,
    pub content: Vec<u8>,
    pub title: String,
    pub performer: String,
    pub duration_ms: i64,
    pub from_ms: i64,
    pub till_ms: i64,
    pub volume: f64,
}

impl AudioTrack {
    pub fn new() -> Self {
        Self {
            volume: 1.0,
            ..Default::default()
        }
    }

    pub fn empty(&self) -> bool {
        self.path.is_empty() && self.content.is_empty()
    }

    /// Effective playing duration in milliseconds.
    /// `photo_editor_common.h:L32-L35`
    pub fn length(&self) -> i64 {
        let end = if self.till_ms > self.from_ms {
            self.till_ms
        } else {
            self.duration_ms
        };
        (end - self.from_ms).max(0)
    }
}

/// Corner rounding preset levels for cropped media.
/// Upstream: `photo_editor_common.h:L38-L46`, `photo_editor_common.cpp:L93-L100`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RoundedCornersLevel {
    Circle,
    #[default]
    Large,
    Medium,
    Small,
    None,
}

impl RoundedCornersLevel {
    /// Corner radius multiplier relative to min(width, height).
    /// `photo_editor_common.cpp:L93-L100`
    pub fn multiplier(self) -> f64 {
        match self {
            Self::Circle => 0.5,
            Self::Large => 0.44, // ForumUserpicRadiusMultiplier
            Self::Medium => 0.2,
            Self::Small => 0.12,
            Self::None => 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CropType {
    #[default]
    Rect,
    Ellipse,
    RoundedRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CropMode {
    #[default]
    Hint,
    Mask,
}

/// Configuration options passed into editor initialization.
/// Upstream: `photo_editor_common.h:L48-L72`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EditorData {
    pub about: String,
    pub confirm: String,
    pub confirm_video: String,
    pub exact_width: u32,
    pub exact_height: u32,
    pub crop_type: CropType,
    pub crop_mode: CropMode,
    pub original_ratio: f64,
    pub keep_aspect_ratio: bool,
    pub fixed_crop: bool,
    pub for_other_user: bool,
    pub compose_animated: bool,
    pub compose_sound: bool,
}

/// Persistent modification state applied to an image.
/// Upstream: `photo_editor_common.h:L74-L87`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhotoModifications {
    pub angle: i32,
    pub flipped: bool,
    pub crop_x: i32,
    pub crop_y: i32,
    pub crop_width: i32,
    pub crop_height: i32,
    pub crop_type: CropType,
    pub crop_mode: CropMode,
    pub corners_level: RoundedCornersLevel,
}

impl PhotoModifications {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn empty(&self) -> bool {
        self.angle == 0
            && !self.flipped
            && self.crop_width <= 0
            && self.crop_height <= 0
            && self.crop_type == CropType::Rect
            && self.corners_level == RoundedCornersLevel::Large
    }
}

/// Target symbol PhotoEditorCommon providing algorithms and helpers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhotoEditorCommon {
    pub default_modifications: PhotoModifications,
}

impl PhotoEditorCommon {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rounded_corners_multiplier(level: RoundedCornersLevel) -> f64 {
        level.multiplier()
    }

    pub fn calculate_shape_radius(width: f64, height: f64, level: RoundedCornersLevel) -> f64 {
        width.min(height) * level.multiplier()
    }
}
