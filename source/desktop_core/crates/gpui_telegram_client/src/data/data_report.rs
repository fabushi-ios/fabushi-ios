//! Target Symbol: DataReport
//! Telegram Desktop 1:1 Submodule: `DataReport`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_msg_id::MsgId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReportInput {
    pub option_id: String,
    pub option_text: String,
    pub comment: String,
    pub ids: Vec<MsgId>,
    pub stories: Vec<u32>,
}

/// Target Symbol for Module 03 Ledger: DataReport coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataReport {
    pub reports: Vec<ReportInput>,
}

impl DataReport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn submit(&mut self, report: ReportInput) {
        self.reports.push(report);
    }

    pub fn count(&self) -> usize {
        self.reports.len()
    }
}
