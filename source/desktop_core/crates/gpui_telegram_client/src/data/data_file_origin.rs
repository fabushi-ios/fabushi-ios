//! Target Symbol: DataFileOrigin
//! Telegram Desktop 1:1 Submodule: `data/data_file_origin`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileOriginContext {
    UserPhoto(u64),
    ChatPhoto(u64),
    ChannelPhoto(u64),
    HistoryMessage { peer_id: u64, msg_id: i32 },
    StickerSet(u64),
    WallPaper(u64),
    Story { peer_id: u64, story_id: i32 },
    WebPage(u64),
    Unknown,
}

impl Default for FileOriginContext {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct DataFileOrigin {
    pub context: FileOriginContext,
    pub timestamp: i64,
}

impl DataFileOrigin {
    pub fn new(context: FileOriginContext) -> Self {
        Self {
            context,
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    pub fn from_message(peer_id: u64, msg_id: i32) -> Self {
        Self::new(FileOriginContext::HistoryMessage { peer_id, msg_id })
    }

    pub fn from_user_photo(user_id: u64) -> Self {
        Self::new(FileOriginContext::UserPhoto(user_id))
    }

    pub fn from_sticker_set(set_id: u64) -> Self {
        Self::new(FileOriginContext::StickerSet(set_id))
    }

    pub fn from_story(peer_id: u64, story_id: i32) -> Self {
        Self::new(FileOriginContext::Story { peer_id, story_id })
    }

    pub fn is_valid(&self) -> bool {
        self.context != FileOriginContext::Unknown
    }
}
