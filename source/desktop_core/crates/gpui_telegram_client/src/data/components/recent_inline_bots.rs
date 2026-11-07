//! Target Symbol: RecentInlineBotsComponents
//! Telegram Desktop 1:1 Submodule: `RecentInlineBotsComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecentInlineBot {
    pub bot_id: UserId,
    pub count: usize,
}

/// Target Symbol for Module 03 Ledger: RecentInlineBotsComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecentInlineBotsComponents {
    pub bots: Vec<RecentInlineBot>,
}

impl RecentInlineBotsComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_use(&mut self, bot_id: UserId) {
        if let Some(entry) = self.bots.iter_mut().find(|b| b.bot_id == bot_id) {
            entry.count += 1;
        } else {
            self.bots.push(RecentInlineBot { bot_id, count: 1 });
        }
        self.bots.sort_by(|a, b| b.count.cmp(&a.count));
    }
}
