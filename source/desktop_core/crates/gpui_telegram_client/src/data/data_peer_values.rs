//! Target Symbol: DataPeerValues
//! Telegram Desktop 1:1 Submodule: `DataPeerValues`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

/// Observable peer change notification wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerValueChange<T> {
    pub peer_id: PeerId,
    pub old_value: T,
    pub new_value: T,
}

/// Target Symbol for Module 03 Ledger: DataPeerValues coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPeerValues {
    pub tracked_peers: Vec<PeerId>,
}

impl DataPeerValues {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn track(&mut self, peer_id: PeerId) {
        if !self.tracked_peers.contains(&peer_id) {
            self.tracked_peers.push(peer_id);
        }
    }

    pub fn untrack(&mut self, peer_id: PeerId) {
        self.tracked_peers.retain(|&p| p != peer_id);
    }
}
