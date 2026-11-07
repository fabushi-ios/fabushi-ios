use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde_json::{Map, Value, json};

use crate::agents::agent_clone::{clone_agent_dir, clone_agent_display_name};
use crate::extensions::session::agent_session::SandAgentSessionStore;
use crate::extensions::session::production::FallbackSession;
use crate::extensions::session::gateway::{
    SessionGatewayError, optional_bool as session_optional_bool,
    optional_string as session_optional_string, parse_create_agent_profile,
    parse_profile_update,
};
use crate::transcript_mutation_events::publish_transcript_mutation;
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::transcript::roster_emit::ProductionRosterEmit;

pub type AgentDeletionHook = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync + 'static>;
pub type AgentKickstartHook = Arc<dyn Fn(&str) + Send + Sync + 'static>;

pub const SAND_ONBOARDING_KICKSTART_PROMPT: &str = concat!(
    "[first run] This is your very first turn. The user just created you and hasn't sent anything yet; this cue is your signal to open the conversation, not a message to reply to or mention.\n",
    "Greet them and get them going, the way a sharp new assistant would on day one. Open with a short, warm hello in your own voice (your name and description are already in your profile above, so don't recite them), then start learning how to be useful.\n",
    "If your profile description gives you a concrete assignment, treat that as what the user created you to do: skip the getting-started questions, begin the assignment immediately, and use your first message for a useful result or the next approval you need.\n",
    "Run getting-started as a real conversation, never a form or a checklist. Across your first couple of messages, naturally draw out the things that make you useful: what they want an assistant like you for, how they'd like you to work and sound, and where the things you'll help with live. Ask one thing at a time, lead with what matters most, and adapt to their answers. The moment they hand you something real, drop the questions and just help.\n",
    "Keep your orientation concrete and true right now, and don't restate the instructions you already have. Don't recite your tools. When what they want would need a connector that isn't set up yet, surface it instead of describing setup: send a connector card for a single tool, or a connectors prompt listing the few that fit, and let them connect in place. Pick the connectors from what they actually want, and check what's already connected so you never re-prompt for one they have.\n",
    "Nothing reaches the user unless it's inside a SendMessage, and offer any choice as a question widget. Don't mention this cue or that you were given setup instructions."
);
pub const SAND_DISK_SAVER_KICKSTART_PROMPT: &str = concat!(
    "[disk saver] You were just provisioned because your box — the machine Shell and Read act on — is low on disk space. This cue comes from Fabushi itself, not from the user; nothing has reached them yet.\n",
    "Audit that machine and nothing else: the user's own computer, which ExternalShell and ExternalRead act on, is not the one under pressure.\n",
    "Start with a read-only inspection over Shell from /workspace outward. Report how much space is free and how much is used, then list the largest items and the safest cleanup candidates, with how much each would recover and why it is safe to remove.\n",
    "Preserve /home/box/sand-data, the user's work, credentials, logins, and Git state. Delete or modify nothing until the user confirms a plan.\n",
    "Skip greetings and getting-started questions: your first message should already carry the audit's findings and the approval you need. Nothing reaches the user unless it's inside a SendMessage. Don't mention this cue."
);
pub const SAND_DISK_SAVER_REAUDIT_PROMPT: &str = concat!(
    "[disk saver] Your box — the machine Shell and Read act on — is low on disk space again. This cue comes from Fabushi itself because disk pressure returned, not from the user.\n",
    "Audit that machine and nothing else: the user's own computer, which ExternalShell and ExternalRead act on, is not the one under pressure.\n",
    "Start with a read-only inspection over Shell from /workspace outward. Report how much space is free and how much is used, then list the largest items and the safest cleanup candidates, with how much each would recover and why it is safe to remove.\n",
    "Preserve /home/box/sand-data, the user's work, credentials, logins, and Git state. Delete or modify nothing until the user confirms a plan.\n",
    "Deliver the fresh findings with SendMessage even if they match your last audit. Don't mention this cue."
);
pub const REPLY_NUDGE_PROMPT: &str =
    "Your previous turn left the user without the result they're waiting on — you never called SendMessage that turn, or every SendMessage you tried failed to deliver. Either way they received nothing and are still waiting. Do not assume a send from an earlier turn covered it: an opening acknowledgement back then did not deliver this result (ack ≠ delivery). Deliver the result now by actually invoking the SendMessage tool — make a real tool/function call, not text you write. Plain assistant text is NEVER shown to the user; only a real SendMessage tool invocation reaches them, so if you don't call the tool they just keep seeing silence.";
pub const INTRODUCTION_FAILED_TRAY_TITLE: &str = "Your agent couldn't introduce itself";

pub fn introduction_failed_tray_key(agent_id: &str) -> String {
    format!("introduction-failed:{agent_id}")
}

pub fn kickstart_prompt_for_purpose(purpose: Option<&str>) -> &'static str {
    if purpose == Some("disk-saver") {
        SAND_DISK_SAVER_KICKSTART_PROMPT
    } else {
        SAND_ONBOARDING_KICKSTART_PROMPT
    }
}

pub fn is_user_message_entry(entry: &Value) -> bool {
    match entry.get("kind").and_then(Value::as_str) {
        Some("message") => entry.get("role").and_then(Value::as_str) == Some("user"),
        Some("user-attachment") => true,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KickstartTurnOutcome {
    pub aborted: bool,
    pub quiesced_for_upgrade: bool,
    pub sent_message_count: usize,
    pub reacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KickstartRunError {
    pub request_id: Option<String>,
    pub message: String,
}

pub trait CreatedAgentKickstartRuntimePort: Send + Sync {
    fn is_run_ready(&self) -> bool;
    fn can_execute(&self) -> bool;
    fn is_disallowed_session(&self, agent_id: &str) -> bool;
    fn is_run_in_flight(&self, agent_id: &str) -> bool;
    fn run_hidden(
        &self,
        agent_id: &str,
        prompt: &str,
        source: &str,
    ) -> Result<KickstartTurnOutcome, KickstartRunError>;
    fn mark_resume_pending(&self, agent_id: &str, source: &str) -> Result<(), String>;
    fn report_failure(&self, agent_id: &str, error: &KickstartRunError);
    fn report_disk_saver_audit_failure(&self, agent_id: &str, error: &KickstartRunError) {
        self.report_failure(agent_id, error);
    }
    fn push_introduction_failure(&self, agent_id: &str, error: &KickstartRunError);
    fn emit_agent_update(&self, agent_id: &str) -> Result<(), String>;
}

pub fn run_created_agent_kickstart(
    sessions: &ProductionSessionWorkers,
    runtime: &dyn CreatedAgentKickstartRuntimePort,
    agent_id: &str,
) -> Result<bool, String> {
    if !sessions.get_agent_introduction_pending(agent_id)? {
        return Ok(false);
    }
    let transcript = sessions.read_agent_transcript_entries(agent_id)?;
    if transcript.iter().any(is_user_message_entry) {
        sessions.set_agent_introduction_pending(agent_id, false)?;
        return Ok(false);
    }
    if runtime.is_disallowed_session(agent_id) {
        return Ok(false);
    }
    if !runtime.is_run_ready() || !runtime.can_execute() {
        return Ok(false);
    }
    if runtime.is_run_in_flight(agent_id) {
        return Ok(true);
    }

    let purpose = sessions.read_agent_purpose(agent_id)?;
    let prompt = kickstart_prompt_for_purpose(purpose.as_deref());
    let first = match runtime.run_hidden(agent_id, prompt, "kickstart") {
        Ok(outcome) => outcome,
        Err(error) => {
            runtime.report_failure(agent_id, &error);
            runtime.push_introduction_failure(agent_id, &error);
            return Ok(true);
        }
    };

    let mut delivered = first.sent_message_count > 0;
    if !first.aborted && !first.quiesced_for_upgrade && first.sent_message_count == 0 {
        let retry = match runtime.run_hidden(agent_id, REPLY_NUDGE_PROMPT, "kickstart") {
            Ok(outcome) => outcome,
            Err(error) => {
                runtime.report_failure(agent_id, &error);
                runtime.push_introduction_failure(agent_id, &error);
                return Ok(true);
            }
        };
        delivered = !retry.aborted && (retry.sent_message_count > 0 || retry.reacted);
        if retry.quiesced_for_upgrade {
            runtime.mark_resume_pending(agent_id, "turn")?;
            sessions.set_agent_introduction_pending(agent_id, false)?;
        } else if delivered {
            sessions.set_agent_introduction_pending(agent_id, false)?;
        }
    } else if first.quiesced_for_upgrade {
        runtime.mark_resume_pending(agent_id, "turn")?;
        sessions.set_agent_introduction_pending(agent_id, false)?;
    } else if !first.aborted && delivered {
        sessions.set_agent_introduction_pending(agent_id, false)?;
    }

    runtime.emit_agent_update(agent_id)?;
    Ok(true)
}

pub fn request_disk_saver_audit(
    sessions: &ProductionSessionWorkers,
    runtime: &dyn CreatedAgentKickstartRuntimePort,
    agent_id: &str,
) -> Result<bool, String> {
    let purpose = sessions.read_agent_purpose(agent_id)?;
    if purpose.as_deref() != Some("disk-saver") || runtime.is_disallowed_session(agent_id) {
        return Ok(false);
    }
    if sessions.get_agent_introduction_pending(agent_id)? {
        return run_created_agent_kickstart(sessions, runtime, agent_id);
    }
    if !runtime.is_run_ready() || !runtime.can_execute() {
        return Ok(false);
    }
    if runtime.is_run_in_flight(agent_id) {
        return Ok(true);
    }

    let first = match runtime.run_hidden(agent_id, SAND_DISK_SAVER_REAUDIT_PROMPT, "event") {
        Ok(outcome) => outcome,
        Err(error) => {
            runtime.report_disk_saver_audit_failure(agent_id, &error);
            return Ok(true);
        }
    };
    if first.quiesced_for_upgrade {
        runtime.mark_resume_pending(agent_id, "event")?;
    } else if !first.aborted && first.sent_message_count == 0 {
        let retry = match runtime.run_hidden(agent_id, REPLY_NUDGE_PROMPT, "event") {
            Ok(outcome) => outcome,
            Err(error) => {
                runtime.report_disk_saver_audit_failure(agent_id, &error);
                return Ok(true);
            }
        };
        if retry.quiesced_for_upgrade {
            runtime.mark_resume_pending(agent_id, "event")?;
        }
    }
    runtime.emit_agent_update(agent_id)?;
    Ok(true)
}

#[derive(Clone, Default)]
pub struct AgentDeletionRuntimeDeps {
    pub mark_deleting: Option<AgentDeletionHook>,
    pub clear_deleting: Option<AgentDeletionHook>,
    pub cancel_runner: Option<AgentDeletionHook>,
    pub forget_ack: Option<AgentDeletionHook>,
    pub sharing_departure: Option<AgentDeletionHook>,
    pub clear_trays: Option<AgentDeletionHook>,
    pub dispose_background_work: Option<AgentDeletionHook>,
    pub drain_runner: Option<AgentDeletionHook>,
    pub release_box: Option<AgentDeletionHook>,
    pub forget_agent_state: Option<AgentDeletionHook>,
    pub forget_handoff: Option<AgentDeletionHook>,
    pub clear_pending_wakes: Option<AgentDeletionHook>,
}

impl AgentDeletionRuntimeDeps {
    fn mark_deleting(&self, agent_id: &str) -> Result<(), String> {
        if let Some(mark_deleting) = self.mark_deleting.as_ref() {
            mark_deleting(agent_id)?;
        }
        Ok(())
    }

    fn clear_deleting(&self, agent_id: &str) -> Result<(), String> {
        if let Some(clear_deleting) = self.clear_deleting.as_ref() {
            clear_deleting(agent_id)?;
        }
        Ok(())
    }

    fn before_delete(&self, agent_id: &str) -> Result<(), String> {
        if let Some(cancel_runner) = self.cancel_runner.as_ref() {
            cancel_runner(agent_id)?;
        }
        if let Some(forget_ack) = self.forget_ack.as_ref() {
            forget_ack(agent_id)?;
        }
        if let Some(sharing_departure) = self.sharing_departure.as_ref() {
            sharing_departure(agent_id)?;
        }
        if let Some(clear_trays) = self.clear_trays.as_ref() {
            clear_trays(agent_id)?;
        }
        if let Some(dispose_background_work) = self.dispose_background_work.as_ref() {
            dispose_background_work(agent_id)?;
        }
        if let Some(drain_runner) = self.drain_runner.as_ref() {
            drain_runner(agent_id)?;
        }
        Ok(())
    }

    fn after_delete(&self, agent_id: &str) -> Result<(), String> {
        if let Some(release_box) = self.release_box.as_ref() {
            release_box(agent_id)?;
        }
        if let Some(forget_agent_state) = self.forget_agent_state.as_ref() {
            forget_agent_state(agent_id)?;
        }
        if let Some(forget_handoff) = self.forget_handoff.as_ref() {
            forget_handoff(agent_id)?;
        }
        if let Some(clear_pending_wakes) = self.clear_pending_wakes.as_ref() {
            clear_pending_wakes(agent_id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct AgentDeletionGuard {
    expected_session_id: Option<String>,
    expected_lifecycle_revision: Option<String>,
    expected_updated_at: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLifecycleGatewayError {
    BadRequest(String),
    Internal(String),
}

impl AgentLifecycleGatewayError {
    fn bad(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

fn map_session_gateway_error(error: SessionGatewayError) -> AgentLifecycleGatewayError {
    match error {
        SessionGatewayError::BadRequest(message) => AgentLifecycleGatewayError::BadRequest(message),
        SessionGatewayError::Internal(message) => AgentLifecycleGatewayError::Internal(message),
    }
}

pub struct ProductionAgentLifecycle {
    store: SandAgentSessionStore,
    deletion_runtime: AgentDeletionRuntimeDeps,
    roster: Option<Arc<ProductionRosterEmit>>,
    kickstart_created_agent: Option<AgentKickstartHook>,
}

impl ProductionAgentLifecycle {
    pub fn new(production: Arc<ProductionSessionWorkers>) -> Self {
        Self::with_runtime_deps(production, AgentDeletionRuntimeDeps::default(), None)
    }

    pub fn with_deletion_runtime(
        production: Arc<ProductionSessionWorkers>,
        deletion_runtime: AgentDeletionRuntimeDeps,
    ) -> Self {
        Self::with_runtime_deps(production, deletion_runtime, None)
    }

    pub fn with_runtime_deps(
        production: Arc<ProductionSessionWorkers>,
        deletion_runtime: AgentDeletionRuntimeDeps,
        roster: Option<Arc<ProductionRosterEmit>>,
    ) -> Self {
        Self::with_all_runtime_deps(production, deletion_runtime, roster, None)
    }

    pub fn with_all_runtime_deps(
        production: Arc<ProductionSessionWorkers>,
        deletion_runtime: AgentDeletionRuntimeDeps,
        roster: Option<Arc<ProductionRosterEmit>>,
        kickstart_created_agent: Option<AgentKickstartHook>,
    ) -> Self {
        Self {
            store: SandAgentSessionStore::new(production),
            deletion_runtime,
            roster,
            kickstart_created_agent,
        }
    }

    fn emit_agents(&self) -> Result<(), String> {
        if let Some(roster) = self.roster.as_ref() {
            roster.emit_agents()?;
        }
        Ok(())
    }

    fn emit_agent_update(&self, agent_id: &str) -> Result<(), String> {
        if let Some(roster) = self.roster.as_ref() {
            roster.emit_agent_update(agent_id)?;
        }
        Ok(())
    }

    fn emit_agent_profile_update(&self, agent_id: &str) -> Result<(), String> {
        self.emit_agent_update(agent_id)?;
        if let Some(roster) = self.roster.as_ref() {
            roster.publish_profile_changed(agent_id);
        }
        Ok(())
    }

    fn finalize_summary_for_rpc<T: serde::Serialize>(
        &self,
        summary: T,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        if let Some(roster) = self.roster.as_ref() {
            roster
                .finalize_summary_for_rpc(summary)
                .map_err(AgentLifecycleGatewayError::internal)
        } else {
            serde_json::to_value(summary)
                .map_err(|error| AgentLifecycleGatewayError::internal(error.to_string()))
        }
    }

    fn create_agent_from_args(
        &self,
        args: &Value,
        activate: bool,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let profile = parse_create_agent_profile(args).map_err(map_session_gateway_error)?;
        let origin = session_optional_string(args, "origin")
            .map_err(map_session_gateway_error)?
            .unwrap_or("user");
        let purpose =
            session_optional_string(args, "purpose").map_err(map_session_gateway_error)?;
        let introduction_suppressed = session_optional_bool(args, "isIntroductionSuppressed")
            .map_err(map_session_gateway_error)?
            .unwrap_or(false);
        let kickstart_requested = session_optional_bool(args, "isKickstartRequested")
            .map_err(map_session_gateway_error)?
            .unwrap_or(false);

        let record = self
            .store
            .create_session(Some(&profile), origin, purpose)
            .map_err(AgentLifecycleGatewayError::internal)?;
        if introduction_suppressed {
            self.store
                .production()
                .set_agent_introduction_pending(&record.id, false)
                .map_err(AgentLifecycleGatewayError::internal)?;
        }
        if activate {
            self.store
                .mark_agent_viewed(&record.id, system_now_ms(), false)
                .map_err(AgentLifecycleGatewayError::internal)?;
            self.store
                .write_active_agent_id(&record.id)
                .map_err(|error| AgentLifecycleGatewayError::internal(error.to_string()))?;
        }
        self.emit_agents()
            .map_err(AgentLifecycleGatewayError::internal)?;

        let summary = self
            .store
            .summarize_agent_by_id(&record.id)
            .map_err(AgentLifecycleGatewayError::internal)?
            .ok_or_else(|| {
                AgentLifecycleGatewayError::internal("failed to summarize newly created agent")
            })?;
        let transcript = self
            .store
            .read_agent_transcript_entries(&record.id)
            .map_err(AgentLifecycleGatewayError::internal)?;
        let agent = self.finalize_summary_for_rpc(summary)?;
        if activate && kickstart_requested {
            if let Some(kickstart) = self.kickstart_created_agent.as_ref() {
                kickstart(&record.id);
            }
        }
        Ok(json!({ "agent": agent, "transcript": transcript }))
    }

    fn update_agent_from_args(
        &self,
        args: &Value,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let agent_id = required_string(args, "id")?;
        let update = parse_profile_update(args).map_err(map_session_gateway_error)?;
        let summary = self
            .store
            .update_agent_profile(agent_id, &update)
            .map_err(AgentLifecycleGatewayError::internal)?;
        let finalized = self.finalize_summary_for_rpc(summary)?;
        self.emit_agent_profile_update(agent_id)
            .map_err(AgentLifecycleGatewayError::internal)?;
        Ok(finalized)
    }

    fn set_agent_unread_from_args(
        &self,
        args: &Value,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let agent_id = required_string(args, "id")?;
        let is_unread = required_bool(args, "isUnread")?;
        let at_ms = args
            .get("atMs")
            .and_then(Value::as_f64)
            .unwrap_or_else(system_now_ms);
        self.store
            .set_session_unread(agent_id, is_unread, at_ms)
            .map_err(AgentLifecycleGatewayError::internal)?;
        self.emit_agent_update(agent_id)
            .map_err(AgentLifecycleGatewayError::internal)?;
        Ok(Value::Null)
    }

    fn set_agent_notify_on_updates_from_args(
        &self,
        args: &Value,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let agent_id = required_string(args, "id")?;
        let enabled = required_bool(args, "isEnabled")?;
        self.store
            .set_session_notify_on_updates(agent_id, enabled)
            .map_err(AgentLifecycleGatewayError::internal)?;
        self.emit_agent_update(agent_id)
            .map_err(AgentLifecycleGatewayError::internal)?;
        Ok(Value::Null)
    }

    fn set_agent_hidden_from_sidebar_from_args(
        &self,
        args: &Value,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let agent_id = required_string(args, "id")?;
        let hidden = required_bool(args, "isHidden")?;
        self.store
            .set_session_hidden_from_sidebar(agent_id, hidden)
            .map_err(AgentLifecycleGatewayError::internal)?;
        self.emit_agent_update(agent_id)
            .map_err(AgentLifecycleGatewayError::internal)?;
        Ok(Value::Null)
    }

    fn set_agent_avatar_bytes_from_args(
        &self,
        args: &Value,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let agent_id = required_string(args, "id")?;
        let png = decode_optional_png(args)?;
        let summary = self
            .store
            .set_agent_avatar_bytes(agent_id, png.as_deref())
            .map_err(AgentLifecycleGatewayError::internal)?;
        let finalized = match summary {
            Some(summary) => self.finalize_summary_for_rpc(summary)?,
            None => Value::Null,
        };
        self.emit_agent_profile_update(agent_id)
            .map_err(AgentLifecycleGatewayError::internal)?;
        Ok(finalized)
    }

    fn get_agent_avatar_from_args(
        &self,
        args: &Value,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let agent_id = required_string(args, "id")?;
        let avatar = self
            .store
            .get_agent_avatar(agent_id)
            .map_err(AgentLifecycleGatewayError::internal)?;
        Ok(json!({
            "version": avatar.version,
            "dataUrl": avatar.data_url,
        }))
    }

    pub fn clone_agent(&self, source_id: &str) -> Result<Value, String> {
        let summary = self
            .store
            .list_agents()?
            .into_iter()
            .find(|agent| agent.id == source_id)
            .ok_or_else(|| "That agent no longer exists.".to_string())?;
        if summary.is_group {
            return Err("Groups can't be duplicated yet.".to_string());
        }

        let source_dir = self.store.get_agent_dir(source_id);
        let clone_name = clone_agent_display_name(&summary.name);
        let production = Arc::clone(self.store.production());
        let new_id = production.mint_agent_with(|new_id| {
            production.with_agent_admitted_filesystem_mutation(source_id, || {
                clone_agent_dir(
                    &source_dir,
                    &production.agents_root().join(new_id),
                    new_id,
                    &clone_name,
                    production.busy_timeout_ms(),
                )
                .map_err(|error| error.to_string())
            })?;
            Ok(new_id.to_string())
        })?;

        let opened = (|| {
            let _ = self.store.open_session(&new_id)?;
            let now = system_now_ms();
            let _ = self.store.mark_agent_viewed(&new_id, now, false)?;
            self.store
                .write_active_agent_id(&new_id)
                .map_err(|error| error.to_string())?;
            let mutation = Map::from_iter([
                (
                    "kind".to_string(),
                    Value::String("agent-needs-reindex".to_string()),
                ),
                ("agentId".to_string(), Value::String(new_id.clone())),
            ]);
            publish_transcript_mutation(&mutation);
            self.emit_agents()?;
            let agent = self
                .store
                .summarize_agent_by_id(&new_id)?
                .ok_or_else(|| "minted agent could not be summarized".to_string())?;
            let transcript = self.store.read_agent_transcript_entries(&new_id)?;
            self.finalize_summary_for_rpc(agent)
                .map_err(|error| match error {
                    AgentLifecycleGatewayError::BadRequest(message)
                    | AgentLifecycleGatewayError::Internal(message) => message,
                })
                .map(|agent| json!({ "agent": agent, "transcript": transcript }))
        })();

        if opened.is_err() {
            let _ = self.store.delete_session(&new_id);
        }
        opened
    }

    fn delete_agent_from_args(
        &self,
        args: &Value,
    ) -> Result<Value, AgentLifecycleGatewayError> {
        let agent_id = required_string(args, "id")?.to_string();
        let expected_string = |key: &str| -> Result<Option<String>, AgentLifecycleGatewayError> {
            let Some(value) = args.get(key) else {
                return Ok(None);
            };
            let Some(value) = value.as_str().map(str::trim).filter(|value| !value.is_empty())
            else {
                return Err(AgentLifecycleGatewayError::bad(format!(
                    "{key} must be a non-empty string"
                )));
            };
            Ok(Some(value.to_string()))
        };
        let expected_updated_at = if args.get("expectedSessionUpdatedAt").is_some() {
            Some(
                args.get("expectedSessionUpdatedAt")
                    .and_then(Value::as_f64)
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| {
                        AgentLifecycleGatewayError::bad(
                            "expectedSessionUpdatedAt must be a finite number",
                        )
                    })?,
            )
        } else {
            None
        };
        let guard = AgentDeletionGuard {
            expected_session_id: expected_string("expectedSessionId")?,
            expected_lifecycle_revision: expected_string("expectedLifecycleRevision")?,
            expected_updated_at,
        };
        self.delete_agents_with_guard(std::slice::from_ref(&agent_id), Some(&guard))
            .map_err(|error| {
                if error == "SESSION_LIFECYCLE_CHANGED" {
                    AgentLifecycleGatewayError::bad(error)
                } else {
                    AgentLifecycleGatewayError::internal(error)
                }
            })
    }

    pub fn delete_agent(&self, agent_id: &str) -> Result<Value, String> {
        self.delete_agents(std::slice::from_ref(&agent_id.to_string()))
    }

    pub fn delete_agents(&self, agent_ids: &[String]) -> Result<Value, String> {
        self.delete_agents_with_guard(agent_ids, None)
    }

    fn assert_delete_guard(
        &self,
        agent_id: &str,
        guard: &AgentDeletionGuard,
    ) -> Result<(), String> {
        let current = self
            .store
            .summarize_agent_by_id(agent_id)?
            .ok_or_else(|| "SESSION_LIFECYCLE_CHANGED".to_string())?;
        if guard
            .expected_session_id
            .as_deref()
            .is_some_and(|expected| expected != current.id)
            || guard
                .expected_lifecycle_revision
                .as_deref()
                .is_some_and(|expected| {
                    current.lifecycle_revision.is_empty()
                        || expected != current.lifecycle_revision
                })
            || guard
                .expected_updated_at
                .is_some_and(|expected| expected != current.updated_at)
        {
            return Err("SESSION_LIFECYCLE_CHANGED".to_string());
        }
        Ok(())
    }

    fn delete_agents_with_guard(
        &self,
        agent_ids: &[String],
        guard: Option<&AgentDeletionGuard>,
    ) -> Result<Value, String> {
        let mut seen = HashSet::new();
        let ids = agent_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .filter(|id| seen.insert((*id).to_string()))
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return self.current_transcript();
        }

        let deleting = ids.iter().cloned().collect::<HashSet<_>>();
        let active_before = self.store.read_active_agent_id();

        for agent_id in &ids {
            if let Some(guard) = guard {
                self.assert_delete_guard(agent_id, guard)?;
            }
            self.deletion_runtime.mark_deleting(agent_id)?;
            let deleted = (|| {
                self.deletion_runtime.before_delete(agent_id)?;
                if let Some(guard) = guard {
                    self.assert_delete_guard(agent_id, guard)?;
                }
                self.store.delete_session(agent_id)?;
                self.deletion_runtime.after_delete(agent_id)?;
                Ok::<(), String>(())
            })();
            if let Err(error) = deleted {
                if self.store.agent_exists(agent_id) {
                    let _ = self.deletion_runtime.clear_deleting(agent_id);
                }
                return Err(error);
            }
        }

        if active_before
            .as_ref()
            .is_some_and(|active| deleting.contains(active))
        {
            let successor = self
                .store
                .list_agent_record_ids()?
                .into_iter()
                .find(|candidate| !deleting.contains(candidate));

            if let Some(successor) = successor {
                let _ = self
                    .store
                    .mark_agent_viewed(&successor, system_now_ms(), false)?;
                self.store
                    .write_active_agent_id(&successor)
                    .map_err(|error| error.to_string())?;
                let transcript = self.store.read_agent_transcript_entries(&successor)?;
                self.emit_agents()?;
                return Ok(json!({ "transcript": transcript }));
            }

            match self.store.create_fallback_session() {
                Ok(fallback) => {
                    let fallback_id = match fallback {
                        FallbackSession::Existing(prepared) => prepared.agent_id,
                        FallbackSession::Created(record) => record.id,
                    };
                    let _ = self
                        .store
                        .mark_agent_viewed(&fallback_id, system_now_ms(), false)?;
                    self.store
                        .write_active_agent_id(&fallback_id)
                        .map_err(|error| error.to_string())?;
                    let transcript = self.store.read_agent_transcript_entries(&fallback_id)?;
                    self.emit_agents()?;
                    return Ok(json!({ "transcript": transcript }));
                }
                Err(error) if error.contains("Agent limit of") => {
                    self.store
                        .clear_active_agent_id()
                        .map_err(|error| error.to_string())?;
                    self.emit_agents()?;
                    return Ok(json!({ "transcript": [] }));
                }
                Err(error) => return Err(error),
            }
        }

        self.emit_agents()?;
        self.current_transcript()
    }

    fn current_transcript(&self) -> Result<Value, String> {
        let Some(active) = self.store.read_active_agent_id() else {
            return Ok(json!({ "transcript": [] }));
        };
        if !self.store.agent_exists(&active) {
            self.store
                .clear_active_agent_id()
                .map_err(|error| error.to_string())?;
            return Ok(json!({ "transcript": [] }));
        }
        Ok(json!({
            "transcript": self.store.read_agent_transcript_entries(&active)?
        }))
    }
}

pub fn dispatch_production_agent_lifecycle_gateway_call(
    production: &Arc<ProductionSessionWorkers>,
    method: &str,
    args: &Value,
) -> Option<Result<Value, AgentLifecycleGatewayError>> {
    dispatch_production_agent_lifecycle_gateway_call_with_runtime(
        production,
        &AgentDeletionRuntimeDeps::default(),
        method,
        args,
    )
}

pub fn dispatch_production_agent_lifecycle_gateway_call_with_runtime(
    production: &Arc<ProductionSessionWorkers>,
    deletion_runtime: &AgentDeletionRuntimeDeps,
    method: &str,
    args: &Value,
) -> Option<Result<Value, AgentLifecycleGatewayError>> {
    dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
        production,
        deletion_runtime,
        None,
        method,
        args,
    )
}

pub fn dispatch_production_agent_lifecycle_gateway_call_with_runtimes(
    production: &Arc<ProductionSessionWorkers>,
    deletion_runtime: &AgentDeletionRuntimeDeps,
    roster: Option<Arc<ProductionRosterEmit>>,
    method: &str,
    args: &Value,
) -> Option<Result<Value, AgentLifecycleGatewayError>> {
    dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes(
        production,
        deletion_runtime,
        roster,
        None,
        method,
        args,
    )
}

pub fn dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes(
    production: &Arc<ProductionSessionWorkers>,
    deletion_runtime: &AgentDeletionRuntimeDeps,
    roster: Option<Arc<ProductionRosterEmit>>,
    kickstart_created_agent: Option<AgentKickstartHook>,
    method: &str,
    args: &Value,
) -> Option<Result<Value, AgentLifecycleGatewayError>> {
    let lifecycle = ProductionAgentLifecycle::with_all_runtime_deps(
        Arc::clone(production),
        deletion_runtime.clone(),
        roster,
        kickstart_created_agent,
    );
    let result = match method {
        "createAgent" => lifecycle.create_agent_from_args(args, true),
        "createBackgroundAgent" => lifecycle.create_agent_from_args(args, false),
        "updateAgent" => lifecycle.update_agent_from_args(args),
        "setAgentUnread" => lifecycle.set_agent_unread_from_args(args),
        "setAgentNotifyOnUpdates" => lifecycle.set_agent_notify_on_updates_from_args(args),
        "setAgentHiddenFromSidebar" => lifecycle.set_agent_hidden_from_sidebar_from_args(args),
        "setAgentAvatarBytes" => lifecycle.set_agent_avatar_bytes_from_args(args),
        "getAgentAvatar" => lifecycle.get_agent_avatar_from_args(args),
        "duplicateAgent" => required_string(args, "id").and_then(|agent_id| {
            lifecycle
                .clone_agent(agent_id)
                .map_err(AgentLifecycleGatewayError::internal)
        }),
        "deleteAgent" => lifecycle.delete_agent_from_args(args),
        "deleteAgents" => parse_agent_ids(args)
            .and_then(|ids| {
                lifecycle
                    .delete_agents(&ids)
                    .map_err(AgentLifecycleGatewayError::internal)
            }),
        _ => return None,
    };
    Some(result)
}

fn parse_agent_ids(args: &Value) -> Result<Vec<String>, AgentLifecycleGatewayError> {
    let values = args
        .get("ids")
        .and_then(Value::as_array)
        .ok_or_else(|| AgentLifecycleGatewayError::bad("missing or invalid ids"))?;
    let mut ids = Vec::with_capacity(values.len());
    for value in values {
        let id = value
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AgentLifecycleGatewayError::bad("invalid ids entry"))?;
        ids.push(id.to_string());
    }
    Ok(ids)
}

fn required_string<'a>(
    args: &'a Value,
    field: &str,
) -> Result<&'a str, AgentLifecycleGatewayError> {
    args.get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AgentLifecycleGatewayError::bad(format!("missing or invalid {field}"))
        })
}

fn required_bool(
    args: &Value,
    field: &str,
) -> Result<bool, AgentLifecycleGatewayError> {
    args.get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| AgentLifecycleGatewayError::bad(format!("missing or invalid {field}")))
}

fn decode_optional_png(args: &Value) -> Result<Option<Vec<u8>>, AgentLifecycleGatewayError> {
    match args.get("pngBase64") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(encoded)) => BASE64_STANDARD
            .decode(encoded.trim())
            .map(Some)
            .map_err(|error| AgentLifecycleGatewayError::bad(format!("invalid pngBase64: {error}"))),
        Some(_) => Err(AgentLifecycleGatewayError::bad(
            "missing or invalid pngBase64",
        )),
    }
}

fn system_now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
