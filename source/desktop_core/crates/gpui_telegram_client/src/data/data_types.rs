//! Target Symbol: DataTypes
//! Telegram Desktop 1:1 Submodule: `DataTypes`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

/// Message album group identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct MessageGroupId {
    pub peer_and_scheduled_flag: u64,
    pub value: u64,
}

impl MessageGroupId {
    pub const SCHEDULED_FLAG: u64 = 1u64 << 55;

    pub fn from_raw(peer: PeerId, value: u64, scheduled: bool) -> Self {
        Self {
            peer_and_scheduled_flag: peer.value | if scheduled { Self::SCHEDULED_FLAG } else { 0 },
            value,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.value == 0
    }

    pub fn is_scheduled(&self) -> bool {
        (self.peer_and_scheduled_flag & Self::SCHEDULED_FLAG) != 0
    }

    pub fn peer_id(&self) -> PeerId {
        PeerId::new(self.peer_and_scheduled_flag & !Self::SCHEDULED_FLAG)
    }
}

/// Upload progress state for files and media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadState {
    pub offset: i64,
    pub size: i64,
    pub waiting_for_album: bool,
    pub preparing: bool,
    pub prepare_progress: f64,
}

impl Default for UploadState {
    fn default() -> Self {
        Self {
            offset: 0,
            size: 0,
            waiting_for_album: false,
            preparing: false,
            prepare_progress: 0.0,
        }
    }
}

impl UploadState {
    pub fn new(size: i64) -> Self {
        Self {
            size,
            ..Default::default()
        }
    }

    pub fn progress(&self) -> f64 {
        if self.preparing {
            self.prepare_progress * 0.1
        } else if self.size > 0 {
            0.1 + (self.offset as f64 / self.size as f64) * 0.9
        } else {
            0.0
        }
    }
}

/// Cache tag constants for local disk caching.
pub const IMAGE_CACHE_TAG: u8 = 0x01;
pub const STICKER_CACHE_TAG: u8 = 0x02;
pub const VOICE_MESSAGE_CACHE_TAG: u8 = 0x03;
pub const VIDEO_MESSAGE_CACHE_TAG: u8 = 0x04;
pub const ANIMATION_CACHE_TAG: u8 = 0x05;

/// Geo point location coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct GeoPointLocation {
    pub lat: f64,
    pub lon: f64,
    pub access_hash: u64,
}

/// Web-hosted file location reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct WebFileLocation {
    pub url: String,
    pub access_hash: u64,
    pub size: i64,
    pub dc_id: i32,
}

/// Audio album thumbnail location.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct AudioAlbumThumbLocation {
    pub document_id: u64,
    pub title: String,
    pub performer: String,
}

/// Storage image file location for local disk and cloud access.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct StorageImageLocation {
    pub dc_id: i32,
    pub id: u64,
    pub volume_id: u64,
    pub local_id: i32,
    pub secret: u64,
}

/// File origin reference tracking authorization contexts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct FileOrigin {
    pub context_key: String,
}

/// Collectible emoji status details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EmojiStatusCollectible {
    pub document_id: u64,
    pub pattern_id: u64,
    pub title: String,
    pub slug: String,
}

/// Target Symbol for Module 03 Ledger: DataTypes registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataTypes {
    pub active_groups: Vec<MessageGroupId>,
}

impl DataTypes {
    pub fn new() -> Self {
        Self {
            active_groups: Vec::new(),
        }
    }

    pub fn register_group(&mut self, group: MessageGroupId) {
        if !self.active_groups.contains(&group) {
            self.active_groups.push(group);
        }
    }

    pub fn make_cache_key(dc_id: i32, id: u64, tag: u8) -> String {
        format!("{}:{}:0x{:02x}", dc_id, id, tag)
    }
}
