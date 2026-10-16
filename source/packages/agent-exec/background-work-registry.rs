use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundWakeup {
    pub conversation_id: String,
    pub id: String,
    pub group_key: Option<String>,
    pub payload: Value,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InMemoryBackgroundWakeupQueue {
    pending: Vec<BackgroundWakeup>,
    in_flight_by_id: BTreeMap<String, BackgroundWakeup>,
    accepted_keys: BTreeSet<String>,
    suppressed_ids_by_conversation: BTreeMap<String, BTreeSet<String>>,
    revision: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct DurableBackgroundWakeupFile {
    version: u32,
    account_id: String,
    account_generation: u64,
    queue: InMemoryBackgroundWakeupQueue,
}

impl InMemoryBackgroundWakeupQueue {
    pub fn restore(path: &Path, expected_account_id: &str, expected_generation: u64) -> Result<Self, String> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(format!("read background wakeup store: {error}")),
        };
        let file: DurableBackgroundWakeupFile = serde_json::from_slice(&bytes)
            .map_err(|error| format!("decode background wakeup store: {error}"))?;
        if file.version != 1 || file.account_id != expected_account_id || file.account_generation != expected_generation {
            return Ok(Self::default());
        }
        let mut queue = file.queue;
        let recovered = std::mem::take(&mut queue.in_flight_by_id).into_values().collect::<Vec<_>>();
        if !recovered.is_empty() {
            let mut pending = recovered;
            pending.extend(std::mem::take(&mut queue.pending));
            queue.pending = pending;
            queue.revision = queue.revision.saturating_add(1);
        }
        queue.rebuild_accepted_keys();
        Ok(queue)
    }

    pub fn persist(&self, path: &Path, account_id: &str, account_generation: u64) -> Result<(), String> {
        if self.pending.is_empty() && self.in_flight_by_id.is_empty() && self.suppressed_ids_by_conversation.is_empty() {
            match std::fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(format!("remove background wakeup store: {error}")),
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("create background wakeup store directory: {error}"))?;
        }
        let file = DurableBackgroundWakeupFile { version: 1, account_id: account_id.to_string(), account_generation, queue: self.clone() };
        let bytes = serde_json::to_vec(&file).map_err(|error| format!("serialize background wakeup store: {error}"))?;
        let part = PathBuf::from(format!("{}.part", path.display()));
        std::fs::write(&part, bytes).map_err(|error| format!("write background wakeup store: {error}"))?;
        match std::fs::rename(&part, path) {
            Ok(()) => Ok(()),
            Err(first_error) if path.exists() => {
                std::fs::remove_file(path).map_err(|error| format!("replace background wakeup store: {error}"))?;
                std::fs::rename(&part, path).map_err(|_| format!("commit background wakeup store: {first_error}"))
            }
            Err(error) => Err(format!("commit background wakeup store: {error}")),
        }
    }

    fn rebuild_accepted_keys(&mut self) {
        self.accepted_keys.clear();
        for wakeup in self.pending.iter().chain(self.in_flight_by_id.values()) {
            self.accepted_keys.insert(Self::key(&wakeup.conversation_id, &wakeup.id));
        }
    }

    fn key(conversation_id: &str, id: &str) -> String {
        format!("{conversation_id}\0{id}")
    }

    pub fn enqueue(&mut self, mut wakeup: BackgroundWakeup) -> bool {
        wakeup.id = wakeup.id.trim().to_string();
        if wakeup.id.is_empty() {
            return false;
        }
        let key = Self::key(&wakeup.conversation_id, &wakeup.id);
        if self.accepted_keys.contains(&key)
            || self
                .suppressed_ids_by_conversation
                .get(&wakeup.conversation_id)
                .is_some_and(|ids| ids.contains(&wakeup.id))
        {
            return false;
        }
        self.accepted_keys.insert(key);
        if let Some(group_key) = wakeup.group_key.as_deref()
            && let Some(index) = self.pending.iter().position(|pending| {
                pending.conversation_id == wakeup.conversation_id
                    && pending.group_key.as_deref() == Some(group_key)
            })
        {
            let replaced = std::mem::replace(&mut self.pending[index], wakeup);
            self.accepted_keys
                .remove(&Self::key(&replaced.conversation_id, &replaced.id));
            self.revision = self.revision.saturating_add(1);
            return true;
        }
        self.pending.push(wakeup);
        self.revision = self.revision.saturating_add(1);
        true
    }

    pub fn pull(&mut self, conversation_id: &str) -> Vec<BackgroundWakeup> {
        let suppressed = self
            .suppressed_ids_by_conversation
            .get(conversation_id)
            .cloned()
            .unwrap_or_default();
        let mut pulled = Vec::new();
        let mut rest = Vec::new();
        let mut changed = false;
        for wakeup in std::mem::take(&mut self.pending) {
            if wakeup.conversation_id != conversation_id {
                rest.push(wakeup);
                continue;
            }
            changed = true;
            if suppressed.contains(&wakeup.id) {
                self.accepted_keys
                    .remove(&Self::key(conversation_id, &wakeup.id));
                continue;
            }
            self.in_flight_by_id
                .insert(wakeup.id.clone(), wakeup.clone());
            pulled.push(wakeup);
        }
        self.pending = rest;
        if changed {
            self.revision = self.revision.saturating_add(1);
        }
        pulled
    }

    pub fn pull_all(&mut self) -> Vec<BackgroundWakeup> {
        let conversations = self
            .pending
            .iter()
            .map(|wakeup| wakeup.conversation_id.clone())
            .collect::<BTreeSet<_>>();
        conversations
            .into_iter()
            .flat_map(|conversation| self.pull(&conversation))
            .collect()
    }

    pub fn ack(&mut self, ids: &[String]) -> Vec<BackgroundWakeup> {
        let mut acked = Vec::new();
        for raw_id in ids {
            let id = raw_id.trim();
            if let Some(wakeup) = self.in_flight_by_id.remove(id) {
                self.accepted_keys
                    .remove(&Self::key(&wakeup.conversation_id, id));
                acked.push(wakeup);
            }
        }
        acked
    }

    pub fn nack(&mut self, ids: &[String], requeue: bool) {
        let requested = ids
            .iter()
            .map(|id| id.trim().to_string())
            .collect::<BTreeSet<_>>();
        let selected_ids = self
            .in_flight_by_id
            .keys()
            .filter(|id| requested.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        let mut selected = selected_ids
            .into_iter()
            .filter_map(|id| self.in_flight_by_id.remove(&id))
            .collect::<Vec<_>>();
        if !requeue {
            for wakeup in selected {
                self.accepted_keys
                    .remove(&Self::key(&wakeup.conversation_id, &wakeup.id));
            }
            return;
        }
        if selected.is_empty() {
            return;
        }
        for pending in std::mem::take(&mut self.pending) {
            if let Some(group_key) = pending.group_key.as_deref()
                && let Some(index) = selected.iter().position(|candidate| {
                    candidate.conversation_id == pending.conversation_id
                        && candidate.group_key.as_deref() == Some(group_key)
                })
            {
                let replaced = std::mem::replace(&mut selected[index], pending);
                self.accepted_keys
                    .remove(&Self::key(&replaced.conversation_id, &replaced.id));
                continue;
            }
            if !selected.iter().any(|candidate| {
                candidate.conversation_id == pending.conversation_id
                    && candidate.id == pending.id
            }) {
                selected.push(pending);
            }
        }
        self.pending = selected;
        self.revision = self.revision.saturating_add(1);
    }

    pub fn suppress(&mut self, conversation_id: &str, raw_id: &str) {
        let id = raw_id.trim();
        if id.is_empty() {
            return;
        }
        self.suppressed_ids_by_conversation
            .entry(conversation_id.to_string())
            .or_default()
            .insert(id.to_string());
        let before = self.pending.len();
        self.pending
            .retain(|wakeup| wakeup.conversation_id != conversation_id || wakeup.id != id);
        let mut changed = self.pending.len() != before;
        if self
            .in_flight_by_id
            .get(id)
            .is_some_and(|wakeup| wakeup.conversation_id == conversation_id)
        {
            self.in_flight_by_id.remove(id);
            changed = true;
        }
        if changed {
            self.accepted_keys.remove(&Self::key(conversation_id, id));
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub fn unsuppress_id(&mut self, raw_id: &str) {
        let id = raw_id.trim();
        let conversations = self
            .suppressed_ids_by_conversation
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for conversation in conversations {
            let empty = self
                .suppressed_ids_by_conversation
                .get_mut(&conversation)
                .is_some_and(|ids| {
                    ids.remove(id);
                    ids.is_empty()
                });
            if empty {
                self.suppressed_ids_by_conversation.remove(&conversation);
            }
        }
    }

    pub fn clear(&mut self, conversation_id: &str) {
        let before_pending = self.pending.len();
        self.pending
            .retain(|wakeup| wakeup.conversation_id != conversation_id);
        let inflight = self
            .in_flight_by_id
            .iter()
            .filter(|(_, wakeup)| wakeup.conversation_id == conversation_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let had_inflight = !inflight.is_empty();
        for id in inflight {
            self.in_flight_by_id.remove(&id);
        }
        let prefix = Self::key(conversation_id, "");
        let before_keys = self.accepted_keys.len();
        self.accepted_keys.retain(|key| !key.starts_with(&prefix));
        let had_suppressed = self
            .suppressed_ids_by_conversation
            .remove(conversation_id)
            .is_some();
        if self.pending.len() != before_pending
            || had_inflight
            || self.accepted_keys.len() != before_keys
            || had_suppressed
        {
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub fn clear_all(&mut self) {
        if self.pending.is_empty()
            && self.in_flight_by_id.is_empty()
            && self.accepted_keys.is_empty()
            && self.suppressed_ids_by_conversation.is_empty()
        {
            return;
        }
        self.pending.clear();
        self.in_flight_by_id.clear();
        self.accepted_keys.clear();
        self.suppressed_ids_by_conversation.clear();
        self.revision = self.revision.saturating_add(1);
    }

    pub fn has_pending(&self, conversation_id: Option<&str>) -> bool {
        match conversation_id {
            Some(conversation_id) => self
                .pending
                .iter()
                .any(|wakeup| wakeup.conversation_id == conversation_id),
            None => !self.pending.is_empty(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wake(conversation: &str, id: &str, group: Option<&str>) -> BackgroundWakeup {
        BackgroundWakeup {
            conversation_id: conversation.into(),
            id: id.into(),
            group_key: group.map(str::to_string),
            payload: json!({"id": id}),
            source: Some("test".into()),
        }
    }

    #[test]
    fn dedupes_identity_and_replaces_group_in_place() {
        let mut queue = InMemoryBackgroundWakeupQueue::default();
        queue.enqueue(wake("a", "one", Some("group")));
        queue.enqueue(wake("a", "one", Some("group")));
        queue.enqueue(wake("a", "two", Some("group")));
        assert_eq!(queue.pending_count(), 1);
        let pulled = queue.pull("a");
        assert_eq!(pulled.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), ["two"]);
    }

    #[test]
    fn pull_ack_nack_requeue_and_revision_match_registry_contract() {
        let mut queue = InMemoryBackgroundWakeupQueue::default();
        queue.enqueue(wake("a", "one", None));
        let after_enqueue = queue.revision();
        let pulled = queue.pull("a");
        assert_eq!(pulled.len(), 1);
        assert!(queue.revision() > after_enqueue);
        queue.nack(&["one".into()], true);
        assert!(queue.has_pending(Some("a")));
        let pulled = queue.pull("a");
        assert_eq!(queue.ack(&[pulled[0].id.clone()]).len(), 1);
        queue.enqueue(wake("a", "one", None));
        assert!(queue.has_pending(Some("a")));
    }

    #[test]
    fn suppression_blocks_duplicate_completion_until_unsuppressed() {
        let mut queue = InMemoryBackgroundWakeupQueue::default();
        queue.enqueue(wake("a", "one", None));
        queue.suppress("a", "one");
        assert!(!queue.has_pending(Some("a")));
        queue.enqueue(wake("a", "one", None));
        assert_eq!(queue.pending_count(), 0);
        queue.unsuppress_id("one");
        queue.enqueue(wake("a", "one", None));
        assert_eq!(queue.pending_count(), 1);
    }

    #[test]
    fn clear_is_conversation_scoped() {
        let mut queue = InMemoryBackgroundWakeupQueue::default();
        queue.enqueue(wake("a", "one", None));
        queue.enqueue(wake("b", "two", None));
        queue.clear("a");
        assert!(!queue.has_pending(Some("a")));
        assert!(queue.has_pending(Some("b")));
    }

    #[test]
    fn durable_store_requeues_inflight_after_restart() {
        let path = std::env::temp_dir().join(format!("fabushi-bg-wakeup-{}-restart.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut queue = InMemoryBackgroundWakeupQueue::default();
        queue.enqueue(wake("conversation-a", "task-1", None));
        assert_eq!(queue.pull("conversation-a").len(), 1);
        queue.persist(&path, "account-a", 7).expect("persist in-flight wakeup");
        let mut restored = InMemoryBackgroundWakeupQueue::restore(&path, "account-a", 7).expect("restore wakeup");
        assert!(restored.has_pending(Some("conversation-a")));
        assert_eq!(restored.pull("conversation-a")[0].id, "task-1");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn durable_store_rejects_stale_account_or_generation() {
        let path = std::env::temp_dir().join(format!("fabushi-bg-wakeup-{}-scope.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut queue = InMemoryBackgroundWakeupQueue::default();
        queue.enqueue(wake("conversation-a", "task-1", None));
        queue.persist(&path, "account-a", 7).expect("persist wakeup");
        let wrong_account = InMemoryBackgroundWakeupQueue::restore(&path, "account-b", 7).expect("restore wrong account");
        assert_eq!(wrong_account.pending_count(), 0);
        let wrong_generation = InMemoryBackgroundWakeupQueue::restore(&path, "account-a", 8).expect("restore wrong generation");
        assert_eq!(wrong_generation.pending_count(), 0);
        let _ = std::fs::remove_file(path);
    }
}
