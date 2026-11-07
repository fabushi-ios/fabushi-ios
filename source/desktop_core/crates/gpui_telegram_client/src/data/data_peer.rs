//! Target Symbol: DataPeer
//! Telegram Desktop 1:1 Submodule: `DataPeer`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::{PeerId, UserId, ChatId, ChannelId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestrictionCheckResult {
    Allowed,
    WithEveryone,
    Explicit,
}

impl RestrictionCheckResult {
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UnavailableReason {
    pub reason: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AllowedReactionsType {
    All,
    Default,
    #[default]
    Some,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AllowedReactions {
    pub some: Vec<String>,
    pub max_count: usize,
    pub reaction_type: AllowedReactionsType,
    pub paid_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PeerBarSettings(pub u32);

impl PeerBarSettings {
    pub const REPORT_SPAM: u32 = 1 << 0;
    pub const ADD_CONTACT: u32 = 1 << 1;
    pub const BLOCK_CONTACT: u32 = 1 << 2;
    pub const SHARE_CONTACT: u32 = 1 << 3;
    pub const NEED_CONTACTS_EXCEPTION: u32 = 1 << 4;
    pub const AUTO_ARCHIVED: u32 = 1 << 5;
    pub const REQUEST_CHAT: u32 = 1 << 6;
    pub const REQUEST_CHAT_IS_BROADCAST: u32 = 1 << 7;
    pub const HAS_BUSINESS_BOT: u32 = 1 << 8;
    pub const BUSINESS_BOT_PAUSED: u32 = 1 << 9;
    pub const BUSINESS_BOT_CAN_REPLY: u32 = 1 << 10;

    pub fn has(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }
}

/// Base domain representation of a Telegram Peer (User, Chat, or Channel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerData {
    pub id: PeerId,
    pub color_index: u8,
    pub background_emoji_id: u64,
    pub color_profile_index: Option<u8>,
    pub profile_background_emoji_id: u64,
    pub emoji_status_id: u64,
    pub stories_hidden: bool,
    pub stars_per_message: i32,
    pub custom_title: Option<String>,
}

impl PeerData {
    pub const SERVICE_NOTIFICATIONS_USER_ID: u64 = 777000;
    pub const REPLIES_CHAT_ID: u64 = 1271266957;

    pub fn new(id: PeerId) -> Self {
        // Derive initial color index deterministically from bare ID
        let color_index = (id.bare() % 7) as u8;
        Self {
            id,
            color_index,
            background_emoji_id: 0,
            color_profile_index: None,
            profile_background_emoji_id: 0,
            emoji_status_id: 0,
            stories_hidden: false,
            stars_per_message: 0,
            custom_title: None,
        }
    }

    pub fn is_user(&self) -> bool {
        self.id.is_user()
    }

    pub fn is_chat(&self) -> bool {
        self.id.is_chat()
    }

    pub fn is_channel(&self) -> bool {
        self.id.is_channel()
    }

    pub fn is_notifications_user(&self) -> bool {
        self.id == PeerId::from_user(UserId(Self::SERVICE_NOTIFICATIONS_USER_ID))
    }

    pub fn as_user_id(&self) -> Option<UserId> {
        self.id.to_user()
    }

    pub fn as_chat_id(&self) -> Option<ChatId> {
        self.id.to_chat()
    }

    pub fn as_channel_id(&self) -> Option<ChannelId> {
        self.id.to_channel()
    }
}

/// Target Symbol for Module 03 Ledger: DataPeer coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeer {
    pub peers: Vec<PeerData>,
}

impl DataPeer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_insert(&mut self, id: PeerId) -> &mut PeerData {
        if let Some(pos) = self.peers.iter().position(|p| p.id == id) {
            &mut self.peers[pos]
        } else {
            self.peers.push(PeerData::new(id));
            self.peers.last_mut().unwrap()
        }
    }

    pub fn find(&self, id: PeerId) -> Option<&PeerData> {
        self.peers.iter().find(|p| p.id == id)
    }

    pub fn count(&self) -> usize {
        self.peers.len()
    }
}
