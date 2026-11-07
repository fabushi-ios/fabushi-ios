//! Target Symbol: DataGroups
//! Telegram Desktop 1:1 Submodule: `DataGroups`
//! Subsystem: `data`, Feature ID: `TCG-N04`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::data::data_types::MessageGroupId;
use crate::data::data_msg_id::MsgId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Group {
    pub message_ids: Vec<MsgId>,
}

/// Target Symbol for Module 03 Ledger: DataGroups coordinator managing message albums.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataGroups {
    pub groups: HashMap<MessageGroupId, Group>,
}

impl DataGroups {
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
        }
    }

    pub fn register_message(&mut self, group_id: MessageGroupId, msg_id: MsgId) {
        let entry = self.groups.entry(group_id).or_default();
        if !entry.message_ids.contains(&msg_id) {
            entry.message_ids.push(msg_id);
        }
    }

    pub fn unregister_message(&mut self, group_id: MessageGroupId, msg_id: MsgId) {
        if let Some(entry) = self.groups.get_mut(&group_id) {
            entry.message_ids.retain(|&id| id != msg_id);
            if entry.message_ids.is_empty() {
                self.groups.remove(&group_id);
            }
        }
    }

    pub fn is_grouped(&self, group_id: MessageGroupId) -> bool {
        self.groups.get(&group_id).map_or(false, |g| g.message_ids.len() > 1)
    }

    pub fn count(&self) -> usize {
        self.groups.len()
    }
}
