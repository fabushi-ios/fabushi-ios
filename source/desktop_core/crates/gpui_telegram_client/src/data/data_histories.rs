//! Target Symbol: DataHistories
//! Telegram Desktop 1:1 Submodule: `DataHistories`
//! Subsystem: `data`, Feature ID: `TCG-N38`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_peer_id::PeerId;
use crate::data::data_msg_id::MsgId;
use crate::data::data_unread_value::UnreadValue;
use crate::data::data_folder::FolderId;
use crate::data::data_messages::MessageItem;

/// Dialog history representation for a single peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub peer_id: PeerId,
    pub unread: UnreadValue,
    pub inbox_read_till_id: MsgId,
    pub outbox_read_till_id: MsgId,
    pub last_message: Option<MessageItem>,
    pub folder_id: Option<FolderId>,
    pub is_pinned: bool,
}

impl History {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            unread: UnreadValue::default(),
            inbox_read_till_id: MsgId(0),
            outbox_read_till_id: MsgId(0),
            last_message: None,
            folder_id: None,
            is_pinned: false,
        }
    }

    pub fn set_unread_count(&mut self, count: i32) {
        self.unread.count = count;
    }

    pub fn read_inbox_till(&mut self, id: MsgId) {
        if id.0 > self.inbox_read_till_id.0 {
            self.inbox_read_till_id = id;
            if let Some(ref mut last) = self.last_message {
                if last.id.0 <= id.0 {
                    last.unread = false;
                }
            }
            self.unread.count = 0;
            self.unread.mark = false;
        }
    }

    pub fn apply_new_message(&mut self, msg: MessageItem) {
        if !msg.out {
            self.unread.count += 1;
        }
        self.last_message = Some(msg);
    }
}

/// Target Symbol for Module 03 Ledger: DataHistories repository and coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataHistories {
    pub histories: HashMap<PeerId, History>,
}

impl DataHistories {
    pub fn new() -> Self {
        Self {
            histories: HashMap::new(),
        }
    }

    pub fn get_or_create(&mut self, peer_id: PeerId) -> &mut History {
        self.histories.entry(peer_id).or_insert_with(|| History::new(peer_id))
    }

    pub fn find(&self, peer_id: PeerId) -> Option<&History> {
        self.histories.get(&peer_id)
    }

    pub fn read_inbox(&mut self, peer_id: PeerId, till_id: MsgId) {
        if let Some(hist) = self.histories.get_mut(&peer_id) {
            hist.read_inbox_till(till_id);
        }
    }

    pub fn total_unread_count(&self) -> i32 {
        self.histories.values().filter(|h| !h.unread.muted).map(|h| h.unread.count).sum()
    }

    pub fn count(&self) -> usize {
        self.histories.len()
    }
}
