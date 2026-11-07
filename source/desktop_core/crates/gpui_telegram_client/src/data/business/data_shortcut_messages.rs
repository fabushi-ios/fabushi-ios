//! Target Symbol: DataShortcutMessagesBusiness
//! Telegram Desktop 1:1 Submodule: `data/business/data_shortcut_messages`
//! Subsystem: `data`, Feature ID: `TCG-N27`
//! Disposition: `rust_domain_or_server_1to1`

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusinessShortcut {
    pub id: u32,
    pub name: String,
    pub message_count: usize,
    pub top_message_id: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DataShortcutMessagesBusiness {
    pub shortcuts: HashMap<u32, BusinessShortcut>,
    pub name_to_id: HashMap<String, u32>,
}

impl DataShortcutMessagesBusiness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_shortcut(&mut self, id: u32, name: impl Into<String>, top_msg_id: i32) {
        let n = name.into();
        self.name_to_id.insert(n.to_ascii_lowercase(), id);
        self.shortcuts.insert(
            id,
            BusinessShortcut {
                id,
                name: n,
                message_count: 1,
                top_message_id: top_msg_id,
            },
        );
    }

    pub fn remove_shortcut(&mut self, id: u32) {
        if let Some(sc) = self.shortcuts.remove(&id) {
            self.name_to_id.remove(&sc.name.to_ascii_lowercase());
        }
    }

    pub fn find_by_name(&self, name: &str) -> Option<&BusinessShortcut> {
        let clean = name.strip_prefix('/').unwrap_or(name).to_ascii_lowercase();
        self.name_to_id
            .get(&clean)
            .and_then(|id| self.shortcuts.get(id))
    }
}
