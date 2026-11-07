//! Target Symbol: DataCommunity
//! Telegram Desktop 1:1 Submodule: `DataCommunity`
//! Subsystem: `data`, Feature ID: `TCG-N11`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use crate::data::data_peer_id::{ChannelId, PeerId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityLinkedPeer {
    pub peer_id: PeerId,
    pub visible: bool,
    pub can_view_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityInfo {
    pub channel_id: ChannelId,
    pub linked_peers: Vec<CommunityLinkedPeer>,
    pub collapsed_in_chat_list: bool,
}

impl CommunityInfo {
    pub fn new(channel_id: ChannelId) -> Self {
        Self {
            channel_id,
            linked_peers: Vec::new(),
            collapsed_in_chat_list: false,
        }
    }

    pub fn add_linked_peer(&mut self, peer_id: PeerId, visible: bool) {
        if !self.linked_peers.iter().any(|p| p.peer_id == peer_id) {
            self.linked_peers.push(CommunityLinkedPeer {
                peer_id,
                visible,
                can_view_history: true,
            });
        }
    }
}

/// Target Symbol for Module 03 Ledger: DataCommunity coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataCommunity {
    pub communities: Vec<CommunityInfo>,
}

impl DataCommunity {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create(&mut self, channel_id: ChannelId) -> &mut CommunityInfo {
        if let Some(pos) = self.communities.iter().position(|c| c.channel_id == channel_id) {
            &mut self.communities[pos]
        } else {
            self.communities.push(CommunityInfo::new(channel_id));
            self.communities.last_mut().unwrap()
        }
    }

    pub fn find(&self, channel_id: ChannelId) -> Option<&CommunityInfo> {
        self.communities.iter().find(|c| c.channel_id == channel_id)
    }
}
