//! Target Symbol: DataAudioMsgId
//! Telegram Desktop 1:1 Submodule: `data/data_audio_msg_id`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AudioMsgType {
    #[default]
    Voice,
    Song,
    RoundVideo,
    ExternalTrack,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct DataAudioMsgId {
    pub peer_id: u64,
    pub msg_id: i32,
    pub document_id: u64,
    pub audio_type: AudioMsgType,
    pub external_url: Option<String>,
}

impl DataAudioMsgId {
    pub fn new(peer_id: u64, msg_id: i32, document_id: u64, audio_type: AudioMsgType) -> Self {
        Self {
            peer_id,
            msg_id,
            document_id,
            audio_type,
            external_url: None,
        }
    }

    pub fn external(url: impl Into<String>) -> Self {
        Self {
            peer_id: 0,
            msg_id: 0,
            document_id: 0,
            audio_type: AudioMsgType::ExternalTrack,
            external_url: Some(url.into()),
        }
    }

    pub fn is_voice(&self) -> bool {
        self.audio_type == AudioMsgType::Voice
    }

    pub fn is_song(&self) -> bool {
        self.audio_type == AudioMsgType::Song
    }
}
