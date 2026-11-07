//! Telegram Desktop 1:1 Parity Submodule: `EditorLinkPill`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_link_pill.h:L1-L59`, `Telegram/SourceFiles/editor/editor_link_pill.cpp:L1-L136`

use serde::{Deserialize, Serialize};

pub const DP_PER_WIDTH: f64 = 360.0;
pub const PADDING_LEFT: f64 = 4.0;
pub const PADDING_TOP: f64 = 4.33;
pub const PADDING_RIGHT: f64 = 7.66;
pub const PADDING_BOTTOM: f64 = 3.0;
pub const ICON_SIZE: f64 = 30.0;
pub const ICON_PADDING: f64 = 3.25;
pub const FONT_SIZE: f64 = 24.0;
pub const RADIUS_RATIO: f64 = 0.20;
pub const SEMI_TRANSPARENT_ALPHA: u8 = 0x4C; // 76 / 255 = 30%

/// Strips `http://` or `https://` prefix and trailing slashes.
/// Upstream: `editor_link_pill.cpp:L27-L37`
pub fn without_scheme(url: &str) -> String {
    let mut s = url;
    if let Some(idx) = s.find("://") {
        s = &s[idx + 3..];
    }
    s = s.trim_end_matches('/');
    s.to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkPreview {
    pub url: String,
    pub name: String,
    pub caption_above: bool,
    pub large_photo: bool,
    pub preview: bool,
    pub dark: bool,
}

impl Default for LinkPreview {
    fn default() -> Self {
        Self {
            url: String::new(),
            name: String::new(),
            caption_above: true,
            large_photo: false,
            preview: true,
            dark: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LinkStyle {
    #[default]
    Framed,
    SemiTransparent,
    Opaque,
}

impl LinkStyle {
    pub fn background(self) -> [u8; 4] {
        match self {
            Self::Framed => [255, 255, 255, 255],
            Self::SemiTransparent => [0, 0, 0, SEMI_TRANSPARENT_ALPHA],
            Self::Opaque => [0, 0, 0, 255],
        }
    }

    pub fn foreground(self) -> [u8; 4] {
        match self {
            Self::Framed => [0x33, 0x91, 0xD4, 255], // Blue (#3391D4)
            Self::SemiTransparent | Self::Opaque => [255, 255, 255, 255],
        }
    }
}

/// Target symbol EditorLinkPill managing the compact graphical link badge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorLinkPill {
    pub preview: LinkPreview,
    pub density: f64,
    pub style: LinkStyle,
    pub display_text: String,
    pub width: f64,
    pub height: f64,
}

pub type LinkPill = EditorLinkPill;

impl Default for EditorLinkPill {
    fn default() -> Self {
        Self::new(LinkPreview::default(), 1.0, 300)
    }
}

impl EditorLinkPill {
    pub fn new(link: LinkPreview, density: f64, max_width: i32) -> Self {
        let text = if link.name.is_empty() {
            without_scheme(&link.url).to_uppercase()
        } else {
            link.name.clone()
        };

        let fixed = (PADDING_LEFT + ICON_SIZE + ICON_PADDING + PADDING_RIGHT) * density;
        let estimated_char_width = (FONT_SIZE * 0.6) * density;
        let text_width = (text.len() as f64 * estimated_char_width).min((max_width as f64 - fixed).max(10.0));
        let width = fixed + text_width;
        let height = (PADDING_TOP + PADDING_BOTTOM) * density + (ICON_SIZE * density).max(FONT_SIZE * density);

        Self {
            preview: link,
            density,
            style: LinkStyle::Framed,
            display_text: text,
            width,
            height,
        }
    }

    pub fn density_for(canvas_width: i32) -> f64 {
        canvas_width as f64 / DP_PER_WIDTH
    }

    pub fn set_style(&mut self, style: LinkStyle) {
        self.style = style;
    }

    pub fn background_color(&self) -> [u8; 4] {
        self.style.background()
    }

    pub fn foreground_color(&self) -> [u8; 4] {
        self.style.foreground()
    }
}
