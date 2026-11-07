//! Telegram Desktop 1:1 Parity Module: `ControllersControllers`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/controllers/controllers.h` (L1-L43)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};
use super::stickers_panel_controller::StickersPanelControllerControllers;
use super::undo_controller::UndoControllerControllers;

/// Authentic aggregator of editor controllers and layer show providers.
/// Upstream: `controllers.h:L20-L41`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControllersControllers {
    pub stickers: StickersPanelControllerControllers,
    pub undo: UndoControllerControllers,
    pub session_id: String,
}

impl Default for ControllersControllers {
    fn default() -> Self {
        Self::new("default_editor".to_string())
    }
}

impl ControllersControllers {
    pub fn new(session_id: String) -> Self {
        Self {
            stickers: StickersPanelControllerControllers::default(),
            undo: UndoControllerControllers::default(),
            session_id,
        }
    }
}
