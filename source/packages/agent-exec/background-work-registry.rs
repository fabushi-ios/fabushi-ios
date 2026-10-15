use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundWakeup {
    pub conversation_id: String,
    pub id: String,
    pub group_key: Option<String>,
    pub payload: Value,
    pub source: Option<String>,
}

#[derive(Debug, Default)]
pub struct InMemoryBackgroundWakeupQueue {
    pending: Vec<BackgroundWakeup>,
    in_flight_by_id: BTreeMap<String, BackgroundWakeup>,
    accepted_keys: BTreeSet<String>,
    suppressed_ids_by_conversation: BTreeMap<String, BTreeSet<String>>,
    revision: u64,
}

impl InMemoryBackgroundWakeupQueue {
    fn key(conversation_id: &str, id: &str) -> String {
        format!("{conversation_id}\0{id}")
    }

    pub fn enqueue(&mut self, mut wakeup: BackgroundWakeup) {
        wakeup.id = wakeup.id.trim().to_string();
        if wakeup.id.is_empty() {
            return;
        }
        let key = Self::key(&wakeup.conversation_id, &wakeup.id);
        if self.accepted_keys.contains(&key)
            || self
                .suppressed_ids_by_conversation
                .get(&wakeup.conversation_id)
                .is_some_and(|ids| ids.contains(&wakeup.id))
        {
            return;
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
            return;
        }
        self.pending.push(wakeup);
        self.revision = self.revision.saturating_add(1);
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
}
