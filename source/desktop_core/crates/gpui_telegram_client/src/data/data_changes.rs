//! Target Symbol: DataChanges
//! Telegram Desktop 1:1 Submodule: `DataChanges`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct PeerUpdateFlag(pub u64);

impl PeerUpdateFlag {
    pub const NONE: u64 = 0;
    pub const NAME: u64 = 1 << 0;
    pub const USERNAME: u64 = 1 << 1;
    pub const PHOTO: u64 = 1 << 2;
    pub const ABOUT: u64 = 1 << 3;
    pub const NOTIFICATIONS: u64 = 1 << 4;
    pub const MIGRATION: u64 = 1 << 5;
    pub const UNAVAILABLE_REASON: u64 = 1 << 6;
    pub const CHAT_THEME_TOKEN: u64 = 1 << 7;
    pub const IS_BLOCKED: u64 = 1 << 9;
    pub const MESSAGES_TTL: u64 = 1 << 10;
    pub const FULL_INFO: u64 = 1 << 11;
    pub const USERNAMES: u64 = 1 << 12;
    pub const COLOR: u64 = 1 << 14;
    pub const ONLINE_STATUS: u64 = 1 << 23;
    pub const BOT_COMMANDS: u64 = 1 << 24;
    pub const BIRTHDAY: u64 = 1 << 34;

    pub fn has(&self, flag: u64) -> bool {
        (self.0 & flag) == flag
    }
}

/// Peer update notification event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerUpdate {
    pub peer_id: PeerId,
    pub flags: PeerUpdateFlag,
}

/// Target Symbol for Module 03 Ledger: DataChanges notification dispatcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataChanges {
    pub pending_updates: Vec<PeerUpdate>,
}

impl DataChanges {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn notify(&mut self, peer_id: PeerId, flags: u64) {
        if let Some(existing) = self.pending_updates.iter_mut().find(|u| u.peer_id == peer_id) {
            existing.flags.0 |= flags;
        } else {
            self.pending_updates.push(PeerUpdate {
                peer_id,
                flags: PeerUpdateFlag(flags),
            });
        }
    }

    pub fn drain(&mut self) -> Vec<PeerUpdate> {
        std::mem::take(&mut self.pending_updates)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending_updates.is_empty()
    }
}
