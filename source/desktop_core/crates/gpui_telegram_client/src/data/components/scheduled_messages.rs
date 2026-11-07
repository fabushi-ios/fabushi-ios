//! Target Symbol: ScheduledMessagesComponents
//! Telegram Desktop 1:1 Submodule: `ScheduledMessagesComponents`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_peer_id::PeerId;
use crate::data::data_msg_id::MsgId;
use crate::data::data_messages::MessageItem;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledMessage {
    pub message: MessageItem,
    pub schedule_date: i64,
}

/// Target Symbol for Module 03 Ledger: ScheduledMessagesComponents coordinator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ScheduledMessagesComponents {
    pub scheduled: HashMap<PeerId, Vec<ScheduledMessage>>,
}

impl ScheduledMessagesComponents {
    pub fn new() -> Self {
        Self {
            scheduled: HashMap::new(),
        }
    }

    pub fn schedule(&mut self, peer_id: PeerId, msg: MessageItem, schedule_date: i64) {
        let list = self.scheduled.entry(peer_id).or_default();
        list.push(ScheduledMessage {
            message: msg,
            schedule_date,
        });
        list.sort_by_key(|m| m.schedule_date);
    }

    pub fn cancel(&mut self, peer_id: PeerId, msg_id: MsgId) -> bool {
        if let Some(list) = self.scheduled.get_mut(&peer_id) {
            let before = list.len();
            list.retain(|m| m.message.id != msg_id);
            list.len() < before
        } else {
            false
        }
    }

    pub fn count_for_peer(&self, peer_id: PeerId) -> usize {
        self.scheduled.get(&peer_id).map_or(0, |l| l.len())
    }
}
