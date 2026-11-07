//! Target Symbol: DataUser
//! Telegram Desktop 1:1 Submodule: `DataUser`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::{PeerId, UserId};
use crate::data::data_peer::PeerData;
use crate::data::data_user_names::UsernamesInfo;
use crate::data::data_lastseen_status::LastseenStatus;
use crate::data::data_birthday::Birthday;
use crate::data::data_peer_bot_command::BotCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct UserDataFlags(pub u32);

impl UserDataFlags {
    pub const CONTACT: u32 = 1 << 0;
    pub const MUTUAL_CONTACT: u32 = 1 << 1;
    pub const DELETED: u32 = 1 << 2;
    pub const VERIFIED: u32 = 1 << 3;
    pub const SCAM: u32 = 1 << 4;
    pub const FAKE: u32 = 1 << 5;
    pub const BOT_INLINE_GEO: u32 = 1 << 6;
    pub const BLOCKED: u32 = 1 << 7;
    pub const HAS_PHONE_CALLS: u32 = 1 << 8;
    pub const PHONE_CALLS_PRIVATE: u32 = 1 << 9;
    pub const SUPPORT: u32 = 1 << 10;
    pub const CAN_PIN_MESSAGES: u32 = 1 << 11;
    pub const DISCARD_MIN_PHOTO: u32 = 1 << 12;
    pub const SELF_USER: u32 = 1 << 13;
    pub const PREMIUM: u32 = 1 << 14;
    pub const UNOFFICIAL_SECURITY_RISK: u32 = 1 << 15;
    pub const VOICE_MESSAGES_FORBIDDEN: u32 = 1 << 16;
    pub const PERSONAL_PHOTO: u32 = 1 << 17;
    pub const STORIES_HIDDEN: u32 = 1 << 18;
    pub const HAS_ACTIVE_STORIES: u32 = 1 << 19;
    pub const HAS_UNREAD_STORIES: u32 = 1 << 20;
    pub const REQUIRES_PREMIUM_TO_WRITE: u32 = 1 << 21;
    pub const FORUM: u32 = 1 << 27;

    pub const fn has(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }

    pub fn add(&mut self, flag: u32) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u32) {
        self.0 &= !flag;
    }
}

/// Extended metadata for Telegram Bots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BotInfo {
    pub description: String,
    pub inline_placeholder: String,
    pub commands: Vec<BotCommand>,
    pub reads_all_history: bool,
    pub cant_join_groups: bool,
    pub supports_business: bool,
    pub has_main_app: bool,
    pub active_users: i32,
}

/// Telegram Stars referral affiliate program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StarRefProgram {
    pub commission: u16,
    pub duration_months: u8,
    pub end_date: i64,
}

/// Full domain model representing a Telegram User.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserData {
    pub peer: PeerData,
    pub access_hash: u64,
    pub flags: UserDataFlags,
    pub first_name: String,
    pub last_name: String,
    pub phone: String,
    pub usernames: UsernamesInfo,
    pub lastseen: LastseenStatus,
    pub birthday: Option<Birthday>,
    pub bot_info: Option<BotInfo>,
    pub common_chats_count: i32,
    pub note: Option<String>,
}

impl UserData {
    pub fn new(user_id: UserId) -> Self {
        Self {
            peer: PeerData::new(PeerId::from_user(user_id)),
            access_hash: 0,
            flags: UserDataFlags::default(),
            first_name: String::new(),
            last_name: String::new(),
            phone: String::new(),
            usernames: UsernamesInfo::default(),
            lastseen: LastseenStatus::default(),
            birthday: None,
            bot_info: None,
            common_chats_count: 0,
            note: None,
        }
    }

    pub fn user_id(&self) -> UserId {
        self.peer.id.to_user().unwrap_or(UserId(0))
    }

    pub fn set_name(&mut self, first: &str, last: &str) {
        self.first_name = first.to_string();
        self.last_name = last.to_string();
    }

    pub fn full_name(&self) -> String {
        if self.first_name.is_empty() && self.last_name.is_empty() {
            if !self.phone.is_empty() {
                self.phone.clone()
            } else if let Some(u) = self.usernames.primary_username() {
                format!("@{}", u)
            } else {
                format!("User#{}", self.user_id().0)
            }
        } else if self.last_name.is_empty() {
            self.first_name.clone()
        } else if self.first_name.is_empty() {
            self.last_name.clone()
        } else {
            format!("{} {}", self.first_name, self.last_name)
        }
    }

    pub fn is_contact(&self) -> bool {
        self.flags.has(UserDataFlags::CONTACT)
    }

    pub fn is_verified(&self) -> bool {
        self.flags.has(UserDataFlags::VERIFIED)
    }

    pub fn is_scam(&self) -> bool {
        self.flags.has(UserDataFlags::SCAM)
    }

    pub fn is_fake(&self) -> bool {
        self.flags.has(UserDataFlags::FAKE)
    }

    pub fn is_premium(&self) -> bool {
        self.flags.has(UserDataFlags::PREMIUM)
    }

    pub fn is_bot(&self) -> bool {
        self.bot_info.is_some()
    }

    pub fn is_self(&self) -> bool {
        self.flags.has(UserDataFlags::SELF_USER)
    }

    pub fn is_deleted(&self) -> bool {
        self.flags.has(UserDataFlags::DELETED)
    }

    pub fn is_support(&self) -> bool {
        self.flags.has(UserDataFlags::SUPPORT)
    }

    pub fn is_online(&self, now: i64) -> bool {
        self.lastseen.is_online(now)
    }
}

/// Target Symbol for Module 03 Ledger: DataUser coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataUser {
    pub users: Vec<UserData>,
}

impl DataUser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_insert(&mut self, user_id: UserId) -> &mut UserData {
        if let Some(pos) = self.users.iter().position(|u| u.user_id() == user_id) {
            &mut self.users[pos]
        } else {
            self.users.push(UserData::new(user_id));
            self.users.last_mut().unwrap()
        }
    }

    pub fn find(&self, user_id: UserId) -> Option<&UserData> {
        self.users.iter().find(|u| u.user_id() == user_id)
    }

    pub fn count(&self) -> usize {
        self.users.len()
    }
}
