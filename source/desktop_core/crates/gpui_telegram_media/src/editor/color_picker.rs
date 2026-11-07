//! Telegram Desktop 1:1 Parity Submodule: `ColorPicker`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/color_picker.h:L1-L121`, `Telegram/SourceFiles/editor/color_picker.cpp:L1-L1117`

use super::photo_editor_inner_common::{Brush, BrushTool};
use serde::{Deserialize, Serialize};

pub const MIN_BRUSH_SIZE: f64 = 0.1;
pub const MIN_BRUSH_WIDTH: f64 = 1.0;
pub const MAX_BRUSH_WIDTH: f64 = 25.0;

pub const CIRCLE_DURATION_MS: i64 = 200;
pub const SIZE_CONTROL_SWITCH_DURATION_MS: i64 = 140;
pub const COLOR_BUTTON_SWITCH_DURATION_MS: i64 = 140;

/// Standard 10-color quick select palette.
/// Upstream: `color_picker.cpp:L266-L279`
pub const PALETTE_COLORS: [[u8; 4]; 10] = [
    [0, 0, 0, 255],       // Black
    [255, 255, 255, 255], // White
    [234, 39, 57, 255],   // Red (#EA2739)
    [252, 150, 77, 255],  // Orange (#FC964D)
    [252, 222, 101, 255], // Yellow (#FCDE65)
    [128, 200, 100, 255], // Green (#80C864)
    [73, 197, 237, 255],  // Light Blue (#49C5ED)
    [48, 81, 227, 255],   // Blue (#3051E3)
    [219, 58, 210, 255],  // Magenta (#DB3AD2)
    [255, 114, 169, 255], // Pink (#FF72A9)
];

/// 7 rainbow conical gradient stops for the color wheel button.
/// Upstream: `color_picker.cpp:L143-L151`
pub const WHEEL_GRADIENT_STOPS: [[u8; 3]; 7] = [
    [0xEB, 0x4B, 0x4B], // 0/7: Red
    [0xFF, 0xA5, 0x00], // 1/7: Orange
    [0xFF, 0xFF, 0x00], // 2/7: Yellow
    [0x8F, 0xCE, 0x00], // 3/7: Green
    [0x00, 0xFF, 0xFF], // 4/7: Cyan
    [0x60, 0x80, 0xE4], // 5/7: Blue
    [0xEE, 0x82, 0xEE], // 6/7: Violet
];

#[inline]
pub fn interpolate_f(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
pub fn interpolation_ratio(from: i32, to: i32, result: i32) -> f64 {
    if to == from {
        0.0
    } else {
        (result - from) as f64 / (to - from) as f64
    }
}

pub fn tool_index(tool: BrushTool) -> usize {
    match tool {
        BrushTool::Pen => 0,
        BrushTool::Arrow => 1,
        BrushTool::Marker => 2,
        BrushTool::Blur => 3,
        BrushTool::Eraser => 4,
    }
}

pub fn tool_from_index(index: usize) -> BrushTool {
    match index {
        0 => BrushTool::Pen,
        1 => BrushTool::Arrow,
        2 => BrushTool::Marker,
        3 => BrushTool::Blur,
        4 => BrushTool::Eraser,
        _ => BrushTool::Pen,
    }
}

pub fn fixed_color_tool(tool: BrushTool) -> bool {
    tool == BrushTool::Eraser || tool == BrushTool::Blur
}

pub fn brush_width_from_ratio(ratio: f64) -> f64 {
    MIN_BRUSH_WIDTH + (MAX_BRUSH_WIDTH - MIN_BRUSH_WIDTH) * ratio.clamp(0.0, 1.0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorButton {
    pub current_color: [u8; 4],
    pub is_wheel_active: bool,
}

impl Default for ColorButton {
    fn default() -> Self {
        Self {
            current_color: [234, 39, 57, 255],
            is_wheel_active: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SizeControl {
    pub top: i32,
    pub bottom: i32,
    pub current_ratio: f64,
    pub is_expanded: bool,
    pub is_dragging: bool,
}

impl SizeControl {
    pub fn new(top: i32, bottom: i32) -> Self {
        Self {
            top,
            bottom,
            current_ratio: 0.5,
            is_expanded: false,
            is_dragging: false,
        }
    }

    pub fn ratio_from_y(&self, y: i32) -> f64 {
        if self.bottom <= self.top {
            0.0
        } else {
            let clamped = y.clamp(self.top, self.bottom);
            (self.bottom - clamped) as f64 / (self.bottom - self.top) as f64
        }
    }

    pub fn y_from_ratio(&self, ratio: f64) -> i32 {
        let clamped = ratio.clamp(0.0, 1.0);
        (self.bottom as f64 - clamped * (self.bottom - self.top) as f64).round() as i32
    }
}

/// Target symbol ColorPicker managing brush size, color selection, and tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorPicker {
    pub is_visible: bool,
    pub palette_visible: bool,
    pub current_tool: BrushTool,
    pub current_brush: Brush,
    pub saved_brushes: [Brush; 5],
    pub color_button: ColorButton,
    pub size_control: SizeControl,
}

impl Default for ColorPicker {
    fn default() -> Self {
        let default_brushes = [
            Brush { size_ratio: 0.9, color: [234, 39, 57, 255], tool: BrushTool::Pen },
            Brush { size_ratio: 0.9, color: [252, 150, 77, 255], tool: BrushTool::Arrow },
            Brush { size_ratio: 0.9, color: [252, 222, 101, 255], tool: BrushTool::Marker },
            Brush { size_ratio: 0.9, color: [0, 0, 0, 255], tool: BrushTool::Blur },
            Brush { size_ratio: 0.9, color: [0, 0, 0, 255], tool: BrushTool::Eraser },
        ];
        Self {
            is_visible: true,
            palette_visible: false,
            current_tool: BrushTool::Pen,
            current_brush: default_brushes[0],
            saved_brushes: default_brushes,
            color_button: ColorButton::default(),
            size_control: SizeControl::new(20, 300),
        }
    }
}

impl ColorPicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_tool(&mut self, tool: BrushTool) {
        let idx = tool_index(tool);
        self.saved_brushes[tool_index(self.current_tool)] = self.current_brush;
        self.current_tool = tool;
        self.current_brush = self.saved_brushes[idx];
        if fixed_color_tool(tool) {
            self.current_brush.color = [0, 0, 0, 255];
        }
        self.color_button.current_color = self.current_brush.color;
    }

    pub fn set_color(&mut self, color: [u8; 4]) {
        if !fixed_color_tool(self.current_tool) {
            self.current_brush.color = color;
            self.color_button.current_color = color;
            self.saved_brushes[tool_index(self.current_tool)].color = color;
        }
    }

    pub fn set_brush_size_ratio(&mut self, ratio: f64) {
        self.current_brush.size_ratio = ratio.clamp(MIN_BRUSH_SIZE, 1.0);
        self.size_control.current_ratio = self.current_brush.size_ratio;
        self.saved_brushes[tool_index(self.current_tool)].size_ratio = self.current_brush.size_ratio;
    }

    pub fn current_brush_width(&self) -> f64 {
        brush_width_from_ratio(self.current_brush.size_ratio)
    }
}
