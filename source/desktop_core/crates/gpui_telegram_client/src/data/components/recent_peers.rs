//! Target Symbol: RecentPeersComponents
//! Telegram Desktop 1:1 Submodule: `RecentPeersComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecentPeerEntry {
    pub peer_id: PeerId,
    pub last_used: i64,
}

/// Target Symbol for Module 03 Ledger: RecentPeersComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecentPeersComponents {
    pub peers: Vec<RecentPeerEntry>,
}

impl RecentPeersComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bump(&mut self, peer_id: PeerId) {
        let now = chrono::Utc::now().timestamp();
        self.peers.retain(|p| p.peer_id != peer_id);
        self.peers.insert(0, RecentPeerEntry { peer_id, last_used: now });
        if self.peers.len() > 50 {
            self.peers.truncate(50);
        }
    }

    pub fn list(&self) -> Vec<PeerId> {
        self.peers.iter().map(|p| p.peer_id).collect()
    }
}
