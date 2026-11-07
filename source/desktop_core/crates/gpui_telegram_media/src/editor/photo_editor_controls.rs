//! Telegram Desktop 1:1 Parity Submodule: `PhotoEditorControls`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/photo_editor_controls.h:L1-L149`, `Telegram/SourceFiles/editor/photo_editor_controls.cpp:L1-L1396`

use super::photo_editor_common::RoundedCornersLevel;
use super::photo_editor_inner_common::{EditorMode, PhotoEditorMode, ShapeRequest, ShapeType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum CropAspectRatioOption {
    Free,
    Original,
    Square1x1,
    Ratio4x3,
    Ratio16x9,
    Ratio3x2,
}

impl CropAspectRatioOption {
    pub fn ratio(self, original_ratio: f64) -> Option<f64> {
        match self {
            Self::Free => None,
            Self::Original => Some(original_ratio),
            Self::Square1x1 => Some(1.0),
            Self::Ratio4x3 => Some(4.0 / 3.0),
            Self::Ratio16x9 => Some(16.0 / 9.0),
            Self::Ratio3x2 => Some(3.0 / 2.0),
        }
    }
}

/// Control button bar representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ButtonBar {
    pub width: i32,
    pub height: i32,
    pub is_paint_mode: bool,
}

/// Target symbol PhotoEditorControls managing top/bottom toolbars, crop ratios, and action triggers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoEditorControls {
    pub controls_height: i32,
    pub bottom_skip: i32,
    pub center_skip: i32,
    pub button_bar_height: i32,
    pub current_mode: PhotoEditorMode,
    pub shapes_filled: bool,
    pub is_shape_tool_active: bool,
    pub active_aspect_ratio: CropAspectRatioOption,
    pub corners_level: RoundedCornersLevel,
    pub is_animating: bool,
}

impl Default for PhotoEditorControls {
    fn default() -> Self {
        Self {
            controls_height: 146,
            bottom_skip: 20,
            center_skip: 6,
            button_bar_height: 48,
            current_mode: PhotoEditorMode {
                mode: EditorMode::Transform,
                action: Default::default(),
            },
            shapes_filled: false,
            is_shape_tool_active: false,
            active_aspect_ratio: CropAspectRatioOption::Free,
            corners_level: RoundedCornersLevel::None,
            is_animating: false,
        }
    }
}

impl PhotoEditorControls {
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes vertical top offset for bottom button bars inside the controls widget.
    /// Upstream: `photo_editor_controls.cpp:L89-L90`
    pub fn bottom_buttons_top(&self, widget_height: i32) -> i32 {
        widget_height - self.bottom_skip - self.button_bar_height
    }

    pub fn apply_mode(&mut self, mode: PhotoEditorMode) {
        self.current_mode = mode;
    }

    pub fn toggle_shapes_fill(&mut self) -> bool {
        self.shapes_filled = !self.shapes_filled;
        self.shapes_filled
    }

    pub fn create_shape_request(&self, shape: ShapeType) -> ShapeRequest {
        ShapeRequest {
            shape,
            action: Default::default(),
        }
    }

    pub fn set_aspect_ratio(&mut self, option: CropAspectRatioOption) {
        self.active_aspect_ratio = option;
    }

    pub fn set_corners_level(&mut self, level: RoundedCornersLevel) {
        self.corners_level = level;
    }

    /// Key press shortcuts matching upstream Telegram Desktop editor.
    /// Upstream: `photo_editor_controls.cpp:L64`
    pub fn handle_key_shortcut(&mut self, key_code: u32) -> Option<&'static str> {
        match key_code {
            0x1B => Some("cancel"), // Escape
            0x0D | 0x20 => Some("done"), // Return / Space
            0x52 => Some("rotate"), // 'R'
            0x46 => Some("flip"), // 'F'
            0x50 => { // 'P'
                if self.current_mode.mode == EditorMode::Paint {
                    self.current_mode.mode = EditorMode::Transform;
                } else {
                    self.current_mode.mode = EditorMode::Paint;
                }
                Some("toggle_paint")
            }
            _ => None,
        }
    }
}
