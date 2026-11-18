use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedWorkspaceEntry {
    pub key: String,
    pub value: Value,
    pub updated_by: String,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SharedGroupWorkspace {
    pub chat_id: i64,
    pub revision: u64,
    pub entries: HashMap<String, SharedWorkspaceEntry>,
}

impl SharedGroupWorkspace {
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            revision: 0,
            entries: HashMap::new(),
        }
    }

    pub fn set(
        &mut self,
        key: impl Into<String>,
        value: Value,
        updated_by: impl Into<String>,
    ) -> u64 {
        let key = key.into();
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.revision = self.revision.saturating_add(1);
        self.entries.insert(
            key.clone(),
            SharedWorkspaceEntry {
                key,
                value,
                updated_by: updated_by.into(),
                updated_at_ms: now_ms,
            },
        );
        self.revision
    }

    pub fn set_with_expected_revision(
        &mut self,
        expected_revision: u64,
        key: impl Into<String>,
        value: Value,
        updated_by: impl Into<String>,
    ) -> Result<u64, String> {
        if self.revision != expected_revision {
            return Err(format!(
                "Workspace revision conflict: expected {expected_revision}, actual {}",
                self.revision
            ));
        }
        Ok(self.set(key, value, updated_by))
    }

    pub fn get(&self, key: &str) -> Option<&SharedWorkspaceEntry> {
        self.entries.get(key)
    }

    pub fn remove(&mut self, key: &str) -> Option<SharedWorkspaceEntry> {
        let removed = self.entries.remove(key);
        if removed.is_some() {
            self.revision = self.revision.saturating_add(1);
        }
        removed
    }

    pub fn snapshot(&self) -> Value {
        serde_json::json!({
            "chat_id": self.chat_id,
            "revision": self.revision,
            "entries": self.entries,
        })
    }
}
