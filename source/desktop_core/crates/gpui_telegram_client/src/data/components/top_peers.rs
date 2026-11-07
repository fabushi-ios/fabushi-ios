//! Target Symbol: TopPeersComponents
//! Telegram Desktop 1:1 Submodule: `TopPeersComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::PeerId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TopPeerType {
    #[default]
    Chat,
    BotGuestChat,
    BotApp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopPeer {
    pub peer_id: PeerId,
    pub rating: f64,
}

/// Target Symbol for Module 03 Ledger: TopPeersComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TopPeersComponents {
    pub list: Vec<TopPeer>,
    pub disabled: bool,
}

impl TopPeersComponents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn increment(&mut self, peer_id: PeerId, delta: f64) {
        if self.disabled {
            return;
        }
        if let Some(entry) = self.list.iter_mut().find(|p| p.peer_id == peer_id) {
            entry.rating += delta;
        } else {
            self.list.push(TopPeer { peer_id, rating: delta });
        }
        self.list.sort_by(|a, b| b.rating.partial_cmp(&a.rating).unwrap_or(std::cmp::Ordering::Equal));
    }

    pub fn top_peers(&self) -> Vec<PeerId> {
        self.list.iter().map(|p| p.peer_id).collect()
    }
}
