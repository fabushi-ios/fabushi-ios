//! Target Symbol: DataSearchController
//! Telegram Desktop 1:1 Submodule: `DataSearchController`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;
use crate::data::data_msg_id::MsgId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SearchQuery {
    pub peer_id: Option<PeerId>,
    pub query: String,
    pub min_date: i64,
    pub max_date: i64,
    pub offset_id: MsgId,
    pub limit: i32,
}

/// Target Symbol for Module 03 Ledger: DataSearchController coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataSearchController {
    pub active_query: Option<SearchQuery>,
    pub total_found: i32,
}

impl DataSearchController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_query(&mut self, query: SearchQuery) {
        self.active_query = Some(query);
    }

    pub fn clear(&mut self) {
        self.active_query = None;
        self.total_found = 0;
    }
}
