//! Telegram Desktop 1:1 Parity Submodule: `PhotoEditor`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/photo_editor.h:L1-L100`, `Telegram/SourceFiles/editor/photo_editor.cpp:L1-L713`

use super::color_picker::ColorPicker;
use super::photo_editor_common::{EditorData, PhotoModifications};
use super::photo_editor_content::PhotoEditorContent;
use super::photo_editor_controls::PhotoEditorControls;
use super::photo_editor_inner_common::{Brush, BrushTool, EditorMode, PhotoEditorMode, TextPrefs};
use serde::{Deserialize, Serialize};

pub const PRECISION: i32 = 100000;
pub const BRUSHES_VERSION: i32 = -2;
pub const DEFAULT_BRUSH_SIZE_RATIO: f64 = 0.9;

pub fn default_brush_color(tool: BrushTool) -> [u8; 4] {
    match tool {
        BrushTool::Pen => [234, 39, 57, 255],     // Red (#EA2739)
        BrushTool::Arrow => [252, 150, 77, 255],  // Orange (#FC964D)
        BrushTool::Marker => [252, 222, 101, 255],// Yellow (#FCDE65)
        BrushTool::Eraser => [0, 0, 0, 255],      // Black
        BrushTool::Blur => [0, 0, 0, 255],        // Black
    }
}

pub fn default_brushes() -> [Brush; 5] {
    [
        Brush { size_ratio: DEFAULT_BRUSH_SIZE_RATIO, color: default_brush_color(BrushTool::Pen), tool: BrushTool::Pen },
        Brush { size_ratio: DEFAULT_BRUSH_SIZE_RATIO, color: default_brush_color(BrushTool::Arrow), tool: BrushTool::Arrow },
        Brush { size_ratio: DEFAULT_BRUSH_SIZE_RATIO, color: default_brush_color(BrushTool::Marker), tool: BrushTool::Marker },
        Brush { size_ratio: DEFAULT_BRUSH_SIZE_RATIO, color: default_brush_color(BrushTool::Blur), tool: BrushTool::Blur },
        Brush { size_ratio: DEFAULT_BRUSH_SIZE_RATIO, color: default_brush_color(BrushTool::Eraser), tool: BrushTool::Eraser },
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrushState {
    pub brushes: [Brush; 5],
    pub tool: BrushTool,
    pub fill_shapes: bool,
    pub text_prefs: TextPrefs,
}

impl Default for BrushState {
    fn default() -> Self {
        Self {
            brushes: default_brushes(),
            tool: BrushTool::Pen,
            fill_shapes: false,
            text_prefs: TextPrefs::default(),
        }
    }
}

/// Target symbol PhotoEditor orchestrating content, controls, color picker, and output modifications.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoEditor {
    pub content: PhotoEditorContent,
    pub controls: PhotoEditorControls,
    pub color_picker: ColorPicker,
    pub brush_state: BrushState,
    pub modifications: PhotoModifications,
    pub is_done: bool,
    pub is_cancelled: bool,
}

impl Default for PhotoEditor {
    fn default() -> Self {
        Self::new(1280, 720, PhotoModifications::default(), EditorData::default())
    }
}

impl PhotoEditor {
    pub fn new(
        width: u32,
        height: u32,
        modifications: PhotoModifications,
        _data: EditorData,
    ) -> Self {
        let brushes = default_brushes();
        Self {
            content: PhotoEditorContent::new(width, height),
            controls: PhotoEditorControls::default(),
            color_picker: ColorPicker::default(),
            brush_state: BrushState {
                brushes,
                tool: BrushTool::Pen,
                fill_shapes: false,
                text_prefs: TextPrefs::default(),
            },
            modifications,
            is_done: false,
            is_cancelled: false,
        }
    }

    pub fn set_mode(&mut self, mode: EditorMode) {
        let pe_mode = PhotoEditorMode {
            mode,
            action: Default::default(),
        };
        self.content.apply_mode(pe_mode);
        self.controls.apply_mode(pe_mode);
        self.color_picker.is_visible = mode == EditorMode::Paint;
    }

    pub fn set_brush_tool(&mut self, tool: BrushTool) {
        self.brush_state.tool = tool;
        self.color_picker.set_tool(tool);
        self.content.apply_brush(self.color_picker.current_brush);
    }

    pub fn save(&mut self) -> PhotoModifications {
        self.modifications = self.content.save_modifications();
        self.is_done = true;
        self.modifications.clone()
    }

    pub fn cancel(&mut self) {
        self.is_cancelled = true;
    }
}
