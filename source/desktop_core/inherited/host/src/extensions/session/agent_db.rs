use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::sqlite_busy::{is_sqlite_busy_error, is_sqlite_corrupt_error};
use crate::storage::store_db::{
    bump_db_write_generation, live_db_handle_count, register_live_db_handle,
    release_live_db_handle,
};
use crate::extensions::memory::memory_service::MemoryRecoveryRecord;
use crate::transcript_mutation_events::{
    TRANSCRIPT_MUTATION_ORIGIN_KEY, publish_transcript_mutation, transcript_mutation_origin,
};

use super::agent_db_recovery::{
    AgentDbRecoveryError, DbRecoveryOptions, open_configured_db,
    probe_human_conversation_store, recover_corrupt_store_db,
};
use super::agent_db_schema::{
    CLEAR_BLOBS_SQL, CLEAR_TRANSCRIPT_ENTRIES_SQL, COMPARE_AND_SET_KV_SQL, DELETE_KV_SQL,
    DELETE_TRANSCRIPT_ENTRY_SQL, GET_KV_SQL, GET_TRANSCRIPT_ENTRY_SQL,
    INSERT_TRANSCRIPT_ENTRY_SQL, LIST_BRANCHED_ENTRIES_SQL, LIST_TRANSCRIPT_ENTRIES_SQL,
    SET_KV_SQL, UPDATE_TRANSCRIPT_ENTRY_SQL,
};
use super::agent_db_serde::{
    AwaitingUserResponse, EpisodeTurn, MemoryPromptSnapshot, RequestRecord,
    SandProfile, SpendGuardState, UnreadState, parse_unread_state,
    serialize_spend_guard_state,
};
pub(crate) fn parse_transcript_entry(raw: &str) -> Option<serde_json::Value> {
    let entry = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    if entry.get("kind").and_then(serde_json::Value::as_str) == Some("error") {
        return None;
    }
    match entry.get("kind").and_then(serde_json::Value::as_str) {
        Some("compaction") => {
            if entry.get("summary").and_then(serde_json::Value::as_str).is_some()
                && entry.get("tokensBefore").and_then(serde_json::Value::as_u64).is_some()
            {
                Some(entry)
            } else {
                None
            }
        }
        Some("send-message") => {
            if entry.get("message").is_some() || entry.get("content").is_some() {
                Some(entry)
            } else {
                super::agent_db_serde::parse_transcript_entry(raw)
            }
        }
        Some("spend-guard-card") | Some("card") => Some(entry),
        Some("background-result-receipt") | Some("session-work-result") | Some("receipt") => Some(entry),
        Some("outgoing-media-settlement") | Some("media-settlement") => Some(entry),
        _ => super::agent_db_serde::parse_transcript_entry(raw),
    }
}
pub use super::agent_db_transcript_pages::{
    HumanTranscriptPage, HumanTranscriptPageQuery,
    TranscriptPage, TranscriptPageQuery, TranscriptWindow, TranscriptWindowQuery,
    read_human_transcript_entries, read_human_transcript_entries_for_topic, read_human_transcript_tail,
    read_transcript_page, read_transcript_tail, read_transcript_window,
};
use fabushi_messaging_core::{
    ActorId, AdminRights, CommunityMember, ConversationId, ForumTopicState, MemberRestrictions,
    MemberStatus,
};

fn member_status_to_str(status: MemberStatus) -> &'static str {
    match status {
        MemberStatus::Owner => "owner",
        MemberStatus::Administrator => "administrator",
        MemberStatus::Member => "member",
        MemberStatus::Restricted => "restricted",
        MemberStatus::Left => "left",
        MemberStatus::Banned => "banned",
    }
}

fn member_status_from_str(s: &str) -> Result<MemberStatus, rusqlite::Error> {
    match s {
        "owner" | "Owner" => Ok(MemberStatus::Owner),
        "administrator" | "Administrator" => Ok(MemberStatus::Administrator),
        "member" | "Member" => Ok(MemberStatus::Member),
        "restricted" | "Restricted" => Ok(MemberStatus::Restricted),
        "left" | "Left" => Ok(MemberStatus::Left),
        "banned" | "Banned" => Ok(MemberStatus::Banned),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid MemberStatus: {other}"),
            )),
        )),
    }
}

#[cfg(test)]
#[path = "human_creation_storage_tests.rs"]
mod human_creation_storage_tests;

#[derive(Debug, thiserror::Error)]
pub enum AgentDbProjectionError {
    #[error("agent session database open/recovery error: {0}")]
    Recovery(#[from] AgentDbRecoveryError),
    #[error("agent session sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("agent metadata hex is invalid: {0}")]
    MetadataHex(String),
    #[error("agent metadata json is invalid utf-8: {0}")]
    MetadataUtf8(#[from] std::string::FromUtf8Error),
    #[error("agent metadata json is invalid: {0}")]
    MetadataJson(#[from] serde_json::Error),
    #[error("agent lifecycle revision unavailable: {0}")]
    LifecycleRevisionUnavailable(String),
    #[error("agent compaction entry is invalid: {0}")]
    CompactionInvalid(String),
    #[error("agent compaction entry conflicts with an existing identity: {0}")]
    CompactionConflict(String),
    #[error("agent session work-admission record is invalid: {0}")]
    WorkAdmissionInvalid(String),
    #[error("latestRootBlobId hex is invalid: {0}")]
    LatestRootHex(String),
    #[error("agent db owner mutex poisoned")]
    OwnerPoisoned,
    #[error("Human transcript page could not be durably prepared")]
    HumanTranscriptUnavailable,
}

pub(crate) const KV_PROFILE: &str = "sandProfile";
pub(crate) const KV_UNREAD: &str = "unreadState";
pub(crate) const KV_AWAITING: &str = "awaitingUserResponse";
pub(crate) const KV_REQUEST_IDS: &str = "requestIds";
pub(crate) const KV_LATEST_REQUEST_ID: &str = "latestRequestId";
pub(crate) const KV_EPISODE: &str = "episodePending";
pub(crate) const KV_MEMORY_SNAPSHOT: &str = "memoryPromptSnapshot";
pub(crate) const KV_PENDING_MEMORY_MUTATION: &str = "pendingExternalMemoryMutation";
pub(crate) const KV_PENDING_PROFILE_MUTATION: &str = "pendingExternalProfileMutation";
pub(crate) const KV_SPEND_GUARD: &str = "automationSpendGuardState";
pub(crate) const KV_SPEND_GUARD_LEGACY: &str = "automationSpendGuardNudgedAt";
pub(crate) const KV_HIDDEN_REPAIR: &str = "hiddenEntryRepairVersion";
pub(crate) const KV_STALE_ROOT_CLEANUP: &str = "staleRootCleanupVersion";
pub(crate) const KV_LEGACY_BLOB_RETIREMENT: &str = "legacyStoreBlobRetirementVersion";
pub(crate) const KV_PROFILE_SNAPSHOT: &str = "agentProfilePromptSnapshot";
pub(crate) const KV_ORIGIN: &str = "origin";
pub(crate) const KV_INTRODUCTION: &str = "introductionPending";
pub(crate) const KV_PURPOSE: &str = "purpose";
pub(crate) const KV_PARTNERS: &str = "conversationPartners";
pub(crate) const KV_SESSION_WORK_ADMISSION: &str = "sessionWorkAdmission";
pub(crate) const MAX_COMPACTION_SUMMARY_CHARS: usize = 16_000;

#[derive(Debug, Clone, PartialEq)]
pub struct AgentMetadataProjection {
    pub agent_id: String,
    pub created_at: f64,
    pub latest_root_blob_id: Vec<u8>,
    pub lifecycle_revision: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentDbSerdeSnapshot {
    pub profile: SandProfile,
    pub unread_state: UnreadState,
    pub spend_guard_state: SpendGuardState,
    pub awaiting_user_response: Option<AwaitingUserResponse>,
    pub request_ids: Vec<RequestRecord>,
    pub pending_episode_turns: Vec<EpisodeTurn>,
    pub memory_prompt_snapshot: Option<MemoryPromptSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionCommit {
    pub entry_id: String,
    pub expected_transcript_tail_id: String,
    pub first_kept_entry_id: Option<String>,
    pub summary: String,
    pub tokens_before: u64,
    pub details: Option<serde_json::Value>,
    pub source_turn_id: String,
    pub expected_lifecycle_revision: String,
}

/// Durable generation and lifecycle facts used by the session work-admission
/// owner. This is intentionally agent-DB local until the full session catalog
/// migration exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionWorkAdmissionRecord {
    pub session_key: String,
    pub session_id: String,
    pub lifecycle_revision: String,
    #[serde(default)]
    pub deletion_pending: bool,
    #[serde(default)]
    pub archived_at_ms: Option<f64>,
    #[serde(default)]
    pub archived_by: Option<String>,
    #[serde(default)]
    pub archive_reason: Option<String>,
    #[serde(default)]
    pub admitted_work_id: Option<String>,
    /// Durable fence for materialized outgoing media that has not yet reached
    /// the transcript owner. This is separate from background work because a
    /// foreground turn may own both identities at once.
    #[serde(default)]
    pub admitted_media_work_id: Option<String>,
    /// Absolute paths materialized for the fenced media work. This is durable
    /// recovery input; it is not itself permission to publish or delete media.
    #[serde(default)]
    pub admitted_media_paths: Vec<String>,
    #[serde(default)]
    pub terminal_result_idempotency_key: Option<String>,
    #[serde(default)]
    pub provider_review_id: Option<String>,
    #[serde(default)]
    pub provider_review_phase: Option<String>,
    #[serde(default)]
    pub provider_review_continuation_run_id: Option<String>,
    #[serde(default)]
    pub provider_review: Option<SessionProviderReview>,
}

/// Durable identity/state projection for one scheduled or event-triggered
/// automation run. Cleanup and transcript archival are deliberately not
/// implemented by this slice; the row is the owner-local handoff required by
/// the later cron reaper.
#[derive(Debug, Clone, PartialEq)]
pub struct CronSessionRunRecord {
    pub session_key: String,
    pub automation_id: String,
    pub run_id: String,
    pub session_id: String,
    pub lifecycle_revision: String,
    pub updated_at_ms: f64,
    pub base_session: bool,
    pub state: String,
    pub terminal_outcome: Option<String>,
    pub terminal_detail: Option<String>,
    pub active_controller: bool,
    pub pending_outgoing_media: bool,
    pub unsettled_subagent: bool,
    pub fence_state_known: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingMemoryMutation {
    pub mutation_id: String,
    pub operation: String,
    pub session_key: String,
    pub session_id: String,
    pub lifecycle_revision: String,
    pub records: Vec<MemoryRecoveryRecord>,
    pub prior_snapshot: Option<MemoryPromptSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileFileSnapshot {
    pub filename: String,
    pub contents: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingProfileMutation {
    pub mutation_id: String,
    pub operation: String,
    pub session_key: String,
    pub session_id: String,
    pub lifecycle_revision: String,
    pub before_files: Vec<ProfileFileSnapshot>,
    pub after_files: Vec<ProfileFileSnapshot>,
    pub previous_profile: SandProfile,
    pub next_profile: Option<SandProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderReviewContinuationOutcome {
    Completed,
    Cancelled,
    Failed,
    OutcomeUnknown,
}

/// Provider-authored refusal facts retained by the session admission owner.
/// This record contains no continuation capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionProviderReview {
    pub id: String,
    pub session_id: String,
    pub run_id: String,
    pub provider: String,
    pub model: String,
    pub runtime_id: String,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub native_thread_id: Option<String>,
    #[serde(default)]
    pub native_turn_id: Option<String>,
    pub explanation: String,
    #[serde(default)]
    pub continuation_message: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionWorkResultReceipt {
    pub session_key: String,
    pub lifecycle_revision: String,
    pub work_id: String,
    pub idempotency_key: String,
    pub content: String,
    pub sent_message_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionWorkResultCommitOutcome {
    Committed(SessionWorkResultReceipt),
    AlreadyCommitted(SessionWorkResultReceipt),
    RejectedStale,
    ConflictingResult,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CompactionCommitOutcome {
    Committed(serde_json::Value),
    AlreadyCommitted(serde_json::Value),
    RejectedStale,
}

pub type AgentDbListener = Arc<dyn Fn() + Send + Sync + 'static>;
pub type AgentDbBusyCallback = Arc<dyn Fn(&str, &str) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct SandAgentDbOptions {
    pub recovery: DbRecoveryOptions,
    pub on_busy_error: Option<AgentDbBusyCallback>,
}

impl Default for SandAgentDbOptions {
    fn default() -> Self {
        Self {
            recovery: DbRecoveryOptions::default(),
            on_busy_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum AgentDbListenerChannel {
    Metadata(String),
    Profile,
    Awaiting,
}

type AgentDbListenerKey = (PathBuf, AgentDbListenerChannel);
type AgentDbListenerMap = HashMap<AgentDbListenerKey, BTreeMap<u64, AgentDbListener>>;

fn agent_db_listener_registry() -> &'static Mutex<AgentDbListenerMap> {
    static REGISTRY: OnceLock<Mutex<AgentDbListenerMap>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_agent_db_listener_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn resolve_agent_db_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

pub(crate) fn notify_agent_db_listeners(
    db_path: &Path,
    channel: AgentDbListenerChannel,
) {
    let key = (resolve_agent_db_path(db_path), channel);
    let listeners = agent_db_listener_registry()
        .lock()
        .ok()
        .and_then(|registry| {
            registry
                .get(&key)
                .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
        })
        .unwrap_or_default();
    for listener in listeners {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener()));
    }
}

pub struct AgentDbSubscription {
    key: AgentDbListenerKey,
    id: u64,
    active: bool,
}

impl AgentDbSubscription {
    pub fn unsubscribe(mut self) {
        self.remove();
    }

    fn remove(&mut self) {
        if !self.active {
            return;
        }
        if let Ok(mut registry) = agent_db_listener_registry().lock() {
            if let Some(listeners) = registry.get_mut(&self.key) {
                listeners.remove(&self.id);
                if listeners.is_empty() {
                    registry.remove(&self.key);
                }
            }
        }
        self.active = false;
    }
}

impl Drop for AgentDbSubscription {
    fn drop(&mut self) {
        self.remove();
    }
}

pub(crate) fn subscribe_agent_db_listener(
    db_path: &Path,
    channel: AgentDbListenerChannel,
    listener: AgentDbListener,
) -> AgentDbSubscription {
    let key = (resolve_agent_db_path(db_path), channel);
    let id = next_agent_db_listener_id();
    agent_db_listener_registry()
        .lock()
        .expect("agent db listener registry poisoned")
        .entry(key.clone())
        .or_default()
        .insert(id, listener);
    AgentDbSubscription {
        key,
        id,
        active: true,
    }
}

/// Long-lived Rust owner for the frozen Grok SandAgentDb lifecycle boundary.
///
/// Production keeps one owner per live agent. The held SQLite connection
/// registers the live-handle fence used by corruption recovery and maintenance,
/// while existing projection helpers remain the single persistence implementation.
pub struct SandAgentDb {
    pub(crate) db_path: PathBuf,
    pub(crate) agent_dir_name: String,
    pub(crate) options: SandAgentDbOptions,
    pub(crate) connection: Mutex<Option<rusqlite::Connection>>,
    pub(crate) recovered: AtomicBool,
    pub(crate) handle_registered: AtomicBool,
    pub(crate) closed: AtomicBool,
}

impl SandAgentDb {
    pub fn open(
        db_path: impl AsRef<Path>,
        busy_timeout_ms: u64,
    ) -> Result<Self, AgentDbProjectionError> {
        let mut options = SandAgentDbOptions::default();
        options.recovery.busy_timeout_ms = busy_timeout_ms;
        Self::open_with_options(db_path, options)
    }

    pub fn open_conversation_store(
        db_path: impl AsRef<Path>,
        busy_timeout_ms: u64,
    ) -> Result<Self, AgentDbProjectionError> {
        let mut options = SandAgentDbOptions::default();
        options.recovery.busy_timeout_ms = busy_timeout_ms;
        Self::open_with_options_and_seed(db_path, options, false)
    }

    /// Human creation must reject corruption without quarantining or resetting
    /// the candidate. The live connection remains the identity authority.
    pub fn open_human_conversation_store(
        db_path: impl AsRef<Path>,
        busy_timeout_ms: u64,
    ) -> Result<Self, AgentDbProjectionError> {
        let db_path = resolve_agent_db_path(db_path.as_ref());
        probe_human_conversation_store(&db_path, busy_timeout_ms)?;
        let mut options = SandAgentDbOptions::default();
        options.recovery.busy_timeout_ms = busy_timeout_ms;
        options.recovery.recover_on_corruption = false;
        options.recovery.reject_if_corrupt = true;
        Self::open_with_options_and_seed(db_path, options, false)
    }

    pub fn open_with_options(
        db_path: impl AsRef<Path>,
        options: SandAgentDbOptions,
    ) -> Result<Self, AgentDbProjectionError> {
        Self::open_with_options_and_seed(db_path, options, true)
    }

    pub(crate) fn open_with_options_and_seed(
        db_path: impl AsRef<Path>,
        options: SandAgentDbOptions,
        seed_default_agent_metadata: bool,
    ) -> Result<Self, AgentDbProjectionError> {
        let db_path = resolve_agent_db_path(db_path.as_ref());
        let agent_dir_name = db_path
            .parent()
            .and_then(Path::file_name)
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let connection = open_configured_db(
            &db_path,
            &agent_dir_name,
            &options.recovery,
            live_db_handle_count(&db_path) > 0,
        )?;
        register_live_db_handle(&db_path);
        let owner = Self {
            db_path,
            agent_dir_name,
            options,
            connection: Mutex::new(Some(connection)),
            recovered: AtomicBool::new(false),
            handle_registered: AtomicBool::new(true),
            closed: AtomicBool::new(false),
        };
        if seed_default_agent_metadata {
            if let Err(error) = owner.seed_default_metadata_if_missing() {
                owner.close(false);
                return Err(error);
            }
        }
        Ok(owner)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn busy_timeout_ms(&self) -> u64 {
        self.options.recovery.busy_timeout_ms
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn read_kv(&self, key: &str) -> Result<Option<String>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(None);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(None);
        };
        Ok(read_kv(db, key)?)
    }

    pub fn write_kv(
        &self,
        key: &str,
        value: &str,
    ) -> Result<bool, AgentDbProjectionError> {
        self.run_write(&format!("writeKv:{key}"), |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(SET_KV_SQL, params![key, value])? > 0)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// Write one session-scoped projection only when the caller still owns
    /// the exact admitted session generation. The admission check and KV
    /// write share one SQLite transaction so an older Gateway request cannot
    /// publish policy into a newer lifecycle generation.
    pub fn write_session_kv_if_generation(
        &self,
        key: &str,
        value: &str,
        expected_session_id: &str,
        expected_lifecycle_revision: &str,
    ) -> Result<bool, AgentDbProjectionError> {
        let expected_session_id = expected_session_id.trim();
        let expected_lifecycle_revision = expected_lifecycle_revision.trim();
        if key.trim().is_empty()
            || expected_session_id.is_empty()
            || expected_lifecycle_revision.is_empty()
        {
            return Ok(false);
        }
        let changed = self.run_write_value("writeSessionKvIfGeneration", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<(bool, bool), rusqlite::Error> {
                let Some(raw_admission) = read_kv(db, KV_SESSION_WORK_ADMISSION)? else {
                    return Ok((false, false));
                };
                let admission = serde_json::from_str::<SessionWorkAdmissionRecord>(&raw_admission)
                    .map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?;
                if admission.session_id != expected_session_id
                    || admission.lifecycle_revision != expected_lifecycle_revision
                    || !session_work_write_is_admitted(db)?
                {
                    return Ok((false, false));
                }
                let changed = db.execute(SET_KV_SQL, params![key, value])? > 0;
                Ok((changed, changed))
            })();
            match result {
                Ok(result) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(result)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata(key.to_string()),
            );
        }
        Ok(changed)
    }

    pub fn delete_kv(&self, key: &str) -> Result<bool, AgentDbProjectionError> {
        self.run_write(&format!("deleteKv:{key}"), |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(DELETE_KV_SQL, params![key])? > 0)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn read_session_work_admission(
        &self,
    ) -> Result<Option<SessionWorkAdmissionRecord>, AgentDbProjectionError> {
        let Some(raw) = self.read_kv(KV_SESSION_WORK_ADMISSION)? else {
            return Ok(None);
        };
        serde_json::from_str(&raw)
            .map(Some)
            .map_err(AgentDbProjectionError::MetadataJson)
    }

    /// Re-seed the owner-local admission record after a cloned database has
    /// received its new directory identity. This is intentionally the one
    /// pre-admission reset that may replace the copied source record.
    pub fn reset_session_work_admission_for_clone(
        &self,
        agent_id: &str,
    ) -> Result<bool, AgentDbProjectionError> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "cloned session admission requires an agent identity".into(),
            ));
        }
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
        let changed = self.run_write("resetSessionWorkAdmissionForClone", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = db.execute(
                SET_KV_SQL,
                params![KV_SESSION_WORK_ADMISSION, encoded],
            );
            match result {
                Ok(changes) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changes > 0)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata(KV_SESSION_WORK_ADMISSION.into()),
            );
        }
        Ok(changed)
    }

    /// Compare-and-set the agent-local work-admission record. A missing
    /// expected revision means create-only; a supplied revision means update
    /// only when the currently stored generation still matches.
    pub fn compare_and_set_session_work_admission(
        &self,
        expected_lifecycle_revision: Option<&str>,
        next: &SessionWorkAdmissionRecord,
    ) -> Result<bool, AgentDbProjectionError> {
        self.compare_and_set_session_work_admission_with_deletion_transition(
            expected_lifecycle_revision,
            next,
            Some(next.deletion_pending),
        )
    }

    /// Clear deletion custody only when the durable record still carries the
    /// deletion fence observed by the deletion owner. Ordinary admission CAS
    /// operations preserve the current fence and cannot clear it implicitly.
    pub fn clear_session_deletion_pending(
        &self,
        expected_lifecycle_revision: &str,
        next: &SessionWorkAdmissionRecord,
    ) -> Result<bool, AgentDbProjectionError> {
        self.transition_session_deletion_pending(
            Some(expected_lifecycle_revision),
            true,
            next,
        )
    }

    /// Transition deletion custody only when the durable record still carries
    /// the expected prior fence value. This is the sole owner operation that
    /// may change `deletion_pending` across a CAS.
    pub fn transition_session_deletion_pending(
        &self,
        expected_lifecycle_revision: Option<&str>,
        expected_current_deletion_pending: bool,
        next: &SessionWorkAdmissionRecord,
    ) -> Result<bool, AgentDbProjectionError> {
        if next.deletion_pending == expected_current_deletion_pending {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "deletion custody transition must change deletion_pending".into(),
            ));
        }
        self.compare_and_set_session_work_admission_with_deletion_transition(
            expected_lifecycle_revision,
            next,
            Some(expected_current_deletion_pending),
        )
    }

    fn compare_and_set_session_work_admission_with_deletion_transition(
        &self,
        expected_lifecycle_revision: Option<&str>,
        next: &SessionWorkAdmissionRecord,
        expected_current_deletion_pending: Option<bool>,
    ) -> Result<bool, AgentDbProjectionError> {
        if next.session_key.trim().is_empty()
            || next.session_id.trim().is_empty()
            || next.lifecycle_revision.trim().is_empty()
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session work admission requires session key, session id, and lifecycle revision"
                    .into(),
            ));
        }
        let encoded = serde_json::to_string(next)?;
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_mut() else {
            return Ok(false);
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<bool, AgentDbProjectionError> {
            let current = db
                .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let matches = match (expected_lifecycle_revision, current.as_deref()) {
                (None, None) => true,
                (Some(expected), Some(raw)) => {
                    serde_json::from_str::<SessionWorkAdmissionRecord>(raw)
                        .ok()
                        .is_some_and(|record| {
                            record.lifecycle_revision == expected
                                && record.session_key == next.session_key
                                && record.session_id == next.session_id
                                && expected_current_deletion_pending
                                    .map_or(true, |expected| {
                                        record.deletion_pending == expected
                                    })
                        })
                }
                _ => false,
            };
            if !matches {
                return Ok(false);
            }
            if let Some(raw_pending) = db
                .query_row(GET_KV_SQL, params![KV_PENDING_MEMORY_MUTATION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
            {
                let current_record = current.as_deref().and_then(|raw| {
                    serde_json::from_str::<SessionWorkAdmissionRecord>(raw).ok()
                });
                if current_record
                    .as_ref()
                    .is_some_and(|record| record.lifecycle_revision != next.lifecycle_revision)
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "pending memory mutation blocks lifecycle revision transition".into(),
                    ));
                }
                if serde_json::from_str::<PendingMemoryMutation>(&raw_pending)
                    .ok()
                    .is_none_or(|mutation| {
                        mutation.mutation_id.trim().is_empty()
                            || mutation.operation != "remove" && mutation.operation != "clear"
                            || mutation.session_key != next.session_key
                            || mutation.session_id != next.session_id
                            || mutation.lifecycle_revision != next.lifecycle_revision
                    })
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "pending memory mutation journal is malformed or rebound".into(),
                    ));
                }
            }
            if let Some(raw_pending) = db
                .query_row(GET_KV_SQL, params![KV_PENDING_PROFILE_MUTATION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
            {
                let current_record = current.as_deref().and_then(|raw| {
                    serde_json::from_str::<SessionWorkAdmissionRecord>(raw).ok()
                });
                if current_record
                    .as_ref()
                    .is_some_and(|record| record.lifecycle_revision != next.lifecycle_revision)
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "pending profile mutation blocks lifecycle revision transition".into(),
                    ));
                }
                if serde_json::from_str::<PendingProfileMutation>(&raw_pending)
                    .ok()
                    .is_none_or(|mutation| {
                        mutation.mutation_id.trim().is_empty()
                            || !matches!(
                                mutation.operation.as_str(),
                                "profile-update" | "avatar-replace" | "avatar-remove"
                            )
                            || mutation.session_key != next.session_key
                            || mutation.session_id != next.session_id
                            || mutation.lifecycle_revision != next.lifecycle_revision
                    })
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "pending profile mutation journal is malformed or rebound".into(),
                    ));
                }
            }
            let changed = match current {
                Some(previous) => db.execute(
                    COMPARE_AND_SET_KV_SQL,
                    params![encoded, KV_SESSION_WORK_ADMISSION, previous],
                )?,
                None => db.execute(SET_KV_SQL, params![KV_SESSION_WORK_ADMISSION, encoded])?,
            };
            Ok(changed == 1)
        })();
        let changed = match result {
            Ok(changed) => {
                match db.execute_batch("COMMIT") {
                    Ok(()) => changed,
                    Err(error) => {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error.into());
                    }
                }
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                return Err(error);
            }
        };
        drop(guard);
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata(KV_SESSION_WORK_ADMISSION.into()),
            );
        }
        Ok(changed)
    }

    pub fn read_session_provider_review(
        &self,
    ) -> Result<Option<SessionProviderReview>, AgentDbProjectionError> {
        Ok(self
            .read_session_work_admission()?
            .and_then(|record| record.provider_review))
    }

    /// Record one provider-authored refusal against the captured admission
    /// generation. Identical terminal notifications retain the first review
    /// identity; conflicting facts fail closed.
    pub fn record_session_provider_review(
        &self,
        expected_lifecycle_revision: &str,
        review: &SessionProviderReview,
    ) -> Result<SessionProviderReview, AgentDbProjectionError> {
        let expected_lifecycle_revision = expected_lifecycle_revision.trim();
        if expected_lifecycle_revision.is_empty()
            || review.id.trim().is_empty()
            || review.session_id.trim().is_empty()
            || review.run_id.trim().is_empty()
            || review.provider.trim().is_empty()
            || review.model.trim().is_empty()
            || review.runtime_id.trim().is_empty()
            || review.explanation.trim().is_empty()
            || review.explanation.len() > 64 * 1024
            || review
                .continuation_message
                .as_ref()
                .is_some_and(|message| message.trim().is_empty() || message.len() > 1024)
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "provider review is missing bounded identity or explanation".into(),
            ));
        }
        let Some(mut record) = self.read_session_work_admission()? else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session work admission record is unavailable".into(),
            ));
        };
        if record.lifecycle_revision != expected_lifecycle_revision
            || record.session_id != review.session_id
            || record.archived_at_ms.is_some()
            || record.deletion_pending
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "provider review admission generation changed".into(),
            ));
        }
        if let Some(previous) = record.provider_review.as_ref() {
            let mut previous_identity_free = previous.clone();
            previous_identity_free.id.clear();
            let mut next_identity_free = review.clone();
            next_identity_free.id.clear();
            if previous_identity_free == next_identity_free {
                return Ok(previous.clone());
            }
            if record.provider_review_phase.as_deref() != Some("accepted") {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "provider review conflicts with the existing terminal facts".into(),
                ));
            }
            // An accepted continuation may produce a new terminal refusal. It
            // replaces the consumed review under the same session generation,
            // and the terminal refusal releases that continuation's work
            // marker in the same owner CAS. Pending reviews remain strict
            // idempotent/conflict-safe CAS.
            if record.provider_review_continuation_run_id.as_deref()
                != Some(review.run_id.as_str())
                || record.admitted_work_id.as_deref() != Some(review.run_id.as_str())
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "provider review continuation work identity changed".into(),
                ));
            }
            record.admitted_work_id = None;
        }
        record.provider_review_id = Some(review.id.clone());
        record.provider_review_phase = Some("pending".into());
        record.provider_review_continuation_run_id = None;
        record.provider_review = Some(review.clone());
        if !self.compare_and_set_session_work_admission(
            Some(expected_lifecycle_revision),
            &record,
        )? {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "provider review lost its lifecycle race".into(),
            ));
        }
        Ok(review.clone())
    }

    /// Settle one admitted provider-review continuation against the exact
    /// lifecycle generation and continuation run that claimed it. A completed
    /// continuation clears the review; known non-success and unknown outcomes
    /// retain the facts in a non-actionable terminal phase.
    pub fn settle_provider_review_continuation(
        &self,
        expected_lifecycle_revision: &str,
        continuation_run_id: &str,
        outcome: ProviderReviewContinuationOutcome,
    ) -> Result<bool, AgentDbProjectionError> {
        let expected_lifecycle_revision = expected_lifecycle_revision.trim();
        let continuation_run_id = continuation_run_id.trim();
        if expected_lifecycle_revision.is_empty() || continuation_run_id.is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "provider review settlement requires lifecycle and continuation identity".into(),
            ));
        }
        let Some(mut record) = self.read_session_work_admission()? else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session work admission record is unavailable".into(),
            ));
        };
        if record.lifecycle_revision != expected_lifecycle_revision
            || record.provider_review_phase.as_deref() != Some("accepted")
            || record.provider_review_continuation_run_id.as_deref() != Some(continuation_run_id)
            || record.admitted_work_id.as_deref() != Some(continuation_run_id)
            || record.deletion_pending
        {
            return Ok(false);
        }
        if record.admitted_work_id.as_deref() == Some(continuation_run_id) {
            record.admitted_work_id = None;
        }
        match outcome {
            ProviderReviewContinuationOutcome::Completed => {
                record.provider_review_id = None;
                record.provider_review_phase = Some("completed".into());
                record.provider_review_continuation_run_id = None;
                record.provider_review = None;
            }
            ProviderReviewContinuationOutcome::Cancelled => {
                record.provider_review_phase = Some("cancelled".into());
                record.provider_review_continuation_run_id = None;
            }
            ProviderReviewContinuationOutcome::Failed => {
                record.provider_review_phase = Some("failed".into());
                record.provider_review_continuation_run_id = None;
            }
            ProviderReviewContinuationOutcome::OutcomeUnknown => {
                record.provider_review_phase = Some("outcome-unknown".into());
                record.provider_review_continuation_run_id = None;
            }
        }
        self.compare_and_set_session_work_admission(
            Some(expected_lifecycle_revision),
            &record,
        )
    }

    /// Roll back a claimed review when the routed Runner never accepted the
    /// dispatch. This is distinct from a Runner terminal failure, which has a
    /// durable in-flight owner and therefore remains non-actionable.
    pub fn release_provider_review_continuation(
        &self,
        expected_lifecycle_revision: &str,
        continuation_run_id: &str,
    ) -> Result<bool, AgentDbProjectionError> {
        let expected_lifecycle_revision = expected_lifecycle_revision.trim();
        let continuation_run_id = continuation_run_id.trim();
        if expected_lifecycle_revision.is_empty() || continuation_run_id.is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "provider review release requires lifecycle and continuation identity".into(),
            ));
        }
        let Some(mut record) = self.read_session_work_admission()? else {
            return Ok(false);
        };
        if record.lifecycle_revision != expected_lifecycle_revision
            || record.provider_review_phase.as_deref() != Some("accepted")
            || record.provider_review_continuation_run_id.as_deref() != Some(continuation_run_id)
            || record.admitted_work_id.as_deref() != Some(continuation_run_id)
            || record.deletion_pending
        {
            return Ok(false);
        }
        record.provider_review_phase = Some("pending".into());
        record.provider_review_continuation_run_id = None;
        self.compare_and_set_session_work_admission(
            Some(expected_lifecycle_revision),
            &record,
        )
    }

    /// Atomically transition archive state for one canonical session
    /// generation. Active admitted work is preserved and blocks archiving;
    /// callers must settle it before retrying.
    pub fn transition_session_work_archive(
        &self,
        expected_session_key: &str,
        expected_session_id: &str,
        expected_lifecycle_revision: Option<&str>,
        archived: bool,
        archived_by: Option<&str>,
        archive_reason: Option<&str>,
        archived_at_ms: f64,
    ) -> Result<SessionWorkAdmissionRecord, AgentDbProjectionError> {
        let expected_session_key = expected_session_key.trim();
        let expected_session_id = expected_session_id.trim();
        if expected_session_key.is_empty()
            || expected_session_id.is_empty()
            || !archived_at_ms.is_finite()
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session archive transition requires a valid session identity and timestamp".into(),
            ));
        }
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_mut() else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "agent database is closed".into(),
            ));
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<
            (SessionWorkAdmissionRecord, bool),
            AgentDbProjectionError,
        > {
            let raw = db
                .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .ok_or_else(|| AgentDbProjectionError::WorkAdmissionInvalid(
                    "session work admission record is unavailable".into(),
                ))?;
            let mut record: SessionWorkAdmissionRecord = serde_json::from_str(&raw)?;
            if !session_admission_identity_is_bound(db, &record)? {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session archive transition owner identity is malformed or rebound".into(),
                ));
            }
            if db
                .query_row(GET_KV_SQL, params![KV_PENDING_MEMORY_MUTATION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .is_some()
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "pending memory mutation blocks archive/restore transition".into(),
                ));
            }
            if db
                .query_row(GET_KV_SQL, params![KV_PENDING_PROFILE_MUTATION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .is_some()
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "pending profile mutation blocks archive/restore transition".into(),
                ));
            }
            if record.session_key != expected_session_key
                || record.session_id != expected_session_id
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session archive transition identity changed".into(),
                ));
            }
            if expected_lifecycle_revision.is_some_and(|expected| {
                record.lifecycle_revision != expected.trim()
            }) {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session archive transition lifecycle changed".into(),
                ));
            }
            if record.archived_at_ms.is_some() == archived {
                return Ok((record, false));
            }
            if archived && record.admitted_work_id.is_some() {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session has admitted work; archive requires settlement before retry".into(),
                ));
            }
            if archived && record.admitted_media_work_id.is_some() {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session has pending outgoing media; archive requires settlement before retry"
                        .into(),
                ));
            }
            if archived {
                record.archived_at_ms = Some(archived_at_ms);
                record.archived_by = archived_by
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned);
                record.archive_reason = archive_reason
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .or_else(|| Some("manual".into()));
            } else {
                record.archived_at_ms = None;
                record.archived_by = None;
                record.archive_reason = None;
            }
            record.lifecycle_revision = Uuid::new_v4().to_string();
            let encoded = serde_json::to_string(&record)?;
            if db.execute(
                COMPARE_AND_SET_KV_SQL,
                params![encoded, KV_SESSION_WORK_ADMISSION, raw],
            )? != 1
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session archive transition lost its lifecycle race".into(),
                ));
            }
            Ok((record, true))
        })();
        match result {
            Ok((record, changed)) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                drop(guard);
                if changed {
                    notify_agent_db_listeners(
                        &self.db_path,
                        AgentDbListenerChannel::Metadata(KV_SESSION_WORK_ADMISSION.into()),
                    );
                }
                Ok(record)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// Commit one background terminal receipt and the admission transition in
    /// the same SQLite transaction. The receipt is a hidden transcript entry
    /// so retries can recover the original payload rather than re-stage it.
    pub fn commit_session_work_result(
        &self,
        session_key: &str,
        expected_lifecycle_revision: &str,
        work_id: &str,
        idempotency_key: &str,
        content: &str,
        sent_message_count: usize,
    ) -> Result<SessionWorkResultCommitOutcome, AgentDbProjectionError> {
        let session_key = session_key.trim();
        let expected_lifecycle_revision = expected_lifecycle_revision.trim();
        let work_id = work_id.trim();
        let idempotency_key = idempotency_key.trim();
        if session_key.is_empty()
            || expected_lifecycle_revision.is_empty()
            || work_id.is_empty()
            || idempotency_key.is_empty()
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "background result requires session identity, generation, work, and idempotency"
                    .into(),
            ));
        }

        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "agent database is closed".into(),
            ));
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<SessionWorkResultCommitOutcome, AgentDbProjectionError> {
            if !session_work_write_is_admitted(db)? {
                return Ok(SessionWorkResultCommitOutcome::RejectedStale);
            }
            let raw_admission = db
                .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let Some(raw_admission) = raw_admission else {
                return Ok(SessionWorkResultCommitOutcome::RejectedStale);
            };
            let admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)?;
            if !session_admission_identity_is_well_formed(&admission)
                || admission.session_key != session_key
                || admission.lifecycle_revision != expected_lifecycle_revision
                || admission.archived_at_ms.is_some()
                || admission.deletion_pending
            {
                return Ok(SessionWorkResultCommitOutcome::RejectedStale);
            }
            let receipt_id = format!("background-result:{idempotency_key}");
            if let Some(raw) = db
                .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![receipt_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
            {
                let Some(entry) = parse_transcript_entry(&raw) else {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "background result receipt contains malformed JSON".into(),
                    ));
                };
                let receipt = parse_session_work_result_receipt(&entry).ok_or_else(|| {
                    AgentDbProjectionError::WorkAdmissionInvalid(
                        "background result receipt is missing required identity".into(),
                    )
                })?;
                if receipt.session_key == session_key
                    && receipt.work_id == work_id
                    && receipt.content == content
                    && receipt.sent_message_count == sent_message_count
                {
                    return Ok(SessionWorkResultCommitOutcome::AlreadyCommitted(receipt));
                }
                return Ok(SessionWorkResultCommitOutcome::ConflictingResult);
            }

            let mut admission = admission;
            if !session_admission_identity_is_well_formed(&admission)
                || admission.session_key != session_key
                || admission.lifecycle_revision != expected_lifecycle_revision
                || admission.archived_at_ms.is_some()
                || admission.deletion_pending
                || admission.admitted_work_id.as_deref() != Some(work_id)
            {
                return Ok(SessionWorkResultCommitOutcome::RejectedStale);
            }

            let receipt = SessionWorkResultReceipt {
                session_key: session_key.to_string(),
                lifecycle_revision: expected_lifecycle_revision.to_string(),
                work_id: work_id.to_string(),
                idempotency_key: idempotency_key.to_string(),
                content: content.to_string(),
                sent_message_count,
            };
            let entry = session_work_result_receipt_entry(&receipt, &receipt_id);
            db.execute(
                INSERT_TRANSCRIPT_ENTRY_SQL,
                params![receipt_id, entry.to_string()],
            )?;
            admission.admitted_work_id = None;
            admission.terminal_result_idempotency_key = Some(idempotency_key.to_string());
            let encoded = serde_json::to_string(&admission)?;
            if db.execute(
                COMPARE_AND_SET_KV_SQL,
                params![encoded, KV_SESSION_WORK_ADMISSION, raw_admission],
            )? != 1
            {
                return Ok(SessionWorkResultCommitOutcome::RejectedStale);
            }
            Ok(SessionWorkResultCommitOutcome::Committed(receipt))
        })();

        match result {
            // A stale CAS after the receipt insert must roll back the whole
            // transaction. Committing here would leave an orphan receipt
            // while the admission marker still owns the work, causing a
            // later retry to be misreported as AlreadyCommitted.
            Ok(SessionWorkResultCommitOutcome::RejectedStale) => {
                let _ = db.execute_batch("ROLLBACK");
                Ok(SessionWorkResultCommitOutcome::RejectedStale)
            }
            Ok(outcome) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                if let SessionWorkResultCommitOutcome::Committed(receipt) = &outcome {
                    let entry_id = format!("background-result:{}", receipt.idempotency_key);
                    let entry = db
                        .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
                            row.get::<_, String>(0)
                        })?;
                    drop(guard);
                    if let Some(entry) = parse_transcript_entry(&entry) {
                        publish_entries_upserted(&self.db_path, vec![entry]);
                    }
                    notify_agent_db_listeners(
                        &self.db_path,
                        AgentDbListenerChannel::Metadata(KV_SESSION_WORK_ADMISSION.into()),
                    );
                }
                Ok(outcome)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn read_metadata(&self) -> Result<serde_json::Value, AgentDbProjectionError> {
        let Some(raw) = self.read_kv("metadata")? else {
            return Ok(default_agent_metadata(&self.agent_dir_name));
        };
        let bytes = decode_hex(&raw).map_err(AgentDbProjectionError::MetadataHex)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn get_metadata(
        &self,
        key: &str,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        Ok(self
            .read_metadata()?
            .as_object()
            .and_then(|metadata| metadata.get(key).cloned()))
    }

    pub fn initialize_metadata_if_missing(
        &self,
        value: serde_json::Value,
    ) -> Result<bool, AgentDbProjectionError> {
        if self.is_closed() || !value.is_object() {
            return Ok(false);
        }
        let encoded = encode_hex(&serde_json::to_vec(&value)?);
        let changed = self.run_write("initializeMetadata", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let existing = db
                    .query_row(GET_KV_SQL, params!["metadata"], |row| row.get::<_, String>(0))
                    .optional()?;
                if existing.is_some() {
                    return Ok(false);
                }
                Ok(db.execute(SET_KV_SQL, params!["metadata", encoded])? == 1)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata("metadata".to_string()),
            );
        }
        Ok(changed)
    }

    /// Seed Human identity only when all durable tables are empty at the same
    /// SQLite write boundary. Callers must read and validate the persisted
    /// metadata before publishing an owner, including after a refused or busy
    /// insert. Identity initialization never attempts corruption recovery.
    pub fn initialize_human_metadata_if_empty(
        &self,
        value: serde_json::Value,
    ) -> Result<bool, AgentDbProjectionError> {
        if self.is_closed() || !value.is_object() {
            return Ok(false);
        }
        let encoded = encode_hex(&serde_json::to_vec(&value)?);
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(false);
        };
        let changed = match db.execute(
            "INSERT INTO kv (key, value)
             SELECT 'metadata', ?1
             WHERE NOT EXISTS (SELECT 1 FROM kv)
               AND NOT EXISTS (SELECT 1 FROM transcript_entries)
               AND NOT EXISTS (SELECT 1 FROM blobs)",
            params![encoded],
        ) {
            Ok(changes) => changes == 1,
            Err(error) if is_sqlite_busy_error(&error) => {
                let message = error.to_string();
                drop(guard);
                if let Some(callback) = self.options.on_busy_error.as_ref() {
                    callback("initializeHumanMetadata", &message);
                }
                return Ok(false);
            }
            Err(error) => return Err(error.into()),
        };
        if changed {
            bump_db_write_generation(&self.db_path);
        }
        drop(guard);
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata("metadata".to_string()),
            );
        }
        Ok(changed)
    }

    pub fn set_metadata(
        &self,
        key: &str,
        value: serde_json::Value,
    ) -> Result<bool, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(false);
        }
        if key == "agentId"
            && value.as_str().map(str::trim) != Some(self.agent_dir_name.as_str())
        {
            return Ok(false);
        }
        let Some(raw) = self.read_kv("metadata")? else {
            return Ok(false);
        };
        let bytes = decode_hex(&raw).map_err(AgentDbProjectionError::MetadataHex)?;
        let mut metadata: serde_json::Value = serde_json::from_slice(&bytes)?;
        let Some(object) = metadata.as_object_mut() else {
            return Ok(false);
        };
        let has_lifecycle_revision = object
            .get("lifecycleRevision")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|revision| !revision.trim().is_empty());
        if object.get(key) == Some(&value) && (key == "lifecycleRevision" || has_lifecycle_revision)
        {
            return Ok(true);
        }
        object.insert(key.to_string(), value);
        if key != "lifecycleRevision" {
            object.insert(
                "lifecycleRevision".to_string(),
                serde_json::json!(Uuid::new_v4().to_string()),
            );
        }
        let next_raw = encode_hex(&serde_json::to_vec(&metadata)?);
        let changed = self.run_write(&format!("setMetadata:{key}"), |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let Some(current_raw) = read_kv(db, "metadata")? else {
                    return Ok(false);
                };
                if current_raw != raw {
                    return Ok(false);
                }
                Ok(db.execute(
                    COMPARE_AND_SET_KV_SQL,
                    params![next_raw, "metadata", raw],
                )? == 1)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata(key.to_string()),
            );
        }
        Ok(changed)
    }

    /// Returns the durable session lifecycle revision, migrating legacy stores
    /// exactly once through the existing metadata CAS owner.
    pub fn ensure_lifecycle_revision(&self) -> Result<String, AgentDbProjectionError> {
        if let Some(existing) = self
            .get_metadata("lifecycleRevision")?
            .and_then(|value| value.as_str().map(str::trim).map(ToOwned::to_owned))
            .filter(|value| !value.is_empty())
        {
            return Ok(existing);
        }

        let candidate = Uuid::new_v4().to_string();
        if self.set_metadata("lifecycleRevision", serde_json::json!(candidate))? {
            return Ok(candidate);
        }

        self.get_metadata("lifecycleRevision")?
            .and_then(|value| value.as_str().map(str::trim).map(ToOwned::to_owned))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AgentDbProjectionError::LifecycleRevisionUnavailable(
                    "lifecycle revision migration did not publish a value".to_string(),
                )
            })
    }

    /// Register one background automation run in the existing session owner.
    /// The identity and lifecycle revision are immutable for the row; a
    /// duplicate registration can only refresh an already-running row.
    pub fn begin_cron_session_run(
        &self,
        session_key: &str,
        automation_id: &str,
        run_id: &str,
        session_id: &str,
        lifecycle_revision: &str,
        started_at_ms: f64,
    ) -> Result<bool, AgentDbProjectionError> {
        let session_key = session_key.trim();
        let automation_id = automation_id.trim();
        let run_id = run_id.trim();
        let session_id = session_id.trim();
        let lifecycle_revision = lifecycle_revision.trim();
        if session_key.is_empty()
            || automation_id.is_empty()
            || run_id.is_empty()
            || session_id.is_empty()
            || lifecycle_revision.is_empty()
            || !started_at_ms.is_finite()
        {
            return Ok(false);
        }
        self.run_write("beginCronSessionRun", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(
                    "INSERT INTO cron_session_runs (session_key, automation_id, run_id, session_id, lifecycle_revision, updated_at_ms, base_session, state) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 'running') \
                     ON CONFLICT(session_key) DO UPDATE SET updated_at_ms = excluded.updated_at_ms \
                     WHERE cron_session_runs.automation_id = excluded.automation_id \
                       AND cron_session_runs.run_id = excluded.run_id \
                       AND cron_session_runs.session_id = excluded.session_id \
                       AND cron_session_runs.lifecycle_revision = excluded.lifecycle_revision \
                       AND cron_session_runs.state = 'running'",
                    params![
                        session_key,
                        automation_id,
                        run_id,
                        session_id,
                        lifecycle_revision,
                        started_at_ms
                    ],
                )? > 0)
            })();
            match result {
                Ok(changed) => {
                    match db.execute_batch("COMMIT") {
                        Ok(()) => Ok(changed),
                        Err(error) => {
                            let _ = db.execute_batch("ROLLBACK");
                            Err(error)
                        }
                    }
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// Settle a registered background automation run only when its durable
    /// identity and lifecycle revision still match. Cleanup/archive remains a
    /// separate later reaper operation.
    pub fn finish_cron_session_run(
        &self,
        session_key: &str,
        session_id: &str,
        lifecycle_revision: &str,
        outcome: &str,
        finished_at_ms: f64,
        detail: Option<&str>,
    ) -> Result<bool, AgentDbProjectionError> {
        let session_key = session_key.trim();
        let session_id = session_id.trim();
        let lifecycle_revision = lifecycle_revision.trim();
        let outcome = outcome.trim();
        if session_key.is_empty()
            || session_id.is_empty()
            || lifecycle_revision.is_empty()
            || outcome.is_empty()
            || !matches!(outcome, "ok" | "error" | "interrupted")
            || !finished_at_ms.is_finite()
        {
            return Ok(false);
        }
        self.run_write("finishCronSessionRun", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(
                    "UPDATE cron_session_runs \
                     SET updated_at_ms = ?1, state = 'terminal', terminal_outcome = ?2, terminal_detail = ?3 \
                     WHERE session_key = ?4 AND session_id = ?5 AND lifecycle_revision = ?6 AND state = 'running'",
                    params![
                        finished_at_ms,
                        outcome,
                        detail,
                        session_key,
                        session_id,
                        lifecycle_revision
                    ],
                )? > 0)
            })();
            match result {
                Ok(changed) => {
                    match db.execute_batch("COMMIT") {
                        Ok(()) => Ok(changed),
                        Err(error) => {
                            let _ = db.execute_batch("ROLLBACK");
                            Err(error)
                        }
                    }
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// Atomically publish a complete terminal fence snapshot with the row's
    /// terminal outcome. A future candidate projection must not observe the
    /// fence marker before the terminal state is durable.
    pub fn finish_cron_session_run_with_fences(
        &self,
        session_key: &str,
        session_id: &str,
        lifecycle_revision: &str,
        outcome: &str,
        finished_at_ms: f64,
        detail: Option<&str>,
        active_controller: bool,
        pending_outgoing_media: bool,
        unsettled_subagent: bool,
    ) -> Result<bool, AgentDbProjectionError> {
        let session_key = session_key.trim();
        let session_id = session_id.trim();
        let lifecycle_revision = lifecycle_revision.trim();
        let outcome = outcome.trim();
        if session_key.is_empty()
            || session_id.is_empty()
            || lifecycle_revision.is_empty()
            || outcome.is_empty()
            || !matches!(outcome, "ok" | "error" | "interrupted")
            || !finished_at_ms.is_finite()
        {
            return Ok(false);
        }
        self.run_write("finishCronSessionRunWithFences", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(
                    "UPDATE cron_session_runs \
                     SET updated_at_ms = ?1, state = 'terminal', terminal_outcome = ?2, \
                         terminal_detail = ?3, active_controller = ?4, \
                         pending_outgoing_media = ?5, unsettled_subagent = ?6, \
                         fence_state_known = 1 \
                     WHERE session_key = ?7 AND session_id = ?8 \
                       AND lifecycle_revision = ?9 AND state = 'running'",
                    params![
                        finished_at_ms,
                        outcome,
                        detail,
                        active_controller,
                        pending_outgoing_media,
                        unsettled_subagent,
                        session_key,
                        session_id,
                        lifecycle_revision,
                    ],
                )? > 0)
            })();
            match result {
                Ok(changed) => {
                    match db.execute_batch("COMMIT") {
                        Ok(()) => Ok(changed),
                        Err(error) => {
                            let _ = db.execute_batch("ROLLBACK");
                            Err(error)
                        }
                    }
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// Publish the complete row-specific activity fence before terminal
    /// settlement. Unknown or stale identities never become known by this
    /// operation.
    pub fn publish_cron_session_run_fences(
        &self,
        session_key: &str,
        session_id: &str,
        lifecycle_revision: &str,
        active_controller: bool,
        pending_outgoing_media: bool,
        unsettled_subagent: bool,
    ) -> Result<bool, AgentDbProjectionError> {
        let session_key = session_key.trim();
        let session_id = session_id.trim();
        let lifecycle_revision = lifecycle_revision.trim();
        if session_key.is_empty() || session_id.is_empty() || lifecycle_revision.is_empty() {
            return Ok(false);
        }
        self.run_write("publishCronSessionRunFences", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                Ok(db.execute(
                    "UPDATE cron_session_runs \
                     SET active_controller = ?1, pending_outgoing_media = ?2, \
                         unsettled_subagent = ?3, fence_state_known = 1 \
                     WHERE session_key = ?4 AND session_id = ?5 \
                       AND lifecycle_revision = ?6 AND state = 'running'",
                    params![
                        active_controller,
                        pending_outgoing_media,
                        unsettled_subagent,
                        session_key,
                        session_id,
                        lifecycle_revision
                    ],
                )? > 0)
            })();
            match result {
                Ok(changed) => {
                    match db.execute_batch("COMMIT") {
                        Ok(()) => Ok(changed),
                        Err(error) => {
                            let _ = db.execute_batch("ROLLBACK");
                            Err(error)
                        }
                    }
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    /// Read the per-run projections owned by this agent database. This is
    /// intentionally a projection only; retention policy and deletion remain
    /// outside this method and must use a later guarded lifecycle operation.
    pub fn list_cron_session_runs(
        &self,
    ) -> Result<Vec<CronSessionRunRecord>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(Vec::new());
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let mut statement = db.prepare(
            "SELECT session_key, automation_id, run_id, session_id, lifecycle_revision, \
                    updated_at_ms, base_session, state, terminal_outcome, terminal_detail, \
                    active_controller, pending_outgoing_media, unsettled_subagent, fence_state_known \
             FROM cron_session_runs ORDER BY updated_at_ms ASC, session_key ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(CronSessionRunRecord {
                session_key: row.get(0)?,
                automation_id: row.get(1)?,
                run_id: row.get(2)?,
                session_id: row.get(3)?,
                lifecycle_revision: row.get(4)?,
                updated_at_ms: row.get(5)?,
                base_session: row.get::<_, i64>(6)? != 0,
                state: row.get(7)?,
                terminal_outcome: row.get(8)?,
                terminal_detail: row.get(9)?,
                active_controller: row.get::<_, i64>(10)? != 0,
                pending_outgoing_media: row.get::<_, i64>(11)? != 0,
                unsettled_subagent: row.get::<_, i64>(12)? != 0,
                fence_state_known: row.get::<_, i64>(13)? != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Compare a single metadata key (including absent versus explicit null)
    /// with the caller's observed value, and atomically CAS the complete SQLite
    /// metadata record. Unlike unconditional set_metadata, this rejects a
    /// stale merge of another writer's source-owned forwarding journal.
    pub fn set_metadata_if_unchanged(
        &self,
        key: &str,
        expected: Option<&serde_json::Value>,
        value: serde_json::Value,
    ) -> Result<bool, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(false);
        }
        let Some(raw) = self.read_kv("metadata")? else {
            return Ok(false);
        };
        let bytes = decode_hex(&raw).map_err(AgentDbProjectionError::MetadataHex)?;
        let mut metadata: serde_json::Value = serde_json::from_slice(&bytes)?;
        let Some(object) = metadata.as_object_mut() else {
            return Ok(false);
        };
        if object.get(key) != expected {
            return Ok(false);
        }
        if object.get(key) == Some(&value) {
            return Ok(true);
        }
        object.insert(key.to_string(), value);
        let next_raw = encode_hex(&serde_json::to_vec(&metadata)?);
        let changed = self.run_write(&format!("setMetadataIfUnchanged:{key}"), |db| {
            db.execute(
                COMPARE_AND_SET_KV_SQL,
                params![next_raw, "metadata", raw],
            )
            .map(|changes| changes == 1)
        })?;
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata(key.to_string()),
            );
        }
        Ok(changed)
    }

    /// Atomically advances the durable Agent root and marks the addressed
    /// user transcript entry as confirmed. Conversation blobs are written
    /// before this call; metadata + recovery watermark must commit together.
    pub fn commit_checkpoint_root_and_confirm_user_message(
        &self,
        expected_root: &[u8],
        next_root: &[u8],
        entry_id: &str,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        let entry_id = entry_id.trim();
        if self.is_closed() || entry_id.is_empty() {
            return Ok(None);
        }
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(None);
        };

        db.execute_batch("BEGIN IMMEDIATE")?;
        let transaction = (|| -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
            if !session_work_write_is_admitted(db).map_err(AgentDbProjectionError::Sqlite)? {
                return Ok(None);
            }
            let raw_metadata = db
                .query_row(GET_KV_SQL, params!["metadata"], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let Some(raw_metadata) = raw_metadata else {
                return Ok(None);
            };
            let bytes =
                decode_hex(&raw_metadata).map_err(AgentDbProjectionError::MetadataHex)?;
            let mut metadata: serde_json::Value = serde_json::from_slice(&bytes)?;
            let current_root = metadata
                .get("latestRootBlobId")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if current_root != encode_hex(expected_root) {
                return Ok(None);
            }

            let raw_entry = db
                .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let Some(mut entry) = raw_entry
                .as_deref()
                .and_then(parse_transcript_entry)
            else {
                return Ok(None);
            };
            if entry.get("kind").and_then(serde_json::Value::as_str) != Some("message")
                || entry.get("role").and_then(serde_json::Value::as_str) != Some("user")
            {
                return Ok(None);
            }
            let Some(entry_object) = entry.as_object_mut() else {
                return Ok(None);
            };
            entry_object.insert("confirmed".into(), serde_json::Value::Bool(true));

            let Some(metadata_object) = metadata.as_object_mut() else {
                return Ok(None);
            };
            metadata_object.insert(
                "latestRootBlobId".into(),
                serde_json::Value::String(encode_hex(next_root)),
            );
            metadata_object.insert(
                "lifecycleRevision".into(),
                serde_json::json!(Uuid::new_v4().to_string()),
            );
            let next_metadata = encode_hex(&serde_json::to_vec(&metadata)?);
            if db.execute(
                COMPARE_AND_SET_KV_SQL,
                params![next_metadata, "metadata", raw_metadata],
            )? != 1
            {
                return Ok(None);
            }
            if db.execute(
                UPDATE_TRANSCRIPT_ENTRY_SQL,
                params![entry.to_string(), entry_id],
            )? != 1
            {
                return Ok(None);
            }
            Ok(Some(entry))
        })();

        let confirmed = match transaction {
            Ok(Some(entry)) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                entry
            }
            Ok(None) => {
                let _ = db.execute_batch("ROLLBACK");
                return Ok(None);
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                if let AgentDbProjectionError::Sqlite(sqlite) = &error {
                    if is_sqlite_busy_error(sqlite) {
                        if let Some(callback) = self.options.on_busy_error.as_ref() {
                            callback(
                                "commitCheckpointRootAndConfirmUserMessage",
                                &sqlite.to_string(),
                            );
                        }
                        return Ok(None);
                    }
                }
                return Err(error);
            }
        };
        drop(guard);
        bump_db_write_generation(&self.db_path);
        notify_agent_db_listeners(
            &self.db_path,
            AgentDbListenerChannel::Metadata("latestRootBlobId".into()),
        );
        publish_entries_upserted(&self.db_path, vec![confirmed.clone()]);
        Ok(Some(confirmed))
    }

    pub fn compare_and_set_latest_root_blob_id(
        &self,
        expected_root: &[u8],
        next_root: &[u8],
    ) -> Result<bool, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(false);
        }
        let Some(raw) = self.read_kv("metadata")? else {
            return Ok(false);
        };
        let bytes = decode_hex(&raw).map_err(AgentDbProjectionError::MetadataHex)?;
        let mut metadata: serde_json::Value = serde_json::from_slice(&bytes)?;
        let current = metadata
            .get("latestRootBlobId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if current != encode_hex(expected_root) {
            return Ok(false);
        }
        let Some(object) = metadata.as_object_mut() else {
            return Ok(false);
        };
        object.insert(
            "latestRootBlobId".into(),
            serde_json::Value::String(encode_hex(next_root)),
        );
        object.insert(
            "lifecycleRevision".into(),
            serde_json::json!(Uuid::new_v4().to_string()),
        );
        let next_raw = encode_hex(&serde_json::to_vec(&metadata)?);
        let changed = self.run_write("compareAndSetLatestRootBlobId", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let Some(current_raw) = read_kv(db, "metadata")? else {
                    return Ok(false);
                };
                if current_raw != raw {
                    return Ok(false);
                }
                Ok(db.execute(
                    COMPARE_AND_SET_KV_SQL,
                    params![next_raw, "metadata", raw],
                )? == 1)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if changed {
            notify_agent_db_listeners(
                &self.db_path,
                AgentDbListenerChannel::Metadata("latestRootBlobId".into()),
            );
        }
        Ok(changed)
    }

    pub fn get_latest_root_blob_id(&self) -> Result<Vec<u8>, AgentDbProjectionError> {
        let root = self
            .get_metadata("latestRootBlobId")?
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_default();
        decode_hex(&root).map_err(AgentDbProjectionError::LatestRootHex)
    }

    pub fn serde_snapshot(&self) -> Result<AgentDbSerdeSnapshot, AgentDbProjectionError> {
        Ok(AgentDbSerdeSnapshot {
            profile: self.get_sand_profile()?,
            unread_state: self.get_unread_state()?,
            spend_guard_state: self.get_automation_spend_guard_state()?,
            awaiting_user_response: self.get_awaiting_user_response()?,
            request_ids: self.get_request_ids()?,
            pending_episode_turns: self.get_pending_episode_turns()?,
            memory_prompt_snapshot: self.get_memory_prompt_snapshot()?,
        })
    }

    pub fn clear_transient_state(&self) -> Result<bool, AgentDbProjectionError> {
        self.run_write("clearTransientState", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let mut changed = false;
                for key in [
                    KV_UNREAD,
                    KV_SPEND_GUARD_LEGACY,
                    KV_SPEND_GUARD,
                    KV_AWAITING,
                    KV_LATEST_REQUEST_ID,
                    KV_REQUEST_IDS,
                    KV_EPISODE,
                    KV_MEMORY_SNAPSHOT,
                    KV_PROFILE_SNAPSHOT,
                ] {
                    changed |= db.execute(DELETE_KV_SQL, params![key])? > 0;
                }
                Ok(changed)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }

    pub fn clear_clone_mutation_journals(&self) -> Result<bool, AgentDbProjectionError> {
        self.run_write("clearCloneMutationJournals", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                let mut changed = false;
                for key in [KV_PENDING_MEMORY_MUTATION, KV_PENDING_PROFILE_MUTATION] {
                    changed |= db.execute(DELETE_KV_SQL, params![key])? > 0;
                }
                Ok(changed)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })
    }


    pub fn append_transcript_entry(
        &self,
        entry: &serde_json::Value,
    ) -> Result<bool, AgentDbProjectionError> {
        Ok(self.append_transcript_entries(std::slice::from_ref(entry))? > 0)
    }

    pub fn append_transcript_entries(
        &self,
        entries: &[serde_json::Value],
    ) -> Result<usize, AgentDbProjectionError> {
        if entries.is_empty() {
            return Ok(0);
        }
        let inserted = self.run_write_value("appendTranscriptEntries", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<Vec<serde_json::Value>, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(Vec::new());
                }
                let mut inserted = Vec::new();
                {
                    let mut statement = db.prepare(INSERT_TRANSCRIPT_ENTRY_SQL)?;
                    for entry in entries {
                        let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) else {
                            continue;
                        };
                        if entry.get("kind").and_then(serde_json::Value::as_str).is_none() {
                            continue;
                        }
                        if statement.execute(params![id, entry.to_string()])? > 0 {
                            inserted.push(entry.clone());
                        }
                    }
                }
                Ok(inserted)
            })();
            match result {
                Ok(inserted) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    let changed = !inserted.is_empty();
                    Ok((inserted, changed))
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if !inserted.is_empty() {
            publish_entries_upserted(&self.db_path, inserted.clone());
        }
        Ok(inserted.len())
    }

    /// Append entries only while the canonical session is currently admitted.
    /// The admission check and transcript insert share one SQLite transaction
    /// so archive/restore cannot race a channel or other session-scoped
    /// producer at the durable write edge.
    pub fn append_transcript_entries_if_session_admitted(
        &self,
        session_key: &str,
        entries: &[serde_json::Value],
    ) -> Result<usize, AgentDbProjectionError> {
        let session_key = session_key.trim();
        if session_key.is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session-scoped transcript append requires a session key".into(),
            ));
        }
        if entries.is_empty() {
            return Ok(0);
        }
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_mut() else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "agent database is closed".into(),
            ));
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
            if !session_work_write_is_admitted(db)? {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session is not admitted for transcript append".into(),
                ));
            }
            let raw_admission = db
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
                    "session is not admitted for transcript append".into(),
                ));
            }
            let mut inserted = Vec::new();
            let mut statement = db.prepare(INSERT_TRANSCRIPT_ENTRY_SQL)?;
            for entry in entries {
                let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if entry.get("kind").and_then(serde_json::Value::as_str).is_none() {
                    continue;
                }
                if entry
                    .get("sessionKey")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    != Some(session_key)
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "session-scoped transcript append requires matching entry provenance"
                            .into(),
                    ));
                }
                if statement.execute(params![id, entry.to_string()])? > 0 {
                    inserted.push(entry.clone());
                }
            }
            Ok(inserted)
        })();
        match result {
            Ok(inserted) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                let count = inserted.len();
                drop(guard);
                if !inserted.is_empty() {
                    publish_entries_upserted(&self.db_path, inserted);
                }
                Ok(count)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// Atomically publish one session-scoped spend-guard card together with
    /// its owner-local state and activity projection. The deletion/archive
    /// fence is checked in the same SQLite transaction as every mutation.
    pub fn append_spend_guard_card_if_session_admitted(
        &self,
        session_key: &str,
        entry: &serde_json::Value,
        state: &SpendGuardState,
        activity_at: f64,
    ) -> Result<bool, AgentDbProjectionError> {
        let session_key = session_key.trim();
        let Some(entry_id) = entry.get("id").and_then(serde_json::Value::as_str) else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "spend-guard card requires an entry identity".into(),
            ));
        };
        if session_key.is_empty()
            || entry.get("kind").and_then(serde_json::Value::as_str).is_none()
            || entry
                .get("sessionKey")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                != Some(session_key)
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "spend-guard card requires matching session provenance".into(),
            ));
        }
        let spend_guard_raw = serialize_spend_guard_state(state);
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_mut() else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "agent database is closed".into(),
            ));
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<(bool, bool), AgentDbProjectionError> {
            if !session_work_write_is_admitted(db)? {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session is not admitted for spend-guard card".into(),
                ));
            }
            let raw_admission = db
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
                    "session is not admitted for spend-guard card".into(),
                ));
            }
            let inserted = db.execute(
                INSERT_TRANSCRIPT_ENTRY_SQL,
                params![entry_id, entry.to_string()],
            )? > 0;
            let spend_changed = if let Some(raw) = spend_guard_raw.as_deref() {
                db.execute(SET_KV_SQL, params![KV_SPEND_GUARD, raw])? > 0
            } else {
                db.execute(DELETE_KV_SQL, params![KV_SPEND_GUARD])? > 0
            };
            let legacy_changed =
                db.execute(DELETE_KV_SQL, params![KV_SPEND_GUARD_LEGACY])? > 0;
            let current = parse_unread_state(
                read_kv(db, KV_UNREAD)?.as_deref(),
            );
            let activity_changed = current.last_activity_at < activity_at;
            if activity_changed {
                let raw = serde_json::json!({
                    "lastActivityAt": activity_at,
                    "lastViewedAt": current.last_viewed_at,
                    "isManuallyUnread": current.is_manually_unread,
                    "unreadCount": current.unread_count + 1.0,
                })
                .to_string();
                db.execute(SET_KV_SQL, params![KV_UNREAD, raw])?;
            }
            Ok((inserted, spend_changed || legacy_changed || activity_changed))
        })();
        match result {
            Ok((inserted, metadata_changed)) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                if inserted || metadata_changed {
                    bump_db_write_generation(&self.db_path);
                }
                if inserted {
                    publish_entries_upserted(&self.db_path, vec![entry.clone()]);
                }
                if metadata_changed {
                    notify_agent_db_listeners(
                        &self.db_path,
                        AgentDbListenerChannel::Metadata(KV_SPEND_GUARD.into()),
                    );
                    notify_agent_db_listeners(
                        &self.db_path,
                        AgentDbListenerChannel::Metadata(KV_UNREAD.into()),
                    );
                }
                Ok(inserted)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// Atomically append transcript entries and clear one captured outgoing
    /// media fence. A stale or rebound fence rolls back the transcript write.
    pub fn append_transcript_entries_and_release_media(
        &self,
        entries: &[serde_json::Value],
        expected_session_key: &str,
        expected_lifecycle_revision: &str,
        media_work_id: &str,
    ) -> Result<usize, AgentDbProjectionError> {
        if entries.is_empty()
            || expected_session_key.trim().is_empty()
            || expected_lifecycle_revision.trim().is_empty()
            || media_work_id.trim().is_empty()
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "media settlement requires transcript entries, session identity, generation, and work identity"
                    .into(),
            ));
        }
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_mut() else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "agent database is closed".into(),
            ));
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
            let raw_admission = db
                .query_row(GET_KV_SQL, params![KV_SESSION_WORK_ADMISSION], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
                .ok_or_else(|| {
                    AgentDbProjectionError::WorkAdmissionInvalid(
                        "session work admission record is unavailable".into(),
                    )
                })?;
            let mut admission: SessionWorkAdmissionRecord = serde_json::from_str(&raw_admission)?;
            if !session_admission_identity_is_well_formed(&admission) {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "outgoing media settlement admission identity is malformed".into(),
                ));
            }
            let receipt_id = format!("outgoing-media-settlement:{}", media_work_id.trim());
            if let Some(raw_receipt) = db
                .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![&receipt_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?
            {
                let receipt = parse_transcript_entry(&raw_receipt).ok_or_else(|| {
                    AgentDbProjectionError::WorkAdmissionInvalid(
                        "outgoing media settlement receipt is malformed".into(),
                    )
                })?;
                let receipt_matches = receipt
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    == Some("outgoing-media-settlement")
                    && receipt
                        .get("sessionKey")
                        .and_then(serde_json::Value::as_str)
                        == Some(expected_session_key.trim())
                    && receipt
                        .get("mediaWorkId")
                        .and_then(serde_json::Value::as_str)
                        == Some(media_work_id.trim())
                    && receipt
                        .get("lifecycleRevision")
                        .and_then(serde_json::Value::as_str)
                        == Some(expected_lifecycle_revision.trim());
                if !receipt_matches {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "outgoing media settlement receipt is rebound".into(),
                    ));
                }
                if !session_admission_identity_is_well_formed(&admission)
                    || admission.session_key != expected_session_key.trim()
                    || admission.lifecycle_revision != expected_lifecycle_revision.trim()
                    || admission.archived_at_ms.is_some()
                    || admission.deletion_pending
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "media settlement replay is stale or session is not admitted".into(),
                    ));
                }
                if admission.admitted_media_work_id.is_some()
                    || !admission.admitted_media_paths.is_empty()
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "outgoing media settlement receipt conflicts with a pending media fence"
                            .into(),
                    ));
                }
                let receipt_entry_ids = receipt
                    .get("transcriptEntryIds")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| {
                        AgentDbProjectionError::WorkAdmissionInvalid(
                            "outgoing media settlement receipt is missing transcript identities"
                                .into(),
                        )
                    })?;
                let incoming_entry_ids = entries
                    .iter()
                    .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
                    .collect::<Vec<_>>();
                if receipt_entry_ids.len() != incoming_entry_ids.len()
                    || !receipt_entry_ids.iter().zip(incoming_entry_ids.iter()).all(
                        |(expected, actual)| expected.as_str() == Some(*actual),
                    )
                {
                    return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                        "outgoing media settlement replay conflicts with the committed transcript"
                            .into(),
                    ));
                }
                for entry in entries {
                    let entry_id = entry
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            AgentDbProjectionError::WorkAdmissionInvalid(
                                "outgoing media settlement replay has no entry identity".into(),
                            )
                        })?;
                    let raw_existing = db
                        .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
                            row.get::<_, String>(0)
                        })
                        .optional()?
                        .ok_or_else(|| {
                            AgentDbProjectionError::WorkAdmissionInvalid(
                                "outgoing media settlement replay lost its transcript entry"
                                    .into(),
                            )
                        })?;
                    if parse_transcript_entry(&raw_existing).as_ref() != Some(entry) {
                        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                            "outgoing media settlement replay conflicts with the committed transcript content"
                                .into(),
                        ));
                    }
                }
                return Ok(Vec::new());
            }
            if !session_work_write_is_admitted(db)? {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session owner identity is not admitted for media settlement".into(),
                ));
            }
            if !session_admission_identity_is_well_formed(&admission)
                || admission.session_key != expected_session_key.trim()
                || admission.lifecycle_revision != expected_lifecycle_revision.trim()
                || admission.admitted_media_work_id.as_deref() != Some(media_work_id.trim())
                || admission.archived_at_ms.is_some()
                || admission.deletion_pending
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "media settlement generation or identity is stale".into(),
                ));
            }
            let mut inserted = Vec::new();
            {
                let mut statement = db.prepare(INSERT_TRANSCRIPT_ENTRY_SQL)?;
                for entry in entries {
                    let id = entry
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|id| !id.trim().is_empty())
                        .ok_or_else(|| {
                            AgentDbProjectionError::WorkAdmissionInvalid(
                                "media settlement requires transcript entry identities".into(),
                            )
                        })?;
                    if entry
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .filter(|kind| !kind.trim().is_empty())
                        .is_none()
                    {
                        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                            "media settlement requires transcript entry kinds".into(),
                        ));
                    }
                    if entry
                        .get("sessionKey")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .is_some_and(|key| key != expected_session_key.trim())
                    {
                        return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                            "media settlement requires matching entry provenance".into(),
                        ));
                    }
                    if statement.execute(params![id, entry.to_string()])? > 0 {
                        inserted.push(entry.clone());
                    }
                }
            }
            if inserted.len() != entries.len() {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "media settlement did not persist every transcript entry".into(),
                ));
            }
            let receipt = media_settlement_receipt_entry(
                &receipt_id,
                expected_session_key.trim(),
                media_work_id.trim(),
                expected_lifecycle_revision.trim(),
                &admission.admitted_media_paths,
                &inserted,
            );
            db.execute(
                INSERT_TRANSCRIPT_ENTRY_SQL,
                params![receipt_id, receipt.to_string()],
            )?;
            admission.admitted_media_work_id = None;
            admission.admitted_media_paths.clear();
            let encoded = serde_json::to_string(&admission)?;
            if db.execute(
                COMPARE_AND_SET_KV_SQL,
                params![encoded, KV_SESSION_WORK_ADMISSION, raw_admission],
            )? != 1
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "media settlement lost its lifecycle race".into(),
                ));
            }
            Ok(inserted)
        })();
        match result {
            Ok(inserted) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                let count = inserted.len();
                if !inserted.is_empty() {
                    publish_entries_upserted(&self.db_path, inserted);
                }
                notify_agent_db_listeners(
                    &self.db_path,
                    AgentDbListenerChannel::Metadata(KV_SESSION_WORK_ADMISSION.into()),
                );
                Ok(count)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// Atomically commits one guarded compaction entry and advances the
    /// session lifecycle revision. Older rows remain durable; replay uses the
    /// latest compaction entry as the boundary for the active context.
    pub fn commit_compaction(
        &self,
        commit: &CompactionCommit,
    ) -> Result<CompactionCommitOutcome, AgentDbProjectionError> {
        let entry_id = commit.entry_id.trim();
        let source_turn_id = commit.source_turn_id.trim();
        let expected_revision = commit.expected_lifecycle_revision.trim();
        let expected_tail_id = commit.expected_transcript_tail_id.trim();
        if entry_id.is_empty()
            || source_turn_id.is_empty()
            || expected_revision.is_empty()
            || expected_tail_id.is_empty()
        {
            return Err(AgentDbProjectionError::CompactionInvalid(
                "entry_id, source_turn_id, expected_transcript_tail_id, and expected_lifecycle_revision are required".into(),
            ));
        }
        let summary = commit.summary.trim();
        if summary.is_empty() {
            return Err(AgentDbProjectionError::CompactionInvalid(
                "summary must be non-empty".into(),
            ));
        }
        if summary.chars().count() > MAX_COMPACTION_SUMMARY_CHARS {
            return Err(AgentDbProjectionError::CompactionInvalid(format!(
                "summary exceeds {MAX_COMPACTION_SUMMARY_CHARS} characters"
            )));
        }
        if let Some(first_kept_entry_id) = commit.first_kept_entry_id.as_deref() {
            if first_kept_entry_id.trim().is_empty() {
                return Err(AgentDbProjectionError::CompactionInvalid(
                    "first_kept_entry_id must be non-empty when present".into(),
                ));
            }
        }

        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Err(AgentDbProjectionError::CompactionInvalid(
                "agent database is closed".into(),
            ));
        };

        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<CompactionCommitOutcome, AgentDbProjectionError> {
            if !session_work_write_is_admitted(db)? {
                return Ok(CompactionCommitOutcome::RejectedStale);
            }
            let existing = db
                .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            if let Some(raw) = existing {
                let Some(existing) = parse_transcript_entry(&raw) else {
                    return Err(AgentDbProjectionError::CompactionInvalid(
                        "existing compaction identity contains malformed JSON".into(),
                    ));
                };
                if compaction_payload_matches(&existing, commit) {
                    return Ok(CompactionCommitOutcome::AlreadyCommitted(existing));
                }
                return Err(AgentDbProjectionError::CompactionConflict(entry_id.to_string()));
            }

            let raw_metadata = db
                .query_row(GET_KV_SQL, params!["metadata"], |row| row.get::<_, String>(0))
                .optional()?;
            let Some(raw_metadata) = raw_metadata else {
                return Err(AgentDbProjectionError::CompactionInvalid(
                    "metadata is missing".into(),
                ));
            };
            let metadata_bytes = decode_hex(&raw_metadata).map_err(AgentDbProjectionError::MetadataHex)?;
            let mut metadata: serde_json::Value = serde_json::from_slice(&metadata_bytes)?;
            let current_revision = metadata
                .get("lifecycleRevision")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    AgentDbProjectionError::LifecycleRevisionUnavailable(
                        "metadata has no lifecycleRevision".into(),
                    )
                })?;
            if !session_work_write_is_admitted(db)? {
                return Ok(CompactionCommitOutcome::RejectedStale);
            }
            if current_revision != expected_revision {
                return Ok(CompactionCommitOutcome::RejectedStale);
            }

            let current_tail_id = db
                .query_row(
                    "SELECT id FROM transcript_entries ORDER BY seq DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if current_tail_id.as_deref() != Some(expected_tail_id) {
                return Ok(CompactionCommitOutcome::RejectedStale);
            }

            if let Some(first_kept_entry_id) = commit.first_kept_entry_id.as_deref() {
                let present = db
                    .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![first_kept_entry_id], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                    .is_some();
                if !present {
                    return Err(AgentDbProjectionError::CompactionInvalid(format!(
                        "first kept entry {first_kept_entry_id} does not exist"
                    )));
                }
            }

            let next_revision = Uuid::new_v4().to_string();
            let mut entry = serde_json::Map::new();
            entry.insert("id".into(), serde_json::Value::String(entry_id.to_string()));
            entry.insert("kind".into(), serde_json::Value::String("compaction".into()));
            entry.insert(
                "firstKeptEntryId".into(),
                commit
                    .first_kept_entry_id
                    .as_ref()
                    .map(|value| serde_json::Value::String(value.trim().to_string()))
                    .unwrap_or(serde_json::Value::Null),
            );
            entry.insert("summary".into(), serde_json::Value::String(summary.to_string()));
            entry.insert(
                "tokensBefore".into(),
                serde_json::Value::Number(serde_json::Number::from(commit.tokens_before)),
            );
            if let Some(details) = commit.details.clone() {
                entry.insert("details".into(), details);
            }
            entry.insert(
                "sourceTurnId".into(),
                serde_json::Value::String(source_turn_id.to_string()),
            );
            entry.insert(
                "sourceLifecycleRevision".into(),
                serde_json::Value::String(expected_revision.to_string()),
            );
            entry.insert(
                "lifecycleRevision".into(),
                serde_json::Value::String(next_revision.clone()),
            );
            let entry = serde_json::Value::Object(entry);
            db.execute(
                INSERT_TRANSCRIPT_ENTRY_SQL,
                params![entry_id, entry.to_string()],
            )?;

            let metadata_object = metadata.as_object_mut().ok_or_else(|| {
                AgentDbProjectionError::CompactionInvalid("metadata is not an object".into())
            })?;
            metadata_object.insert(
                "lifecycleRevision".into(),
                serde_json::Value::String(next_revision),
            );
            let next_metadata = encode_hex(&serde_json::to_vec(&metadata)?);
            if db.execute(
                COMPARE_AND_SET_KV_SQL,
                params![next_metadata, "metadata", raw_metadata],
            )? != 1
            {
                return Ok(CompactionCommitOutcome::RejectedStale);
            }
            Ok(CompactionCommitOutcome::Committed(entry))
        })();

        match result {
            Ok(outcome @ CompactionCommitOutcome::Committed(_)) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                // Listeners may synchronously read this same owner.
                drop(guard);
                bump_db_write_generation(&self.db_path);
                if let CompactionCommitOutcome::Committed(entry) = &outcome {
                    publish_entries_upserted(&self.db_path, vec![entry.clone()]);
                    notify_agent_db_listeners(
                        &self.db_path,
                        AgentDbListenerChannel::Metadata("lifecycleRevision".into()),
                    );
                }
                Ok(outcome)
            }
            Ok(outcome) => {
                let _ = db.execute_batch("ROLLBACK");
                Ok(outcome)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// Returns the active replay projection after the newest compaction.
    /// Malformed compaction boundaries fail closed instead of silently
    /// replaying an incomplete conversation.
    pub fn get_compacted_transcript_entries(
        &self,
    ) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
        let entries = self.get_transcript_entries()?;
        let Some(compaction_index) = entries
            .iter()
            .rposition(|entry| entry.get("kind").and_then(serde_json::Value::as_str) == Some("compaction"))
        else {
            return Ok(entries);
        };
        let compaction = entries[compaction_index].clone();
        if compaction.get("firstKeptEntryId").is_none()
            || compaction.get("summary").and_then(serde_json::Value::as_str).is_none()
            || compaction.get("tokensBefore").and_then(serde_json::Value::as_u64).is_none()
        {
            return Err(AgentDbProjectionError::CompactionInvalid(
                "compaction entry is missing replay fields".into(),
            ));
        }
        let first_kept = compaction
            .get("firstKeptEntryId")
            .and_then(serde_json::Value::as_str);
        let start = match first_kept {
            Some(first_kept) if !first_kept.is_empty() => entries
                .iter()
                .position(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(first_kept))
                .ok_or_else(|| {
                    AgentDbProjectionError::CompactionInvalid(format!(
                        "compaction boundary {first_kept} is missing"
                    ))
                })?,
            Some(_) | None => entries.len(),
        };
        let mut replay = vec![compaction];
        replay.extend(
            entries
                .into_iter()
                .skip(start)
                .filter(|entry| entry.get("kind").and_then(serde_json::Value::as_str) != Some("compaction")),
        );
        Ok(replay)
    }

    pub fn update_transcript_entry(
        &self,
        entry_id: &str,
        next: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        let updated = self.run_write_value("updateTranscriptEntry", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<(Option<serde_json::Value>, bool), rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok((None, false));
                }
                let present = db
                    .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?
                    .is_some();
                if !present {
                    return Ok((None, false));
                }
                let changed = db.execute(
                    UPDATE_TRANSCRIPT_ENTRY_SQL,
                    params![next.to_string(), entry_id],
                )? > 0;
                Ok((changed.then(|| next.clone()), changed))
            })();
            match result {
                Ok(outcome) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(outcome)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if let Some(entry) = updated.as_ref() {
            publish_entries_upserted(&self.db_path, vec![entry.clone()]);
        }
        Ok(updated)
    }

    /// Update one transcript entry only while the captured session remains
    /// admitted. The admission check and update share one SQLite transaction
    /// so an archived/rebound session cannot settle a permission card after a
    /// preflight check has passed.
    pub fn update_transcript_entry_if_session_admitted(
        &self,
        session_key: &str,
        entry_id: &str,
        next: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        if session_key.trim().is_empty() || entry_id.trim().is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session-scoped transcript update requires session key and entry identity".into(),
            ));
        }
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_mut() else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "agent database is closed".into(),
            ));
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
            if !session_work_write_is_admitted(db)? {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session is not admitted for transcript update".into(),
                ));
            }
            let raw_admission = db
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
                || admission.session_key != session_key.trim()
                || admission.archived_at_ms.is_some()
                || admission.deletion_pending
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session is not admitted for transcript update".into(),
                ));
            }
            let raw_entry = db
                .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let Some(raw_entry) = raw_entry else {
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
                != Some(session_key.trim())
            {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "target transcript entry provenance does not match admitted session".into(),
                ));
            }
            if db.execute(
                UPDATE_TRANSCRIPT_ENTRY_SQL,
                params![next.to_string(), entry_id],
            )? != 1
            {
                return Ok(None);
            }
            Ok(Some(next.clone()))
        })();
        match result {
            Ok(updated) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                drop(guard);
                if let Some(entry) = updated.as_ref() {
                    publish_entries_upserted(&self.db_path, vec![entry.clone()]);
                }
                Ok(updated)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn settle_transcript_entry_identity(
        &self,
        entry_id: &str,
        next: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        let Some(next_id) = next.get("id").and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        let updated = self.run_write_value("settleTranscriptEntryIdentity", |db| {
            // The owner write wrapper serializes access but does not start a
            // transaction. Parent identity and unsent reply edges must commit
            // together, including when a child write or COMMIT is rejected.
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<Vec<serde_json::Value>, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(Vec::new());
                }
                let Some(current) = read_transcript_entry_from_db(db, entry_id)? else {
                    return Ok(Vec::new());
                };
                let author = current.get("authorId").and_then(serde_json::Value::as_str);
                let nonce = current.get("clientNonce").and_then(serde_json::Value::as_str);
                let remote_id = next.get("remoteMessageId").and_then(serde_json::Value::as_str);
                let repair_replies = entry_id != next_id
                    && current.get("authorKind").and_then(serde_json::Value::as_str) == Some("human")
                    && current.get("remoteMessageId").is_none()
                    && matches!(current.get("delivery").and_then(serde_json::Value::as_str), Some("pending" | "dispatching"))
                    && author.is_some()
                    && author == next.get("authorId").and_then(serde_json::Value::as_str)
                    && nonce.is_some_and(|nonce| !nonce.is_empty() && entry_id == format!("human-message:{nonce}"))
                    && nonce == next.get("clientNonce").and_then(serde_json::Value::as_str)
                    && next.get("delivery").and_then(serde_json::Value::as_str) == Some("sent")
                    && remote_id.is_some_and(|id| !id.is_empty() && next_id == format!("human-server-message:{id}"));
                if db.execute(
                    "UPDATE transcript_entries SET id = ?1, entry = ?2 WHERE id = ?3",
                    params![next_id, next.to_string(), entry_id],
                )? != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
                let mut updated = vec![next.clone()];
                if repair_replies {
                    for mut child in read_transcript_entries_from_db(db)? {
                        if child.get("authorId").and_then(serde_json::Value::as_str) != author
                            || child.get("authorKind").and_then(serde_json::Value::as_str) != Some("human")
                            || child.get("remoteMessageId").is_some()
                            || !matches!(child.get("delivery").and_then(serde_json::Value::as_str), Some("pending" | "dispatching"))
                            || child.get("replyToId").and_then(serde_json::Value::as_str) != Some(entry_id)
                        {
                            continue;
                        }
                        let Some(child_id) = child.get("id").and_then(serde_json::Value::as_str).map(str::to_owned) else {
                            continue;
                        };
                        if !child.get("clientNonce").and_then(serde_json::Value::as_str)
                            .is_some_and(|nonce| !nonce.is_empty() && child_id == format!("human-message:{nonce}"))
                        {
                            continue;
                        }
                        child["replyToId"] = serde_json::json!(next_id);
                        if db.execute(UPDATE_TRANSCRIPT_ENTRY_SQL, params![child.to_string(), child_id])? != 1 {
                            return Err(rusqlite::Error::QueryReturnedNoRows);
                        }
                        updated.push(child);
                    }
                }
                Ok(updated)
            })();
            match result {
                Ok(updated) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    let changed = !updated.is_empty();
                    Ok((updated, changed))
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        let settled = updated.first().cloned();
        if settled.is_some() {
            if entry_id != next_id {
                let mut mutation = serde_json::Map::new();
                mutation.insert("kind".into(), serde_json::Value::String("entry-deleted".into()));
                mutation.insert("agentId".into(), serde_json::Value::String(self.agent_dir_name.clone()));
                mutation.insert("entryId".into(), serde_json::Value::String(entry_id.to_string()));
                publish_transcript_mutation(&mutation);
            }
            publish_entries_upserted(&self.db_path, updated);
        }
        Ok(settled)
    }

    /// Settle one transcript identity only while the captured session remains
    /// admitted. The existing row and replacement row must both retain the
    /// same session provenance inside the owner transaction.
    pub fn settle_transcript_entry_identity_if_session_admitted(
        &self,
        session_key: &str,
        entry_id: &str,
        next: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
        let session_key = session_key.trim();
        let entry_id = entry_id.trim();
        let Some(next_id) = next.get("id").and_then(serde_json::Value::as_str) else {
            return Ok(None);
        };
        if session_key.is_empty() || entry_id.is_empty() || next_id.trim().is_empty() {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session-scoped transcript identity settlement requires session key and entry identities"
                    .into(),
            ));
        }
        if next
            .get("sessionKey")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            != Some(session_key)
        {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "session-scoped transcript identity settlement requires matching entry provenance"
                    .into(),
            ));
        }
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_mut() else {
            return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                "agent database is closed".into(),
            ));
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<Option<serde_json::Value>, AgentDbProjectionError> {
            if !session_work_write_is_admitted(db)? {
                return Err(AgentDbProjectionError::WorkAdmissionInvalid(
                    "session owner identity is not admitted for transcript identity settlement".into(),
                ));
            }
            let raw_admission = db
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
                    "session is not admitted for transcript identity settlement".into(),
                ));
            }
            let raw_entry = db
                .query_row(GET_TRANSCRIPT_ENTRY_SQL, params![entry_id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()?;
            let Some(raw_entry) = raw_entry else {
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
            if db.execute(
                "UPDATE transcript_entries SET id = ?1, entry = ?2 WHERE id = ?3",
                params![next_id, next.to_string(), entry_id],
            )? != 1
            {
                return Ok(None);
            }
            Ok(Some(next.clone()))
        })();
        match result {
            Ok(updated) => {
                if let Err(error) = db.execute_batch("COMMIT") {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error.into());
                }
                drop(guard);
                if let Some(entry) = updated.as_ref() {
                    if entry_id != next_id {
                        let mut mutation = serde_json::Map::new();
                        mutation.insert(
                            "kind".into(),
                            serde_json::Value::String("entry-deleted".into()),
                        );
                        mutation.insert(
                            "agentId".into(),
                            serde_json::Value::String(self.agent_dir_name.clone()),
                        );
                        mutation.insert(
                            "entryId".into(),
                            serde_json::Value::String(entry_id.to_string()),
                        );
                        publish_transcript_mutation(&mutation);
                    }
                    publish_entries_upserted(&self.db_path, vec![entry.clone()]);
                }
                Ok(updated)
            }
            Err(error) => {
                let _ = db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn delete_transcript_entry(
        &self,
        entry_id: &str,
    ) -> Result<bool, AgentDbProjectionError> {
        let committed = self.run_write("deleteTranscriptEntry", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                db.execute(DELETE_TRANSCRIPT_ENTRY_SQL, params![entry_id])?;
                Ok(true)
            })();
            match result {
                Ok(changed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(changed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if committed {
            let mut mutation = serde_json::Map::new();
            mutation.insert(
                "kind".into(),
                serde_json::Value::String("entry-deleted".into()),
            );
            mutation.insert(
                "agentId".into(),
                serde_json::Value::String(self.agent_dir_name.clone()),
            );
            mutation.insert(
                "entryId".into(),
                serde_json::Value::String(entry_id.to_string()),
            );
            publish_transcript_mutation(&mutation);
        }
        Ok(committed)
    }

    pub fn get_human_transcript_entries(
        &self,
    ) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
        if self.is_closed() {
            return Err(AgentDbProjectionError::HumanTranscriptUnavailable);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Err(AgentDbProjectionError::HumanTranscriptUnavailable);
        };
        Ok(read_human_transcript_entries(db)?)
    }

    pub fn get_human_transcript_entries_for_topic(
        &self,
        topic_id: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, AgentDbProjectionError> {
        if self.is_closed() {
            return Err(AgentDbProjectionError::HumanTranscriptUnavailable);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Err(AgentDbProjectionError::HumanTranscriptUnavailable);
        };
        Ok(read_human_transcript_entries_for_topic(db, topic_id)?)
    }

    pub fn get_human_transcript_tail(
        &self,
        query: HumanTranscriptPageQuery,
    ) -> Result<HumanTranscriptPage, AgentDbProjectionError> {
        // Preparing the first page durably records its sequence index. Reuse
        // the same write/recovery owner and reject its closed/busy no-op result.
        let page = self.run_write_value("humanTranscriptTail", |db| {
            let metadata = read_metadata_json(db).map_err(|error| match error {
                AgentDbProjectionError::Sqlite(error) => error,
                error => rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    error.to_string(),
                ))),
            })?.unwrap_or(serde_json::Value::Null);
            read_human_transcript_tail(db, &query, &metadata)
                .map(|(page, changed)| (Some(page), changed))
        })?;
        page.ok_or(AgentDbProjectionError::HumanTranscriptUnavailable)
    }

    pub fn clear_conversation(&self) -> Result<bool, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(false);
        }
        let raw_metadata = self.read_kv("metadata")?;
        let next_metadata = if let Some(raw) = raw_metadata.as_deref() {
            let bytes = decode_hex(raw).map_err(AgentDbProjectionError::MetadataHex)?;
            let mut metadata: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(AgentDbProjectionError::MetadataJson)?;
            if let Some(object) = metadata.as_object_mut() {
                object.insert(
                    "latestRootBlobId".into(),
                    serde_json::Value::String(String::new()),
                );
                object.insert(
                    "currentPlanUri".into(),
                    serde_json::Value::String(String::new()),
                );
                object.insert(
                    "lifecycleRevision".into(),
                    serde_json::json!(Uuid::new_v4().to_string()),
                );
            }
            Some(encode_hex(&serde_json::to_vec(&metadata)?))
        } else {
            None
        };

        let committed = self.run_write("clearConversation", |db| {
            db.execute_batch("BEGIN IMMEDIATE")?;
            let result = (|| -> Result<bool, rusqlite::Error> {
                if !session_work_write_is_admitted(db)? {
                    return Ok(false);
                }
                db.execute(CLEAR_BLOBS_SQL, [])?;
                db.execute(CLEAR_TRANSCRIPT_ENTRIES_SQL, [])?;
                for key in [
                    KV_AWAITING,
                    KV_LATEST_REQUEST_ID,
                    KV_REQUEST_IDS,
                    KV_EPISODE,
                    KV_MEMORY_SNAPSHOT,
                    KV_PROFILE_SNAPSHOT,
                ] {
                    db.execute(DELETE_KV_SQL, params![key])?;
                }
                if let Some(next_metadata) = next_metadata.as_deref() {
                    db.execute(SET_KV_SQL, params!["metadata", next_metadata])?;
                }
                Ok(true)
            })();
            match result {
                Ok(committed) => {
                    if let Err(error) = db.execute_batch("COMMIT") {
                        let _ = db.execute_batch("ROLLBACK");
                        return Err(error);
                    }
                    Ok(committed)
                }
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    Err(error)
                }
            }
        })?;
        if !committed {
            return Ok(false);
        }

        let mut mutation = serde_json::Map::new();
        mutation.insert(
            "kind".into(),
            serde_json::Value::String("conversation-cleared".into()),
        );
        mutation.insert(
            "agentId".into(),
            serde_json::Value::String(self.agent_dir_name.clone()),
        );
        publish_transcript_mutation(&mutation);
        notify_agent_db_listeners(&self.db_path, AgentDbListenerChannel::Awaiting);
        notify_agent_db_listeners(
            &self.db_path,
            AgentDbListenerChannel::Metadata("latestRootBlobId".into()),
        );
        Ok(true)
    }

    pub(crate) fn run_write<F>(
        &self,
        operation: &str,
        mut write: F,
    ) -> Result<bool, AgentDbProjectionError>
    where
        F: FnMut(&rusqlite::Connection) -> Result<bool, rusqlite::Error>,
    {
        self.run_write_value(operation, |db| {
            write(db).map(|changed| (changed, changed))
        })
    }

    pub(crate) fn run_write_value<T, F>(
        &self,
        operation: &str,
        mut write: F,
    ) -> Result<T, AgentDbProjectionError>
    where
        T: Default,
        F: FnMut(&rusqlite::Connection) -> Result<(T, bool), rusqlite::Error>,
    {
        if self.is_closed() {
            return Ok(T::default());
        }
        let mut guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(T::default());
        };
        match write(db) {
            Ok((value, changed)) => {
                if changed {
                    bump_db_write_generation(&self.db_path);
                }
                return Ok(value);
            }
            Err(error) if is_sqlite_busy_error(&error) => {
                let message = error.to_string();
                drop(guard);
                if let Some(callback) = self.options.on_busy_error.as_ref() {
                    callback(operation, &message);
                }
                return Ok(T::default());
            }
            Err(error)
                if self.options.recovery.recover_on_corruption
                    && !self.recovered.load(Ordering::Acquire)
                    && is_sqlite_corrupt_error(&error) =>
            {
                let own_handle = usize::from(
                    self.handle_registered.load(Ordering::Acquire),
                );
                if live_db_handle_count(&self.db_path).saturating_sub(own_handle) > 0 {
                    return Err(error.into());
                }
                let cause = error.to_string();
                let old = guard.take();
                drop(old);
                match recover_corrupt_store_db(
                    &self.db_path,
                    &self.agent_dir_name,
                    &self.options.recovery,
                    &cause,
                ) {
                    Ok(recovered) => {
                        *guard = Some(recovered);
                        self.recovered.store(true, Ordering::Release);
                    }
                    Err(recovery_error) => {
                        self.closed.store(true, Ordering::Release);
                        if self
                            .handle_registered
                            .swap(false, Ordering::AcqRel)
                        {
                            release_live_db_handle(&self.db_path);
                        }
                        return Err(recovery_error.into());
                    }
                }
            }
            Err(error) => return Err(error.into()),
        }

        let Some(db) = guard.as_ref() else {
            return Ok(T::default());
        };
        match write(db) {
            Ok((value, changed)) => {
                if changed {
                    bump_db_write_generation(&self.db_path);
                }
                Ok(value)
            }
            Err(error) if is_sqlite_busy_error(&error) => {
                let message = error.to_string();
                drop(guard);
                if let Some(callback) = self.options.on_busy_error.as_ref() {
                    callback(operation, &message);
                }
                Ok(T::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn upsert_conversation_topic(
        &self,
        topic: &ForumTopicState,
    ) -> Result<(), AgentDbProjectionError> {
        self.run_write_value("upsertConversationTopic", |db| {
            let changed = db.execute(
                "INSERT INTO conversation_topics (
                    topic_id, title, icon, creator_id, created_at_ms,
                    pinned, closed, hidden, last_message_id, unread_count
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(topic_id) DO UPDATE SET
                    title = excluded.title,
                    icon = excluded.icon,
                    creator_id = excluded.creator_id,
                    created_at_ms = excluded.created_at_ms,
                    pinned = excluded.pinned,
                    closed = excluded.closed,
                    hidden = excluded.hidden,
                    last_message_id = excluded.last_message_id,
                    unread_count = excluded.unread_count",
                params![
                    topic.id,
                    topic.title,
                    topic.icon,
                    topic.creator_id.0,
                    topic.created_at_ms,
                    if topic.pinned { 1i64 } else { 0i64 },
                    if topic.closed { 1i64 } else { 0i64 },
                    if topic.hidden { 1i64 } else { 0i64 },
                    topic.last_message_id,
                    topic.unread_count as i64,
                ],
            )? > 0;
            Ok(((), changed))
        })
    }

    pub fn get_conversation_topic(
        &self,
        topic_id: &str,
    ) -> Result<Option<ForumTopicState>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(None);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(None);
        };
        let mut statement = db.prepare(
            "SELECT topic_id, title, icon, creator_id, created_at_ms, pinned, closed, hidden, last_message_id, unread_count
             FROM conversation_topics WHERE topic_id = ?1",
        )?;
        let result = statement.query_row(params![topic_id], |row| {
            let topic_id = row.get::<_, String>(0)?;
            let title = row.get::<_, String>(1)?;
            let icon = row.get::<_, Option<String>>(2)?;
            let creator_id = row.get::<_, String>(3)?;
            let created_at_ms = row.get::<_, i64>(4)?;
            let pinned = row.get::<_, i64>(5)? != 0;
            let closed = row.get::<_, i64>(6)? != 0;
            let hidden = row.get::<_, i64>(7)? != 0;
            let last_message_id = row.get::<_, Option<String>>(8)?;
            let unread_count = row.get::<_, i64>(9)? as u32;
            Ok(ForumTopicState {
                id: topic_id,
                conversation_id: ConversationId::new(&self.agent_dir_name),
                title,
                icon,
                creator_id: ActorId::new(creator_id),
                created_at_ms,
                pinned,
                closed,
                hidden,
                unread_count,
                last_message_id,
            })
        }).optional()?;
        Ok(result)
    }

    pub fn list_conversation_topics(
        &self,
    ) -> Result<Vec<ForumTopicState>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(Vec::new());
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let mut statement = db.prepare(
            "SELECT topic_id, title, icon, creator_id, created_at_ms, pinned, closed, hidden, last_message_id, unread_count
             FROM conversation_topics
             ORDER BY pinned DESC, created_at_ms DESC, topic_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let topic_id = row.get::<_, String>(0)?;
            let title = row.get::<_, String>(1)?;
            let icon = row.get::<_, Option<String>>(2)?;
            let creator_id = row.get::<_, String>(3)?;
            let created_at_ms = row.get::<_, i64>(4)?;
            let pinned = row.get::<_, i64>(5)? != 0;
            let closed = row.get::<_, i64>(6)? != 0;
            let hidden = row.get::<_, i64>(7)? != 0;
            let last_message_id = row.get::<_, Option<String>>(8)?;
            let unread_count = row.get::<_, i64>(9)? as u32;
            Ok(ForumTopicState {
                id: topic_id,
                conversation_id: ConversationId::new(&self.agent_dir_name),
                title,
                icon,
                creator_id: ActorId::new(creator_id),
                created_at_ms,
                pinned,
                closed,
                hidden,
                unread_count,
                last_message_id,
            })
        })?;
        let mut topics = Vec::new();
        for row in rows {
            topics.push(row?);
        }
        Ok(topics)
    }

    pub fn upsert_conversation_member(
        &self,
        member: &CommunityMember,
        updated_at_ms: i64,
    ) -> Result<(), AgentDbProjectionError> {
        let admin_rights_json = serde_json::to_string(&member.admin_rights).ok();
        let restrictions_json = serde_json::to_string(&member.restrictions).ok();
        self.run_write_value("upsertConversationMember", |db| {
            let changed = db.execute(
                "INSERT INTO conversation_members (
                    actor_id, status, admin_title, admin_rights,
                    restrictions, joined_at_ms, invited_by, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(actor_id) DO UPDATE SET
                    status = excluded.status,
                    admin_title = excluded.admin_title,
                    admin_rights = excluded.admin_rights,
                    restrictions = excluded.restrictions,
                    joined_at_ms = excluded.joined_at_ms,
                    invited_by = excluded.invited_by,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    member.actor_id.0,
                    member_status_to_str(member.status),
                    member.admin_title,
                    admin_rights_json,
                    restrictions_json,
                    member.joined_at_ms,
                    member.invited_by.as_ref().map(|id| id.0.clone()),
                    updated_at_ms,
                ],
            )? > 0;
            Ok(((), changed))
        })
    }

    pub fn get_conversation_member(
        &self,
        actor_id: &str,
    ) -> Result<Option<CommunityMember>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(None);
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(None);
        };
        let mut statement = db.prepare(
            "SELECT actor_id, status, admin_title, admin_rights, restrictions, joined_at_ms, invited_by
             FROM conversation_members WHERE actor_id = ?1",
        )?;
        let result = statement.query_row(params![actor_id], |row| {
            let actor_id = row.get::<_, String>(0)?;
            let status_str = row.get::<_, String>(1)?;
            let status = member_status_from_str(&status_str)?;
            let admin_title = row.get::<_, Option<String>>(2)?;
            let admin_rights_str = row.get::<_, Option<String>>(3)?;
            let admin_rights: AdminRights = admin_rights_str
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let restrictions_str = row.get::<_, Option<String>>(4)?;
            let restrictions: MemberRestrictions = restrictions_str
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let joined_at_ms = row.get::<_, i64>(5)?;
            let invited_by = row.get::<_, Option<String>>(6)?.map(ActorId::new);

            Ok(CommunityMember {
                actor_id: ActorId::new(actor_id),
                status,
                admin_title,
                admin_rights,
                restrictions,
                joined_at_ms,
                invited_by,
            })
        }).optional()?;
        Ok(result)
    }

    pub fn list_conversation_members(
        &self,
    ) -> Result<Vec<CommunityMember>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(Vec::new());
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let mut statement = db.prepare(
            "SELECT actor_id, status, admin_title, admin_rights, restrictions, joined_at_ms, invited_by
             FROM conversation_members
             ORDER BY joined_at_ms ASC, actor_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            let actor_id = row.get::<_, String>(0)?;
            let status_str = row.get::<_, String>(1)?;
            let status = member_status_from_str(&status_str)?;
            let admin_title = row.get::<_, Option<String>>(2)?;
            let admin_rights_str = row.get::<_, Option<String>>(3)?;
            let admin_rights: AdminRights = admin_rights_str
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let restrictions_str = row.get::<_, Option<String>>(4)?;
            let restrictions: MemberRestrictions = restrictions_str
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let joined_at_ms = row.get::<_, i64>(5)?;
            let invited_by = row.get::<_, Option<String>>(6)?.map(ActorId::new);

            Ok(CommunityMember {
                actor_id: ActorId::new(actor_id),
                status,
                admin_title,
                admin_rights,
                restrictions,
                joined_at_ms,
                invited_by,
            })
        })?;
        let mut members = Vec::new();
        for row in rows {
            members.push(row?);
        }
        Ok(members)
    }

    pub fn pin_conversation_message(
        &self,
        child_scope: &str,
        message_id: &str,
        pinned_at_ms: i64,
        pinned_by: &str,
    ) -> Result<(), AgentDbProjectionError> {
        self.run_write_value("pinConversationMessage", |db| {
            let changed = db.execute(
                "INSERT INTO conversation_pins (child_scope, message_id, pinned_at_ms, pinned_by)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(child_scope, message_id) DO UPDATE SET
                    pinned_at_ms = excluded.pinned_at_ms,
                    pinned_by = excluded.pinned_by",
                params![child_scope, message_id, pinned_at_ms, pinned_by],
            )? > 0;
            Ok(((), changed))
        })
    }

    pub fn unpin_conversation_message(
        &self,
        child_scope: &str,
        message_id: &str,
    ) -> Result<(), AgentDbProjectionError> {
        self.run_write_value("unpinConversationMessage", |db| {
            let changed = db.execute(
                "DELETE FROM conversation_pins WHERE child_scope = ?1 AND message_id = ?2",
                params![child_scope, message_id],
            )? > 0;
            Ok(((), changed))
        })
    }

    pub fn list_conversation_pins(
        &self,
        child_scope: &str,
    ) -> Result<Vec<String>, AgentDbProjectionError> {
        if self.is_closed() {
            return Ok(Vec::new());
        }
        let guard = self
            .connection
            .lock()
            .map_err(|_| AgentDbProjectionError::OwnerPoisoned)?;
        let Some(db) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        let mut statement = db.prepare(
            "SELECT message_id FROM conversation_pins WHERE child_scope = ?1 ORDER BY pinned_at_ms DESC",
        )?;
        let rows = statement.query_map(params![child_scope], |row| row.get::<_, String>(0))?;
        let mut pins = Vec::new();
        for row in rows {
            pins.push(row?);
        }
        Ok(pins)
    }

}

/// Check owner-local metadata writes at the same SQLite transaction edge as
/// the write. Legacy stores without an admission record retain compatibility;
/// malformed, archived, or deleting records fail closed.
pub(crate) fn session_work_write_is_admitted(
    db: &rusqlite::Connection,
) -> Result<bool, rusqlite::Error> {
    let Some(raw) = read_kv(db, KV_SESSION_WORK_ADMISSION)? else {
        return Ok(true);
    };
    let Ok(admission) = serde_json::from_str::<SessionWorkAdmissionRecord>(&raw) else {
        return Ok(false);
    };
    if !session_admission_identity_is_bound(db, &admission)? {
        return Ok(false);
    }
    Ok(!admission.deletion_pending && admission.archived_at_ms.is_none())
}

pub(crate) fn session_admission_identity_is_bound(
    db: &rusqlite::Connection,
    admission: &SessionWorkAdmissionRecord,
) -> Result<bool, rusqlite::Error> {
    if !session_admission_identity_is_well_formed(admission) {
        return Ok(false);
    }
    let Some(raw_metadata) = read_kv(db, "metadata")? else {
        return Ok(false);
    };
    let Ok(metadata_bytes) = decode_hex(&raw_metadata) else {
        return Ok(false);
    };
    let Ok(metadata) = serde_json::from_slice::<serde_json::Value>(&metadata_bytes) else {
        return Ok(false);
    };
    let Some(agent_id) = metadata
        .get("agentId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let expected_session_key = format!("agent:{agent_id}:main");
    Ok(admission.session_key == expected_session_key)
}

pub(crate) fn session_admission_identity_is_well_formed(
    admission: &SessionWorkAdmissionRecord,
) -> bool {
    !admission.session_key.trim().is_empty()
        && !admission.session_id.trim().is_empty()
        && !admission.lifecycle_revision.trim().is_empty()
}

pub use super::ocrm_agent_db_persisted_metadata::*;
pub use super::ocrm_agent_db_persisted_ops::*;
pub use super::ocrm_agent_db_transcript_queries::*;

fn delete_persisted_kv(
    db_path: &Path,
    busy_timeout_ms: u64,
    key: &str,
) -> Result<bool, AgentDbProjectionError> {
    let db = open_projection_db(db_path, busy_timeout_ms)?;
    let wrote = db.execute(DELETE_KV_SQL, params![key])? > 0;
    if wrote {
        bump_db_write_generation(db_path);
    }
    Ok(wrote)
}

fn read_transcript_entries_from_db(
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

fn read_transcript_entry_from_db(
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

fn read_branched_entries_from_db(
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

fn branch_reply_counts_from_entries(
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

fn thread_descendants_from_entries(
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

fn resolve_branch_root_from_entries(
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

fn main_transcript_entries_from_entries(
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

fn is_main_branched_entry(entry: &serde_json::Value) -> bool {
    matches!(
        entry.get("kind").and_then(serde_json::Value::as_str),
        Some("message" | "send-message" | "user-attachment" | "notice")
    ) && entry.get("branched").and_then(serde_json::Value::as_bool) == Some(true)
}

fn main_entry_reply_to(entry: &serde_json::Value) -> Option<&str> {
    if !matches!(
        entry.get("kind").and_then(serde_json::Value::as_str),
        Some("message" | "send-message" | "user-attachment" | "notice")
    ) {
        return None;
    }
    entry.get("replyTo").and_then(serde_json::Value::as_str)
}

fn resolve_main_branch_root(
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

fn publish_entries_upserted(db_path: &Path, entries: Vec<serde_json::Value>) {
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
    if let Some(origin) = transcript_mutation_origin(db_path) {
        mutation.insert(TRANSCRIPT_MUTATION_ORIGIN_KEY.into(), serde_json::Value::String(origin));
    }
    publish_transcript_mutation(&mutation);
}

fn agent_id_for_db_path(db_path: &Path) -> String {
    db_path
        .parent()
        .and_then(Path::file_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn json_number(value: f64) -> serde_json::Value {
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}

pub(crate) fn read_kv(
    db: &rusqlite::Connection,
    key: &str,
) -> Result<Option<String>, rusqlite::Error> {
    db.query_row(GET_KV_SQL, params![key], |row| row.get::<_, String>(0))
        .optional()
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn decode_hex(raw: &str) -> Result<Vec<u8>, String> {
    let clean = raw.trim();
    if clean.len() % 2 != 0 {
        return Err("odd-length hex string".into());
    }
    let mut output = Vec::with_capacity(clean.len() / 2);
    for index in (0..clean.len()).step_by(2) {
        let pair = &clean[index..index + 2];
        let byte = u8::from_str_radix(pair, 16)
            .map_err(|_| format!("invalid hex pair at offset {index}: {pair}"))?;
        output.push(byte);
    }
    Ok(output)
}
