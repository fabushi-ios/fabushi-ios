//! Target Symbol: DataPeerNotifySettingsNotify
//! Telegram Desktop 1:1 Submodule: `data/notify/data_peer_notify_settings`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataPeerNotifySettingsNotify {
    pub peer_id: u64,
    pub mute_until: i64, // 0 = unmuted, i64::MAX = forever
    pub sound_document_id: Option<u64>,
    pub show_previews: Option<bool>,
    pub stories_muted: bool,
}

impl DataPeerNotifySettingsNotify {
    pub fn new(peer_id: u64) -> Self {
        Self {
            peer_id,
            mute_until: 0,
            sound_document_id: None,
            show_previews: None,
            stories_muted: false,
        }
    }

    pub fn is_muted(&self, now: i64) -> bool {
        self.mute_until > now
    }

    pub fn mute_forever(&mut self) {
        self.mute_until = i64::MAX;
    }

    pub fn unmute(&mut self) {
        self.mute_until = 0;
    }

    pub fn mute_for_seconds(&mut self, seconds: i64, now: i64) {
        self.mute_until = now.saturating_add(seconds);
    }
}
