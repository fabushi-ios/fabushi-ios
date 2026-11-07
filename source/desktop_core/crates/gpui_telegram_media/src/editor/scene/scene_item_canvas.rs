//! Telegram Desktop 1:1 Parity Module: `SceneItemCanvasScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_canvas.cpp` (L1-L518)
//! - `Telegram/SourceFiles/editor/scene/scene_item_canvas.h` (L1-L110)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream minimum point distance base: `scene_item_canvas.cpp:L19`
pub const K_MIN_POINT_DISTANCE_BASE: f64 = 2.0;

/// Upstream maximum point distance: `scene_item_canvas.cpp:L20`
pub const K_MAX_POINT_DISTANCE: f64 = 15.0;

/// Upstream stroke smoothing strength: `scene_item_canvas.cpp:L21`
pub const K_SMOOTHING_STRENGTH: f64 = 0.5;

/// Upstream stroke segment overlap: `scene_item_canvas.cpp:L22`
pub const K_SEGMENT_OVERLAP: usize = 3;

/// Upstream origin start jump ratio: `scene_item_canvas.cpp:L23`
pub const K_ORIGIN_START_JUMP_RATIO: f64 = 0.25;

/// Upstream smoothing half strength: `scene_item_canvas.cpp:L24`
pub const K_HALF_STRENGTH: f64 = K_SMOOTHING_STRENGTH / 2.0;

/// Upstream smoothing inverse strength: `scene_item_canvas.cpp:L25`
pub const K_INV_STRENGTH: f64 = 1.0 - K_SMOOTHING_STRENGTH;

/// Upstream Euclidean distance: `scene_item_canvas.cpp:L27-L31`
#[inline]
pub fn point_distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BrushTool {
    #[default]
    Pen,
    Marker,
    Neon,
    Arrow,
    Blur,
    Eraser,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    pub pos: (f64, f64),
    pub pressure: f64,
    pub timestamp_ms: u64,
}

/// Authentic representation of vector drawing canvas and stroke smoothing.
/// Upstream: `scene_item_canvas.h:L19-L50` & `scene_item_canvas.cpp:L39-L300`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemCanvasScene {
    pub canvas_rect: (f64, f64, f64, f64),
    pub current_tool: BrushTool,
    pub stroke_color_rgba: [u8; 4],
    pub stroke_size: f64,
    pub points: Vec<StrokePoint>,
    pub smoothed_points: Vec<(f64, f64)>,
    pub zoom: f64,
}

impl Default for SceneItemCanvasScene {
    fn default() -> Self {
        Self::new((0.0, 0.0, 1080.0, 1920.0))
    }
}

impl SceneItemCanvasScene {
    pub fn new(rect: (f64, f64, f64, f64)) -> Self {
        Self {
            canvas_rect: rect,
            current_tool: BrushTool::Pen,
            stroke_color_rgba: [255, 255, 255, 255],
            stroke_size: 4.0,
            points: Vec::new(),
            smoothed_points: Vec::new(),
            zoom: 1.0,
        }
    }

    pub fn set_brush(&mut self, tool: BrushTool, color_rgba: [u8; 4], size: f64) {
        self.current_tool = tool;
        self.stroke_color_rgba = color_rgba;
        self.stroke_size = size.max(1.0);
    }

    /// Adds a stroke point if distance threshold is satisfied.
    /// Upstream: `scene_item_canvas.cpp:L19-L26`
    pub fn add_point(&mut self, pos: (f64, f64), pressure: f64, timestamp_ms: u64) -> bool {
        let min_dist = K_MIN_POINT_DISTANCE_BASE / self.zoom;
        if let Some(last) = self.points.last() {
            let dist = point_distance(last.pos, pos);
            if dist < min_dist {
                return false;
            }
        }
        self.points.push(StrokePoint {
            pos,
            pressure: pressure.clamp(0.0, 1.0),
            timestamp_ms,
        });

        // Recompute smoothed quadratic points
        if self.points.len() >= 3 {
            let p_prev = self.points[self.points.len() - 3].pos;
            let p_curr = self.points[self.points.len() - 2].pos;
            let p_next = self.points[self.points.len() - 1].pos;
            let sx = p_prev.0 * K_HALF_STRENGTH + p_curr.0 * K_INV_STRENGTH + p_next.0 * K_HALF_STRENGTH;
            let sy = p_prev.1 * K_HALF_STRENGTH + p_curr.1 * K_INV_STRENGTH + p_next.1 * K_HALF_STRENGTH;
            self.smoothed_points.push((sx, sy));
        } else {
            self.smoothed_points.push(pos);
        }
        true
    }

    pub fn clear_stroke(&mut self) {
        self.points.clear();
        self.smoothed_points.clear();
    }
}
