//! Target Symbol: DataUnreadValue
//! Telegram Desktop 1:1 Submodule: `DataUnreadValue`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

/// Unread counter and notification state for a single chat/dialog row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct UnreadValue {
    pub count: i32,
    pub muted: bool,
    pub mark: bool,
}

impl UnreadValue {
    pub const fn new(count: i32, muted: bool, mark: bool) -> Self {
        Self { count, muted, mark }
    }

    pub fn is_empty(&self) -> bool {
        self.count <= 0 && !self.mark
    }

    pub fn display_badge_count(&self) -> i32 {
        if self.count > 0 {
            self.count
        } else if self.mark {
            1
        } else {
            0
        }
    }
}

/// Aggregate unread statistics across all dialogs or a specific folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct UnreadState {
    pub messages: i32,
    pub messages_muted: i32,
    pub chats: i32,
    pub chats_muted: i32,
    pub marks: i32,
    pub marks_muted: i32,
    pub mentions: i32,
    pub mentions_muted: i32,
    pub reactions: i32,
    pub reactions_muted: i32,
}

impl UnreadState {
    pub fn all_messages(&self) -> i32 {
        self.messages + self.messages_muted
    }

    pub fn all_chats(&self) -> i32 {
        self.chats + self.chats_muted
    }

    pub fn unmuted_messages(&self) -> i32 {
        self.messages
    }
}

/// Target Symbol for Module 03 Ledger: DataUnreadValue coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataUnreadValue {
    pub unread_state: UnreadState,
}

impl DataUnreadValue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_state(unread_state: UnreadState) -> Self {
        Self { unread_state }
    }

    pub fn has_unmuted_unread(&self) -> bool {
        self.unread_state.messages > 0 || self.unread_state.mentions > 0
    }
}
