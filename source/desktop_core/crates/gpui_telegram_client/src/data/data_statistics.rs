//! Target Symbol: DataStatistics
//! Telegram Desktop 1:1 Submodule: `data/data_statistics`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataStatistics {
    pub channel_id: u64,
    pub period_start_ts: i64,
    pub period_end_ts: i64,
    pub current_members: u32,
    pub members_delta: i32,
    pub view_count: u32,
    pub forward_count: u32,
    pub reaction_count: u32,
    pub enabled_notifications_percentage: f64,
}

impl DataStatistics {
    pub fn new(channel_id: u64) -> Self {
        Self {
            channel_id,
            ..Default::default()
        }
    }

    pub fn set_period(&mut self, start: i64, end: i64) {
        self.period_start_ts = start;
        self.period_end_ts = end;
    }

    pub fn set_metrics(&mut self, members: u32, delta: i32, views: u32, forwards: u32, reactions: u32) {
        self.current_members = members;
        self.members_delta = delta;
        self.view_count = views;
        self.forward_count = forwards;
        self.reaction_count = reactions;
    }
}
