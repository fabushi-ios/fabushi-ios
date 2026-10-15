use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

pub const REMOTE_TURN_BUDGET_MAX: usize = 30;
pub const REMOTE_TURN_BUDGET_WINDOW_MS: u64 = 10 * 60_000;
pub const REMOTE_MEMBER_UNREACHABLE_MS: u64 = 10 * 60_000;
pub const REMOTE_AGENT_ID_PREFIX: &str = "sand-remote:";

#[derive(Default)]
pub struct RemoteTurnBudget {
    accepted_at_ms: Mutex<BTreeMap<String, VecDeque<u64>>>,
}

impl RemoteTurnBudget {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accept(&self, key: &str, now_ms: u64) -> bool {
        let key = key.trim();
        if key.is_empty() {
            return false;
        }
        let mut budgets = self
            .accepted_at_ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let accepted = budgets.entry(key.to_string()).or_default();
        while accepted.front().is_some_and(|timestamp| {
            now_ms.saturating_sub(*timestamp) >= REMOTE_TURN_BUDGET_WINDOW_MS
        }) {
            accepted.pop_front();
        }
        if accepted.len() >= REMOTE_TURN_BUDGET_MAX {
            return false;
        }
        accepted.push_back(now_ms);
        true
    }

    pub fn clear(&self) {
        self.accepted_at_ms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

fn decode_component(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return None;
        }
        let encoded = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
        output.push(u8::from_str_radix(encoded, 16).ok()?);
        index += 3;
    }
    String::from_utf8(output).ok()
}

pub fn parse_remote_agent_id(value: &str) -> Option<(String, String)> {
    let body = value.strip_prefix(REMOTE_AGENT_ID_PREFIX)?;
    let (owner_auth_id, agent_id) = body.split_once('/')?;
    let owner_auth_id = decode_component(owner_auth_id)?;
    let agent_id = decode_component(agent_id)?;
    if owner_auth_id.trim().is_empty() || agent_id.trim().is_empty() {
        return None;
    }
    Some((owner_auth_id, agent_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_keyed_allows_thirty_turns_then_recovers_after_window() {
        let budget = RemoteTurnBudget::new();
        for index in 0..REMOTE_TURN_BUDGET_MAX {
            assert!(budget.accept("room-a\0host-a\0agent-a", index as u64));
        }
        assert!(!budget.accept("room-a\0host-a\0agent-a", 100));
        assert!(budget.accept("room-b\0host-a\0agent-a", 100));
        assert!(budget.accept(
            "room-a\0host-a\0agent-a",
            REMOTE_TURN_BUDGET_WINDOW_MS + 1
        ));
    }

    #[test]
    fn parses_canonical_remote_agent_id() {
        assert_eq!(
            parse_remote_agent_id("sand-remote:owner%3A1/agent%2Ftwo"),
            Some(("owner:1".into(), "agent/two".into()))
        );
        assert!(parse_remote_agent_id("sand-remote:/agent").is_none());
    }
}
