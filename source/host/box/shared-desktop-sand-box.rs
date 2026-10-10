use crate::box_windows::{
    SAND_BOX_FIRST_FORK_WINDOW_INDEX, SAND_BOX_PRIMARY_WINDOW_INDEX,
    is_primary_window_index,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const ASSIGNMENTS_LOAD_TIMEOUT_MS: u64 = 30_000;
pub const ASSIGNMENTS_LOAD_RETRY_INTERVAL_MS: u64 = 1_000;
pub const DEFAULT_SHARED_BOX_ID: &str = "shared";
pub const SHARED_DESKTOP_ASSIGNMENTS_BOX_PATH: &str =
    "/home/box/.sand-window-assignments.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAssignments {
    pub assignments: BTreeMap<String, u32>,
    pub tokens: BTreeMap<String, String>,
    pub is_corrupt: bool,
}

pub fn parse_assignments(
    bytes: &[u8],
    max_window_count: u32,
) -> ParsedAssignments {
    let empty = || ParsedAssignments {
        assignments: BTreeMap::new(),
        tokens: BTreeMap::new(),
        is_corrupt: false,
    };
    let Ok(root) = serde_json::from_slice::<Value>(bytes) else {
        return ParsedAssignments {
            is_corrupt: true,
            ..empty()
        };
    };
    let Some(root) = root.as_object() else {
        return empty();
    };
    let Some(raw_assignments) = root
        .get("assignments")
        .and_then(Value::as_object)
    else {
        return empty();
    };
    let raw_tokens = root.get("tokens").and_then(Value::as_object);

    let mut assignments = BTreeMap::new();
    let mut tokens = BTreeMap::new();
    let mut used_forks = BTreeSet::new();

    let mut agent_ids = raw_assignments.keys().cloned().collect::<Vec<_>>();
    agent_ids.sort();
    for agent_id in agent_ids {
        let Some(index) = raw_assignments
            .get(&agent_id)
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
        else {
            continue;
        };
        if index < SAND_BOX_PRIMARY_WINDOW_INDEX
            || index > max_window_count
        {
            continue;
        }
        if index >= SAND_BOX_FIRST_FORK_WINDOW_INDEX
            && !used_forks.insert(index)
        {
            continue;
        }
        assignments.insert(agent_id.clone(), index);
        if let Some(token) = raw_tokens
            .and_then(|tokens| tokens.get(&agent_id))
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
        {
            tokens.insert(agent_id, token.to_owned());
        }
    }

    ParsedAssignments {
        assignments,
        tokens,
        is_corrupt: false,
    }
}

pub fn resolve_shared_box_id(
    explicit: Option<&str>,
    env: &BTreeMap<String, String>,
) -> String {
    explicit
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            env.get("SAND_SHARED_BOX_ID")
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| DEFAULT_SHARED_BOX_ID.to_owned())
}

pub struct SharedDesktopSandBox {
    shared_box_id: String,
    max_window_count: u32,
    agent_windows: BTreeMap<String, u32>,
    agent_window_tokens: BTreeMap<String, String>,
    windows_tearing_down: BTreeSet<u32>,
}

impl SharedDesktopSandBox {
    pub fn new(
        shared_box_id: impl Into<String>,
        max_window_count: u32,
    ) -> Self {
        Self {
            shared_box_id: shared_box_id.into(),
            max_window_count: max_window_count.max(1),
            agent_windows: BTreeMap::new(),
            agent_window_tokens: BTreeMap::new(),
            windows_tearing_down: BTreeSet::new(),
        }
    }

    pub fn shared_box_id(&self) -> &str {
        &self.shared_box_id
    }

    pub fn adopt_persisted(&mut self, parsed: ParsedAssignments) {
        let mut used = self
            .agent_windows
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        for (agent_id, index) in parsed.assignments {
            if self.agent_windows.contains_key(&agent_id)
                || used.contains(&index)
            {
                continue;
            }
            self.agent_windows.insert(agent_id.clone(), index);
            used.insert(index);
            if let Some(token) = parsed.tokens.get(&agent_id) {
                self.agent_window_tokens
                    .entry(agent_id)
                    .or_insert_with(|| token.clone());
            }
        }
    }

    fn take_free_fork_index(&mut self, agent_id: &str) -> Option<u32> {
        let used = self
            .agent_windows
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        for index in
            SAND_BOX_FIRST_FORK_WINDOW_INDEX..=self.max_window_count
        {
            if !used.contains(&index)
                && !self.windows_tearing_down.contains(&index)
            {
                self.agent_windows.insert(agent_id.to_owned(), index);
                return Some(index);
            }
        }
        None
    }

    pub fn assign_window(
        &mut self,
        agent_id: &str,
        mint_owner_token: impl FnOnce() -> String,
    ) -> Option<u32> {
        if let Some(index) = self.agent_windows.get(agent_id).copied() {
            return Some(index);
        }
        let index = self.take_free_fork_index(agent_id)?;
        self.agent_window_tokens
            .entry(agent_id.to_owned())
            .or_insert_with(mint_owner_token);
        Some(index)
    }

    pub fn migrate_legacy_primary_seat(
        &mut self,
        agent_id: &str,
        supports_fork_windows: bool,
    ) -> u32 {
        let assigned = self
            .agent_windows
            .get(agent_id)
            .copied()
            .unwrap_or(SAND_BOX_PRIMARY_WINDOW_INDEX);
        if !supports_fork_windows || !is_primary_window_index(assigned) {
            return assigned;
        }
        self.take_free_fork_index(agent_id).unwrap_or(assigned)
    }

    pub fn begin_release(&mut self, agent_id: &str) -> Option<u32> {
        let index = self.agent_windows.remove(agent_id);
        self.agent_window_tokens.remove(agent_id);
        if let Some(index) = index.filter(|index| {
            *index >= SAND_BOX_FIRST_FORK_WINDOW_INDEX
        }) {
            self.windows_tearing_down.insert(index);
        }
        index
    }

    pub fn finish_release(&mut self, index: u32) {
        self.windows_tearing_down.remove(&index);
    }

    pub fn agent_window_index(&self, agent_id: &str) -> Option<u32> {
        self.agent_windows.get(agent_id).copied()
    }

    pub fn owner_token(&self, agent_id: &str) -> Option<&str> {
        self.agent_window_tokens
            .get(agent_id)
            .map(String::as_str)
    }

    pub fn persistence_bytes(&self) -> Vec<u8> {
        let mut assignments = Map::new();
        for (agent, index) in &self.agent_windows {
            assignments.insert(agent.clone(), Value::from(*index));
        }
        let mut tokens = Map::new();
        for (agent, token) in &self.agent_window_tokens {
            tokens.insert(agent.clone(), Value::String(token.clone()));
        }
        serde_json::to_vec(&Value::Object(Map::from_iter([
            ("assignments".into(), Value::Object(assignments)),
            ("tokens".into(), Value::Object(tokens)),
        ])))
        .expect("shared desktop assignments are JSON serializable")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_rejects_corrupt_json_and_duplicate_forks() {
        assert!(parse_assignments(b"{", 4).is_corrupt);

        let parsed = parse_assignments(
            br#"{
                "assignments":{"b":2,"a":2,"primary":1,"bad":9},
                "tokens":{"a":"token-a","b":"token-b"}
            }"#,
            4,
        );
        assert_eq!(parsed.assignments.get("a"), Some(&2));
        assert!(!parsed.assignments.contains_key("b"));
        assert_eq!(parsed.assignments.get("primary"), Some(&1));
        assert_eq!(
            parsed.tokens.get("a").map(String::as_str),
            Some("token-a")
        );
    }

    #[test]
    fn assignment_uses_forks_and_blocks_reuse_until_teardown_finishes() {
        let mut shared = SharedDesktopSandBox::new("shared", 3);
        assert_eq!(
            shared.assign_window("a", || "token-a".into()),
            Some(2)
        );
        assert_eq!(
            shared.assign_window("b", || "token-b".into()),
            Some(3)
        );
        let released = shared.begin_release("a").unwrap();
        assert_eq!(released, 2);
        assert_eq!(
            shared.assign_window("c", || "token-c".into()),
            None
        );
        shared.finish_release(2);
        assert_eq!(
            shared.assign_window("c", || "token-c".into()),
            Some(2)
        );
    }

    #[test]
    fn persistence_round_trips_assignments_and_tokens() {
        let mut shared = SharedDesktopSandBox::new("shared", 5);
        shared.assign_window("agent", || "owner-token".into());
        let parsed = parse_assignments(&shared.persistence_bytes(), 5);
        assert_eq!(parsed.assignments.get("agent"), Some(&2));
        assert_eq!(
            parsed.tokens.get("agent").map(String::as_str),
            Some("owner-token")
        );
    }
}
