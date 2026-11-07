//! Target Symbol: RecentMoneyRecipientsComponents
//! Telegram Desktop 1:1 Submodule: `RecentMoneyRecipientsComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MoneyRecipient {
    pub peer_id: PeerId,
    pub last_date: i64,
}

/// Target Symbol for Module 03 Ledger: RecentMoneyRecipientsComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecentMoneyRecipientsComponents {
    pub recipients: Vec<MoneyRecipient>,
}

impl RecentMoneyRecipientsComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_transfer(&mut self, peer_id: PeerId) {
        let now = chrono::Utc::now().timestamp();
        if let Some(entry) = self.recipients.iter_mut().find(|r| r.peer_id == peer_id) {
            entry.last_date = now;
        } else {
            self.recipients.push(MoneyRecipient { peer_id, last_date: now });
        }
        self.recipients.sort_by(|a, b| b.last_date.cmp(&a.last_date));
    }
}
