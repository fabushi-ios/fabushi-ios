//! Target Symbol: SponsoredMessagesComponents
//! Telegram Desktop 1:1 Submodule: `SponsoredMessagesComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SponsoredMessage {
    pub random_id: Vec<u8>,
    pub title: String,
    pub text: String,
    pub link: String,
    pub button_text: String,
    pub sponsor_info: String,
}

/// Target Symbol for Module 03 Ledger: SponsoredMessagesComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SponsoredMessagesComponents {
    pub channel_sponsored: HashMap<PeerId, Vec<SponsoredMessage>>,
}

impl SponsoredMessagesComponents {
    pub fn new() -> Self {
        Self {
            channel_sponsored: HashMap::new(),
        }
    }

    pub fn set_messages(&mut self, channel_id: PeerId, list: Vec<SponsoredMessage>) {
        self.channel_sponsored.insert(channel_id, list);
    }

    pub fn get_messages(&self, channel_id: PeerId) -> Option<&[SponsoredMessage]> {
        self.channel_sponsored.get(&channel_id).map(|v| v.as_slice())
    }

    pub fn clear(&mut self, channel_id: PeerId) {
        self.channel_sponsored.remove(&channel_id);
    }
}
