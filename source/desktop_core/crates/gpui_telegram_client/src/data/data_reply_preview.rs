//! Target Symbol: DataReplyPreview
//! Telegram Desktop 1:1 Submodule: `data/data_reply_preview`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataReplyPreview {
    pub message_id: i32,
    pub sender_id: u64,
    pub sender_name: String,
    pub preview_text: String,
    pub quote_text: Option<String>,
    pub quote_offset: Option<usize>,
    pub has_media: bool,
    pub media_description: Option<String>,
    pub thumbnail_bytes: Option<Vec<u8>>,
}

impl DataReplyPreview {
    pub fn new(message_id: i32, sender_name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            message_id,
            sender_id: 0,
            sender_name: sender_name.into(),
            preview_text: text.into(),
            quote_text: None,
            quote_offset: None,
            has_media: false,
            media_description: None,
            thumbnail_bytes: None,
        }
    }

    pub fn with_quote(mut self, quote: impl Into<String>, offset: usize) -> Self {
        self.quote_text = Some(quote.into());
        self.quote_offset = Some(offset);
        self
    }

    pub fn with_media(mut self, description: impl Into<String>, thumb: Option<Vec<u8>>) -> Self {
        self.has_media = true;
        self.media_description = Some(description.into());
        self.thumbnail_bytes = thumb;
        self
    }
}
