//! Target Symbol: EphemeralMessagesComponents
//! Telegram Desktop 1:1 Submodule: `EphemeralMessagesComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EphemeralTtl {
    pub seconds: i32,
}

/// Target Symbol for Module 03 Ledger: EphemeralMessagesComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EphemeralMessagesComponents {
    pub chat_ttl: HashMap<PeerId, EphemeralTtl>,
}

impl EphemeralMessagesComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_ttl(&mut self, peer_id: PeerId, seconds: i32) {
        self.chat_ttl.insert(peer_id, EphemeralTtl { seconds });
    }

    pub fn get_ttl(&self, peer_id: PeerId) -> Option<i32> {
        self.chat_ttl.get(&peer_id).map(|t| t.seconds)
    }

    pub fn is_ephemeral(&self, peer_id: PeerId) -> bool {
        self.get_ttl(peer_id).map_or(false, |s| s > 0)
    }
}
