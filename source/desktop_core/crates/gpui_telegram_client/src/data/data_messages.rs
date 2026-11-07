//! Target Symbol: DataMessages
//! Telegram Desktop 1:1 Submodule: `DataMessages`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;
use crate::data::data_msg_id::{MsgId, FullMsgId};

/// Single concrete message entity stored in chat history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageItem {
    pub id: MsgId,
    pub peer_id: PeerId,
    pub date: i64,
    pub text: String,
    pub out: bool,
    pub unread: bool,
    pub media_type: Option<String>,
    pub reply_to: Option<MsgId>,
}

impl MessageItem {
    pub fn new(id: MsgId, peer_id: PeerId, text: impl Into<String>) -> Self {
        Self {
            id,
            peer_id,
            date: chrono::Utc::now().timestamp(),
            text: text.into(),
            out: false,
            unread: true,
            media_type: None,
            reply_to: None,
        }
    }

    pub fn full_id(&self) -> FullMsgId {
        FullMsgId::new(self.peer_id, self.id)
    }
}

/// Paginated slice of messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MessagesSlice {
    pub messages: Vec<MessageItem>,
    pub full_count: usize,
    pub skipped_before: usize,
    pub skipped_after: usize,
}

impl MessagesSlice {
    pub fn new(messages: Vec<MessageItem>, full_count: usize) -> Self {
        Self {
            messages,
            full_count,
            skipped_before: 0,
            skipped_after: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Target Symbol for Module 03 Ledger: DataMessages coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataMessages {
    pub messages: Vec<MessageItem>,
}

impl DataMessages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, msg: MessageItem) {
        if let Some(pos) = self.messages.iter().position(|m| m.full_id() == msg.full_id()) {
            self.messages[pos] = msg;
        } else {
            self.messages.push(msg);
        }
    }

    pub fn remove(&mut self, peer_id: PeerId, id: MsgId) {
        self.messages.retain(|m| !(m.peer_id == peer_id && m.id == id));
    }

    pub fn find(&self, peer_id: PeerId, id: MsgId) -> Option<&MessageItem> {
        self.messages.iter().find(|m| m.peer_id == peer_id && m.id == id)
    }

    pub fn count(&self) -> usize {
        self.messages.len()
    }
}
