//! Target Symbol: DataStatisticsLists
//! Telegram Desktop 1:1 Submodule: `data/data_statistics_lists`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageInteractionEntry {
    pub msg_id: i32,
    pub views: u32,
    pub forwards: u32,
    pub reactions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferrerEntry {
    pub source_name: String,
    pub joins_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataStatisticsLists {
    pub top_messages: Vec<MessageInteractionEntry>,
    pub recent_referrers: Vec<ReferrerEntry>,
}

impl DataStatisticsLists {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_message(&mut self, msg_id: i32, views: u32, forwards: u32, reactions: u32) {
        self.top_messages.push(MessageInteractionEntry {
            msg_id,
            views,
            forwards,
            reactions,
        });
    }

    pub fn add_referrer(&mut self, source_name: impl Into<String>, joins_count: u32) {
        self.recent_referrers.push(ReferrerEntry {
            source_name: source_name.into(),
            joins_count,
        });
    }
}
