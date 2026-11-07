use std::path::Path;
use rusqlite::params;
use rusqlite::OptionalExtension;
use uuid::Uuid;

use crate::storage::agent_paths::get_sand_agents_root_dir;
use crate::storage::store_db::{bump_db_write_generation, live_db_handle_count};

use super::agent_db::{
    decode_hex, encode_hex, newest_divider_anchor_timestamp_ms, notify_agent_db_listeners,
    open_projection_db, read_metadata_json, read_kv, AgentDbListenerChannel,
    AgentDbProjectionError, AgentDbSerdeSnapshot, AgentMetadataProjection, SandAgentDb,
    SandAgentDbOptions, SessionWorkAdmissionRecord, KV_AWAITING, KV_EPISODE, KV_HIDDEN_REPAIR,
    KV_INTRODUCTION, KV_LEGACY_BLOB_RETIREMENT, KV_LATEST_REQUEST_ID, KV_MEMORY_SNAPSHOT,
    KV_ORIGIN, KV_PARTNERS, KV_PENDING_MEMORY_MUTATION, KV_PENDING_PROFILE_MUTATION,
    KV_PROFILE, KV_PROFILE_SNAPSHOT, KV_PURPOSE, KV_REQUEST_IDS, KV_SESSION_WORK_ADMISSION,
    KV_SPEND_GUARD, KV_SPEND_GUARD_LEGACY, KV_STALE_ROOT_CLEANUP, KV_UNREAD,
};
use super::agent_db_recovery::{open_configured_db, DbRecoveryOptions};
use super::agent_db_schema::*;
use super::agent_db_serde::*;

pub fn read_persisted_latest_root_blob_id(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<Vec<u8>, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let Some(metadata) = read_metadata_json(&db)? else {
        return Ok(Vec::new());
    };
    let Some(root_hex) = metadata
        .get("latestRootBlobId")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Vec::new());
    };
    decode_hex(root_hex).map_err(AgentDbProjectionError::LatestRootHex)
}

pub fn read_persisted_agent_metadata_projection(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<Option<AgentMetadataProjection>, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    if owner.read_kv("metadata")?.is_none() {
        return Ok(None);
    }
    let metadata = owner.read_metadata()?;
    let lifecycle_revision = owner.ensure_lifecycle_revision()?;
    let agent_id = metadata
        .get("agentId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let created_at = metadata
        .get("createdAt")
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or_default();
    let latest_root_blob_id = metadata
        .get("latestRootBlobId")
        .and_then(serde_json::Value::as_str)
        .map(decode_hex)
        .transpose()
        .map_err(AgentDbProjectionError::LatestRootHex)?
        .unwrap_or_default();
    Ok(Some(AgentMetadataProjection {
        agent_id,
        created_at,
        latest_root_blob_id,
        lifecycle_revision,
    }))
}

pub fn read_persisted_agent_origin(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<String, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    Ok(if read_kv(&db, KV_ORIGIN)?.as_deref() == Some("dev") {
        "dev".to_string()
    } else {
        "user".to_string()
    })
}

pub fn read_persisted_introduction_pending(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    Ok(read_kv(&db, KV_INTRODUCTION)?.as_deref() == Some("1"))
}

pub fn read_persisted_agent_purpose(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<Option<String>, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    Ok(read_kv(&db, KV_PURPOSE)?
        .filter(|value| valid_agent_purpose(value)))
}

pub fn read_persisted_conversation_partner_ids(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<Vec<String>, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let Some(raw) = read_kv(&db, KV_PARTNERS)? else {
        return Ok(Vec::new());
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(Vec::new());
    };
    Ok(value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default())
}

pub fn add_persisted_conversation_partner(
    db_path: &Path,
    busy_timeout_ms: u64,
    partner_id: &str,
) -> Result<bool, AgentDbProjectionError> {
    let id = partner_id.trim();
    if id.is_empty() {
        return Ok(false);
    }
    let mut db = open_projection_db(db_path, busy_timeout_ms)?;
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let own_id = read_metadata_json(&transaction)?
        .and_then(|metadata| metadata.get("agentId").and_then(serde_json::Value::as_str).map(ToOwned::to_owned))
        .unwrap_or_default();
    if id == own_id {
        transaction.rollback()?;
        return Ok(false);
    }
    let current = transaction
        .query_row(GET_KV_SQL, params![KV_PARTNERS], |row| row.get::<_, String>(0))
        .optional()?;
    let mut values = current
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(ToOwned::to_owned))
        .collect::<std::collections::BTreeSet<_>>();
    if !values.insert(id.to_string()) {
        transaction.rollback()?;
        return Ok(false);
    }
    transaction.execute(
        SET_KV_SQL,
        params![KV_PARTNERS, serde_json::to_string(&values.into_iter().collect::<Vec<_>>())?],
    )?;
    transaction.commit()?;
    bump_db_write_generation(db_path);
    Ok(true)
}

pub(crate) fn valid_agent_purpose(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-'
        })
}

pub fn read_persisted_agent_name(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<Option<String>, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    Ok(read_metadata_json(&db)?
        .and_then(|metadata| {
            metadata
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        }))
}

pub fn set_persisted_agent_id(
    db_path: &Path,
    busy_timeout_ms: u64,
    agent_id: &str,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let Some(raw) = read_kv(&db, "metadata")? else {
        return Ok(false);
    };
    let metadata_bytes = decode_hex(&raw).map_err(AgentDbProjectionError::MetadataHex)?;
    let mut metadata: serde_json::Value = serde_json::from_slice(&metadata_bytes)?;
    if metadata
        .get("agentId")
        .and_then(serde_json::Value::as_str)
        == Some(agent_id)
    {
        return Ok(true);
    }
    let Some(object) = metadata.as_object_mut() else {
        return Ok(false);
    };
    object.insert(
        "agentId".into(),
        serde_json::Value::String(agent_id.to_string()),
    );
    let next_raw = encode_hex(&serde_json::to_vec(&metadata)?);
    let changed = db.execute(SET_KV_SQL, params!["metadata", next_raw])? > 0;
    if changed {
        bump_db_write_generation(db_path);
        notify_agent_db_listeners(
            db_path,
            AgentDbListenerChannel::Metadata("agentId".into()),
        );
    }
    Ok(changed)
}

/// Re-seed the owner-local session admission when an agent database is cloned.
/// Cloning is a pre-admission setup operation: the copied source record belongs
/// to the source agent and must never be reused by the new agent identity.
pub fn reset_persisted_session_work_admission(
    db_path: &Path,
    busy_timeout_ms: u64,
    agent_id: &str,
) -> Result<bool, AgentDbProjectionError> {
    let agent_id = agent_id.trim();
    if agent_id.is_empty() {
        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
            "cloned session admission requires an agent identity".into(),
        ));
    }
    let mut db = open_projection_db(db_path, busy_timeout_ms)?;
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let record = SessionWorkAdmissionRecord {
        session_key: format!("agent:{agent_id}:main"),
        session_id: agent_id.to_string(),
        lifecycle_revision: Uuid::new_v4().to_string(),
        deletion_pending: false,
        archived_at_ms: None,
        archived_by: None,
        archive_reason: None,
        admitted_work_id: None,
        admitted_media_work_id: None,
        admitted_media_paths: Vec::new(),
        terminal_result_idempotency_key: None,
        provider_review_id: None,
        provider_review_phase: None,
        provider_review_continuation_run_id: None,
        provider_review: None,
    };
    let encoded = serde_json::to_string(&record)?;
    let changed = transaction.execute(
        SET_KV_SQL,
        params![KV_SESSION_WORK_ADMISSION, encoded],
    )? > 0;
    transaction.commit()?;
    if changed {
        bump_db_write_generation(db_path);
        notify_agent_db_listeners(
            db_path,
            AgentDbListenerChannel::Metadata(KV_SESSION_WORK_ADMISSION.into()),
        );
    }
    Ok(changed)
}

pub fn set_persisted_agent_name(
    db_path: &Path,
    busy_timeout_ms: u64,
    name: &str,
) -> Result<bool, AgentDbProjectionError> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(false);
    }
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    let metadata = owner.read_metadata()?;
    if metadata
        .get("name")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|current| current == name)
    {
        return Ok(false);
    }
    owner.set_metadata("name", serde_json::Value::String(name.to_string()))
}

pub fn reseed_minimal_persisted_agent_db_if_missing(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<(), AgentDbProjectionError> {
    if db_path.is_file() {
        return Ok(());
    }
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    drop(db);
    bump_db_write_generation(db_path);
    Ok(())
}

pub fn initialize_persisted_agent_record(
    db_path: &Path,
    busy_timeout_ms: u64,
    agent_id: &str,
    origin: &str,
    purpose: Option<&str>,
    created_at_ms: u64,
    blob_encryption_key_hex: &str,
) -> Result<(), AgentDbProjectionError> {
    let mut db = open_projection_db(db_path, busy_timeout_ms)?;
    let transaction = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing = transaction
        .query_row(GET_KV_SQL, params!["metadata"], |row| row.get::<_, String>(0))
        .optional()?;
    if existing.is_none() {
        let metadata = serde_json::json!({
            "agentId": agent_id,
            "latestRootBlobId": "",
            "name": "New Agent",
            "mode": "default",
            "isRunEverything": false,
            "createdAt": created_at_ms,
            "blobEncryptionKey": blob_encryption_key_hex,
        });
        transaction.execute(
            SET_KV_SQL,
            params!["metadata", encode_hex(&serde_json::to_vec(&metadata)?)],
        )?;
    }
    transaction.execute(
        SET_KV_SQL,
        params!["origin", if origin == "dev" { "dev" } else { "user" }],
    )?;
    transaction.execute(
        SET_KV_SQL,
        params!["introductionPending", "1"],
    )?;
    if let Some(purpose) = purpose.map(str::trim).filter(|value| !value.is_empty()) {
        transaction.execute(SET_KV_SQL, params!["purpose", purpose])?;
    }
    transaction.commit()?;
    bump_db_write_generation(db_path);
    Ok(())
}

pub fn compare_and_set_persisted_latest_root_blob_id(
    db_path: &Path,
    busy_timeout_ms: u64,
    expected_root: &[u8],
    next_root: &[u8],
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.compare_and_set_latest_root_blob_id(expected_root, next_root)
}

pub fn read_persisted_version(
    db_path: &Path,
    busy_timeout_ms: u64,
    key: &str,
) -> Result<u64, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    Ok(read_kv(&db, key)?
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_default())
}

pub fn write_persisted_version(
    db_path: &Path,
    busy_timeout_ms: u64,
    key: &str,
    version: u64,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.write_kv(key, &version.to_string())
}

pub fn stale_root_cleanup_version(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<u64, AgentDbProjectionError> {
    read_persisted_version(db_path, busy_timeout_ms, KV_STALE_ROOT_CLEANUP)
}

pub fn set_stale_root_cleanup_version(
    db_path: &Path,
    busy_timeout_ms: u64,
    version: u64,
) -> Result<bool, AgentDbProjectionError> {
    write_persisted_version(db_path, busy_timeout_ms, KV_STALE_ROOT_CLEANUP, version)
}

pub fn hidden_entry_repair_version(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<u64, AgentDbProjectionError> {
    read_persisted_version(db_path, busy_timeout_ms, KV_HIDDEN_REPAIR)
}

pub fn set_hidden_entry_repair_version(
    db_path: &Path,
    busy_timeout_ms: u64,
    version: u64,
) -> Result<bool, AgentDbProjectionError> {
    write_persisted_version(db_path, busy_timeout_ms, KV_HIDDEN_REPAIR, version)
}

pub fn legacy_blob_retirement_version(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<u64, AgentDbProjectionError> {
    read_persisted_version(db_path, busy_timeout_ms, KV_LEGACY_BLOB_RETIREMENT)
}

pub fn has_persisted_legacy_conversation_blobs(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    Ok(db
        .query_row(HAS_LEGACY_BLOB_SQL, [], |_| Ok(()))
        .optional()?
        .is_some())
}

pub fn retire_persisted_legacy_conversation_blobs(
    db_path: &Path,
    busy_timeout_ms: u64,
    version: u64,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.retire_legacy_conversation_blobs(version)
}

pub fn read_persisted_automation_spend_guard_state(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<SpendGuardState, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let state = read_kv(&db, KV_SPEND_GUARD)?;
    let legacy = read_kv(&db, KV_SPEND_GUARD_LEGACY)?;
    Ok(resolve_spend_guard_state(state.as_deref(), legacy.as_deref()))
}

pub fn set_persisted_automation_spend_guard_state(
    db_path: &Path,
    busy_timeout_ms: u64,
    state: &SpendGuardState,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.set_automation_spend_guard_state(state)
}

pub fn read_persisted_agent_profile_prompt_snapshot(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let Some(raw) = read_kv(&db, KV_PROFILE_SNAPSHOT)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<serde_json::Value>(&raw).ok())
}

pub fn clear_persisted_transient_state(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.clear_transient_state()
}

/// Remove source-agent cross-store recovery custody from a cloned database.
/// These journals belong to the source identity and must not be replayed or
/// rebound after clone admission is reseeded. The source database is untouched.
pub fn clear_persisted_clone_mutation_journals(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.clear_clone_mutation_journals()
}

pub fn read_persisted_agent_serde_snapshot(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<AgentDbSerdeSnapshot, AgentDbProjectionError> {
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

    let profile = read_kv(&db, KV_PROFILE)?;
    let unread = read_kv(&db, KV_UNREAD)?;
    let spend_guard = read_kv(&db, KV_SPEND_GUARD)?;
    let spend_guard_legacy = read_kv(&db, KV_SPEND_GUARD_LEGACY)?;
    let awaiting = read_kv(&db, KV_AWAITING)?;
    let request_ids_raw = read_kv(&db, KV_REQUEST_IDS)?;
    let legacy_request_id = read_kv(&db, KV_LATEST_REQUEST_ID)?;
    let episode = read_kv(&db, KV_EPISODE)?;
    let memory_snapshot = read_kv(&db, KV_MEMORY_SNAPSHOT)?;

    let mut request_ids = parse_request_records(request_ids_raw.as_deref());
    if request_ids.is_empty() {
        if let Some(legacy) = legacy_request_id.as_deref() {
            let id = legacy.trim();
            if !id.is_empty() {
                request_ids.push(RequestRecord {
                    id: id.to_string(),
                    at: 0.0,
                    prompt: None,
                    source: None,
                });
            }
        }
    }

    Ok(AgentDbSerdeSnapshot {
        profile: parse_profile(profile.as_deref()),
        unread_state: parse_unread_state(unread.as_deref()),
        spend_guard_state: resolve_spend_guard_state(
            spend_guard.as_deref(),
            spend_guard_legacy.as_deref(),
        ),
        awaiting_user_response: parse_awaiting_state(awaiting.as_deref()),
        request_ids,
        pending_episode_turns: parse_pending_episode_turns(episode.as_deref()),
        memory_prompt_snapshot: parse_memory_prompt_snapshot(memory_snapshot.as_deref()),
    })
}

pub fn set_persisted_sand_profile(
    db_path: &Path,
    busy_timeout_ms: u64,
    profile: &SandProfile,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.set_sand_profile(profile)
}

pub(crate) fn write_unread_state(
    db_path: &Path,
    busy_timeout_ms: u64,
    state: &UnreadState,
) -> Result<bool, AgentDbProjectionError> {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = busy_timeout_ms;
    let owner = SandAgentDb::open_with_options_and_seed(db_path, options, false)?;
    owner.set_unread_state(state)
}

pub fn mark_persisted_activity(
    db_path: &Path,
    busy_timeout_ms: u64,
    at: f64,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let current = parse_unread_state(read_kv(&db, KV_UNREAD)?.as_deref());
    drop(db);
    if current.last_activity_at >= at {
        return Ok(false);
    }
    write_unread_state(
        db_path,
        busy_timeout_ms,
        &UnreadState {
            last_activity_at: at,
            unread_count: current.unread_count + 1.0,
            ..current
        },
    )
}

pub fn mark_persisted_viewed(
    db_path: &Path,
    busy_timeout_ms: u64,
    at: f64,
    preserve_manual_unread: bool,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let current = parse_unread_state(read_kv(&db, KV_UNREAD)?.as_deref());
    drop(db);
    if preserve_manual_unread && current.is_manually_unread {
        return Ok(false);
    }
    if !current.is_manually_unread && current.last_viewed_at >= at {
        return Ok(false);
    }
    write_unread_state(
        db_path,
        busy_timeout_ms,
        &UnreadState {
            last_viewed_at: at,
            is_manually_unread: false,
            unread_count: 0.0,
            ..current
        },
    )
}
