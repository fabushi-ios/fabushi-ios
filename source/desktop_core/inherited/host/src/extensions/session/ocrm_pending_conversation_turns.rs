use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingConversationTurnReply {
    pub id: String,
    pub conversation_ref: String,
    pub message_id: String,
    pub reply_to_id: String,
    pub thread_id: Option<String>,
    pub text: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingConversationTurnSnapshot {
    pub id: String,
    pub agent_id: String,
    pub conversation_ref: String,
    pub session_id: String,
    pub thread_id: Option<String>,
    pub outbound_message_id: Option<String>,
    pub claimed: bool,
    pub correlation_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingConversationTurnClaimResult {
    NotFound,
    NotReady,
    Claimed(PendingConversationTurnReply),
}

#[derive(Clone, Default)]
pub struct PendingConversationTurnRegistry {
    inner: Arc<Mutex<PendingConversationTurnRegistryInner>>,
}

struct PendingConversationTurn {
    snapshot: PendingConversationTurnSnapshot,
    created_at: Instant,
    expires_at: Instant,
}

#[derive(Default)]
struct PendingConversationTurnRegistryInner {
    turns: HashMap<String, PendingConversationTurn>,
    by_outbound_id: HashMap<String, Vec<String>>,
}

impl PendingConversationTurnRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &self,
        agent_id: &str,
        id: Option<&str>,
        conversation_ref: &str,
        session_id: &str,
        thread_id: Option<&str>,
        timeout: Duration,
    ) -> Result<String, String> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err("conversation turn requires an agent id".to_string());
        }
        let conversation_ref = conversation_ref.trim();
        let session_id = session_id.trim();
        if conversation_ref.is_empty() || session_id.is_empty() {
            return Err("conversation turn requires conversation and session identity".to_string());
        }
        let id = id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let key = turn_key(agent_id, &id);
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prune_expired_locked(&mut inner);
        if inner.turns.contains_key(&key) {
            return Err(format!("conversation turn already pending for {agent_id}: {id}"));
        }
        let now = Instant::now();
        inner.turns.insert(
            key,
            PendingConversationTurn {
                snapshot: PendingConversationTurnSnapshot {
                    id: id.clone(),
                    agent_id: agent_id.to_string(),
                    conversation_ref: conversation_ref.to_string(),
                    session_id: session_id.to_string(),
                    thread_id: thread_id.map(str::trim).filter(|value| !value.is_empty()).map(ToOwned::to_owned),
                    outbound_message_id: None,
                    claimed: false,
                    correlation_ready: false,
                },
                created_at: now,
                expires_at: now + timeout,
            },
        );
        Ok(id)
    }

    pub fn set_outbound_message_id(&self, agent_id: &str, id: &str, message_id: &str) -> bool {
        let key = turn_key(agent_id.trim(), id.trim());
        let message_id = message_id.trim();
        if message_id.is_empty() {
            self.cancel(agent_id, id);
            return false;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prune_expired_locked(&mut inner);
        let Some(old_message_id) = inner
            .turns
            .get(&key)
            .map(|turn| turn.snapshot.outbound_message_id.clone())
        else {
            return false;
        };
        remove_outbound_index(
            &mut inner.by_outbound_id,
            &key,
            old_message_id
                .as_deref()
                .map(|value| outbound_key(agent_id.trim(), value)),
        );
        let turn = inner.turns.get_mut(&key).expect("pending turn after lookup");
        turn.snapshot.outbound_message_id = Some(message_id.to_string());
        inner
            .by_outbound_id
            .entry(outbound_key(agent_id.trim(), message_id))
            .or_default()
            .push(key);
        true
    }

    pub fn mark_ready(&self, agent_id: &str, id: &str) -> bool {
        let key = turn_key(agent_id.trim(), id.trim());
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prune_expired_locked(&mut inner);
        let Some(has_outbound_message) = inner
            .turns
            .get(&key)
            .map(|turn| turn.snapshot.outbound_message_id.is_some())
        else {
            return false;
        };
        if !has_outbound_message {
            return false;
        }
        let turn = inner.turns.get_mut(&key).expect("pending turn after lookup");
        turn.snapshot.correlation_ready = true;
        true
    }

    pub fn snapshot(&self, agent_id: &str, id: &str) -> Option<PendingConversationTurnSnapshot> {
        let key = turn_key(agent_id.trim(), id.trim());
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prune_expired_locked(&mut inner);
        inner.turns.get(&key).map(|turn| turn.snapshot.clone())
    }

    pub fn find_pending(&self, agent_id: &str, conversation_ref: &str) -> Vec<PendingConversationTurnSnapshot> {
        let agent_id = agent_id.trim();
        let conversation_ref = conversation_ref.trim();
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prune_expired_locked(&mut inner);
        inner
            .turns
            .values()
            .filter(|turn| {
                !turn.snapshot.claimed
                    && turn.snapshot.agent_id == agent_id
                    && turn.snapshot.conversation_ref == conversation_ref
            })
            .map(|turn| turn.snapshot.clone())
            .collect()
    }

    pub fn claim_reply(
        &self,
        agent_id: &str,
        conversation_ref: &str,
        parent_conversation_ref: Option<&str>,
        session_id: &str,
        message_id: &str,
        reply_to_id: Option<&str>,
        thread_id: Option<&str>,
        text: &str,
        timestamp_ms: u64,
    ) -> PendingConversationTurnClaimResult {
        let Some(reply_to_id) = reply_to_id.map(str::trim).filter(|value| !value.is_empty()) else {
            return PendingConversationTurnClaimResult::NotFound;
        };
        let agent_id = agent_id.trim();
        let conversation_ref = conversation_ref.trim();
        let session_id = session_id.trim();
        let thread_id = thread_id.map(str::trim).filter(|value| !value.is_empty());
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prune_expired_locked(&mut inner);
        let candidates = inner.by_outbound_id.get(&outbound_key(agent_id, reply_to_id)).cloned().unwrap_or_default();
        let mut selected: Option<(String, Instant)> = None;
        for key in candidates {
            let Some(turn) = inner.turns.get(&key) else { continue };
            let same_conversation = match turn.snapshot.thread_id.as_deref() {
                Some(expected_thread_id) => {
                    thread_id == Some(expected_thread_id)
                        && (parent_conversation_ref
                            == Some(turn.snapshot.conversation_ref.as_str())
                            || (parent_conversation_ref.is_none()
                                && turn.snapshot.conversation_ref == conversation_ref))
                }
                None => {
                    thread_id.is_none()
                        && parent_conversation_ref.is_none()
                        && turn.snapshot.conversation_ref == conversation_ref
                }
            };
            if turn.snapshot.claimed
                || !same_conversation
                || turn.snapshot.session_id != session_id
            {
                continue;
            }
            if selected.as_ref().is_none_or(|(_, created_at)| turn.created_at < *created_at) {
                selected = Some((key, turn.created_at));
            }
        }
        let Some((key, _)) = selected else {
            return PendingConversationTurnClaimResult::NotFound;
        };
        let turn = inner.turns.get_mut(&key).expect("selected pending turn");
        if !turn.snapshot.correlation_ready {
            return PendingConversationTurnClaimResult::NotReady;
        }
        turn.snapshot.claimed = true;
        PendingConversationTurnClaimResult::Claimed(PendingConversationTurnReply {
            id: turn.snapshot.id.clone(),
            conversation_ref: conversation_ref.to_string(),
            message_id: message_id.to_string(),
            reply_to_id: reply_to_id.to_string(),
            thread_id: thread_id.map(ToOwned::to_owned),
            text: text.to_string(),
            timestamp_ms,
        })
    }

    pub fn complete(&self, agent_id: &str, id: &str) -> bool {
        let key = turn_key(agent_id.trim(), id.trim());
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let claimed = inner.turns.get(&key).is_some_and(|turn| turn.snapshot.claimed);
        claimed && remove_turn_locked(&mut inner, &key)
    }

    pub fn release(&self, agent_id: &str, id: &str) -> bool {
        let key = turn_key(agent_id.trim(), id.trim());
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(turn) = inner.turns.get_mut(&key) else { return false };
        if !turn.snapshot.claimed { return false; }
        turn.snapshot.claimed = false;
        true
    }

    pub fn cancel(&self, agent_id: &str, id: &str) -> bool {
        let key = turn_key(agent_id.trim(), id.trim());
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        remove_turn_locked(&mut inner, &key)
    }

    fn prune_expired_locked(&self, inner: &mut PendingConversationTurnRegistryInner) {
        let expired = inner.turns.iter().filter_map(|(key, turn)| (turn.expires_at <= Instant::now()).then_some(key.clone())).collect::<Vec<_>>();
        for key in expired { remove_turn_locked(inner, &key); }
    }
}

fn turn_key(agent_id: &str, id: &str) -> String { format!("{agent_id}\u{0}{id}") }

fn outbound_key(agent_id: &str, message_id: &str) -> String { turn_key(agent_id, message_id) }

fn remove_outbound_index(index: &mut HashMap<String, Vec<String>>, turn_key: &str, old_key: Option<String>) {
    if let Some(key) = old_key {
        if let Some(keys) = index.get_mut(&key) {
            keys.retain(|candidate| candidate != turn_key);
            if keys.is_empty() { index.remove(&key); }
        }
    }
}

fn remove_turn_locked(inner: &mut PendingConversationTurnRegistryInner, key: &str) -> bool {
    let Some(turn) = inner.turns.remove(key) else { return false };
    remove_outbound_index(
        &mut inner.by_outbound_id,
        key,
        turn.snapshot
            .outbound_message_id
            .as_deref()
            .map(|value| outbound_key(&turn.snapshot.agent_id, value)),
    );
    true
}
