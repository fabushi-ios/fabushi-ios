use std::collections::HashMap;
use std::path::Path;
use rusqlite::params;
use rusqlite::OptionalExtension;

use crate::storage::store_db::{bump_db_write_generation, live_db_handle_count};
use crate::transcript_mutation_events::{publish_transcript_mutation, TranscriptMutation};

use super::agent_db::{
    decode_hex, encode_hex, newest_divider_anchor_timestamp_ms, open_projection_db, read_kv,
    parse_transcript_entry, session_admission_identity_is_well_formed, session_work_write_is_admitted,
    AgentDbProjectionError, CompactionCommit, SandAgentDb, SandAgentDbOptions, SessionProviderReview,
    SessionWorkAdmissionRecord, KV_AWAITING, KV_EPISODE, KV_INTRODUCTION, KV_MEMORY_SNAPSHOT,
    KV_ORIGIN, KV_PARTNERS, KV_PROFILE_SNAPSHOT, KV_PURPOSE, KV_SESSION_WORK_ADMISSION,
    KV_SPEND_GUARD, KV_UNREAD,
};
use super::agent_db_recovery::{open_configured_db, DbRecoveryOptions};
use super::agent_db_schema::*;
use super::agent_db_serde::{
    parse_awaiting_state, parse_unread_state, AwaitingUserResponse, EpisodeTurn,
    MemoryPromptSnapshot, UnreadState,
};
use super::agent_db_transcript_pages::{
    read_transcript_tail, TranscriptPage, TranscriptWindowQuery,
};
use super::ocrm_agent_db_persisted_metadata::write_unread_state;

pub fn read_persisted_newest_divider_anchor_timestamp_ms(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<f64, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    Ok(newest_divider_anchor_timestamp_ms(&db)?)
}

pub fn mark_persisted_unread(
    db_path: &Path,
    busy_timeout_ms: u64,
    at: f64,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let current = parse_unread_state(read_kv(&db, KV_UNREAD)?.as_deref());
    let newest = newest_divider_anchor_timestamp_ms(&db)?;
    drop(db);
    let last_activity_at = if current.last_activity_at == 0.0 {
        at
    } else {
        current.last_activity_at
    };
    let newest_bound = if newest > 0.0 {
        newest - 1.0
    } else {
        f64::INFINITY
    };
    let last_viewed_at = current
        .last_viewed_at
        .min(last_activity_at - 1.0)
        .min(at - 1.0)
        .min(newest_bound);
    write_unread_state(
        db_path,
        busy_timeout_ms,
        &UnreadState {
            last_activity_at,
            last_viewed_at,
            is_manually_unread: true,
            unread_count: current.unread_count.max(1.0),
        },
    )
}

pub fn mark_persisted_read(
    db_path: &Path,
    busy_timeout_ms: u64,
    at: f64,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let current = parse_unread_state(read_kv(&db, KV_UNREAD)?.as_deref());
    drop(db);
    write_unread_state(
        db_path,
        busy_timeout_ms,
        &UnreadState {
            last_viewed_at: current.last_activity_at.max(at),
            is_manually_unread: false,
            unread_count: 0.0,
            ..current
        },
    )
}

pub fn set_persisted_awaiting_user_response(
    db_path: &Path,
    busy_timeout_ms: u64,
    state: Option<&AwaitingUserResponse>,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.set_awaiting_user_response(state)
}

pub fn set_persisted_awaiting_user_response_for_tab(
    db_path: &Path,
    busy_timeout_ms: u64,
    tab_id: &str,
    state: Option<&AwaitingUserResponse>,
    if_since_before: Option<f64>,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.set_awaiting_user_response_for_tab(tab_id, state, if_since_before)
}

pub fn record_persisted_request_id(
    db_path: &Path,
    busy_timeout_ms: u64,
    id: &str,
    at: f64,
    prompt: Option<&str>,
    source: Option<&str>,
) -> Result<bool, AgentDbProjectionError> {
    let id = id.trim();
    if id.is_empty() {
        return Ok(false);
    }
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.record_request_id(id, at, prompt, source)
}

pub fn record_persisted_episode_turn(
    db_path: &Path,
    busy_timeout_ms: u64,
    turn: &EpisodeTurn,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.record_episode_turn(turn)
}

pub fn clear_persisted_pending_episode_turns(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    delete_persisted_kv(db_path, busy_timeout_ms, KV_EPISODE)
}

pub fn set_persisted_memory_prompt_snapshot(
    db_path: &Path,
    busy_timeout_ms: u64,
    snapshot: &serde_json::Value,
) -> Result<bool, AgentDbProjectionError> {
    write_persisted_kv(db_path, busy_timeout_ms, KV_MEMORY_SNAPSHOT, &snapshot.to_string())
}

pub fn clear_persisted_memory_prompt_snapshot(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    delete_persisted_kv(db_path, busy_timeout_ms, KV_MEMORY_SNAPSHOT)
}

pub fn set_persisted_agent_profile_prompt_snapshot(
    db_path: &Path,
    busy_timeout_ms: u64,
    snapshot: &serde_json::Value,
) -> Result<bool, AgentDbProjectionError> {
    write_persisted_kv(db_path, busy_timeout_ms, KV_PROFILE_SNAPSHOT, &snapshot.to_string())
}

pub fn clear_persisted_agent_profile_prompt_snapshot(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    delete_persisted_kv(db_path, busy_timeout_ms, KV_PROFILE_SNAPSHOT)
}

pub fn set_persisted_agent_origin(
    db_path: &Path,
    busy_timeout_ms: u64,
    origin: &str,
) -> Result<bool, AgentDbProjectionError> {
    write_persisted_kv(
        db_path,
        busy_timeout_ms,
        KV_ORIGIN,
        if origin == "dev" { "dev" } else { "user" },
    )
}

pub fn set_persisted_introduction_pending(
    db_path: &Path,
    busy_timeout_ms: u64,
    pending: bool,
) -> Result<bool, AgentDbProjectionError> {
    if pending {
        write_persisted_kv(db_path, busy_timeout_ms, KV_INTRODUCTION, "1")
    } else {
        delete_persisted_kv(db_path, busy_timeout_ms, KV_INTRODUCTION)
    }
}

pub fn set_persisted_agent_purpose(
    db_path: &Path,
    busy_timeout_ms: u64,
    purpose: Option<&str>,
) -> Result<bool, AgentDbProjectionError> {
    if let Some(purpose) = purpose {
        write_persisted_kv(db_path, busy_timeout_ms, KV_PURPOSE, purpose)
    } else {
        delete_persisted_kv(db_path, busy_timeout_ms, KV_PURPOSE)
    }
}

pub fn read_persisted_transcript_entry(
    db_path: &Path,
    busy_timeout_ms: u64,
    entry_id: &str,
) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let raw = db
        .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    Ok(raw.as_deref().and_then(parse_transcript_entry))
}

pub fn update_persisted_transcript_entry(
    db_path: &Path,
    busy_timeout_ms: u64,
    entry_id: &str,
    next: &serde_json::Value,
) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.update_transcript_entry(entry_id, next)
}

pub fn update_persisted_transcript_entry_if_session_admitted(
    db_path: &Path,
    busy_timeout_ms: u64,
    session_key: &str,
    entry_id: &str,
    next: &serde_json::Value,
) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
    let session_key = session_key.trim();
    if session_key.is_empty() || entry_id.trim().is_empty() {
        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
            "session-scoped persisted transcript update requires session key and entry identity"
                .into(),
        ));
    }
    let mut db = open_projection_db(db_path, busy_timeout_ms)?;
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if !session_work_write_is_admitted(&transaction)? {
        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
            "session owner identity is not admitted for persisted transcript update".into(),
        ));
    }
    let raw_admission = transaction
        .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .ok_or_else(|| {
            AgentDbProjectionError::WorkAdmissionInvalid(
                "session work admission record is unavailable".into(),
            )
        })?;
    let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)?;
    if !session_admission_identity_is_well_formed(&admission)
        || admission.session_key != session_key
        || admission.archived_at_ms.is_some()
        || admission.deletion_pending
    {
        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
            "session is not admitted for persisted transcript update".into(),
        ));
    }
    let raw_entry = transaction
        .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    let Some(raw_entry) = raw_entry else {
        transaction.commit()?;
        return Ok(None);
    };
    let entry = parse_transcript_entry(&raw_entry).ok_or_else(|| {
        AgentDbProjectionError::WorkAdmissionInvalid(
            "target transcript entry is not valid JSON".into(),
        )
    })?;
    if entry
        .get("sessionKey")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some(session_key)
    {
        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
            "target transcript entry provenance does not match admitted session".into(),
        ));
    }
    if transaction.execute(
        UPDATE_TRANSCRIPT_ENTRY_SQL,
        params![next.to_string(), entry_id],
    )? != 1
    {
        transaction.commit()?;
        return Ok(None);
    }
    transaction.commit()?;
    bump_db_write_generation(db_path);
    publish_entries_upserted(db_path, vec![next.clone()]);
    Ok(Some(next.clone()))
}

pub fn clear_persisted_conversation(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.clear_conversation()
}

fn write_persisted_kv(
    db_path: &Path,
    busy_timeout_ms: u64,
    key: &str,
    value: &str,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.write_kv(key, value)
}

fn delete_persisted_kv(
    db_path: &Path,
    busy_timeout_ms: u64,
    key: &str,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.delete_kv(key)
}


pub(crate) fn read_transcript_entries_from_db(
    db: &rusqlite::Connection,
) -> Result<Vec<serde_json::Value>, rusqlite::Error> {
    let mut statement = db.prepare(LIST_TRANSCRIPT_ENTRIES_SQL)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut entries = Vec::new();
    for row in rows {
        if let Some(entry) = parse_transcript_entry(&row?) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub(crate) fn compaction_payload_matches(
    existing: &serde_json::Value,
    commit: &CompactionCommit,
) -> bool {
    existing.get("kind").and_then(serde_json::Value::as_str) == Some("compaction")
        && existing.get("id").and_then(serde_json::Value::as_str) == Some(commit.entry_id.trim())
        && existing
            .get("firstKeptEntryId")
            .and_then(serde_json::Value::as_str)
            == commit.first_kept_entry_id.as_deref().map(str::trim)
        && existing.get("summary").and_then(serde_json::Value::as_str)
            == Some(commit.summary.trim())
        && existing
            .get("tokensBefore")
            .and_then(serde_json::Value::as_u64)
            == Some(commit.tokens_before)
        && existing.get("details") == commit.details.as_ref()
        && existing.get("sourceTurnId").and_then(serde_json::Value::as_str)
            == Some(commit.source_turn_id.trim())
}

pub(crate) fn read_transcript_entry_from_db(
    db: &rusqlite::Connection,
    entry_id: &str,
) -> Result<Option<serde_json::Value>, rusqlite::Error> {
    let raw = db
        .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
            row.get::<_, String>(0)
        })
        .optional()?;
    Ok(raw.as_deref().and_then(parse_transcript_entry))
}

pub(crate) fn read_branched_entries_from_db(
    db: &rusqlite::Connection,
) -> Result<Vec<serde_json::Value>, rusqlite::Error> {
    let mut statement = db.prepare(LIST_BRANCHED_ENTRIES_SQL)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut entries = Vec::new();
    for row in rows {
        if let Some(entry) = parse_transcript_entry(&row?) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

pub(crate) fn branch_reply_counts_from_entries(
    branched: &[serde_json::Value],
) -> HashMap<String, usize> {
    let by_id = branched
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("id")?.as_str()?.to_string(),
                entry,
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut counts = HashMap::new();
    for entry in branched {
        if let Some(root) = resolve_branch_root_from_entries(entry, &by_id) {
            *counts.entry(root).or_insert(0) += 1;
        }
    }
    counts
}

pub(crate) fn thread_descendants_from_entries(
    root_id: &str,
    branched: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let by_id = branched
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("id")?.as_str()?.to_string(),
                entry,
            ))
        })
        .collect::<HashMap<_, _>>();
    branched
        .iter()
        .filter(|entry| {
            resolve_branch_root_from_entries(entry, &by_id).as_deref() == Some(root_id)
        })
        .cloned()
        .collect()
}

pub(crate) fn resolve_branch_root_from_entries(
    entry: &serde_json::Value,
    branched_by_id: &HashMap<String, &serde_json::Value>,
) -> Option<String> {
    let mut current = entry;
    let mut seen = std::collections::HashSet::new();
    if let Some(id) = current.get("id").and_then(serde_json::Value::as_str) {
        seen.insert(id.to_string());
    }
    loop {
        let parent_id = current.get("replyTo").and_then(serde_json::Value::as_str)?;
        let Some(parent) = branched_by_id.get(parent_id) else {
            return Some(parent_id.to_string());
        };
        if !seen.insert(parent_id.to_string()) {
            return None;
        }
        current = parent;
    }
}

pub(crate) fn main_transcript_entries_from_entries(
    entries: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let by_id = entries
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("id")?.as_str()?.to_string(),
                entry,
            ))
        })
        .collect::<HashMap<_, _>>();
    let is_thread_message = |entry: &serde_json::Value| {
        is_main_branched_entry(entry)
            && resolve_main_branch_root(entry, &by_id).is_some()
    };
    if !entries.iter().any(is_thread_message) {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|entry| !is_thread_message(entry))
        .cloned()
        .collect()
}

pub(crate) fn is_main_branched_entry(entry: &serde_json::Value) -> bool {
    matches!(
        entry.get("kind").and_then(serde_json::Value::as_str),
        Some("message" | "send-message" | "user-attachment" | "notice")
    ) && entry.get("branched").and_then(serde_json::Value::as_bool) == Some(true)
}

pub(crate) fn main_entry_reply_to(entry: &serde_json::Value) -> Option<&str> {
    if !matches!(
        entry.get("kind").and_then(serde_json::Value::as_str),
        Some("message" | "send-message" | "user-attachment" | "notice")
    ) {
        return None;
    }
    entry.get("replyTo").and_then(serde_json::Value::as_str)
}

pub(crate) fn resolve_main_branch_root(
    entry: &serde_json::Value,
    by_id: &HashMap<String, &serde_json::Value>,
) -> Option<String> {
    let mut current = entry;
    let mut seen = std::collections::HashSet::new();
    if let Some(id) = current.get("id").and_then(serde_json::Value::as_str) {
        seen.insert(id.to_string());
    }
    loop {
        let parent_id = main_entry_reply_to(current)?;
        let parent = by_id.get(parent_id)?;
        if !is_main_branched_entry(parent) {
            return Some(parent_id.to_string());
        }
        if !seen.insert(parent_id.to_string()) {
            return None;
        }
        current = parent;
    }
}

pub(crate) fn publish_entries_upserted(db_path: &Path, entries: Vec<serde_json::Value>) {
    let mut mutation = serde_json::Map::new();
    mutation.insert(
        "kind".into(),
        serde_json::Value::String("entries-upserted".into()),
    );
    mutation.insert(
        "agentId".into(),
        serde_json::Value::String(agent_id_for_db_path(db_path)),
    );
    mutation.insert("entries".into(), serde_json::Value::Array(entries));
    publish_transcript_mutation(&mutation);
}

fn agent_id_for_db_path(db_path: &Path) -> String {
    db_path
        .parent()
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn json_number(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

pub fn read_persisted_transcript_tail(
    db_path: &Path,
    busy_timeout_ms: u64,
    limit: i64,
) -> Result<TranscriptPage, AgentDbProjectionError> {
    let agent_dir_name = db_path
        .parent()
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let options = DbRecoveryOptions {
        busy_timeout_ms,
        ..DbRecoveryOptions::default()
    };
    let db = open_configured_db(
        db_path,
        &agent_dir_name,
        &options,
        live_db_handle_count(db_path) > 0,
    )?;
    Ok(read_transcript_tail(
        &db,
        TranscriptWindowQuery {
            before_seq: None,
            limit,
        },
    )?)
}
