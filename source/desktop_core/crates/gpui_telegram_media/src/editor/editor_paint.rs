//! Telegram Desktop 1:1 Parity Submodule: `EditorPaint`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_paint.h:L1-L203`, `Telegram/SourceFiles/editor/editor_paint.cpp:L1-L1197`

use super::photo_editor_inner_common::{Brush, ShapeType, TextPrefs};
use serde::{Deserialize, Serialize};

pub const MAX_BRUSH: f64 = 25.0;
pub const MIN_BRUSH: f64 = 1.0;
pub const SHAPE_SIZE_RATIO: f64 = 2.0 / 5.0; // 0.40
pub const MEDIA_SIZE_RATIO: f64 = 1.0 / 2.0; // 0.50
pub const IMAGE_MAX_SIZE_RATIO: f64 = 4.0;

pub const MESSAGE_MAX_WIDTH_RATIO: f64 = 0.88;
pub const MESSAGE_MAX_HEIGHT_RATIO: f64 = 0.70;
pub const MESSAGES_CASCADE_RATIO: f64 = 1.0 / 20.0; // 0.05

pub const MIN_CANVAS_ZOOM: f64 = 1.0;
pub const MAX_CANVAS_ZOOM: f64 = 8.0;
pub const CANVAS_ZOOM_STEP: f64 = 1.15;
pub const CANVAS_ZOOM_STEP_FINE: f64 = 1.015;
pub const ZOOM_SMOOTH_TAU: f64 = 60.0;
pub const TEXT_BAKE_DELAY_MS: i64 = 300;
pub const DEFAULT_FONT_SIZE_DIVISOR: f64 = 15.0;

#[inline]
pub fn brush_size(size_ratio: f64) -> f64 {
    MIN_BRUSH + (MAX_BRUSH - MIN_BRUSH) * size_ratio.clamp(0.0, 1.0)
}

#[inline]
pub fn default_shape_size(image_width: u32, image_height: u32) -> u32 {
    let min_side = image_width.min(image_height) as f64;
    (min_side * SHAPE_SIZE_RATIO).round().max(1.0) as u32
}

#[inline]
pub fn default_font_size(image_width: u32, image_height: u32) -> f64 {
    let min_side = image_width.min(image_height) as f64;
    min_side / DEFAULT_FONT_SIZE_DIVISOR
}

/// Computes the left and right arrowhead barb endpoints for a vector arrow stroke.
/// Upstream: `editor_paint.cpp`, `editor.style:L188-L190`
pub fn arrow_head_points(
    start: (f64, f64),
    end: (f64, f64),
    stroke_width: f64,
) -> ((f64, f64), (f64, f64)) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-4 {
        return (end, end);
    }
    let ux = dx / len;
    let uy = dy / len;

    let head_length = (stroke_width * 2.5).min(len * 0.5);
    let angle_rad = 26.0_f64.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    // Barb 1 rotated by +26 deg backwards
    let b1x = end.0 - head_length * (ux * cos_a - uy * sin_a);
    let b1y = end.1 - head_length * (ux * sin_a + uy * cos_a);

    // Barb 2 rotated by -26 deg backwards
    let b2x = end.0 - head_length * (ux * cos_a + uy * sin_a);
    let b2y = end.1 - head_length * (-ux * sin_a + uy * cos_a);

    ((b1x, b1y), (b2x, b2y))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaintStroke {
    pub brush: Brush,
    pub points: Vec<StrokePoint>,
    pub is_filled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PaintElement {
    Stroke(PaintStroke),
    Text {
        text: String,
        x: f64,
        y: f64,
        prefs: TextPrefs,
        color: [u8; 4],
    },
    Shape {
        shape_type: ShapeType,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        brush: Brush,
        is_filled: bool,
    },
}

/// Target symbol EditorPaint providing raster drawing, zoom canvas, and undo/redo stacks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorPaint {
    pub image_width: u32,
    pub image_height: u32,
    pub canvas_zoom: f64,
    pub active_brush: Brush,
    pub elements: Vec<PaintElement>,
    pub undo_stack: Vec<PaintElement>,
    pub is_dirty: bool,
}

pub type Paint = EditorPaint;

impl Default for EditorPaint {
    fn default() -> Self {
        Self {
            image_width: 1280,
            image_height: 720,
            canvas_zoom: 1.0,
            active_brush: Brush::default(),
            elements: Vec::new(),
            undo_stack: Vec::new(),
            is_dirty: false,
        }
    }
}

impl EditorPaint {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            image_width: width,
            image_height: height,
            ..Default::default()
        }
    }

    pub fn apply_brush(&mut self, brush: Brush) {
        self.active_brush = brush;
    }

    pub fn add_stroke(&mut self, stroke: PaintStroke) {
        self.elements.push(PaintElement::Stroke(stroke));
        self.undo_stack.clear();
        self.is_dirty = true;
    }

    pub fn add_shape(&mut self, shape_type: ShapeType, is_filled: bool) {
        let size = default_shape_size(self.image_width, self.image_height) as f64;
        let x = (self.image_width as f64 - size) / 2.0;
        let y = (self.image_height as f64 - size) / 2.0;
        let elem = PaintElement::Shape {
            shape_type,
            x,
            y,
            width: size,
            height: size,
            brush: self.active_brush,
            is_filled,
        };
        self.elements.push(elem);
        self.undo_stack.clear();
        self.is_dirty = true;
    }

    pub fn undo(&mut self) -> bool {
        if let Some(elem) = self.elements.pop() {
            self.undo_stack.push(elem);
            self.is_dirty = true;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(elem) = self.undo_stack.pop() {
            self.elements.push(elem);
            self.is_dirty = true;
            true
        } else {
            false
        }
    }

    pub fn zoom_canvas(&mut self, factor: f64) {
        self.canvas_zoom = (self.canvas_zoom * factor).clamp(MIN_CANVAS_ZOOM, MAX_CANVAS_ZOOM);
    }

    pub fn reset_zoom(&mut self) {
        self.canvas_zoom = 1.0;
    }
}
