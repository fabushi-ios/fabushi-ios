//! Telegram Desktop 1:1 Parity Submodule: `EditorLinkBox`
//! Subsystem: `editor` | Feature ID: `TCG-N06`
//! Upstream Source: `Telegram/SourceFiles/editor/editor_link_box.h:L1-L38`, `Telegram/SourceFiles/editor/editor_link_box.cpp:L1-L1009`

use super::editor_link_pill::LinkPreview;
use serde::{Deserialize, Serialize};

pub const RESOLVE_DELAY_MS: i64 = 700;
pub const LOADING_TITLE_RATIO: f64 = 0.45;
pub const LOADING_LINE_RATIOS: [f64; 3] = [0.32, 0.58, 0.86];
pub const GLARE_TIMEOUT_MS: i64 = 1000;
pub const GLARE_DURATION_MS: i64 = 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LinkBoxResult {
    pub message_id: Option<i64>,
    pub pill: Option<LinkPreview>,
    pub dark: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadingMessage {
    pub caption_above: bool,
    pub title_ratio: f64,
    pub line_ratios: [f64; 3],
}

impl Default for LoadingMessage {
    fn default() -> Self {
        Self {
            caption_above: true,
            title_ratio: LOADING_TITLE_RATIO,
            line_ratios: LOADING_LINE_RATIOS,
        }
    }
}

/// Target symbol EditorLinkBox managing link URL input, preview resolving, and theme styling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorLinkBox {
    pub url: String,
    pub is_resolving: bool,
    pub preview: Option<LinkPreview>,
    pub dark_theme: bool,
    pub loading: LoadingMessage,
    pub result: Option<LinkBoxResult>,
}

pub type LinkBox = EditorLinkBox;

impl Default for EditorLinkBox {
    fn default() -> Self {
        Self {
            url: String::new(),
            is_resolving: false,
            preview: None,
            dark_theme: false,
            loading: LoadingMessage::default(),
            result: None,
        }
    }
}

impl EditorLinkBox {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Default::default()
        }
    }

    pub fn set_url(&mut self, url: impl Into<String>) {
        self.url = url.into();
        self.is_resolving = !self.url.is_empty();
    }

    pub fn resolve_success(&mut self, preview: LinkPreview) {
        self.preview = Some(preview.clone());
        self.is_resolving = false;
        self.result = Some(LinkBoxResult {
            message_id: None,
            pill: Some(preview),
            dark: self.dark_theme,
        });
    }

    pub fn toggle_dark_theme(&mut self) -> bool {
        self.dark_theme = !self.dark_theme;
        if let Some(ref mut p) = self.preview {
            p.dark = self.dark_theme;
        }
        self.dark_theme
    }
}
