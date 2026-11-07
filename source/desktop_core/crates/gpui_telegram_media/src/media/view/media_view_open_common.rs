//! Telegram Desktop 1:1 Parity Submodule: `MediaViewOpenCommonView`
//! Subsystem: `media` | Feature ID: `TCG-N10`
//! Disposition: `gpui_ui_1to1`
//! Upstream Sources:
//! - `Telegram/SourceFiles/media/view/media_view_open_common.h`
//! - `Telegram/SourceFiles/media/view/media_view_open_common.cpp:L16-L39`

use serde::{Deserialize, Serialize};

/// Type of entity contained within formatted text
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityType {
    Bold,
    Italic,
    Underline,
    StrikeOut,
    Code,
    Pre,
    Blockquote,
    Url,
    CustomEmoji,
}

/// A formatted entity span within a text buffer
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityInText {
    pub entity_type: EntityType,
    pub offset: usize,
    pub length: usize,
}

/// Media target opened in the media lightbox viewer
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OpenTarget {
    Photo {
        photo_id: u64,
        peer_id: Option<u64>,
    },
    Document {
        document_id: u64,
        cloud_theme: Option<String>,
    },
    Story {
        story_id: u64,
        peer_id: u64,
        context: u32,
    },
    GroupCall {
        link_slug: String,
        join_message_id: u64,
    },
}

/// Parameters for opening a media item in the lightbox viewer
/// Faithful to `OpenRequest` in `media_view_open_common.h:L24-L178`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenRequest {
    pub target: OpenTarget,
    pub item_id: Option<u64>,
    pub topic_root_id: u64,
    pub monoforum_peer_id: u64,
    pub continue_streaming: bool,
    pub start_time: i64,
    pub show_draw_button: bool,
}

impl OpenRequest {
    pub fn for_photo(photo_id: u64, item_id: Option<u64>) -> Self {
        Self {
            target: OpenTarget::Photo { photo_id, peer_id: None },
            item_id,
            topic_root_id: 0,
            monoforum_peer_id: 0,
            continue_streaming: false,
            start_time: 0,
            show_draw_button: true,
        }
    }

    pub fn for_document(
        document_id: u64,
        item_id: Option<u64>,
        continue_streaming: bool,
        start_time: i64,
    ) -> Self {
        Self {
            target: OpenTarget::Document { document_id, cloud_theme: None },
            item_id,
            topic_root_id: 0,
            monoforum_peer_id: 0,
            continue_streaming,
            start_time,
            show_draw_button: false,
        }
    }
}

/// Extracts video timestamp from item media or webpage metadata
/// Upstream: `media_view_open_common.cpp:L16-L26`
pub fn extract_video_timestamp(
    media_timestamp: Option<i64>,
    webpage_timestamp: Option<i64>,
) -> i64 {
    if let Some(ts) = media_timestamp {
        if ts > 0 {
            return ts;
        }
    }
    if let Some(ts) = webpage_timestamp {
        if ts > 0 {
            return ts;
        }
    }
    0
}

/// Strips blockquote entities and converts preformatted blocks to inline code
/// Upstream: `media_view_open_common.cpp:L28-L39`
pub fn strip_quote_entities(entities: &[EntityInText]) -> Vec<EntityInText> {
    let mut result = Vec::with_capacity(entities.len());
    for entity in entities {
        match entity.entity_type {
            EntityType::Blockquote => {
                // Erase blockquotes in media lightbox view
                continue;
            }
            EntityType::Pre => {
                // Convert Pre to standard Code
                result.push(EntityInText {
                    entity_type: EntityType::Code,
                    offset: entity.offset,
                    length: entity.length,
                });
            }
            _ => {
                result.push(entity.clone());
            }
        }
    }
    result
}

/// Primary target symbol for `media_view_open_common.rs`
#[derive(Debug, Clone, Default)]
pub struct MediaViewOpenCommonView {
    pub active_request: Option<OpenRequest>,
    pub last_extracted_timestamp: i64,
}

impl MediaViewOpenCommonView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, request: OpenRequest) {
        self.active_request = Some(request);
    }

    pub fn close(&mut self) {
        self.active_request = None;
    }

    pub fn handle_timestamp(&mut self, media_ts: Option<i64>, web_ts: Option<i64>) -> i64 {
        let ts = extract_video_timestamp(media_ts, web_ts);
        self.last_extracted_timestamp = ts;
        ts
    }
}
