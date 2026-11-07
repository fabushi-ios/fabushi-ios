//! Target Symbol: DataChat
//! Telegram Desktop 1:1 Submodule: `DataChat`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::{PeerId, ChatId, ChannelId, UserId};
use crate::data::data_peer::PeerData;
use crate::data::data_chat_participant_status::{ChatAdminRights, ChatRestrictions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ChatDataFlags(pub u32);

impl ChatDataFlags {
    pub const LEFT: u32 = 1 << 0;
    pub const CREATOR: u32 = 1 << 2;
    pub const DEACTIVATED: u32 = 1 << 3;
    pub const FORBIDDEN: u32 = 1 << 4;
    pub const CALL_ACTIVE: u32 = 1 << 5;
    pub const CALL_NOT_EMPTY: u32 = 1 << 6;
    pub const CAN_SET_USERNAME: u32 = 1 << 7;
    pub const NO_FORWARDS: u32 = 1 << 8;
    pub const HAS_WELCOME_MESSAGES: u32 = 1 << 9;

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

/// Small basic group chat model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatData {
    pub peer: PeerData,
    pub title: String,
    pub flags: ChatDataFlags,
    pub admin_rights: ChatAdminRights,
    pub default_restrictions: ChatRestrictions,
    pub count: usize,
    pub date: i64,
    pub creator: UserId,
    pub participants: Vec<UserId>,
    pub admins: Vec<UserId>,
    pub invite_link: Option<String>,
    pub migrated_to: Option<ChannelId>,
}

impl ChatData {
    pub fn new(chat_id: ChatId) -> Self {
        Self {
            peer: PeerData::new(PeerId::from_chat(chat_id)),
            title: String::new(),
            flags: ChatDataFlags::default(),
            admin_rights: ChatAdminRights::default(),
            default_restrictions: ChatRestrictions::default(),
            count: 0,
            date: 0,
            creator: UserId(0),
            participants: Vec::new(),
            admins: Vec::new(),
            invite_link: None,
            migrated_to: None,
        }
    }

    pub fn chat_id(&self) -> ChatId {
        self.peer.id.to_chat().unwrap_or(ChatId(0))
    }

    pub fn am_in(&self) -> bool {
        !self.is_forbidden() && !self.is_deactivated() && !self.have_left()
    }

    pub fn have_left(&self) -> bool {
        self.flags.has(ChatDataFlags::LEFT)
    }

    pub fn am_creator(&self) -> bool {
        self.flags.has(ChatDataFlags::CREATOR)
    }

    pub fn is_deactivated(&self) -> bool {
        self.flags.has(ChatDataFlags::DEACTIVATED)
    }

    pub fn is_forbidden(&self) -> bool {
        self.flags.has(ChatDataFlags::FORBIDDEN)
    }

    pub fn is_migrated(&self) -> bool {
        self.migrated_to.is_some()
    }

    pub fn allows_forwarding(&self) -> bool {
        !self.flags.has(ChatDataFlags::NO_FORWARDS)
    }

    pub fn can_delete_messages(&self) -> bool {
        self.am_creator() || self.admin_rights.can_delete_messages()
    }

    pub fn can_add_members(&self) -> bool {
        self.am_creator() || self.admin_rights.has(ChatAdminRights::INVITE_BY_LINK_OR_ADD)
    }
}

/// Target Symbol for Module 03 Ledger: DataChat coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataChat {
    pub chats: Vec<ChatData>,
}

impl DataChat {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_insert(&mut self, chat_id: ChatId) -> &mut ChatData {
        if let Some(pos) = self.chats.iter().position(|c| c.chat_id() == chat_id) {
            &mut self.chats[pos]
        } else {
            self.chats.push(ChatData::new(chat_id));
            self.chats.last_mut().unwrap()
        }
    }

    pub fn find(&self, chat_id: ChatId) -> Option<&ChatData> {
        self.chats.iter().find(|c| c.chat_id() == chat_id)
    }

    pub fn count(&self) -> usize {
        self.chats.len()
    }
}
