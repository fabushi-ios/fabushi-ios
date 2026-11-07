//! Target Symbol: FactchecksComponents
//! Telegram Desktop 1:1 Submodule: `FactchecksComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_msg_id::FullMsgId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Factcheck {
    pub hash: u64,
    pub text: String,
    pub country: String,
}

/// Target Symbol for Module 03 Ledger: FactchecksComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FactchecksComponents {
    pub factchecks: HashMap<FullMsgId, Factcheck>,
}

impl FactchecksComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_factcheck(&mut self, id: FullMsgId, check: Factcheck) {
        self.factchecks.insert(id, check);
    }

    pub fn get_factcheck(&self, id: FullMsgId) -> Option<&Factcheck> {
        self.factchecks.get(&id)
    }
}
