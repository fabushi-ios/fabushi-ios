//! Telegram Desktop 1:1 Parity Module: `SceneItemLinkScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_link.cpp` (L1-L181)
//! - `Telegram/SourceFiles/editor/scene/scene_item_link.h` (L1-L59)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream maximum link pill width ratio: `scene_item_link.cpp:L20`
pub const K_MAX_WIDTH_RATIO: f64 = 0.9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LinkStyle {
    #[default]
    Framed,
    SemiTransparent,
    Opaque,
}

/// Upstream cyclic forward style transition.
/// Upstream: `scene_item_link.cpp:L22-L29`
pub fn next_link_style(style: LinkStyle) -> LinkStyle {
    match style {
        LinkStyle::Framed => LinkStyle::SemiTransparent,
        LinkStyle::SemiTransparent => LinkStyle::Opaque,
        LinkStyle::Opaque => LinkStyle::Framed,
    }
}

/// Upstream cyclic backward style transition.
/// Upstream: `scene_item_link.cpp:L31-L38`
pub fn previous_link_style(style: LinkStyle) -> LinkStyle {
    match style {
        LinkStyle::Framed => LinkStyle::Opaque,
        LinkStyle::SemiTransparent => LinkStyle::Framed,
        LinkStyle::Opaque => LinkStyle::SemiTransparent,
    }
}

/// Authentic representation of interactive story link sticker pill.
/// Upstream: `scene_item_link.h:L15-L58` & `scene_item_link.cpp:L42-L180`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemLinkScene {
    pub url: String,
    pub title: String,
    pub style: LinkStyle,
    pub width: f64,
    pub height: f64,
}

impl Default for SceneItemLinkScene {
    fn default() -> Self {
        Self::new("https://t.me/telegram".to_string(), "Telegram".to_string(), 1080.0)
    }
}

impl SceneItemLinkScene {
    pub fn new(url: String, title: String, canvas_width: f64) -> Self {
        let max_w = canvas_width * K_MAX_WIDTH_RATIO;
        let char_count = title.chars().count().max(1);
        let approx_w = (char_count as f64 * 14.0 + 48.0).min(max_w);
        let h = 44.0;
        Self {
            url,
            title,
            style: LinkStyle::Framed,
            width: approx_w,
            height: h,
        }
    }

    pub fn cycle_style(&mut self) -> LinkStyle {
        self.style = next_link_style(self.style);
        self.style
    }
}
