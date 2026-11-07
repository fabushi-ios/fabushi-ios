use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::agents::agent_profile::SandAgentProfile;
use crate::storage::store_db::delete_sand_agent_db_write_generation;
use crate::transcript_mutation_events::publish_transcript_mutation;
use crate::extensions::memory::memory_service::FileMemoryStore;
use crate::extensions::inference::model_picker_service::{
    ApplySessionModelSelectionParams, ApplySessionModelSelectionResult, SessionModelEntryState,
};

use super::agent_db_serde::AwaitingUserResponse;
use super::agent_db_transcript_pages::{
    TranscriptPage, TranscriptPageQuery, TranscriptWindow, TranscriptWindowQuery,
};
use super::session_conversation_state::{
    ConversationOutlineItem, PendingConversationTurnRegistry, TranscriptThread,
};
use super::channel_store::{ChannelConfig, ChannelConnection, FileChannelStore};
use super::production::{PreparedAgentBlobStore, ProductionSessionWorkers};
use super::session_materialization::MaterializedAgentRecord;
use super::session_paths::ACTIVE_AGENT_FILENAME;
use super::session_profile_files::{
    AgentAvatarResponse, AgentProfileUpdate, resolve_profile_name as resolve_profile_name_impl,
};
use super::session_summaries::AgentSummary;

const SESSION_MODEL_SELECTION_KV: &str = "sessionModelSelection";
const SESSION_COMMUNICATION_POLICY_KV: &str = "sessionCommunicationPolicy";

/// Read the authoritative per-session model selection from the session's
/// SQLite KV owner. The record is deliberately scoped by agent id; malformed
/// or cross-session state is rejected instead of being applied to another
/// session.
pub fn read_session_model_entry(
    db: &crate::extensions::session::agent_db::SandAgentDb,
    agent_id: &str,
) -> Result<SessionModelEntryState, String> {
    let Some(raw) = db
        .read_kv(SESSION_MODEL_SELECTION_KV)
        .map_err(|error| error.to_string())?
    else {
        return Ok(SessionModelEntryState {
            session_id: agent_id.to_string(),
            ..SessionModelEntryState::default()
        });
    };
    let state = serde_json::from_str::<SessionModelEntryState>(&raw)
        .map_err(|error| format!("invalid session model selection: {error}"))?;
    if state.session_id != agent_id {
        return Err(format!(
            "session model selection belongs to {}, not {agent_id}",
            state.session_id
        ));
    }
    Ok(state)
}

/// Persist a validated session model selection without touching agent-level
/// settings or defaults.
pub fn write_session_model_entry(
    db: &crate::extensions::session::agent_db::SandAgentDb,
    agent_id: &str,
    expected_lifecycle_revision: &str,
    state: &SessionModelEntryState,
) -> Result<bool, String> {
    if state.session_id != agent_id {
        return Err(format!(
            "cannot persist session model selection for {}, expected {agent_id}",
            state.session_id
        ));
    }
    let raw = serde_json::to_string(state)
        .map_err(|error| format!("serialize session model selection: {error}"))?;
    db.write_session_kv_if_generation(
        SESSION_MODEL_SELECTION_KV,
        &raw,
        agent_id,
        expected_lifecycle_revision,
    )
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionCommunicationMode {
    Always,
    Ask,
    Never,
}

impl SessionCommunicationMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "always" => Some(Self::Always),
            "ask" => Some(Self::Ask),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct SessionCommunicationPolicyOverride {
    pub send: Option<SessionCommunicationMode>,
    pub receive: Option<SessionCommunicationMode>,
}

/// Read the optional communication policy through the existing agent DB owner.
/// Missing state is intentionally distinct from malformed state: callers may
/// apply the documented compatibility default only to the former.
pub fn read_session_communication_policy(
    db: &crate::extensions::session::agent_db::SandAgentDb,
    agent_id: &str,
) -> Result<Option<SessionCommunicationPolicyOverride>, String> {
    let Some(raw) = db
        .read_kv(SESSION_COMMUNICATION_POLICY_KV)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    serde_json::from_str::<SessionCommunicationPolicyOverride>(&raw)
        .map(Some)
        .map_err(|error| format!("invalid session communication policy for {agent_id}: {error}"))
}

pub fn write_session_communication_policy(
    db: &crate::extensions::session::agent_db::SandAgentDb,
    expected_session_id: &str,
    expected_lifecycle_revision: &str,
    policy: &SessionCommunicationPolicyOverride,
) -> Result<bool, String> {
    let raw = serde_json::to_string(policy)
        .map_err(|error| format!("serialize session communication policy: {error}"))?;
    db.write_session_kv_if_generation(
        SESSION_COMMUNICATION_POLICY_KV,
        &raw,
        expected_session_id,
        expected_lifecycle_revision,
    )
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveSessionCommunicationPolicy {
    pub send: SessionCommunicationMode,
    pub receive: SessionCommunicationMode,
}

pub fn resolve_session_communication_policy(
    defaults: Option<SessionCommunicationPolicyOverride>,
    entry: Option<SessionCommunicationPolicyOverride>,
) -> EffectiveSessionCommunicationPolicy {
    let defaults = defaults.unwrap_or_default();
    let entry = entry.unwrap_or_default();
    EffectiveSessionCommunicationPolicy {
        send: entry
            .send
            .or(defaults.send)
            .unwrap_or(SessionCommunicationMode::Always),
        receive: entry
            .receive
            .or(defaults.receive)
            .unwrap_or(SessionCommunicationMode::Always),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunicationEndpoint {
    pub agent_id: String,
    pub session_key: String,
    pub store_path: String,
    pub session_id: Option<String>,
    pub communication: Option<SessionCommunicationPolicyOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommunicationApproval {
    pub direction: SessionCommunicationDirection,
    pub endpoint: CommunicationEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCommunicationDirection {
    Send,
    Receive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCommunicationDecision {
    Allowed { approvals: Vec<CommunicationApproval> },
    Denied { error: String },
}

fn is_same_session(
    source: Option<&CommunicationEndpoint>,
    target: Option<&CommunicationEndpoint>,
) -> bool {
    let (Some(source), Some(target)) = (source, target) else {
        return false;
    };
    source.session_id.is_some()
        && source.session_id == target.session_id
        && source.agent_id == target.agent_id
        && source.session_key == target.session_key
        && source.store_path == target.store_path
}

/// Plan peer communication admission without bypassing the existing session owner.
pub fn plan_session_communication(
    defaults: Option<SessionCommunicationPolicyOverride>,
    source: &[CommunicationEndpoint],
    target: &[CommunicationEndpoint],
    owned_task: bool,
) -> SessionCommunicationDecision {
    if owned_task || is_same_session(source.first(), target.first()) {
        return SessionCommunicationDecision::Allowed { approvals: Vec::new() };
    }

    let mut approvals = Vec::new();
    for (direction, endpoints) in [
        (SessionCommunicationDirection::Send, source),
        (SessionCommunicationDirection::Receive, target),
    ] {
        for endpoint in endpoints {
            let policy = resolve_session_communication_policy(defaults, endpoint.communication);
            let mode = match direction {
                SessionCommunicationDirection::Send => policy.send,
                SessionCommunicationDirection::Receive => policy.receive,
            };
            match mode {
                SessionCommunicationMode::Always => {}
                SessionCommunicationMode::Ask => approvals.push(CommunicationApproval {
                    direction,
                    endpoint: endpoint.clone(),
                }),
                SessionCommunicationMode::Never => {
                    let direction_name = match direction {
                        SessionCommunicationDirection::Send => "send",
                        SessionCommunicationDirection::Receive => "receive",
                    };
                    return SessionCommunicationDecision::Denied {
                        error: format!(
                            "Session communication {direction_name} is disabled for {}.",
                            endpoint.session_key
                        ),
                    };
                }
            }
        }
    }
    SessionCommunicationDecision::Allowed { approvals }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionDbStat {
    pub size: u64,
    pub mtime_ms: f64,
}

pub fn resolve_profile_name(
    trimmed_name: &str,
    current: Option<&SandAgentProfile>,
) -> String {
    resolve_profile_name_impl(trimmed_name, current)
}

pub use super::ocrm_agent_session_automations::*;

pub struct SandAgentSessionStore {
    pub(crate) production: Arc<ProductionSessionWorkers>,
}

impl SandAgentSessionStore {
    pub fn new(production: Arc<ProductionSessionWorkers>) -> Self {
        Self { production }
    }

    pub fn production(&self) -> &Arc<ProductionSessionWorkers> {
        &self.production
    }

    /// Apply and persist a session-scoped model selection through the existing
    /// model-picker policy owner. Only an `Applied` result is persisted; policy
    /// rejection and generation conflicts leave the previous KV state intact.
    pub fn apply_session_model_selection(
        &self,
        agent_id: &str,
        params: ApplySessionModelSelectionParams<'_>,
        current_time_ms: u64,
    ) -> Result<ApplySessionModelSelectionResult, String> {
        self.production
            .apply_session_model_selection(agent_id, params, current_time_ms)
    }

    pub fn read_session_model_entry(
        &self,
        agent_id: &str,
    ) -> Result<SessionModelEntryState, String> {
        let db = self.production.open_agent_db_owner(agent_id)?;
        read_session_model_entry(&db, agent_id)
    }

    pub fn begin_cron_session_run(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        started_at_ms: f64,
    ) -> Result<bool, String> {
        self.production
            .begin_cron_session_run(agent_id, automation_id, run_id, started_at_ms)
    }

    pub fn finish_cron_session_run(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        outcome: &str,
        finished_at_ms: f64,
        detail: Option<&str>,
    ) -> Result<bool, String> {
        self.production.finish_cron_session_run(
            agent_id,
            automation_id,
            run_id,
            outcome,
            finished_at_ms,
            detail,
        )
    }

    pub fn finish_cron_session_run_with_fences(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        outcome: &str,
        finished_at_ms: f64,
        detail: Option<&str>,
        active_controller: bool,
        pending_outgoing_media: bool,
        unsettled_subagent: bool,
    ) -> Result<bool, String> {
        self.production.finish_cron_session_run_with_fences(
            agent_id,
            automation_id,
            run_id,
            outcome,
            finished_at_ms,
            detail,
            active_controller,
            pending_outgoing_media,
            unsettled_subagent,
        )
    }

    pub fn publish_cron_session_run_fences(
        &self,
        agent_id: &str,
        automation_id: &str,
        run_id: &str,
        active_controller: bool,
        pending_outgoing_media: bool,
        unsettled_subagent: bool,
    ) -> Result<bool, String> {
        self.production.publish_cron_session_run_fences(
            agent_id,
            automation_id,
            run_id,
            active_controller,
            pending_outgoing_media,
            unsettled_subagent,
        )
    }

    pub fn list_cron_session_runs(
        &self,
        agent_id: &str,
    ) -> Result<Vec<crate::extensions::session::agent_db::CronSessionRunRecord>, String> {
        self.production.list_cron_session_runs(agent_id)
    }

    pub fn list_expired_cron_session_runs(
        &self,
        agent_id: &str,
        now_ms: f64,
        retention_ms: Option<u64>,
    ) -> Result<Vec<crate::extensions::session::agent_db::CronSessionRunRecord>, String> {
        self.production
            .list_expired_cron_session_runs(agent_id, now_ms, retention_ms)
    }

    pub fn pending_conversation_turns(&self) -> PendingConversationTurnRegistry {
        self.production.pending_conversation_turns()
    }

    pub fn create_memory_store(&self, agent_dir: impl AsRef<Path>) -> FileMemoryStore {
        self.production.memory_service().create_agent_store(agent_dir)
    }

    pub fn agent_has_content(&self, agent_dir: impl AsRef<Path>) -> bool {
        self.production.memory_service().agent_has_content(agent_dir)
    }

    pub fn get_user_time_zone(&self) -> Option<String> {
        self.production.resolve_user_time_zone()
    }

    pub fn get_root_dir(&self) -> &Path {
        self.production.agents_root()
    }

    pub fn get_agent_dir(&self, agent_id: &str) -> PathBuf {
        self.get_root_dir().join(agent_id)
    }

    pub fn agent_exists(&self, agent_id: &str) -> bool {
        self.production
            .session_db_path(agent_id)
            .map(|path| path.is_file())
            .unwrap_or(false)
    }

    pub fn agent_dir_exists(&self, agent_id: &str) -> bool {
        self.get_agent_dir(agent_id).is_dir()
    }

    pub fn active_agent_pointer_path(&self) -> PathBuf {
        self.get_root_dir().join(ACTIVE_AGENT_FILENAME)
    }

    pub fn read_active_agent_id(&self) -> Option<String> {
        let raw = fs::read_to_string(self.active_agent_pointer_path()).ok()?;
        let parsed = serde_json::from_str::<Value>(&raw).ok()?;
        parsed
            .get("activeAgentId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }

    pub fn write_active_agent_id(&self, agent_id: &str) -> io::Result<()> {
        fs::create_dir_all(self.get_root_dir())?;
        let path = self.active_agent_pointer_path();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = PathBuf::from(format!(
            "{}.{}.{}.tmp",
            path.display(),
            process::id(),
            nanos
        ));
        fs::write(
            &temporary,
            serde_json::to_vec(&serde_json::json!({ "activeAgentId": agent_id }))
                .map_err(io::Error::other)?,
        )?;
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    pub fn clear_active_agent_id(&self) -> io::Result<()> {
        match fs::remove_file(self.active_agent_pointer_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub fn list_agent_record_ids(&self) -> Result<Vec<String>, String> {
        self.production.list_agent_record_ids()
    }

    pub fn count_owned_agents(&self) -> Result<usize, String> {
        self.production.count_owned_agents()
    }

    pub fn list_agent_ids(&self) -> Result<Vec<String>, String> {
        self.production.list_agent_record_ids()
    }

    pub fn is_agent_cap_reached(&self) -> Result<bool, String> {
        self.production.is_agent_cap_reached()
    }

    pub fn mint_agent<T>(
        &self,
        mint: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<T, String> {
        self.production.mint_agent_with(mint)
    }

    pub fn mint_agent_with<T>(
        &self,
        mint: impl FnOnce(&str) -> Result<T, String>,
    ) -> Result<T, String> {
        self.mint_agent(mint)
    }

    pub fn write_agent_profile_file(
        &self,
        agent_id: &str,
        update: &AgentProfileUpdate,
    ) -> Result<(), String> {
        let active = self.read_active_agent_id();
        self.production
            .update_agent_profile(agent_id, update, active.as_deref())
            .map(|_| ())
    }

    pub fn create_session(
        &self,
        profile: Option<&SandAgentProfile>,
        origin: &str,
        purpose: Option<&str>,
    ) -> Result<MaterializedAgentRecord, String> {
        let active = self.read_active_agent_id();
        self.production
            .materialize_session_with_active(
                profile,
                origin,
                purpose,
                active.as_deref(),
            )
            .map(|session| session.record)
    }

    pub fn create_fallback_session(
        &self,
    ) -> Result<super::production::FallbackSession, String> {
        let active = self.read_active_agent_id();
        self.production.create_fallback_session(active.as_deref())
    }

    pub fn open_session(
        &self,
        agent_id: &str,
    ) -> Result<Option<PreparedAgentBlobStore>, String> {
        self.production
            .open_materialized_session(agent_id)
            .map(|session| session.map(|session| session.prepared))
    }

    pub fn delete_session(&self, agent_id: &str) -> Result<(), String> {
        self.production.begin_agent_delete(agent_id)?;
        let result = self.production.with_agent_filesystem_mutation(agent_id, || {
            let db_path = self.production.session_db_path(agent_id)?;
            self.production.retire_agent_owners_locked(agent_id, false)?;
            delete_sand_agent_db_write_generation(&db_path);
            match fs::remove_dir_all(self.get_agent_dir(agent_id)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
            let mutation = Map::from_iter([
                ("kind".to_string(), Value::String("agent-removed".to_string())),
                ("agentId".to_string(), Value::String(agent_id.to_string())),
            ]);
            publish_transcript_mutation(&mutation);
            Ok(())
        });
        match result {
            Err(error) => {
                self.production.clear_agent_delete_fast_path(agent_id);
                Err(error)
            }
            Ok(()) => self.production.end_agent_delete(agent_id),
        }
    }

    pub fn list_agents(&self) -> Result<Vec<AgentSummary>, String> {
        let active = self.read_active_agent_id();
        self.production.list_agent_summaries(active.as_deref())
    }

    pub fn summarize_agent_by_id(
        &self,
        agent_id: &str,
    ) -> Result<Option<AgentSummary>, String> {
        let active = self.read_active_agent_id();
        self.production
            .summarize_agent_by_id(agent_id, active.as_deref())
    }

    pub fn summarize_open_session(
        &self,
        session: &PreparedAgentBlobStore,
    ) -> Result<Option<AgentSummary>, String> {
        self.production
            .summarize_agent_by_id(&session.agent_id, Some(&session.agent_id))
    }

    pub fn summarize_session(
        &self,
        session: &PreparedAgentBlobStore,
        active_agent_id: Option<&str>,
    ) -> Result<Option<AgentSummary>, String> {
        self.production
            .summarize_agent_by_id(&session.agent_id, active_agent_id)
    }

    pub fn update_agent_profile(
        &self,
        agent_id: &str,
        update: &AgentProfileUpdate,
    ) -> Result<Option<AgentSummary>, String> {
        let active = self.read_active_agent_id();
        self.production
            .update_agent_profile(agent_id, update, active.as_deref())
    }

    pub fn get_agent_profile_text(
        &self,
        agent_id: &str,
    ) -> Result<Option<SandAgentProfile>, String> {
        self.production.get_agent_profile_text(agent_id)
    }

    pub fn get_agent_avatar(&self, agent_id: &str) -> Result<AgentAvatarResponse, String> {
        self.production.get_agent_avatar(agent_id)
    }

    pub fn get_agent_avatar_png(&self, agent_id: &str) -> Result<Option<Vec<u8>>, String> {
        self.production.get_agent_avatar_png(agent_id)
    }

    pub fn set_agent_avatar_bytes(
        &self,
        agent_id: &str,
        png_bytes: Option<&[u8]>,
    ) -> Result<Option<AgentSummary>, String> {
        let active = self.read_active_agent_id();
        self.production
            .set_agent_avatar_bytes(agent_id, png_bytes, active.as_deref())
    }

    pub fn set_agent_avatar_bytes_by_id(
        &self,
        agent_id: &str,
        png_bytes: Option<&[u8]>,
    ) -> Result<Option<AgentSummary>, String> {
        self.set_agent_avatar_bytes(agent_id, png_bytes)
    }

    pub fn read_agent_transcript_entries(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Value>, String> {
        self.production.read_agent_transcript_entries(agent_id)
    }

    pub fn get_transcript_entries(
        &self,
        session: &PreparedAgentBlobStore,
    ) -> Result<Vec<Value>, String> {
        self.production.read_agent_transcript_entries(&session.agent_id)
    }

    pub fn read_agent_transcript_page(
        &self,
        agent_id: &str,
        query: TranscriptPageQuery,
    ) -> Result<TranscriptPage, String> {
        self.production.read_agent_transcript_page(agent_id, query)
    }

    pub fn read_agent_transcript_window(
        &self,
        agent_id: &str,
        query: TranscriptWindowQuery,
    ) -> Result<TranscriptWindow<std::collections::BTreeMap<String, usize>>, String> {
        self.production.read_agent_transcript_window(agent_id, query)
    }

    pub fn read_agent_transcript_tail(
        &self,
        agent_id: &str,
        query: TranscriptWindowQuery,
    ) -> Result<TranscriptPage, String> {
        self.production.read_agent_transcript_tail(agent_id, query)
    }

    pub fn read_agent_thread(
        &self,
        agent_id: &str,
        root_id: &str,
    ) -> Result<TranscriptThread, String> {
        self.production.read_agent_thread(agent_id, root_id)
    }

    pub fn get_agent_transcript_entries(
        &self,
        agent_id: &str,
    ) -> Result<Vec<Value>, String> {
        self.production.read_agent_transcript_entries(agent_id)
    }

    pub fn get_agent_outline(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ConversationOutlineItem>, String> {
        self.production.read_agent_outline(agent_id)
    }

    pub fn get_session_outline(
        &self,
        session: &PreparedAgentBlobStore,
    ) -> Result<Vec<ConversationOutlineItem>, String> {
        self.production.read_agent_outline(&session.agent_id)
    }

    pub fn mark_session_viewed(
        &self,
        session: &PreparedAgentBlobStore,
        at: f64,
        preserve_manual_unread: bool,
    ) -> Result<bool, String> {
        self.production
            .mark_agent_viewed(&session.agent_id, at, preserve_manual_unread)
    }

    pub fn mark_session_viewed_now(
        &self,
        session: &PreparedAgentBlobStore,
        at: f64,
        preserve_manual_unread: bool,
    ) -> Result<bool, String> {
        self.mark_session_viewed(session, at, preserve_manual_unread)
    }

    pub fn mark_session_activity(
        &self,
        session: &PreparedAgentBlobStore,
        at: f64,
    ) -> Result<bool, String> {
        self.production.mark_agent_activity(&session.agent_id, at)
    }

    pub fn mark_agent_viewed(
        &self,
        agent_id: &str,
        at: f64,
        preserve_manual_unread: bool,
    ) -> Result<bool, String> {
        self.production
            .mark_agent_viewed(agent_id, at, preserve_manual_unread)
    }

    pub fn mark_agent_activity(&self, agent_id: &str, at: f64) -> Result<bool, String> {
        self.production.mark_agent_activity(agent_id, at)
    }

    pub fn set_session_unread(
        &self,
        agent_id: &str,
        unread: bool,
        at: f64,
    ) -> Result<bool, String> {
        self.production.set_agent_unread(agent_id, unread, at)
    }

    pub fn set_awaiting_user_response(
        &self,
        agent_id: &str,
        state: Option<&AwaitingUserResponse>,
    ) -> Result<bool, String> {
        self.production
            .set_agent_awaiting_user_response(agent_id, state)
    }

    pub fn set_awaiting_user_response_for_tab(
        &self,
        agent_id: &str,
        tab_id: &str,
        state: Option<&AwaitingUserResponse>,
        if_since_before: Option<f64>,
    ) -> Result<bool, String> {
        self.production.set_agent_awaiting_user_response_for_tab(
            agent_id,
            tab_id,
            state,
            if_since_before,
        )
    }

    pub fn expire_pending_auto_review_approvals(
        &self,
        agent_id: &str,
        only_request_id: Option<&str>,
    ) -> Result<Vec<String>, String> {
        self.production
            .expire_pending_auto_review_approvals(agent_id, only_request_id)
    }

    pub fn expire_pending_local_tool_permission_asks(
        &self,
        agent_id: &str,
        only_request_id: Option<&str>,
        if_pending_before_ms: Option<f64>,
    ) -> Result<Vec<String>, String> {
        self.production.expire_pending_local_tool_permission_asks(
            agent_id,
            only_request_id,
            if_pending_before_ms,
        )
    }

    pub fn clear_agent_memory_prompt_snapshot(
        &self,
        agent_id: &str,
    ) -> Result<bool, String> {
        self.production.clear_agent_memory_prompt_snapshot(agent_id)
    }

    pub fn ensure_conversation_capacity_for_turn(
        &self,
        session: &PreparedAgentBlobStore,
    ) -> Result<(), String> {
        self.production.ensure_capacity_for_turn(session)
    }

    pub fn set_session_notify_on_updates(
        &self,
        agent_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        self.write_settings(
            agent_id,
            Map::from_iter([(
                "notifyOnAgentUpdates".to_string(),
                Value::Bool(enabled),
            )]),
        )
    }

    pub fn set_session_hidden_from_sidebar(
        &self,
        agent_id: &str,
        hidden: bool,
    ) -> Result<(), String> {
        self.write_settings(
            agent_id,
            Map::from_iter([(
                "hiddenFromSidebar".to_string(),
                Value::Bool(hidden),
            )]),
        )
    }

    fn write_settings(&self, agent_id: &str, update: Map<String, Value>) -> Result<(), String> {
        self.production.write_agent_settings(agent_id, update)
    }

    pub fn open_channel_store(&self, agent_id: &str) -> Result<FileChannelStore, String> {
        self.production.open_channel_store(agent_id)
    }

    pub fn list_agent_channels(&self, agent_id: &str) -> Result<Vec<ChannelConnection>, String> {
        self.production.list_agent_channels(agent_id)
    }

    pub fn list_channel_configs(&self, agent_id: &str) -> Result<Vec<ChannelConfig>, String> {
        self.production.list_channel_configs(agent_id)
    }

    pub fn store_connector_credential(
        &self,
        agent_id: &str,
        platform: &str,
        field: &str,
        value: &str,
    ) -> Result<bool, String> {
        self.production
            .store_connector_credential(agent_id, platform, field, value)
    }

    pub fn disconnect_channel(&self, agent_id: &str, platform: &str) -> Result<bool, String> {
        self.production.disconnect_channel(agent_id, platform)
    }

    pub fn stat_open_db(
        &self,
        session: &PreparedAgentBlobStore,
    ) -> Option<SessionDbStat> {
        let metadata = fs::metadata(&session.session_db_path).ok()?;
        let mtime_ms = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs_f64() * 1000.0)
            .unwrap_or_default();
        Some(SessionDbStat {
            size: metadata.len(),
            mtime_ms,
        })
    }

    pub fn close_worker_pool(&self) {
        self.production.shutdown();
    }
}

pub(crate) fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}
