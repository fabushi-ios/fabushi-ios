//! Target Symbol: DataMediaTypes
//! Telegram Desktop 1:1 Submodule: `data/data_media_types`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SharedMediaType {
    Photo,
    Video,
    PhotoVideo,
    File,
    MusicFile,
    RoundVoiceFile,
    Link,
    ChatPhoto,
    Pinned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaItemPreview {
    pub title: String,
    pub description: String,
    pub media_type: SharedMediaType,
    pub duration_seconds: Option<u32>,
    pub file_size_bytes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataMediaTypes {
    pub supported_types: Vec<SharedMediaType>,
}

impl Default for DataMediaTypes {
    fn default() -> Self {
        Self {
            supported_types: vec![
                SharedMediaType::Photo,
                SharedMediaType::Video,
                SharedMediaType::PhotoVideo,
                SharedMediaType::File,
                SharedMediaType::MusicFile,
                SharedMediaType::RoundVoiceFile,
                SharedMediaType::Link,
                SharedMediaType::ChatPhoto,
                SharedMediaType::Pinned,
            ],
        }
    }
}

impl DataMediaTypes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn type_name(media_type: SharedMediaType) -> &'static str {
        match media_type {
            SharedMediaType::Photo => "Photo",
            SharedMediaType::Video => "Video",
            SharedMediaType::PhotoVideo => "Media",
            SharedMediaType::File => "File",
            SharedMediaType::MusicFile => "Music",
            SharedMediaType::RoundVoiceFile => "Voice",
            SharedMediaType::Link => "Link",
            SharedMediaType::ChatPhoto => "Chat Photo",
            SharedMediaType::Pinned => "Pinned",
        }
    }

    pub fn matches_filter(media_type: SharedMediaType, filter: SharedMediaType) -> bool {
        if media_type == filter {
            return true;
        }
        if filter == SharedMediaType::PhotoVideo {
            return media_type == SharedMediaType::Photo || media_type == SharedMediaType::Video;
        }
        false
    }
}
