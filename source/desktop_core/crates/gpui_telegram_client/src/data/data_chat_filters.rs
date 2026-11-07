//! Target Symbol: DataChatFilters
//! Telegram Desktop 1:1 Submodule: `DataChatFilters`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

pub type FilterId = i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct ChatFilterFlags(pub u32);

impl ChatFilterFlags {
    pub const CONTACTS: u32 = 1 << 0;
    pub const NON_CONTACTS: u32 = 1 << 1;
    pub const GROUPS: u32 = 1 << 2;
    pub const CHANNELS: u32 = 1 << 3;
    pub const BOTS: u32 = 1 << 4;
    pub const NO_MUTED: u32 = 1 << 5;
    pub const NO_READ: u32 = 1 << 6;
    pub const NO_ARCHIVED: u32 = 1 << 7;
    pub const CHATLIST: u32 = 1 << 8;
    pub const HAS_MY_LINKS: u32 = 1 << 9;

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

/// Evaluation input attributes for a chat when filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChatFilterCandidate {
    pub peer_id: PeerId,
    pub is_contact: bool,
    pub is_non_contact: bool,
    pub is_group: bool,
    pub is_channel: bool,
    pub is_bot: bool,
    pub is_muted: bool,
    pub is_read: bool,
    pub is_archived: bool,
}

/// Custom tab folder filter specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatFilter {
    pub id: FilterId,
    pub title: String,
    pub icon_emoji: String,
    pub color_index: Option<u8>,
    pub flags: ChatFilterFlags,
    pub always: Vec<PeerId>,
    pub pinned: Vec<PeerId>,
    pub never: Vec<PeerId>,
}

impl ChatFilter {
    pub fn new(id: FilterId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            icon_emoji: String::new(),
            color_index: None,
            flags: ChatFilterFlags::default(),
            always: Vec::new(),
            pinned: Vec::new(),
            never: Vec::new(),
        }
    }

    pub fn matches(&self, candidate: &ChatFilterCandidate) -> bool {
        // 1. Explicit always include
        if self.always.contains(&candidate.peer_id) || self.pinned.contains(&candidate.peer_id) {
            return true;
        }

        // 2. Explicit never exclude
        if self.never.contains(&candidate.peer_id) {
            return false;
        }

        // 3. Flags-based negative exclusions
        if self.flags.has(ChatFilterFlags::NO_ARCHIVED) && candidate.is_archived {
            return false;
        }
        if self.flags.has(ChatFilterFlags::NO_READ) && candidate.is_read {
            return false;
        }
        if self.flags.has(ChatFilterFlags::NO_MUTED) && candidate.is_muted {
            return false;
        }

        // 4. Flags-based entity type inclusions
        let mut type_filter_active = false;
        let mut type_matched = false;

        if self.flags.has(ChatFilterFlags::CONTACTS) {
            type_filter_active = true;
            if candidate.is_contact {
                type_matched = true;
            }
        }
        if self.flags.has(ChatFilterFlags::NON_CONTACTS) {
            type_filter_active = true;
            if candidate.is_non_contact {
                type_matched = true;
            }
        }
        if self.flags.has(ChatFilterFlags::GROUPS) {
            type_filter_active = true;
            if candidate.is_group {
                type_matched = true;
            }
        }
        if self.flags.has(ChatFilterFlags::CHANNELS) {
            type_filter_active = true;
            if candidate.is_channel {
                type_matched = true;
            }
        }
        if self.flags.has(ChatFilterFlags::BOTS) {
            type_filter_active = true;
            if candidate.is_bot {
                type_matched = true;
            }
        }

        if type_filter_active {
            type_matched
        } else {
            true
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataChatFilters coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataChatFilters {
    pub filters: Vec<ChatFilter>,
}

impl DataChatFilters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, filter: ChatFilter) {
        if let Some(pos) = self.filters.iter().position(|f| f.id == filter.id) {
            self.filters[pos] = filter;
        } else {
            self.filters.push(filter);
        }
    }

    pub fn remove(&mut self, id: FilterId) {
        self.filters.retain(|f| f.id != id);
    }

    pub fn find(&self, id: FilterId) -> Option<&ChatFilter> {
        self.filters.iter().find(|f| f.id == id)
    }

    pub fn count(&self) -> usize {
        self.filters.len()
    }
}
