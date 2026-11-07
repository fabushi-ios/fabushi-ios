//! Telegram Administrative Log Filtering, Event Grouping & Pagination
//!
//! Citations:
//! - `Telegram/SourceFiles/history/admin_log/history_admin_log_filter_value.h:L13-L35` (FilterValue::Flag bitmasks)
//! - `Telegram/SourceFiles/history/admin_log/history_admin_log_filter.cpp:L17-L84` (FilterValueLabels category groupings)
//! - `Telegram/SourceFiles/history/admin_log/history_admin_log_inner.cpp:L89-L103` (Pagination and grouping constants)

use serde::{Deserialize, Serialize};

/// Upstream `history_admin_log_inner.cpp:L89-L103` constants
pub const K_MAX_CHANNEL_ADMINS: usize = 200;
pub const K_EVENTS_FIRST_PAGE: usize = 100;
pub const K_EVENTS_PER_PAGE: usize = 100;
pub const K_PRELOAD_UP_DISPLAY_ITEMS_BUFFER: usize = 4;
pub const K_NEW_EVENTS_CHECK_INTERVAL_MS: i64 = 5000;
pub const K_DELETE_GROUP_TIME_WINDOW_SECS: i64 = 5;

/// Administrative Log Event Filter Bitmasks matching `history_admin_log_filter_value.h:L13-L35`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AdminLogFilterFlags(pub u32);

impl AdminLogFilterFlags {
    pub const JOIN: u32 = 1 << 0;
    pub const LEAVE: u32 = 1 << 1;
    pub const INVITE: u32 = 1 << 2;
    pub const BAN: u32 = 1 << 3;
    pub const UNBAN: u32 = 1 << 4;
    pub const KICK: u32 = 1 << 5;
    pub const UNKICK: u32 = 1 << 6;
    pub const PROMOTE: u32 = 1 << 7;
    pub const DEMOTE: u32 = 1 << 8;
    pub const INFO: u32 = 1 << 9;
    pub const SETTINGS: u32 = 1 << 10;
    pub const PINNED: u32 = 1 << 11;
    pub const EDIT: u32 = 1 << 12;
    pub const DELETE: u32 = 1 << 13;
    pub const GROUP_CALL: u32 = 1 << 14;
    pub const INVITES: u32 = 1 << 15;
    pub const TOPICS: u32 = 1 << 16;
    pub const SUB_EXTEND: u32 = 1 << 17;
    pub const EDIT_RANK: u32 = 1 << 18;
    pub const MAX_FIELD: u32 = 1 << 18;

    // Composite bitmasks from `history_admin_log_filter.cpp:L21-L27`
    pub const ADMIN_RIGHTS: u32 = Self::PROMOTE | Self::DEMOTE;
    pub const RESTRICTIONS: u32 = Self::BAN | Self::UNBAN | Self::KICK | Self::UNKICK;
    pub const MEMBERS_NEW: u32 = Self::JOIN | Self::INVITE;
    pub const MEMBERS_REMOVED: u32 = Self::LEAVE;
    pub const INFO_SETTINGS: u32 = Self::INFO | Self::SETTINGS;

    pub fn new(bits: u32) -> Self {
        Self(bits)
    }

    pub fn all() -> Self {
        Self((1 << 19) - 1)
    }

    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) == flag
    }

    pub fn intersects(&self, flag: u32) -> bool {
        (self.0 & flag) != 0
    }

    pub fn insert(&mut self, flag: u32) {
        self.0 |= flag;
    }

    pub fn remove(&mut self, flag: u32) {
        self.0 &= !flag;
    }
}

/// Upstream `history_admin_log_filter_value.h:L12-L44`: `FilterValue`
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLogFilterValue {
    /// None means all event types
    pub flags: Option<AdminLogFilterFlags>,
    /// None means all admins
    pub admin_user_ids: Option<Vec<i64>>,
}

impl AdminLogFilterValue {
    pub fn all() -> Self {
        Self {
            flags: None,
            admin_user_ids: None,
        }
    }

    pub fn matches_event(&self, event_flag: u32, admin_id: i64) -> bool {
        if let Some(flags) = self.flags {
            if !flags.intersects(event_flag) {
                return false;
            }
        }
        if let Some(admins) = &self.admin_user_ids {
            if !admins.contains(&admin_id) {
                return false;
            }
        }
        true
    }
}

/// Upstream `history_admin_log_filter.cpp:L17-L84`: Filter Category Label Grouping
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLogFilterEntry {
    pub flags: u32,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminLogFilterSection {
    pub title: String,
    pub entries: Vec<AdminLogFilterEntry>,
}

/// Upstream `history_admin_log_filter.cpp:L17-L103`: Builds section groupings for Channel vs Supergroup
pub fn build_admin_log_filter_sections(is_channel: bool) -> Vec<AdminLogFilterSection> {
    let members_entries = vec![
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::ADMIN_RIGHTS,
            title: "New Admins".to_string(),
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::EDIT_RANK,
            title: "Admin Titles".to_string(),
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::RESTRICTIONS,
            title: "Restrictions".to_string(),
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::MEMBERS_NEW,
            title: if is_channel {
                "New Subscribers".to_string()
            } else {
                "New Members".to_string()
            },
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::MEMBERS_REMOVED,
            title: if is_channel {
                "Subscribers Removed".to_string()
            } else {
                "Members Removed".to_string()
            },
        },
    ];

    let mut settings_entries = vec![
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::INFO_SETTINGS,
            title: if is_channel {
                "Channel Info".to_string()
            } else {
                "Group Info".to_string()
            },
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::INVITES,
            title: "Invite Links".to_string(),
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::GROUP_CALL,
            title: "Voice Chats".to_string(),
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::SUB_EXTEND,
            title: "Subscription Extended".to_string(),
        },
    ];
    if !is_channel {
        settings_entries.push(AdminLogFilterEntry {
            flags: AdminLogFilterFlags::TOPICS,
            title: "Topics".to_string(),
        });
    }

    let mut messages_entries = vec![
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::DELETE,
            title: "Deleted Messages".to_string(),
        },
        AdminLogFilterEntry {
            flags: AdminLogFilterFlags::EDIT,
            title: "Edited Messages".to_string(),
        },
    ];
    if !is_channel {
        messages_entries.push(AdminLogFilterEntry {
            flags: AdminLogFilterFlags::PINNED,
            title: "Pinned Messages".to_string(),
        });
    }

    vec![
        AdminLogFilterSection {
            title: if is_channel {
                "Subscribers".to_string()
            } else {
                "Members".to_string()
            },
            entries: members_entries,
        },
        AdminLogFilterSection {
            title: if is_channel {
                "Channel Settings".to_string()
            } else {
                "Group Settings".to_string()
            },
            entries: settings_entries,
        },
        AdminLogFilterSection {
            title: "Messages".to_string(),
            entries: messages_entries,
        },
    ]
}

/// Upstream `history_admin_log_inner.h:L53-L59` & `history_admin_log_inner.cpp:L102-L104`: `DeleteGroup`
/// Consecutive message deletions by the same admin within 5 seconds are coalesced
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminLogDeleteGroup {
    pub event_id: u64,
    pub admin_id: i64,
    pub first_event_date: i64,
    pub last_event_date: i64,
    pub message_ids: Vec<i64>,
}

impl AdminLogDeleteGroup {
    pub fn new(event_id: u64, admin_id: i64, date: i64, initial_msg_id: i64) -> Self {
        Self {
            event_id,
            admin_id,
            first_event_date: date,
            last_event_date: date,
            message_ids: vec![initial_msg_id],
        }
    }

    /// Upstream `kDeleteGroupTimeWindowSeconds = 5`: returns true if message was coalesced
    pub fn try_coalesce(&mut self, admin_id: i64, date: i64, msg_id: i64) -> bool {
        if self.admin_id != admin_id {
            return false;
        }
        let diff = (date - self.first_event_date).abs();
        if diff <= K_DELETE_GROUP_TIME_WINDOW_SECS {
            self.last_event_date = date;
            self.message_ids.push(msg_id);
            true
        } else {
            false
        }
    }

    pub fn count(&self) -> usize {
        self.message_ids.len()
    }
}
