//! Target Symbol: DataChannel
//! Telegram Desktop 1:1 Submodule: `DataChannel`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::{PeerId, ChannelId, UserId};
use crate::data::data_peer::PeerData;
use crate::data::data_chat_participant_status::{ChatAdminRights, ChatRestrictions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ChannelDataFlags(pub u64);

impl ChannelDataFlags {
    pub const LEFT: u64 = 1 << 0;
    pub const CREATOR: u64 = 1 << 1;
    pub const FORBIDDEN: u64 = 1 << 2;
    pub const CALL_ACTIVE: u64 = 1 << 3;
    pub const CALL_NOT_EMPTY: u64 = 1 << 4;
    pub const SIGNATURES: u64 = 1 << 5;
    pub const VERIFIED: u64 = 1 << 6;
    pub const SCAM: u64 = 1 << 7;
    pub const FAKE: u64 = 1 << 8;
    pub const MEGAGROUP: u64 = 1 << 9;
    pub const BROADCAST: u64 = 1 << 10;
    pub const GIGAGROUP: u64 = 1 << 11;
    pub const USERNAME: u64 = 1 << 12;
    pub const CAN_SET_USERNAME: u64 = 1 << 14;
    pub const PRE_HISTORY_HIDDEN: u64 = 1 << 16;
    pub const SLOWMODE_ENABLED: u64 = 1 << 19;
    pub const NO_FORWARDS: u64 = 1 << 20;
    pub const JOIN_TO_WRITE: u64 = 1 << 21;
    pub const REQUEST_TO_JOIN: u64 = 1 << 22;
    pub const FORUM: u64 = 1 << 23;
    pub const ANTI_SPAM: u64 = 1 << 24;
    pub const PAID_MEDIA_ALLOWED: u64 = 1 << 33;
    pub const MONOFORUM: u64 = 1 << 39;

    pub const fn has(&self, flag: u64) -> bool {
        (self.0 & flag) == flag
    }

    pub fn add(&mut self, flag: u64) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u64) {
        self.0 &= !flag;
    }
}

/// Extra state maintained for Supergroups (megagroups).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MegagroupInfo {
    pub creator: Option<UserId>,
    pub admins: Vec<UserId>,
    pub last_participants_count: usize,
    pub slowmode_seconds: i32,
    pub slowmode_last_message: i64,
}

/// Supergroup or Broadcast Channel model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelData {
    pub peer: PeerData,
    pub title: String,
    pub username: Option<String>,
    pub access_hash: u64,
    pub flags: ChannelDataFlags,
    pub members_count: i32,
    pub admins_count: i32,
    pub admin_rights: ChatAdminRights,
    pub default_restrictions: ChatRestrictions,
    pub megagroup_info: Option<MegagroupInfo>,
}

impl ChannelData {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            peer: PeerData::new(PeerId::from_channel(channel_id)),
            title: String::new(),
            username: None,
            access_hash: 0,
            flags: ChannelDataFlags::default(),
            members_count: 0,
            admins_count: 0,
            admin_rights: ChatAdminRights::default(),
            default_restrictions: ChatRestrictions::default(),
            megagroup_info: None,
        }
    }

    pub fn channel_id(&self) -> ChannelId {
        self.peer.id.to_channel().unwrap_or(ChannelId(0))
    }

    pub fn is_megagroup(&self) -> bool {
        self.flags.has(ChannelDataFlags::MEGAGROUP)
    }

    pub fn is_broadcast(&self) -> bool {
        self.flags.has(ChannelDataFlags::BROADCAST)
    }

    pub fn is_gigagroup(&self) -> bool {
        self.flags.has(ChannelDataFlags::GIGAGROUP)
    }

    pub fn is_forum(&self) -> bool {
        self.flags.has(ChannelDataFlags::FORUM)
    }

    pub fn is_verified(&self) -> bool {
        self.flags.has(ChannelDataFlags::VERIFIED)
    }

    pub fn am_in(&self) -> bool {
        !self.flags.has(ChannelDataFlags::LEFT) && !self.flags.has(ChannelDataFlags::FORBIDDEN)
    }

    pub fn am_creator(&self) -> bool {
        self.flags.has(ChannelDataFlags::CREATOR)
    }

    pub fn can_post_messages(&self) -> bool {
        if self.is_broadcast() {
            self.am_creator() || self.admin_rights.can_post_messages()
        } else {
            self.am_in() && self.default_restrictions.can_send_messages()
        }
    }

    pub fn can_edit_messages(&self) -> bool {
        self.am_creator() || self.admin_rights.can_edit_messages()
    }

    pub fn can_delete_messages(&self) -> bool {
        self.am_creator() || self.admin_rights.can_delete_messages()
    }

    pub fn can_manage_topics(&self) -> bool {
        self.is_forum() && (self.am_creator() || self.admin_rights.can_manage_topics())
    }

    pub fn slowmode_seconds(&self) -> i32 {
        self.megagroup_info.as_ref().map_or(0, |info| info.slowmode_seconds)
    }
}

/// Target Symbol for Module 03 Ledger: DataChannel coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataChannel {
    pub channels: Vec<ChannelData>,
}

impl DataChannel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_insert(&mut self, channel_id: ChannelId) -> &mut ChannelData {
        if let Some(pos) = self.channels.iter().position(|c| c.channel_id() == channel_id) {
            &mut self.channels[pos]
        } else {
            self.channels.push(ChannelData::new(channel_id));
            self.channels.last_mut().unwrap()
        }
    }

    pub fn find(&self, channel_id: ChannelId) -> Option<&ChannelData> {
        self.channels.iter().find(|c| c.channel_id() == channel_id)
    }

    pub fn count(&self) -> usize {
        self.channels.len()
    }
}
