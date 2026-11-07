//! Target Symbol: DataForumTopic
//! Telegram Desktop 1:1 Submodule: `data/data_forum_topic`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

pub const FORUM_GENERAL_TOPIC_ID: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataForumTopic {
    pub channel_id: u64,
    pub root_id: i32,
    pub title: String,
    pub icon_id: u64,
    pub color_id: i32,
    pub creator_id: u64,
    pub creation_date: i64,
    pub is_my: bool,
    pub is_closed: bool,
    pub is_hidden: bool,
    pub is_pinned: bool,
    pub unread_count: u32,
    pub unread_mentions_count: u32,
    pub unread_reactions_count: u32,
    pub last_message_id: i32,
    pub last_server_message_id: i32,
}

impl Default for DataForumTopic {
    fn default() -> Self {
        Self {
            channel_id: 0,
            root_id: FORUM_GENERAL_TOPIC_ID,
            title: "General".to_string(),
            icon_id: 0,
            color_id: 0,
            creator_id: 0,
            creation_date: 0,
            is_my: false,
            is_closed: false,
            is_hidden: false,
            is_pinned: false,
            unread_count: 0,
            unread_mentions_count: 0,
            unread_reactions_count: 0,
            last_message_id: 0,
            last_server_message_id: 0,
        }
    }
}

impl DataForumTopic {
    pub fn new(channel_id: u64, root_id: i32, title: impl Into<String>) -> Self {
        Self {
            channel_id,
            root_id,
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn general(channel_id: u64) -> Self {
        Self::new(channel_id, FORUM_GENERAL_TOPIC_ID, "General")
    }

    pub fn is_general(&self) -> bool {
        self.root_id == FORUM_GENERAL_TOPIC_ID
    }

    pub fn with_icon(mut self, icon_id: u64, color_id: i32) -> Self {
        self.icon_id = icon_id;
        self.color_id = color_id;
        self
    }

    pub fn set_closed(&mut self, closed: bool) {
        self.is_closed = closed;
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.is_pinned = pinned;
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        self.is_hidden = hidden;
    }

    pub fn read_till_end(&mut self) {
        self.unread_count = 0;
        self.unread_mentions_count = 0;
        self.unread_reactions_count = 0;
    }

    pub fn apply_new_message(&mut self, msg_id: i32, is_incoming: bool, has_mention: bool, has_reaction: bool) {
        self.last_message_id = msg_id;
        self.last_server_message_id = msg_id;
        if is_incoming {
            self.unread_count = self.unread_count.saturating_add(1);
            if has_mention {
                self.unread_mentions_count = self.unread_mentions_count.saturating_add(1);
            }
            if has_reaction {
                self.unread_reactions_count = self.unread_reactions_count.saturating_add(1);
            }
        }
    }
}
