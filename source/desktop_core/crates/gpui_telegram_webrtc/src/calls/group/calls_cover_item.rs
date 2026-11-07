//! Telegram Desktop 1:1 Parity Submodule: `CallsCoverItemGroup`
//! Subsystem: `calls` | Feature ID: `TCG-N25`
//! Upstream Sources:
//!   - Telegram/SourceFiles/calls/group/calls_cover_item.h (L1-L69)
//!   - Telegram/SourceFiles/calls/group/calls_cover_item.cpp (L1-L107)

use serde::{Deserialize, Serialize};

/// Short information cover display metadata.
/// Upstream: `calls_cover_item.h:L17-L22`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShortInfoCover {
    pub title: String,
    pub subtitle: String,
    pub userpic_peer_id: u64,
    pub has_custom_cover: bool,
    pub cover_width: i32,
    pub cover_height: i32,
}

/// Visual layout dimensions for the group call cover card.
/// Upstream: `calls_cover_item.cpp:L65-L85`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverLayout {
    pub padding_horizontal: f32,
    pub padding_vertical: f32,
    pub userpic_radius: f32,
    pub content_width: f32,
    pub content_height: f32,
}

impl Default for CoverLayout {
    fn default() -> Self {
        Self {
            padding_horizontal: 16.0,
            padding_vertical: 12.0,
            userpic_radius: 36.0,
            content_width: 320.0,
            content_height: 180.0,
        }
    }
}

/// Parity component for `CallsCoverItemGroup`.
#[derive(Debug, Clone, Default)]
pub struct CallsCoverItemGroup {
    pub info: ShortInfoCover,
    pub layout: CoverLayout,
    pub is_scheduled: bool,
    pub schedule_timestamp: Option<i64>,
}

impl CallsCoverItemGroup {
    pub fn new(title: &str, subtitle: &str, peer_id: u64) -> Self {
        Self {
            info: ShortInfoCover {
                title: title.to_string(),
                subtitle: subtitle.to_string(),
                userpic_peer_id: peer_id,
                has_custom_cover: false,
                cover_width: 0,
                cover_height: 0,
            },
            layout: CoverLayout::default(),
            is_scheduled: false,
            schedule_timestamp: None,
        }
    }

    pub fn set_custom_cover(&mut self, width: i32, height: i32) {
        self.info.has_custom_cover = true;
        self.info.cover_width = width;
        self.info.cover_height = height;
    }

    pub fn set_scheduled(&mut self, timestamp: i64) {
        self.is_scheduled = true;
        self.schedule_timestamp = Some(timestamp);
        self.info.subtitle = format!("Scheduled for {}", timestamp);
    }

    pub fn compute_card_size(&self, container_width: f32) -> (f32, f32) {
        let width = container_width.clamp(200.0, 600.0);
        let height = if self.info.has_custom_cover {
            width * 0.5625 // 16:9 ratio
        } else {
            120.0
        };
        (width, height)
    }
}
