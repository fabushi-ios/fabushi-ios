//! Target Symbol: DataBusinessChatbotsBusiness
//! Telegram Desktop 1:1 Submodule: `data/business/data_business_chatbots`
//! Subsystem: `data`, Feature ID: `TCG-N27`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChatbotsPermissions {
    pub can_reply: bool,
    pub can_read: bool,
    pub can_mark_read: bool,
    pub can_delete_sent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatbotsSettings {
    pub bot_user_id: u64,
    pub permissions: ChatbotsPermissions,
    pub exclude_contacts: bool,
    pub exclude_non_contacts: bool,
    pub excluded_peers: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataBusinessChatbotsBusiness {
    pub settings: Option<ChatbotsSettings>,
    pub paused_peer_ids: Vec<u64>,
}

impl DataBusinessChatbotsBusiness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_bot(&mut self, bot_id: u64, permissions: ChatbotsPermissions) {
        self.settings = Some(ChatbotsSettings {
            bot_user_id: bot_id,
            permissions,
            exclude_contacts: false,
            exclude_non_contacts: false,
            excluded_peers: Vec::new(),
        });
    }

    pub fn toggle_paused(&mut self, peer_id: u64, paused: bool) {
        if paused {
            if !self.paused_peer_ids.contains(&peer_id) {
                self.paused_peer_ids.push(peer_id);
            }
        } else {
            self.paused_peer_ids.retain(|&id| id != peer_id);
        }
    }

    pub fn is_paused_for(&self, peer_id: u64) -> bool {
        self.paused_peer_ids.contains(&peer_id)
    }

    pub fn can_reply(&self) -> bool {
        self.settings.as_ref().map(|s| s.permissions.can_reply).unwrap_or(false)
    }
}
