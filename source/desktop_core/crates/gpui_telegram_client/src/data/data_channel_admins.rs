//! Target Symbol: DataChannelAdmins
//! Telegram Desktop 1:1 Submodule: `DataChannelAdmins`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_peer_id::{ChannelId, UserId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AdminEntry {
    pub user_id: UserId,
    pub rank: String,
}

/// Target Symbol for Module 03 Ledger: DataChannelAdmins coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataChannelAdmins {
    pub channel_admins: HashMap<ChannelId, Vec<AdminEntry>>,
}

impl DataChannelAdmins {
    pub fn new() -> Self {
        Self {
            channel_admins: HashMap::new(),
        }
    }

    pub fn set_admin(&mut self, channel_id: ChannelId, user_id: UserId, rank: impl Into<String>) {
        let list = self.channel_admins.entry(channel_id).or_default();
        if let Some(pos) = list.iter().position(|a| a.user_id == user_id) {
            list[pos].rank = rank.into();
        } else {
            list.push(AdminEntry {
                user_id,
                rank: rank.into(),
            });
        }
    }

    pub fn remove_admin(&mut self, channel_id: ChannelId, user_id: UserId) {
        if let Some(list) = self.channel_admins.get_mut(&channel_id) {
            list.retain(|a| a.user_id != user_id);
        }
    }

    pub fn is_admin(&self, channel_id: ChannelId, user_id: UserId) -> bool {
        self.channel_admins
            .get(&channel_id)
            .map_or(false, |list| list.iter().any(|a| a.user_id == user_id))
    }

    pub fn get_rank(&self, channel_id: ChannelId, user_id: UserId) -> Option<&str> {
        self.channel_admins.get(&channel_id).and_then(|list| {
            list.iter().find(|a| a.user_id == user_id).map(|a| a.rank.as_str())
        })
    }
}
