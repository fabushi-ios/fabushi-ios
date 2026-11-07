//! Target Symbol: DataSparseIds
//! Telegram Desktop 1:1 Submodule: `DataSparseIds`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_msg_id::{MsgId, FullMsgId};
use crate::data::data_peer_id::PeerId;
use crate::data::data_abstract_sparse_ids::AbstractSparseIds;

pub type SparseIdsSlice = AbstractSparseIds<MsgId>;

/// Merged message stream slice across regular messages and migrated pre-history messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SparseIdsMergedSlice {
    pub peer_id: PeerId,
    pub migrated_peer_id: Option<PeerId>,
    pub slice: SparseIdsSlice,
}

impl SparseIdsMergedSlice {
    pub fn new(peer_id: PeerId, slice: SparseIdsSlice) -> Self {
        Self {
            peer_id,
            migrated_peer_id: None,
            slice,
        }
    }

    pub fn full_count(&self) -> Option<usize> {
        self.slice.full_count
    }

    pub fn size(&self) -> usize {
        self.slice.size()
    }

    pub fn index_of(&self, full_id: FullMsgId) -> Option<usize> {
        if full_id.peer_id == self.peer_id {
            self.slice.index_of(&full_id.msg_id)
        } else {
            None
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataSparseIds coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataSparseIds {
    pub slice: SparseIdsMergedSlice,
}

impl DataSparseIds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_peer(peer_id: PeerId, msg_ids: Vec<MsgId>) -> Self {
        let count = msg_ids.len();
        let sparse = AbstractSparseIds::new(msg_ids, Some(count), Some(0), Some(0));
        Self {
            slice: SparseIdsMergedSlice::new(peer_id, sparse),
        }
    }

    pub fn count(&self) -> usize {
        self.slice.size()
    }
}
