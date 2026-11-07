//! Telegram Desktop 1:1 Parity Submodule: `EditorKeysLegend`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_keys_legend.h:L1-L50`, `Telegram/SourceFiles/editor/editor_keys_legend.cpp:L1-L302`

use super::photo_editor_inner_common::EditorMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeysLegendContext {
    pub mode: EditorMode,
    pub fixed_crop: bool,
    pub timeline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegendRow {
    pub keys: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegendSection {
    pub title: String,
    pub rows: Vec<LegendRow>,
}

pub fn fixed_crop_rows() -> Vec<LegendRow> {
    vec![
        LegendRow {
            keys: vec!["Wheel".to_string()],
            description: "Zoom".to_string(),
        },
        LegendRow {
            keys: vec!["Shift + Wheel".to_string()],
            description: "Fine Zoom".to_string(),
        },
        LegendRow {
            keys: vec!["Middle Drag".to_string()],
            description: "Pan Canvas".to_string(),
        },
    ]
}

pub fn crop_section(context: &KeysLegendContext) -> LegendSection {
    if context.fixed_crop {
        LegendSection {
            title: "Crop".to_string(),
            rows: fixed_crop_rows(),
        }
    } else {
        LegendSection {
            title: "Crop".to_string(),
            rows: vec![
                LegendRow {
                    keys: vec!["Ctrl".to_string()],
                    description: "Expansion Room".to_string(),
                },
                LegendRow {
                    keys: vec!["Ctrl + Drag Edge".to_string()],
                    description: "Expand Bounds".to_string(),
                },
            ],
        }
    }
}

pub fn canvas_section() -> LegendSection {
    LegendSection {
        title: "Canvas & Paint".to_string(),
        rows: vec![
            LegendRow {
                keys: vec!["R".to_string()],
                description: "Rotate 90°".to_string(),
            },
            LegendRow {
                keys: vec!["F".to_string()],
                description: "Flip Horizontal".to_string(),
            },
            LegendRow {
                keys: vec!["P".to_string()],
                description: "Toggle Paint Mode".to_string(),
            },
            LegendRow {
                keys: vec!["Ctrl + Z".to_string()],
                description: "Undo".to_string(),
            },
            LegendRow {
                keys: vec!["Ctrl + Shift + Z".to_string()],
                description: "Redo".to_string(),
            },
        ],
    }
}

/// Target symbol EditorKeysLegend providing keyboard shortcuts menu and legend hints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorKeysLegend {
    pub is_menu_open: bool,
    pub context: KeysLegendContext,
    pub sections: Vec<LegendSection>,
}

pub type KeysLegendButton = EditorKeysLegend;

impl Default for EditorKeysLegend {
    fn default() -> Self {
        let ctx = KeysLegendContext::default();
        let sections = vec![crop_section(&ctx), canvas_section()];
        Self {
            is_menu_open: false,
            context: ctx,
            sections,
        }
    }
}

impl EditorKeysLegend {
    pub fn new(context: KeysLegendContext) -> Self {
        let sections = vec![crop_section(&context), canvas_section()];
        Self {
            is_menu_open: false,
            context,
            sections,
        }
    }

    pub fn toggle(&mut self) -> bool {
        self.is_menu_open = !self.is_menu_open;
        self.is_menu_open
    }

    pub fn update_context(&mut self, context: KeysLegendContext) {
        self.context = context;
        self.sections = vec![crop_section(&self.context), canvas_section()];
    }

    pub fn tooltip_text(&self) -> &'static str {
        "Keyboard Shortcuts"
    }
}
