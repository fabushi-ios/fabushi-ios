//! Target Symbol: DataSendAction
//! Telegram Desktop 1:1 Submodule: `data/data_send_action`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SendActionType {
    #[default]
    Typing,
    RecordVoice,
    UploadVoice,
    RecordRound,
    UploadRound,
    UploadPhoto,
    UploadVideo,
    UploadDocument,
    ChooseSticker,
    PlayGame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataSendAction {
    pub peer_id: u64,
    pub user_id: u64,
    pub action_type: SendActionType,
    pub timestamp_ms: i64,
    pub progress: Option<u32>, // 0..100
}

impl Default for DataSendAction {
    fn default() -> Self {
        Self {
            peer_id: 0,
            user_id: 0,
            action_type: SendActionType::Typing,
            timestamp_ms: 0,
            progress: None,
        }
    }
}

impl DataSendAction {
    pub fn new(peer_id: u64, user_id: u64, action_type: SendActionType) -> Self {
        Self {
            peer_id,
            user_id,
            action_type,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            progress: None,
        }
    }

    pub fn with_progress(mut self, progress: u32) -> Self {
        self.progress = Some(progress.min(100));
        self
    }

    pub fn is_expired(&self, now_ms: i64, timeout_ms: i64) -> bool {
        (now_ms - self.timestamp_ms) > timeout_ms
    }

    pub fn display_text(&self) -> &'static str {
        match self.action_type {
            SendActionType::Typing => "typing...",
            SendActionType::RecordVoice => "recording voice...",
            SendActionType::UploadVoice => "sending voice...",
            SendActionType::RecordRound => "recording video message...",
            SendActionType::UploadRound => "sending video message...",
            SendActionType::UploadPhoto => "sending photo...",
            SendActionType::UploadVideo => "sending video...",
            SendActionType::UploadDocument => "sending file...",
            SendActionType::ChooseSticker => "choosing sticker...",
            SendActionType::PlayGame => "playing game...",
        }
    }
}
