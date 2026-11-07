//! Target Symbol: WelcomeMessagesComponents
//! Telegram Desktop 1:1 Submodule: `WelcomeMessagesComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WelcomeMessage {
    pub text: String,
    pub enabled: bool,
}

/// Target Symbol for Module 03 Ledger: WelcomeMessagesComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WelcomeMessagesComponents {
    pub messages: HashMap<PeerId, WelcomeMessage>,
}

impl WelcomeMessagesComponents {
    pub fn new() -> Self {
        Self {
            messages: HashMap::new(),
        }
    }

    pub fn set_welcome(&mut self, peer_id: PeerId, text: impl Into<String>, enabled: bool) {
        self.messages.insert(peer_id, WelcomeMessage {
            text: text.into(),
            enabled,
        });
    }

    pub fn get_welcome(&self, peer_id: PeerId) -> Option<&WelcomeMessage> {
        self.messages.get(&peer_id).filter(|m| m.enabled)
    }
}
