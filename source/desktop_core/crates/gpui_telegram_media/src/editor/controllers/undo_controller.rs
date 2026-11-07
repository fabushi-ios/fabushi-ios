//! Telegram Desktop 1:1 Parity Module: `UndoControllerControllers`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/controllers/undo_controller.cpp` (L1-L39)
//! - `Telegram/SourceFiles/editor/controllers/undo_controller.h` (L1-L41)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndoCommand {
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnableRequest {
    pub command: UndoCommand,
    pub enable: bool,
}

/// Authentic representation of editor undo/redo controller.
/// Upstream: `undo_controller.h:L17-L39` & `undo_controller.cpp:L15-L38`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct UndoControllerControllers {
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_history_count: usize,
    pub redo_history_count: usize,
}

impl UndoControllerControllers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update_state(&mut self, undo_count: usize, redo_count: usize) {
        self.undo_history_count = undo_count;
        self.redo_history_count = redo_count;
        self.can_undo = undo_count > 0;
        self.can_redo = redo_count > 0;
    }

    pub fn can_perform(&self, command: UndoCommand) -> bool {
        match command {
            UndoCommand::Undo => self.can_undo,
            UndoCommand::Redo => self.can_redo,
        }
    }
}
