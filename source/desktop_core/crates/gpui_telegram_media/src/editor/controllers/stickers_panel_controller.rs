//! Telegram Desktop 1:1 Parity Module: `StickersPanelControllerControllers`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/controllers/stickers_panel_controller.cpp` (L1-L113)
//! - `Telegram/SourceFiles/editor/controllers/stickers_panel_controller.h` (L1-L53)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShowRequest {
    ToggleAnimated,
    ShowAnimated,
    HideAnimated,
    HideFast,
}

/// Authentic representation of stickers and emoji drawer controller.
/// Upstream: `stickers_panel_controller.h:L23-L51` & `stickers_panel_controller.cpp:L25-L110`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StickersPanelControllerControllers {
    pub is_shown: bool,
    pub with_audio: bool,
    pub anchor_pos: (i32, i32),
    pub selected_document_id: Option<u64>,
}

impl Default for StickersPanelControllerControllers {
    fn default() -> Self {
        Self {
            is_shown: false,
            with_audio: false,
            anchor_pos: (0, 0),
            selected_document_id: None,
        }
    }
}

impl StickersPanelControllerControllers {
    pub fn new(with_audio: bool) -> Self {
        Self {
            is_shown: false,
            with_audio,
            anchor_pos: (0, 0),
            selected_document_id: None,
        }
    }

    pub fn handle_show_request(&mut self, request: ShowRequest) -> bool {
        match request {
            ShowRequest::ToggleAnimated => self.is_shown = !self.is_shown,
            ShowRequest::ShowAnimated => self.is_shown = true,
            ShowRequest::HideAnimated | ShowRequest::HideFast => self.is_shown = false,
        }
        self.is_shown
    }

    pub fn set_anchor(&mut self, middle_x: i32, plain_y: i32) {
        self.anchor_pos = (middle_x, plain_y);
    }
}
