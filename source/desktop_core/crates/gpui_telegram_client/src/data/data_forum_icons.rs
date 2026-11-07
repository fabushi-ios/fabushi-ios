//! Target Symbol: DataForumIcons
//! Telegram Desktop 1:1 Submodule: `data/data_forum_icons`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

pub const FORUM_ICON_COLORS: [u32; 6] = [
    0x6FB9F0, // Sky Blue
    0xFFD67E, // Amber
    0xCB86DB, // Purple
    0x8EEE98, // Mint
    0xFF93B2, // Pink
    0xFB6F5F, // Coral
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataForumIcons {
    pub custom_icon_document_id: Option<u64>,
    pub color_index: usize,
}

impl DataForumIcons {
    pub fn new(color_index: usize) -> Self {
        Self {
            custom_icon_document_id: None,
            color_index: color_index % FORUM_ICON_COLORS.len(),
        }
    }

    pub fn with_custom_icon(document_id: u64, color_index: usize) -> Self {
        Self {
            custom_icon_document_id: Some(document_id),
            color_index: color_index % FORUM_ICON_COLORS.len(),
        }
    }

    pub fn color_rgb(&self) -> u32 {
        FORUM_ICON_COLORS[self.color_index % FORUM_ICON_COLORS.len()]
    }

    pub fn color_hex(&self) -> String {
        format!("#{:06X}", self.color_rgb())
    }

    pub fn resolve_color(index: i32) -> u32 {
        let idx = (index.max(0) as usize) % FORUM_ICON_COLORS.len();
        FORUM_ICON_COLORS[idx]
    }
}
