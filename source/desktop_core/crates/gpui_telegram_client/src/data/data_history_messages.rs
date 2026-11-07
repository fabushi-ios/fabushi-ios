//! Target Symbol: DataHistoryMessages
//! Telegram Desktop 1:1 Submodule: `DataHistoryMessages`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;
use crate::data::data_msg_id::MsgId;
use crate::data::data_messages::MessageItem;

/// Ordered message stream for a single peer history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMessages {
    pub peer_id: PeerId,
    pub items: Vec<MessageItem>,
    pub min_id: MsgId,
    pub max_id: MsgId,
}

impl HistoryMessages {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            items: Vec::new(),
            min_id: MsgId(0),
            max_id: MsgId(0),
        }
    }

    pub fn insert_or_update(&mut self, msg: MessageItem) {
        if self.items.is_empty() {
            self.min_id = msg.id;
            self.max_id = msg.id;
            self.items.push(msg);
            return;
        }

        if let Some(pos) = self.items.iter().position(|m| m.id == msg.id) {
            self.items[pos] = msg;
        } else {
            let pos = self.items.iter().position(|m| m.id.0 > msg.id.0);
            if let Some(idx) = pos {
                self.items.insert(idx, msg);
            } else {
                self.items.push(msg);
            }
        }

        if let Some(first) = self.items.first() {
            self.min_id = first.id;
        }
        if let Some(last) = self.items.last() {
            self.max_id = last.id;
        }
    }

    pub fn remove(&mut self, id: MsgId) {
        self.items.retain(|m| m.id != id);
        if let Some(first) = self.items.first() {
            self.min_id = first.id;
        } else {
            self.min_id = MsgId(0);
        }
        if let Some(last) = self.items.last() {
            self.max_id = last.id;
        } else {
            self.max_id = MsgId(0);
        }
    }

    pub fn find(&self, id: MsgId) -> Option<&MessageItem> {
        self.items.iter().find(|m| m.id == id)
    }

    pub fn count(&self) -> usize {
        self.items.len()
    }
}

/// Target Symbol for Module 03 Ledger: DataHistoryMessages coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataHistoryMessages {
    pub history_messages: Option<HistoryMessages>,
}

impl DataHistoryMessages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_peer(peer_id: PeerId) -> Self {
        Self {
            history_messages: Some(HistoryMessages::new(peer_id)),
        }
    }

    pub fn count(&self) -> usize {
        self.history_messages.as_ref().map_or(0, |h| h.count())
    }
}
