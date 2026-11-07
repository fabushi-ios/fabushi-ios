//! Target Symbol: DataRepliesList
//! Telegram Desktop 1:1 Submodule: `data/data_replies_list`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataRepliesList {
    pub channel_id: u64,
    pub root_message_id: i32,
    pub replies_count: u32,
    pub unread_count: u32,
    pub message_ids: Vec<i32>,
    pub max_id: i32,
    pub read_inbox_max_id: i32,
    pub read_outbox_max_id: i32,
}

impl Default for DataRepliesList {
    fn default() -> Self {
        Self {
            channel_id: 0,
            root_message_id: 0,
            replies_count: 0,
            unread_count: 0,
            message_ids: Vec::new(),
            max_id: 0,
            read_inbox_max_id: 0,
            read_outbox_max_id: 0,
        }
    }
}

impl DataRepliesList {
    pub fn new(channel_id: u64, root_message_id: i32) -> Self {
        Self {
            channel_id,
            root_message_id,
            ..Default::default()
        }
    }

    pub fn add_reply(&mut self, reply_id: i32, is_incoming: bool) {
        if !self.message_ids.contains(&reply_id) {
            self.message_ids.push(reply_id);
            self.replies_count = self.message_ids.len() as u32;
            self.max_id = self.max_id.max(reply_id);
            if is_incoming && reply_id > self.read_inbox_max_id {
                self.unread_count = self.unread_count.saturating_add(1);
            }
        }
    }

    pub fn mark_read_inbox(&mut self, up_to_id: i32) {
        self.read_inbox_max_id = self.read_inbox_max_id.max(up_to_id);
        self.unread_count = self
            .message_ids
            .iter()
            .filter(|&&id| id > self.read_inbox_max_id)
            .count() as u32;
    }
}
