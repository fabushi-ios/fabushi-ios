use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use crate::agent_store_conflict_events::{
    ConflictJournalEvent, atomic_write_private, ensure_safe_rel_path, read_conflict_events,
    read_optional_text, recover_pending, remove_optional, rewrite_conflict_events, sync_dir,
};

pub const MAX_ANNOUNCED_EVENT_IDS: usize = 65_536;
pub const ANNOUNCED_EVICTION_BATCH: usize = MAX_ANNOUNCED_EVENT_IDS / 8;

const CLAIM_STATE_FILE_NAME: &str = "conflict-notice-claims.json";
const CLAIM_STATE_VERSION: u8 = 1;
const DEFAULT_CONVERSATION_KEY: &str = "__default__";
const MAX_CONVERSATION_ID_BYTES: usize = 512;
const MAX_WRITTEN_PATHS: usize = 4_096;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConflictNoticeClaimState {
    #[serde(default = "claim_state_version")]
    v: u8,
    #[serde(default)]
    announced_order: Vec<String>,
    #[serde(default)]
    claims: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    deferred_eager_written_paths: BTreeMap<String, Vec<String>>,
}

fn claim_state_version() -> u8 {
    CLAIM_STATE_VERSION
}

fn conversation_key(conversation_id: Option<&str>) -> Result<String, String> {
    let Some(raw) = conversation_id else {
        return Ok(DEFAULT_CONVERSATION_KEY.to_string());
    };
    let value = raw.trim();
    if value.is_empty()
        || value.len() > MAX_CONVERSATION_ID_BYTES
        || value.as_bytes().iter().any(|byte| *byte < 32 || *byte == 127)
    {
        return Err("invalid Agent Store conflict notice conversationId".into());
    }
    Ok(value.to_string())
}

fn validate_event_ids(event_ids: &[String]) -> Result<(), String> {
    if event_ids.len() > MAX_ANNOUNCED_EVENT_IDS {
        return Err("too many Agent Store conflict notice event ids".into());
    }
    for event_id in event_ids {
        if event_id.is_empty()
            || event_id.len() > 128
            || event_id.as_bytes().iter().any(|byte| *byte < 32 || *byte == 127)
        {
            return Err("invalid Agent Store conflict notice event id".into());
        }
    }
    Ok(())
}

fn claim_state_path(account_root: &Path) -> Result<std::path::PathBuf, String> {
    Ok(sync_dir(account_root)?.join(CLAIM_STATE_FILE_NAME))
}

fn load_state(account_root: &Path) -> Result<ConflictNoticeClaimState, String> {
    let path = claim_state_path(account_root)?;
    let Some(body) = read_optional_text(&path)? else {
        return Ok(ConflictNoticeClaimState {
            v: CLAIM_STATE_VERSION,
            ..Default::default()
        });
    };
    let state: ConflictNoticeClaimState =
        serde_json::from_str(&body).map_err(|error| format!("invalid conflict notice claim state: {error}"))?;
    if state.v != CLAIM_STATE_VERSION {
        return Err(format!("unsupported conflict notice claim state version {}", state.v));
    }
    Ok(state)
}

fn save_state(account_root: &Path, state: &ConflictNoticeClaimState) -> Result<(), String> {
    let path = claim_state_path(account_root)?;
    if state.announced_order.is_empty()
        && state.claims.values().all(Vec::is_empty)
        && state.deferred_eager_written_paths.values().all(Vec::is_empty)
    {
        return remove_optional(&path);
    }
    let body = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    atomic_write_private(&path, &body)
}

fn prune_missing_events(state: &mut ConflictNoticeClaimState, live_ids: &HashSet<String>) {
    state.announced_order.retain(|event_id| live_ids.contains(event_id));
    state.claims.retain(|_, event_ids| {
        event_ids.retain(|event_id| live_ids.contains(event_id));
        !event_ids.is_empty()
    });
}

fn enforce_announced_bound(state: &mut ConflictNoticeClaimState) {
    if state.announced_order.len() <= MAX_ANNOUNCED_EVENT_IDS {
        return;
    }
    let overflow = state
        .announced_order
        .len()
        .saturating_sub(MAX_ANNOUNCED_EVENT_IDS);
    let evict = overflow.max(ANNOUNCED_EVICTION_BATCH).min(state.announced_order.len());
    let evicted = state
        .announced_order
        .drain(..evict)
        .collect::<HashSet<_>>();
    state.claims.retain(|_, event_ids| {
        event_ids.retain(|event_id| !evicted.contains(event_id));
        !event_ids.is_empty()
    });
}

fn event_value(event: &ConflictJournalEvent) -> Result<Value, String> {
    serde_json::to_value(event).map_err(|error| error.to_string())
}

pub fn peek(account_root: &Path, conversation_id: Option<&str>) -> Result<Value, String> {
    recover_pending(account_root)?;
    let conversation = conversation_key(conversation_id)?;
    let events = read_conflict_events(account_root)?;
    let live_ids = events
        .iter()
        .map(|event| event.event_id.clone())
        .collect::<HashSet<_>>();
    let mut state = load_state(account_root)?;
    prune_missing_events(&mut state, &live_ids);

    let mut announced = state
        .announced_order
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let claimed = state.claims.entry(conversation.clone()).or_default();
    for event in &events {
        if announced.insert(event.event_id.clone()) {
            state.announced_order.push(event.event_id.clone());
            claimed.push(event.event_id.clone());
        }
    }
    enforce_announced_bound(&mut state);

    let current_ids = state
        .claims
        .get(&conversation)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect::<HashSet<_>>();
    let selected = events
        .iter()
        .filter(|event| current_ids.contains(&event.event_id))
        .map(event_value)
        .collect::<Result<Vec<_>, _>>()?;
    let deferred = state
        .deferred_eager_written_paths
        .get(&conversation)
        .cloned()
        .unwrap_or_default();
    save_state(account_root, &state)?;
    Ok(json!({
        "kind": if selected.is_empty() { "none" } else { "conflicts" },
        "events": selected,
        "deferredEagerWrittenPaths": deferred,
    }))
}

pub fn ack(
    account_root: &Path,
    conversation_id: Option<&str>,
    event_ids: &[String],
) -> Result<Value, String> {
    validate_event_ids(event_ids)?;
    if event_ids.is_empty() {
        return Ok(json!({"kind":"acked","count":0}));
    }
    recover_pending(account_root)?;
    let conversation = conversation_key(conversation_id)?;
    let targets = event_ids.iter().cloned().collect::<HashSet<_>>();
    let events = read_conflict_events(account_root)?;
    let live_ids = events
        .iter()
        .map(|event| event.event_id.clone())
        .collect::<HashSet<_>>();
    let removed = targets
        .iter()
        .filter(|event_id| live_ids.contains(*event_id))
        .count();
    let kept = events
        .into_iter()
        .filter(|event| !targets.contains(&event.event_id))
        .collect::<Vec<_>>();
    rewrite_conflict_events(account_root, &kept)?;

    let mut state = load_state(account_root)?;
    state.announced_order.retain(|event_id| !targets.contains(event_id));
    for ids in state.claims.values_mut() {
        ids.retain(|event_id| !targets.contains(event_id));
    }
    state.claims.retain(|_, ids| !ids.is_empty());
    if let Some(paths) = state.deferred_eager_written_paths.get_mut(&conversation) {
        if paths.is_empty() {
            state.deferred_eager_written_paths.remove(&conversation);
        }
    }
    save_state(account_root, &state)?;
    Ok(json!({"kind":"acked","count":removed}))
}

pub fn release(
    account_root: &Path,
    conversation_id: Option<&str>,
    event_ids: &[String],
) -> Result<Value, String> {
    validate_event_ids(event_ids)?;
    let conversation = conversation_key(conversation_id)?;
    let targets = event_ids.iter().cloned().collect::<HashSet<_>>();
    let mut state = load_state(account_root)?;
    let mut released = 0usize;
    if let Some(ids) = state.claims.get_mut(&conversation) {
        let before = ids.len();
        ids.retain(|event_id| !targets.contains(event_id));
        released = before.saturating_sub(ids.len());
    }
    state.claims.retain(|_, ids| !ids.is_empty());

    let still_claimed = state
        .claims
        .values()
        .flatten()
        .cloned()
        .collect::<HashSet<_>>();
    state.announced_order.retain(|event_id| {
        !targets.contains(event_id) || still_claimed.contains(event_id)
    });
    save_state(account_root, &state)?;
    Ok(json!({"kind":"released","count":released}))
}

pub fn note_deferred_eager_written_paths(
    account_root: &Path,
    conversation_id: Option<&str>,
    written_paths: &[String],
) -> Result<Value, String> {
    if written_paths.len() > MAX_WRITTEN_PATHS {
        return Err("too many deferred eager Agent Store paths".into());
    }
    let conversation = conversation_key(conversation_id)?;
    let mut unique = BTreeSet::new();
    for path in written_paths {
        ensure_safe_rel_path(path)?;
        unique.insert(path.clone());
    }

    let mut state = load_state(account_root)?;
    if unique.is_empty() {
        state.deferred_eager_written_paths.remove(&conversation);
    } else {
        state
            .deferred_eager_written_paths
            .insert(conversation, unique.into_iter().collect());
    }
    save_state(account_root, &state)?;
    Ok(json!({
        "kind":"noted",
        "count": written_paths.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_store_conflict_events::record_merge_conflicts;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-agent-store-notice-{label}-{}",
            Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn peek_claims_ack_removes_and_release_reannounces() {
        let account = root("lifecycle");
        record_merge_conflicts(
            &account,
            "agent-1",
            &["memory/state.json".to_string(), "notes/today.md".to_string()],
        )
        .unwrap();

        let first = peek(&account, Some("conversation-a")).unwrap();
        let events = first["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        let released_id = events[0]["event_id"].as_str().unwrap().to_string();
        let acked_id = events[1]["event_id"].as_str().unwrap().to_string();

        release(&account, Some("conversation-a"), std::slice::from_ref(&released_id)).unwrap();
        ack(&account, Some("conversation-a"), std::slice::from_ref(&acked_id)).unwrap();

        let second = peek(&account, Some("conversation-b")).unwrap();
        let ids = second["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["event_id"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![released_id.as_str()]);
        assert_eq!(read_conflict_events(&account).unwrap().len(), 1);

        let _ = fs::remove_dir_all(account);
    }

    #[test]
    fn deferred_eager_paths_are_account_and_conversation_scoped() {
        let account_a = root("deferred-a");
        let account_b = root("deferred-b");
        note_deferred_eager_written_paths(
            &account_a,
            Some("conversation-a"),
            &["memory/state.json".to_string()],
        )
        .unwrap();

        assert_eq!(
            peek(&account_a, Some("conversation-a")).unwrap()["deferredEagerWrittenPaths"],
            json!(["memory/state.json"])
        );
        assert_eq!(
            peek(&account_a, Some("conversation-b")).unwrap()["deferredEagerWrittenPaths"],
            json!([])
        );
        assert_eq!(
            peek(&account_b, Some("conversation-a")).unwrap()["deferredEagerWrittenPaths"],
            json!([])
        );

        let _ = fs::remove_dir_all(account_a);
        let _ = fs::remove_dir_all(account_b);
    }
}
