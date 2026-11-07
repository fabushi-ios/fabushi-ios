//! Telegram Desktop 1:1 Parity Submodule: `EditorCrop`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_crop.h:L1-L126`, `Telegram/SourceFiles/editor/editor_crop.cpp:L1-L664`

use super::photo_editor_common::RoundedCornersLevel;
use serde::{Deserialize, Serialize};

pub const MAX_CANVAS_RATIO: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CropEdges(pub u8);

impl CropEdges {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1);
    pub const BOTTOM: Self = Self(2);
    pub const LEFT: Self = Self(4);
    pub const RIGHT: Self = Self(8);
    pub const TOP_LEFT: Self = Self(1 | 4);
    pub const TOP_RIGHT: Self = Self(1 | 8);
    pub const BOTTOM_LEFT: Self = Self(2 | 4);
    pub const BOTTOM_RIGHT: Self = Self(2 | 8);
    pub const ALL: Self = Self(1 | 2 | 4 | 8);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Flips dimensions when angle is rotated 90 or 270 degrees.
/// Upstream: `editor_crop.cpp:L49-L51`
pub fn flip_size_by_rotation(width: f64, height: f64, angle: i32) -> (f64, f64) {
    if ((angle / 90).abs() % 2) == 1 {
        (height, width)
    } else {
        (width, height)
    }
}

/// Fits inner size into outer bounds maintaining aspect ratio and centering.
/// Upstream: `editor_crop.cpp:L53-L60`
pub fn original_crop(
    outer_w: f64,
    outer_h: f64,
    inner_w: f64,
    inner_h: f64,
) -> (f64, f64, f64, f64) {
    let scale = (outer_w / inner_w).min(outer_h / inner_h);
    let sw = inner_w * scale;
    let sh = inner_h * scale;
    let x = (outer_w - sw) / 2.0;
    let y = (outer_h - sh) / 2.0;
    (x, y, sw, sh)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct CropRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl CropRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn adjust_to_inner(&mut self, inner_x: f64, inner_y: f64, inner_w: f64, inner_h: f64) -> bool {
        let orig = *self;
        self.width = self.width.min(inner_w);
        self.height = self.height.min(inner_h);
        if self.x < inner_x {
            self.x = inner_x;
        }
        if self.y < inner_y {
            self.y = inner_y;
        }
        if self.right() > inner_x + inner_w {
            self.x = inner_x + inner_w - self.width;
        }
        if self.bottom() > inner_y + inner_h {
            self.y = inner_y + inner_h - self.height;
        }
        *self != orig
    }
}

/// Target symbol EditorCrop managing media crop rectangle, rotation, aspect ratio, and grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorCrop {
    pub image_width: f64,
    pub image_height: f64,
    pub crop: CropRect,
    pub angle: i32,
    pub flipped: bool,
    pub aspect_ratio: Option<f64>,
    pub corners_level: RoundedCornersLevel,
    pub is_dragging: bool,
}

pub type Crop = EditorCrop;

impl Default for EditorCrop {
    fn default() -> Self {
        Self {
            image_width: 1280.0,
            image_height: 720.0,
            crop: CropRect::new(0.0, 0.0, 1280.0, 720.0),
            angle: 0,
            flipped: false,
            aspect_ratio: None,
            corners_level: RoundedCornersLevel::None,
            is_dragging: false,
        }
    }
}

impl EditorCrop {
    pub fn new(width: f64, height: f64) -> Self {
        Self {
            image_width: width,
            image_height: height,
            crop: CropRect::new(0.0, 0.0, width, height),
            ..Default::default()
        }
    }

    pub fn rotate_cw_90(&mut self) {
        self.angle = (self.angle + 90) % 360;
        let (w, h) = flip_size_by_rotation(self.image_width, self.image_height, self.angle);
        self.crop = CropRect::new(0.0, 0.0, w, h);
    }

    pub fn flip_horizontal(&mut self) {
        self.flipped = !self.flipped;
    }

    pub fn set_aspect_ratio(&mut self, ratio: Option<f64>) {
        self.aspect_ratio = ratio;
        if let Some(r) = ratio {
            if r > 0.0 {
                let current_ratio = self.crop.width / self.crop.height.max(1.0);
                if current_ratio > r {
                    let new_w = self.crop.height * r;
                    self.crop.x += (self.crop.width - new_w) / 2.0;
                    self.crop.width = new_w;
                } else {
                    let new_h = self.crop.width / r;
                    self.crop.y += (self.crop.height - new_h) / 2.0;
                    self.crop.height = new_h;
                }
            }
        }
    }

    pub fn set_corners_level(&mut self, level: RoundedCornersLevel) {
        self.corners_level = level;
    }

    /// Computes the 2 horizontal and 2 vertical Rule-of-Thirds guide lines within the crop rect.
    /// Upstream: `editor_crop.cpp:L268-L279`
    pub fn grid_lines_3x3(&self) -> ([(f64, f64, f64, f64); 2], [(f64, f64, f64, f64); 2]) {
        let h_step = self.crop.width / 3.0;
        let v_step = self.crop.height / 3.0;

        let vertical = [
            (self.crop.x + h_step, self.crop.y, self.crop.x + h_step, self.crop.bottom()),
            (self.crop.x + h_step * 2.0, self.crop.y, self.crop.x + h_step * 2.0, self.crop.bottom()),
        ];
        let horizontal = [
            (self.crop.x, self.crop.y + v_step, self.crop.right(), self.crop.y + v_step),
            (self.crop.x, self.crop.y + v_step * 2.0, self.crop.right(), self.crop.y + v_step * 2.0),
        ];
        (horizontal, vertical)
    }
}
