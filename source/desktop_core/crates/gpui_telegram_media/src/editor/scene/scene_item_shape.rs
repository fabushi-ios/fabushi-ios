//! Telegram Desktop 1:1 Parity Module: `SceneItemShapeScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_shape.cpp` (L1-L463)
//! - `Telegram/SourceFiles/editor/scene/scene_item_shape.h` (L1-L93)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream star inner radius ratio: `scene_item_shape.cpp:L21`
pub const K_STAR_INNER_RATIO: f64 = 0.4316;

/// Upstream rounding ratio: `scene_item_shape.cpp:L22`
pub const K_ROUNDING_RATIO: f64 = 0.234;

/// Upstream speech bubble body ratio: `scene_item_shape.cpp:L23`
pub const K_BUBBLE_BODY_RATIO: f64 = 2.0 / 2.25;

/// Upstream speech bubble tail ratio: `scene_item_shape.cpp:L24`
pub const K_BUBBLE_TAIL_RATIO: f64 = 0.75;

/// Upstream arrow head ratio: `scene_item_shape.cpp:L25`
pub const K_ARROW_HEAD_RATIO: f64 = 0.16;

/// Upstream arrow head stroke factor: `scene_item_shape.cpp:L26`
pub const K_ARROW_HEAD_STROKE_FACTOR: f64 = 2.5;

/// Upstream arrow head angle (30 degrees in radians): `scene_item_shape.cpp:L27`
pub const K_ARROW_HEAD_ANGLE: f64 = 30.0 * std::f64::consts::PI / 180.0;

/// Upstream maximum arrow bend ratio: `scene_item_shape.cpp:L28`
pub const K_ARROW_MAX_BEND_RATIO: f64 = 0.35;

/// Upstream minimum shape side: `scene_item_shape.cpp:L29`
pub const K_MIN_SHAPE_SIDE: f64 = 1.0;

/// Upstream star bounding box width: `scene_item_shape.cpp:L30`
pub const K_STAR_BOX_WIDTH: f64 = 1.902113;

/// Upstream star bounding box height: `scene_item_shape.cpp:L31`
pub const K_STAR_BOX_HEIGHT: f64 = 1.809017;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShapeType {
    #[default]
    Rectangle,
    Circle,
    Star,
    SpeechBubble,
    Arrow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArrowPoints {
    pub start: (f64, f64),
    pub control: (f64, f64),
    pub end: (f64, f64),
    pub head_side: f64,
}

/// Authentic representation of vector shape graphics item.
/// Upstream: `scene_item_shape.h:L14-L92` & `scene_item_shape.cpp:L35-L460`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemShapeScene {
    pub shape: ShapeType,
    pub color_rgba: [u8; 4],
    pub stroke_width: f64,
    pub fill: bool,
    pub bend: f64,
    pub width: f64,
    pub height: f64,
    pub flipped: bool,
}

impl Default for SceneItemShapeScene {
    fn default() -> Self {
        Self::new(ShapeType::Star, [255, 255, 255, 255], 3.0, false, 120.0)
    }
}

impl SceneItemShapeScene {
    pub fn new(shape: ShapeType, color_rgba: [u8; 4], stroke_width: f64, fill: bool, size: f64) -> Self {
        let mut item = Self {
            shape,
            color_rgba,
            stroke_width: stroke_width.max(1.0),
            fill,
            bend: 0.0,
            width: size.max(K_MIN_SHAPE_SIDE),
            height: size.max(K_MIN_SHAPE_SIDE),
            flipped: false,
        };
        let ratio = item.default_aspect_ratio();
        item.height = item.width * ratio;
        item
    }

    /// Upstream default aspect ratio based on shape geometry: `scene_item_shape.cpp:L46-L52`
    pub fn default_aspect_ratio(&self) -> f64 {
        match self.shape {
            ShapeType::Star => K_STAR_BOX_HEIGHT / K_STAR_BOX_WIDTH,
            ShapeType::SpeechBubble => K_BUBBLE_BODY_RATIO,
            ShapeType::Arrow => 0.5,
            ShapeType::Rectangle | ShapeType::Circle => 1.0,
        }
    }

    /// Computes arrow control points and triangular head metrics.
    /// Upstream: `scene_item_shape.cpp:L25-L28`
    pub fn compute_arrow_points(&self) -> ArrowPoints {
        let hw = self.width / 2.0;
        let _hh = self.height / 2.0;
        let start = (-hw, 0.0);
        let end = (hw, 0.0);
        let max_bend = self.width * K_ARROW_MAX_BEND_RATIO;
        let bend_offset = self.bend.clamp(-1.0, 1.0) * max_bend;
        let control = (0.0, bend_offset);
        let head_side = (self.width * K_ARROW_HEAD_RATIO).max(self.stroke_width * K_ARROW_HEAD_STROKE_FACTOR);

        ArrowPoints {
            start,
            control,
            end,
            head_side,
        }
    }

    /// Generates the 10 vertex coordinates for the 5-point star polygon.
    /// Upstream: `scene_item_shape.cpp:L21`, `L30-L31`
    pub fn star_vertices(&self, outer_radius: f64) -> Vec<(f64, f64)> {
        let inner_radius = outer_radius * K_STAR_INNER_RATIO;
        let mut vertices = Vec::with_capacity(10);
        for i in 0..10 {
            let r = if i % 2 == 0 { outer_radius } else { inner_radius };
            let angle = (i as f64) * std::f64::consts::PI / 5.0 - std::f64::consts::FRAC_PI_2;
            vertices.push((r * angle.cos(), r * angle.sin()));
        }
        vertices
    }

    pub fn set_bend(&mut self, bend: f64) {
        self.bend = bend.clamp(-1.0, 1.0);
    }
}
