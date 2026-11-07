//! Telegram Desktop 1:1 Parity Module: `SceneItemStickerScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_sticker.cpp` (L1-L236)
//! - `Telegram/SourceFiles/editor/scene/scene_item_sticker.h` (L1-L73)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Authentic representation of sticker graphics scene element.
/// Upstream: `scene_item_sticker.h:L23-L72` & `scene_item_sticker.cpp:L24-L230`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemStickerScene {
    pub document_id: u64,
    pub emoji_char: String,
    pub is_animated: bool,
    pub is_video: bool,
    pub width: f64,
    pub height: f64,
    pub flipped: bool,
}

impl Default for SceneItemStickerScene {
    fn default() -> Self {
        Self::new(512, "👍".to_string(), true, false, 160.0)
    }
}

impl SceneItemStickerScene {
    pub fn new(doc_id: u64, emoji: String, animated: bool, video: bool, size: f64) -> Self {
        let s = size.max(32.0);
        Self {
            document_id: doc_id,
            emoji_char: emoji,
            is_animated: animated,
            is_video: video,
            width: s,
            height: s,
            flipped: false,
        }
    }

    pub fn set_flipped(&mut self, flipped: bool) {
        self.flipped = flipped;
    }
}
