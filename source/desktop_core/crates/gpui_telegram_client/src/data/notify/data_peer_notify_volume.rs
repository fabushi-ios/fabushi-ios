//! Target Symbol: DataPeerNotifyVolumeNotify
//! Telegram Desktop 1:1 Submodule: `data/notify/data_peer_notify_volume`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataPeerNotifyVolumeNotify {
    pub peer_id: u64,
    pub volume_percent: u8, // 0..100
    pub is_custom: bool,
}

impl Default for DataPeerNotifyVolumeNotify {
    fn default() -> Self {
        Self {
            peer_id: 0,
            volume_percent: 100,
            is_custom: false,
        }
    }
}

impl DataPeerNotifyVolumeNotify {
    pub fn new(peer_id: u64, volume_percent: u8) -> Self {
        Self {
            peer_id,
            volume_percent: volume_percent.min(100),
            is_custom: true,
        }
    }

    pub fn set_volume(&mut self, volume: u8) {
        self.volume_percent = volume.min(100);
        self.is_custom = true;
    }
}
