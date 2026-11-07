//! Target Symbol: DataPollMessages
//! Telegram Desktop 1:1 Submodule: `data/data_poll_messages`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PollMessageLocation {
    pub peer_id: u64,
    pub msg_id: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataPollMessages {
    pub poll_to_messages: HashMap<u64, HashSet<PollMessageLocation>>,
}

impl DataPollMessages {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, poll_id: u64, peer_id: u64, msg_id: i32) {
        self.poll_to_messages
            .entry(poll_id)
            .or_default()
            .insert(PollMessageLocation { peer_id, msg_id });
    }

    pub fn unregister(&mut self, poll_id: u64, peer_id: u64, msg_id: i32) {
        if let Some(set) = self.poll_to_messages.get_mut(&poll_id) {
            set.remove(&PollMessageLocation { peer_id, msg_id });
            if set.is_empty() {
                self.poll_to_messages.remove(&poll_id);
            }
        }
    }

    pub fn messages_for_poll(&self, poll_id: u64) -> Vec<PollMessageLocation> {
        self.poll_to_messages
            .get(&poll_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }
}
