//! Target Symbol: DataChatParticipantStatus
//! Telegram Desktop 1:1 Submodule: `DataChatParticipantStatus`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Bitflags representing administrative powers in groups and channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ChatAdminRights(pub u32);

impl ChatAdminRights {
    pub const CHANGE_INFO: u32 = 1 << 0;
    pub const POST_MESSAGES: u32 = 1 << 1;
    pub const EDIT_MESSAGES: u32 = 1 << 2;
    pub const DELETE_MESSAGES: u32 = 1 << 3;
    pub const BAN_USERS: u32 = 1 << 4;
    pub const INVITE_BY_LINK_OR_ADD: u32 = 1 << 5;
    pub const PIN_MESSAGES: u32 = 1 << 7;
    pub const ADD_ADMINS: u32 = 1 << 9;
    pub const ANONYMOUS: u32 = 1 << 10;
    pub const MANAGE_CALL: u32 = 1 << 11;
    pub const OTHER: u32 = 1 << 12;
    pub const MANAGE_TOPICS: u32 = 1 << 13;
    pub const POST_STORIES: u32 = 1 << 14;
    pub const EDIT_STORIES: u32 = 1 << 15;
    pub const DELETE_STORIES: u32 = 1 << 16;
    pub const MANAGE_DIRECT: u32 = 1 << 17;
    pub const MANAGE_RANKS: u32 = 1 << 18;
    pub const PROCESS_JOIN_REQUESTS: u32 = 1 << 19;
    pub const MANAGE_LINKED_PEERS: u32 = 1 << 20;
    pub const MANAGE_WELCOME_MESSAGES: u32 = 1 << 21;

    pub const ALL_GROUP_RIGHTS: u32 = Self::CHANGE_INFO
        | Self::DELETE_MESSAGES
        | Self::BAN_USERS
        | Self::INVITE_BY_LINK_OR_ADD
        | Self::PIN_MESSAGES
        | Self::ADD_ADMINS
        | Self::MANAGE_CALL
        | Self::MANAGE_TOPICS;

    pub const ALL_CHANNEL_RIGHTS: u32 = Self::CHANGE_INFO
        | Self::POST_MESSAGES
        | Self::EDIT_MESSAGES
        | Self::DELETE_MESSAGES
        | Self::INVITE_BY_LINK_OR_ADD
        | Self::ADD_ADMINS
        | Self::MANAGE_CALL
        | Self::POST_STORIES
        | Self::EDIT_STORIES
        | Self::DELETE_STORIES;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn all() -> Self {
        Self(0x003F_FFFF)
    }

    pub const fn has(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }

    pub fn add(&mut self, flag: u32) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u32) {
        self.0 &= !flag;
    }

    pub fn can_delete_messages(&self) -> bool {
        self.has(Self::DELETE_MESSAGES)
    }

    pub fn can_ban_users(&self) -> bool {
        self.has(Self::BAN_USERS)
    }

    pub fn can_post_messages(&self) -> bool {
        self.has(Self::POST_MESSAGES)
    }

    pub fn can_edit_messages(&self) -> bool {
        self.has(Self::EDIT_MESSAGES)
    }

    pub fn can_add_admins(&self) -> bool {
        self.has(Self::ADD_ADMINS)
    }

    pub fn can_manage_topics(&self) -> bool {
        self.has(Self::MANAGE_TOPICS)
    }
}

/// Bitflags representing participant restrictions in groups and supergroups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ChatRestrictions(pub u32);

impl ChatRestrictions {
    pub const VIEW_MESSAGES: u32 = 1 << 0;
    pub const SEND_MESSAGES: u32 = 1 << 1;
    pub const SEND_MEDIA: u32 = 1 << 2;
    pub const SEND_STICKERS: u32 = 1 << 3;
    pub const SEND_GIFS: u32 = 1 << 4;
    pub const SEND_GAMES: u32 = 1 << 5;
    pub const SEND_INLINE: u32 = 1 << 6;
    pub const EMBED_LINKS: u32 = 1 << 7;
    pub const SEND_POLLS: u32 = 1 << 8;
    pub const CHANGE_INFO: u32 = 1 << 10;
    pub const ADD_PARTICIPANTS: u32 = 1 << 15;
    pub const PIN_MESSAGES: u32 = 1 << 17;
    pub const CREATE_TOPICS: u32 = 1 << 18;
    pub const SEND_PHOTOS: u32 = 1 << 19;
    pub const SEND_VIDEOS: u32 = 1 << 20;
    pub const SEND_VIDEO_MESSAGES: u32 = 1 << 21;
    pub const SEND_MUSIC: u32 = 1 << 22;
    pub const SEND_VOICE_MESSAGES: u32 = 1 << 23;
    pub const SEND_FILES: u32 = 1 << 24;
    pub const SEND_OTHER: u32 = 1 << 25;
    pub const EDIT_RANK: u32 = 1 << 26;
    pub const SEND_REACTIONS: u32 = 1 << 27;
    pub const MANAGE_LINKED_PEERS: u32 = 1 << 28;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn all() -> Self {
        Self(0x1FFF_FFFF)
    }

    pub const fn has(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }

    pub fn add(&mut self, flag: u32) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u32) {
        self.0 &= !flag;
    }

    pub fn can_send_messages(&self) -> bool {
        !self.has(Self::SEND_OTHER)
    }

    pub fn can_send_media(&self) -> bool {
        !self.has(Self::SEND_PHOTOS)
            && !self.has(Self::SEND_VIDEOS)
            && !self.has(Self::SEND_FILES)
    }
}

/// Admin rights with metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChatAdminRightsInfo {
    pub flags: ChatAdminRights,
}

/// Chat restrictions with expiration timestamp (0 = forever).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChatRestrictionsInfo {
    pub flags: ChatRestrictions,
    pub until: i64,
}

impl ChatRestrictionsInfo {
    pub fn is_restricted_at(&self, flag: u32, now: i64) -> bool {
        if self.until != 0 && self.until <= now {
            false
        } else {
            self.flags.has(flag)
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataChatParticipantStatus coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataChatParticipantStatus {
    pub admin_rights: ChatAdminRights,
    pub restrictions: ChatRestrictionsInfo,
}

impl DataChatParticipantStatus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_admin(rights: ChatAdminRights) -> Self {
        Self {
            admin_rights: rights,
            restrictions: ChatRestrictionsInfo::default(),
        }
    }

    pub fn is_admin(&self) -> bool {
        self.admin_rights.0 != 0
    }
}
