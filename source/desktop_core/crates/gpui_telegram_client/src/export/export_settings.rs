//! Target Symbol: ExportSettings
//! Telegram Desktop 1:1 Submodule: `Telegram/SourceFiles/export/export_settings.h`
//! Subsystem: `export`, Feature ID: `TCG-N43`
#![allow(dead_code, unused_imports, unused_variables, non_snake_case)]

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ExportFormat {
    #[default]
    Html,
    Json,
    HtmlAndJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ExportTypeFlags(pub u32);

impl ExportTypeFlags {
    pub const PERSONAL_INFO: u32 = 0x001;
    pub const USERPICS: u32 = 0x002;
    pub const CONTACTS: u32 = 0x004;
    pub const DIALOGS: u32 = 0x010;
    pub const STORIES: u32 = 0x020;
    pub const PROFILE_MUSIC: u32 = 0x040;
    pub const OTHER_DATA: u32 = 0x080;
    pub const SESSIONS: u32 = 0x100;

    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }

    pub fn set(&mut self, flag: u32, enabled: bool) {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ChatTypeFlags(pub u32);

impl ChatTypeFlags {
    pub const PERSONAL_CHATS: u32 = 0x001;
    pub const BOT_CHATS: u32 = 0x002;
    pub const PRIVATE_GROUPS: u32 = 0x004;
    pub const ONLY_MY_MESSAGES: u32 = 0x008;
    pub const PUBLIC_GROUPS: u32 = 0x010;
    pub const PUBLIC_CHANNELS: u32 = 0x020;

    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct MediaTypeFlags(pub u32);

impl MediaTypeFlags {
    pub const PHOTO: u32 = 0x01;
    pub const VIDEO: u32 = 0x02;
    pub const VOICE_MESSAGE: u32 = 0x04;
    pub const VIDEO_MESSAGE: u32 = 0x08;
    pub const STICKER: u32 = 0x10;
    pub const ANIMATED_GIF: u32 = 0x20;
    pub const FILE: u32 = 0x40;

    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportMediaSettings {
    pub types: MediaTypeFlags,
    pub size_limit: i64,
}

impl Default for ExportMediaSettings {
    fn default() -> Self {
        Self {
            types: MediaTypeFlags(MediaTypeFlags::PHOTO | MediaTypeFlags::VOICE_MESSAGE | MediaTypeFlags::VIDEO_MESSAGE),
            size_limit: 8 * 1024 * 1024, // 8 MB default
        }
    }
}

/// User-configured export parameters matching upstream `Export::Settings`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportSettings {
    pub path: PathBuf,
    pub format: ExportFormat,
    pub types: ExportTypeFlags,
    pub full_chats: ChatTypeFlags,
    pub media: ExportMediaSettings,
    pub single_peer_id: Option<i64>,
    pub single_topic_id: Option<i32>,
    pub date_from: Option<i64>,
    pub date_till: Option<i64>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            path: PathBuf::from("Telegram_Export"),
            format: ExportFormat::Html,
            types: ExportTypeFlags(
                ExportTypeFlags::PERSONAL_INFO
                    | ExportTypeFlags::USERPICS
                    | ExportTypeFlags::CONTACTS
                    | ExportTypeFlags::DIALOGS
                    | ExportTypeFlags::STORIES
                    | ExportTypeFlags::OTHER_DATA
                    | ExportTypeFlags::SESSIONS,
            ),
            full_chats: ChatTypeFlags(ChatTypeFlags::PERSONAL_CHATS | ChatTypeFlags::BOT_CHATS | ChatTypeFlags::PRIVATE_GROUPS),
            media: ExportMediaSettings::default(),
            single_peer_id: None,
            single_topic_id: None,
            date_from: None,
            date_till: None,
        }
    }
}

impl ExportSettings {
    pub fn new(path: impl Into<PathBuf>, format: ExportFormat) -> Self {
        Self {
            path: path.into(),
            format,
            ..Default::default()
        }
    }

    pub fn validate(&mut self) -> bool {
        let must_be_full = ChatTypeFlags::PERSONAL_CHATS | ChatTypeFlags::BOT_CHATS;
        let must_not_be_full = ChatTypeFlags::PUBLIC_GROUPS | ChatTypeFlags::PUBLIC_CHANNELS;

        // Force full extraction on 1-on-1 personal/bot chats
        self.full_chats.0 |= must_be_full;
        // Do not force full extraction on large public groups/channels
        self.full_chats.0 &= !must_not_be_full;

        if self.media.size_limit <= 0 {
            self.media.size_limit = 8 * 1024 * 1024;
        }

        !self.path.as_os_str().is_empty()
    }

    pub fn only_single_peer(&self) -> bool {
        self.single_peer_id.is_some()
    }

    pub fn only_single_topic(&self) -> bool {
        self.single_topic_id.is_some()
    }
}
