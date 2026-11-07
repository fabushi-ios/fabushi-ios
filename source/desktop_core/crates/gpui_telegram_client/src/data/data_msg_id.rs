//! Target Symbol: DataMsgId
//! Telegram Desktop 1:1 Submodule: `DataMsgId`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

/// Message identifier type. Positive values represent server message IDs,
/// while negative or high-offset values represent local/client pending messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct MsgId(pub i64);

impl MsgId {
    pub const SERVER_MAX_MSG_ID: i64 = 0x3FFF_FFFF;
    pub const CLIENT_MSG_ID_MASK: i64 = 0x4000_0000;
    pub const ZERO: Self = Self(0);

    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    pub const fn bare(self) -> i64 {
        self.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }

    pub const fn is_server(self) -> bool {
        self.0 > 0 && self.0 <= Self::SERVER_MAX_MSG_ID
    }

    pub const fn is_client(self) -> bool {
        self.0 < 0 || (self.0 & Self::CLIENT_MSG_ID_MASK) != 0
    }
}

impl std::fmt::Display for MsgId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "msg#{}", self.0)
    }
}

/// Pair of (PeerId, MsgId) uniquely identifying a message across all chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct FullMsgId {
    pub peer_id: PeerId,
    pub msg_id: MsgId,
}

impl FullMsgId {
    pub const fn new(peer_id: PeerId, msg_id: MsgId) -> Self {
        Self { peer_id, msg_id }
    }

    pub const fn is_valid(&self) -> bool {
        self.peer_id.is_valid() && self.msg_id.is_valid()
    }
}

/// Reply reference containing target message and optional reply quote range.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct FullReplyTo {
    pub message_id: FullMsgId,
    pub quote_text: Option<String>,
    pub quote_offset: Option<usize>,
}

/// Story identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct StoryId(pub u32);

/// Full story identifier combining peer ID and story ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct FullStoryId {
    pub peer_id: PeerId,
    pub story_id: StoryId,
}

/// Global message identifier for search results and global indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct GlobalMsgId(pub u64);

/// Message highlight indicator for search matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct MessageHighlightId(pub u64);

/// Message ID range [from, till].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct MsgRange {
    pub from: MsgId,
    pub till: MsgId,
}

impl MsgRange {
    pub const fn new(from: MsgId, till: MsgId) -> Self {
        Self { from, till }
    }

    pub fn contains(&self, id: MsgId) -> bool {
        id.0 >= self.from.0 && id.0 <= self.till.0
    }

    pub fn is_empty(&self) -> bool {
        self.from.0 > self.till.0
    }
}

/// Business quick-reply shortcut identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct BusinessShortcutId(pub u32);

/// Rich text quote metadata in replies.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct RichQuote {
    pub text: String,
    pub offset: usize,
    pub length: usize,
}

/// Options when requesting suggested message actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SuggestOptions {
    pub allow_quote: bool,
    pub allow_forward: bool,
}

/// Target Symbol for Module 03 Ledger: DataMsgId coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataMsgId {
    pub last_client_msg_id: i64,
}

impl DataMsgId {
    pub fn new() -> Self {
        Self {
            last_client_msg_id: -1,
        }
    }

    pub fn next_client_id(&mut self) -> MsgId {
        let id = self.last_client_msg_id;
        self.last_client_msg_id -= 1;
        MsgId(id)
    }

    pub fn make_full(peer_id: PeerId, msg_id: MsgId) -> FullMsgId {
        FullMsgId::new(peer_id, msg_id)
    }

    pub fn make_range(from: MsgId, till: MsgId) -> MsgRange {
        MsgRange::new(from, till)
    }
}
