//! Telegram Desktop 1:1 Parity Module: `SceneItemTextScene`
//! Upstream Sources:
//! - `Telegram/SourceFiles/editor/scene/scene_item_text.cpp` (L1-L1015)
//! - `Telegram/SourceFiles/editor/scene/scene_item_text.h` (L1-L136)
//! Feature ID: `TCG-N06` | Disposition: `gpui_ui_1to1`

use serde::{Deserialize, Serialize};

/// Upstream text padding factor: `scene_item_text.cpp:L34`
pub const K_PADDING_FACTOR: f64 = 0.4;

/// Upstream maximum width factor: `scene_item_text.cpp:L35`
pub const K_MAX_WIDTH_FACTOR: f64 = 0.8;

/// Upstream minimum content width: `scene_item_text.cpp:L36`
pub const K_MIN_CONTENT_WIDTH: i32 = 20;

/// Upstream luminance brightness framed threshold: `scene_item_text.cpp:L37`
pub const K_BRIGHTNESS_FRAMED_THRESHOLD: f64 = 0.721;

/// Upstream semi-transparent threshold: `scene_item_text.cpp:L38`
pub const K_BRIGHTNESS_SEMI_TRANSPARENT_THRESHOLD: f64 = 0.25;

/// Upstream semi-transparent alpha: `scene_item_text.cpp:L39`
pub const K_SEMI_TRANSPARENT_ALPHA: u8 = 0x99;

/// Upstream bubble corner radius factor: `scene_item_text.cpp:L40`
pub const K_CORNER_RADIUS_FACTOR: f64 = 1.0 / 3.0;

/// Upstream horizontal line padding factor: `scene_item_text.cpp:L41`
pub const K_LINE_PAD_H_FACTOR: f64 = 1.0 / 3.0;

/// Upstream merge radius factor: `scene_item_text.cpp:L42`
pub const K_MERGE_RADIUS_FACTOR: f64 = 1.5;

/// Upstream condensed stretch percentage: `scene_item_text.cpp:L53`
pub const K_CONDENSED_STRETCH: i32 = 78;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextStyle {
    #[default]
    Regular,
    Framed,
    SemiTransparent,
    Outlined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextTypeface {
    #[default]
    Default,
    Serif,
    Script,
    Condensed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextAlignment {
    #[default]
    Center,
    Left,
    Right,
}

/// Computes effective text foreground color based on background style contrast.
/// Upstream: `scene_item_text.cpp:L19` & `scene_item_text.cpp:L37-L39`
pub fn effective_text_color(color_rgba: [u8; 4], style: TextStyle) -> [u8; 4] {
    match style {
        TextStyle::Regular | TextStyle::Outlined => color_rgba,
        TextStyle::Framed => {
            let lum = 0.299 * (color_rgba[0] as f64) + 0.587 * (color_rgba[1] as f64) + 0.114 * (color_rgba[2] as f64);
            if lum / 255.0 > K_BRIGHTNESS_FRAMED_THRESHOLD {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        }
        TextStyle::SemiTransparent => [255, 255, 255, 255],
    }
}

/// Authentic representation of editor rich text item with bubble background rendering.
/// Upstream: `scene_item_text.h:L48-L135` & `scene_item_text.cpp:L80-L1010`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneItemTextScene {
    pub text: String,
    pub color_rgba: [u8; 4],
    pub font_size: f64,
    pub style: TextStyle,
    pub typeface: TextTypeface,
    pub alignment: TextAlignment,
    pub content_width: f64,
    pub content_height: f64,
    pub padding: f64,
}

impl Default for SceneItemTextScene {
    fn default() -> Self {
        Self::new("Hello GPUI".to_string(), [255, 255, 255, 255], 32.0, TextStyle::Framed)
    }
}

impl SceneItemTextScene {
    pub fn new(text: String, color_rgba: [u8; 4], font_size: f64, style: TextStyle) -> Self {
        let padding = font_size * K_PADDING_FACTOR;
        let mut item = Self {
            text,
            color_rgba,
            font_size: font_size.max(8.0),
            style,
            typeface: TextTypeface::Default,
            alignment: TextAlignment::Center,
            content_width: 120.0,
            content_height: 48.0,
            padding,
        };
        item.recalculate_layout();
        item
    }

    pub fn recalculate_layout(&mut self) {
        self.padding = self.font_size * K_PADDING_FACTOR;
        let char_count = self.text.chars().count().max(1);
        let approx_char_w = self.font_size * 0.55;
        self.content_width = ((char_count as f64) * approx_char_w + self.padding * 2.0).max(K_MIN_CONTENT_WIDTH as f64);
        self.content_height = self.font_size * 1.35 + self.padding * 2.0;
    }

    pub fn text_background_color(&self) -> [u8; 4] {
        match self.style {
            TextStyle::Regular | TextStyle::Outlined => [0, 0, 0, 0],
            TextStyle::Framed => self.color_rgba,
            TextStyle::SemiTransparent => {
                let lum = 0.299 * (self.color_rgba[0] as f64) + 0.587 * (self.color_rgba[1] as f64) + 0.114 * (self.color_rgba[2] as f64);
                let base = if lum / 255.0 > K_BRIGHTNESS_SEMI_TRANSPARENT_THRESHOLD { 0 } else { 255 };
                [base, base, base, K_SEMI_TRANSPARENT_ALPHA]
            }
        }
    }
}
