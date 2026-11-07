//! Telegram Desktop 1:1 Parity Submodule: `PhotoEditorInnerCommon`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/photo_editor_inner_common.h:L1-L106`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EditorMode {
    #[default]
    Transform,
    Paint,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EditorAction {
    #[default]
    None,
    Save,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PhotoEditorMode {
    pub mode: EditorMode,
    pub action: EditorAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BrushTool {
    #[default]
    Pen,
    Arrow,
    Marker,
    Eraser,
    Blur,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    pub size_ratio: f64,
    pub color: [u8; 4], // RGBA
    pub tool: BrushTool,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            size_ratio: 0.9,
            color: [234, 39, 57, 255], // Red (#EA2739)
            tool: BrushTool::Pen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextStyle {
    Framed,
    SemiTransparent,
    #[default]
    Plain,
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextTypeface {
    #[default]
    Default,
    Italic,
    Serif,
    Condensed,
    Monospace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextAlignment {
    #[default]
    Center,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TextPrefs {
    pub style: TextStyle,
    pub typeface: TextTypeface,
    pub alignment: TextAlignment,
    pub size_ratio: f64,
}

impl Default for TextPrefs {
    fn default() -> Self {
        Self {
            style: TextStyle::Plain,
            typeface: TextTypeface::Default,
            alignment: TextAlignment::Center,
            size_ratio: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShapeType {
    #[default]
    Circle,
    Rectangle,
    Star,
    Bubble,
    Arrow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShapeAction {
    #[default]
    Arm,
    Immediate,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShapeRequest {
    pub shape: ShapeType,
    pub action: ShapeAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SaveState {
    #[default]
    Save,
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VideoTrim {
    pub from: i64,
    pub till: i64,
}

/// Target symbol representing inner common settings and presets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhotoEditorInnerCommon {
    pub mode: PhotoEditorMode,
    pub current_brush: Brush,
    pub text_prefs: TextPrefs,
    pub shape_request: ShapeRequest,
    pub video_trim: VideoTrim,
}

impl PhotoEditorInnerCommon {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_tool(&mut self, tool: BrushTool) {
        self.current_brush.tool = tool;
        if tool == BrushTool::Eraser || tool == BrushTool::Blur {
            self.current_brush.color = [0, 0, 0, 255];
        }
    }

    pub fn is_fixed_color_tool(&self) -> bool {
        self.current_brush.tool == BrushTool::Eraser || self.current_brush.tool == BrushTool::Blur
    }
}
