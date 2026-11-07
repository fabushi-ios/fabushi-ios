//! Telegram Desktop 1:1 Parity Submodule: `PhotoEditorLayerWidget`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/photo_editor_layer_widget.h:L1-L57`, `Telegram/SourceFiles/editor/photo_editor_layer_widget.cpp:L1-L182`

use super::photo_editor::PhotoEditor;
use super::photo_editor_common::{EditorData, PhotoModifications};
use serde::{Deserialize, Serialize};

/// Standard profile avatar square resolution limit.
/// Upstream: `photo_editor_layer_widget.h:L28`
pub const PROFILE_PHOTO_SIZE: i32 = 640;

/// Scales dimensions so that both width and height meet or exceed the profile photo size.
pub fn prepare_profile_photo_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (PROFILE_PHOTO_SIZE as u32, PROFILE_PHOTO_SIZE as u32);
    }
    let target = PROFILE_PHOTO_SIZE as f64;
    let scale = (target / width as f64).max(target / height as f64).max(1.0);
    ((width as f64 * scale).round() as u32, (height as f64 * scale).round() as u32)
}

/// Target symbol PhotoEditorLayerWidget managing modal presentation and avatar preparation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhotoEditorLayerWidget {
    pub is_active: bool,
    pub editor: PhotoEditor,
    pub preview_width: i32,
    pub side_limit: i32,
    pub is_profile_photo_flow: bool,
}

impl Default for PhotoEditorLayerWidget {
    fn default() -> Self {
        Self {
            is_active: false,
            editor: PhotoEditor::default(),
            preview_width: 600,
            side_limit: 1280,
            is_profile_photo_flow: false,
        }
    }
}

impl PhotoEditorLayerWidget {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_editor(
        &mut self,
        width: u32,
        height: u32,
        modifications: PhotoModifications,
        data: EditorData,
    ) {
        self.is_profile_photo_flow = data.exact_width == PROFILE_PHOTO_SIZE as u32;
        self.editor = PhotoEditor::new(width, height, modifications, data);
        self.is_active = true;
    }

    pub fn close_editor(&mut self) -> Option<PhotoModifications> {
        if self.is_active {
            self.is_active = false;
            Some(self.editor.save())
        } else {
            None
        }
    }
}
