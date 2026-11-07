//! Target Symbol: DataSearchCalendar
//! Telegram Desktop 1:1 Submodule: `DataSearchCalendar`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_msg_id::MsgId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CalendarPeriod {
    pub date: i64,
    pub min_msg_id: MsgId,
    pub max_msg_id: MsgId,
    pub count: i32,
}

/// Target Symbol for Module 03 Ledger: DataSearchCalendar coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataSearchCalendar {
    pub periods: Vec<CalendarPeriod>,
}

impl DataSearchCalendar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_period(&mut self, period: CalendarPeriod) {
        self.periods.push(period);
    }

    pub fn total_messages(&self) -> i32 {
        self.periods.iter().map(|p| p.count).sum()
    }
}
