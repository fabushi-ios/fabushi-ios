use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};

pub const MAX_HANDLED_REASONS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SnapshotReasonType { AgentRequestStart, AgentRequestEnd }
impl SnapshotReasonType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentRequestStart => "AGENT_REQUEST_START",
            Self::AgentRequestEnd => "AGENT_REQUEST_END",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapshotReason {
    pub reason_type: SnapshotReasonType,
    pub request_id: String,
}

struct State {
    handled: HashSet<String>,
    order: VecDeque<String>,
}

pub struct CodebaseSnapshotTrigger {
    state: Mutex<State>,
}

impl Default for CodebaseSnapshotTrigger {
    fn default() -> Self {
        Self { state: Mutex::new(State { handled: HashSet::new(), order: VecDeque::new() }) }
    }
}

impl CodebaseSnapshotTrigger {
    pub fn claim(&self, reason: &SnapshotReason) -> bool {
        let key = format!("{}\u{1f}{}", reason.reason_type.as_str(), reason.request_id);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.handled.contains(&key) { return false; }
        state.handled.insert(key.clone());
        state.order.push_back(key);
        while state.handled.len() > MAX_HANDLED_REASONS {
            if let Some(oldest) = state.order.pop_front() { state.handled.remove(&oldest); }
        }
        true
    }

    pub fn release(&self, reason: &SnapshotReason) {
        let key = format!("{}\u{1f}{}", reason.reason_type.as_str(), reason.request_id);
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.handled.remove(&key);
        state.order.retain(|entry| entry != &key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicate_snapshot_reasons_are_claimed_once_and_failed_claim_can_retry() {
        let trigger = CodebaseSnapshotTrigger::default();
        let reason = SnapshotReason { reason_type: SnapshotReasonType::AgentRequestStart, request_id: "req".into() };
        assert!(trigger.claim(&reason));
        assert!(!trigger.claim(&reason));
        trigger.release(&reason);
        assert!(trigger.claim(&reason));
    }
}
