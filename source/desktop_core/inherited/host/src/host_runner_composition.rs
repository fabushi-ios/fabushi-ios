use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::extensions::local_tool_permission::local_tool_permission_controller::{
    SandLocalToolControllerEvent, SandLocalToolControllerEventKind,
    SandLocalToolControllerSubscription, SandLocalToolPermissionController,
    SandLocalToolRequestStatus,
};
use crate::extensions::memory::agent_state::{
    AgentAutomationMutation, AgentProjectMembershipMutation, AgentWorkflowMutation,
    AvatarBoxFileReader, SandAgentState, StateWriteResult,
};
use crate::extensions::session::agent_session::SandAgentSessionStore;
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::runner::TurnUsage;
use crate::runner::background_work::{RunnerBackgroundShellWatches, RunnerCloudAgentWatches};
use crate::runner::computer_use::{
    ComputerControlLease, ComputerUseCoordination, ComputerUsePrewarmStage,
};
use crate::runner::sand_action_audit::ActionAuditRecord;
use crate::runner::production_agent_checkpoint::{
    AgentStateCheckpointSink, ProductionAgentStateCheckpointSink,
};
use crate::runner::production_turn_agent_owner::{
    ProductionTurnAgentBuildBindings, ProductionTurnAgentLifecycleBindings,
};
use crate::runner::prompt_collector_glue::PromptCollectorAutomationReminderState;
use crate::runner::shell_terminal_watch::{
    MaterializedTurn, MaterializedTurnKind, MaterializedUserMessage, WatermarkResult,
    find_confirmed_user_turn_watermark,
};
use crate::runner::sand_agent_runner::SandAgentRunner;
use crate::runner::subagent_runtime::SubagentRuntime;
use crate::runner::tools::sand_multitask_todo_tool::{
    MultitaskTodoState, ProductionMultitaskTodoState,
};
use crate::runner::tools::sand_state_tool::SandStateWriter;
use crate::runner::turn_agent_composition::TurnAgentComposition;
use crate::runner::turn_observation::TurnObservationHandle;
pub use crate::runner_production_bridge::ProductionRunnerCompositionHooks as ProductionTurnCompositionHooks;
use crate::runner_production_bridge::{
    ProductionRunnerCompositionInput, create_production_runner,
    create_production_runner_composition_with_hooks,
};
use crate::transcript_mirror::conversation_state_binary::{
    ConversationTurnStructureFields, conversation_compaction_epoch,
    decode_conversation_state_recovery_fields, decode_conversation_turn_structure_fields,
    decode_user_message_identity_fields,
};
use crate::transcript_mirror::generated_occurrence_codec::{
    CanonicalGeneratedToolJsonProjection, GeneratedTranscriptOccurrenceCodec,
};
use crate::transcript_mirror::production_provider::ProductionTranscriptMirrorProvider;
use crate::transcript_mirror::transcript_mirror::JournalOutcomeReporter;
use crate::transcript_mirror::transcript_mirror_router::JournalEnabledReader;

type PermissionEventSink = Arc<dyn Fn(&SandLocalToolControllerEvent) + Send + Sync>;
type CanonicalTranscriptMirrorProvider = ProductionTranscriptMirrorProvider<
    GeneratedTranscriptOccurrenceCodec<CanonicalGeneratedToolJsonProjection>,
>;

/// Host-resolved state surfaces consumed by the canonical Runner bridge.
///
/// Concrete state owners remain in Session/Memory; HostRunnerComposition only
/// resolves their turn-scoped handles before delegating immutable projection.
pub struct ProductionTurnStateSurfaces {
    pub state_writer: Option<Arc<dyn SandStateWriter>>,
    pub multitask_todo_state: Option<Arc<dyn MultitaskTodoState>>,
}

pub struct HostRunnerComposition {
    controller: Arc<SandLocalToolPermissionController>,
    sink: PermissionEventSink,
    surfaces: Mutex<HashMap<String, SandLocalToolControllerSubscription>>,
    prompt_automation_reminders: Mutex<HashMap<String, PromptCollectorAutomationReminderState>>,
    computer_use: Arc<Mutex<ComputerUseCoordination>>,
    transcript_providers: Mutex<HashMap<PathBuf, Arc<CanonicalTranscriptMirrorProvider>>>,
}

impl HostRunnerComposition {
    pub fn production(
        controller: Arc<SandLocalToolPermissionController>,
        sessions: Arc<ProductionSessionWorkers>,
    ) -> Self {
        let sink = Arc::new(move |event: &SandLocalToolControllerEvent| {
            if let Err(error) = persist_local_permission_event(&sessions, event) {
                eprintln!(
                    "mahayana-host local permission transcript projection failed agent={} request={} error={error}",
                    event.request.agent_id, event.request.id,
                );
            }
        });
        Self::with_sink(controller, sink)
    }

    pub fn with_sink(
        controller: Arc<SandLocalToolPermissionController>,
        sink: PermissionEventSink,
    ) -> Self {
        Self {
            controller,
            sink,
            surfaces: Mutex::new(HashMap::new()),
            prompt_automation_reminders: Mutex::new(HashMap::new()),
            computer_use: Arc::new(Mutex::new(ComputerUseCoordination::new(true))),
            transcript_providers: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve turn-scoped state surfaces through the canonical Host composition owner.
    pub fn compose_turn_state_surfaces(
        &self,
        sessions: Arc<ProductionSessionWorkers>,
        agent_id: &str,
        group_member_turn: bool,
        multitask_enabled: bool,
        avatar_box_file_reader: Option<AvatarBoxFileReader>,
    ) -> Result<ProductionTurnStateSurfaces, String> {
        if group_member_turn {
            return Ok(ProductionTurnStateSurfaces {
                state_writer: None,
                multitask_todo_state: None,
            });
        }
        let sand_root = sessions
            .memory_service()
            .agents_root_dir()
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "production memory agents root has no sand root parent".to_string())?;
        let mut state = SandAgentState::new(sand_root, agent_id.to_string())?;
        if let Some(reader) = avatar_box_file_reader {
            state = state.with_box_file_reader(reader);
        }
        let memory_sessions = Arc::clone(&sessions);
        let memory_agent_id = agent_id.to_string();
        let write_memory_sessions = Arc::clone(&sessions);
        let write_memory_agent_id = agent_id.to_string();
        state = state.with_agent_memory_writer(Arc::new(move |content, tier| {
            write_memory_sessions.write_agent_memory(&write_memory_agent_id, content, tier)
        }));
        state = state.with_agent_memory_remover(Arc::new(move |memory_id| {
            match memory_sessions.remove_agent_memory(&memory_agent_id, memory_id) {
                Ok(true) => StateWriteResult::success("Forgot the fact from agent memory."),
                Ok(false) => StateWriteResult::failure(
                    "no fact with exactly that text is recorded in agent memory.",
                ),
                Err(error) => StateWriteResult::failure(format!(
                    "could not update agent memory: {error}"
                )),
            }
        }));
        let profile_sessions = Arc::clone(&sessions);
        let profile_agent_id = agent_id.to_string();
        state = state.with_agent_profile_writer(Arc::new(move |name, description| {
            match profile_sessions.update_agent_profile_fields(
                &profile_agent_id,
                name.as_deref(),
                description.as_deref(),
            ) {
                Ok(()) => StateWriteResult::success("Updated your profile."),
                Err(error) => StateWriteResult::failure(format!(
                    "could not update your profile: {error}"
                )),
            }
        }));
        let settings_sessions = Arc::clone(&sessions);
        let settings_agent_id = agent_id.to_string();
        state = state.with_agent_settings_writer(Arc::new(move |update| {
            match settings_sessions.write_agent_settings(&settings_agent_id, update) {
                Ok(()) => StateWriteResult::success("Updated your settings."),
                Err(error) => StateWriteResult::failure(format!(
                    "could not update your settings: {error}"
                )),
            }
        }));
        let channel_sessions = Arc::clone(&sessions);
        let channel_agent_id = agent_id.to_string();
        state = state.with_agent_channel_disconnector(Arc::new(move |platform| {
            match channel_sessions.disconnect_channel(&channel_agent_id, platform) {
                Ok(true) => StateWriteResult::success(format!("Disconnected {platform}.")),
                Ok(false) => StateWriteResult::failure(format!(
                    "{platform} is not connected."
                )),
                Err(error) => StateWriteResult::failure(format!(
                    "could not disconnect {platform}: {error}"
                )),
            }
        }));
        let automation_sessions = Arc::clone(&sessions);
        let automation_agent_id = agent_id.to_string();
        state = state.with_agent_automation_mutator(Arc::new(move |mutation| {
            let session_store = SandAgentSessionStore::new(Arc::clone(&automation_sessions));
            let result = match mutation {
                AgentAutomationMutation::Create(spec) => session_store
                    .create_agent_automation(&automation_agent_id, &spec)
                    .map(|_| "Saved routine."),
                AgentAutomationMutation::Update { id, spec } => session_store
                    .update_agent_automation(&automation_agent_id, &id, &spec)
                    .map(|_| "Updated routine."),
                AgentAutomationMutation::SetEnabled { id, enabled } => session_store
                    .set_agent_automation_enabled(&automation_agent_id, &id, enabled)
                    .map(|_| if enabled { "Resumed routine." } else { "Paused routine." }),
                AgentAutomationMutation::Remove { id } => session_store
                    .remove_agent_automation(&automation_agent_id, &id)
                    .map(|_| "Deleted routine."),
            };
            match result {
                Ok(message) => StateWriteResult::success(message),
                Err(error) => StateWriteResult::failure(format!(
                    "could not update routine: {error}"
                )),
            }
        }));
        let workflow_sessions = Arc::clone(&sessions);
        let workflow_agent_id = agent_id.to_string();
        state = state.with_agent_workflow_mutator(Arc::new(move |mutation| {
            let session_store = SandAgentSessionStore::new(Arc::clone(&workflow_sessions));
            let result = match mutation {
                AgentWorkflowMutation::Upsert { id, spec } => {
                    if let Some(id) = id {
                        session_store
                            .update_agent_workflow(&workflow_agent_id, &id, &spec)
                            .map(|_| "Updated workflow.")
                    } else {
                        session_store
                            .create_agent_workflow(&workflow_agent_id, &spec)
                            .map(|_| "Saved workflow.")
                    }
                }
                AgentWorkflowMutation::Remove { id } => session_store
                    .remove_agent_workflow(&workflow_agent_id, &id)
                    .map(|_| "Deleted workflow."),
            };
            match result {
                Ok(message) => StateWriteResult::success(message),
                Err(error) => StateWriteResult::failure(format!(
                    "could not update workflow: {error}"
                )),
            }
        }));
        let project_sessions = Arc::clone(&sessions);
        let project_agent_id = agent_id.to_string();
        state = state.with_agent_project_membership_mutator(Arc::new(move |mutation| {
            let session_store = SandAgentSessionStore::new(Arc::clone(&project_sessions));
            let result = match mutation {
                AgentProjectMembershipMutation::Join { slug } => {
                    session_store.join_agent_project(&project_agent_id, &slug)
                }
                AgentProjectMembershipMutation::Leave { slug } => {
                    session_store.leave_agent_project(&project_agent_id, &slug)
                }
            };
            match result {
                Ok(true) => StateWriteResult::success("Updated project membership."),
                Ok(false) => StateWriteResult::failure("project membership was not changed."),
                Err(error) => StateWriteResult::failure(format!(
                    "could not update project membership: {error}"
                )),
            }
        }));
        let project_creator_sessions = Arc::clone(&sessions);
        let project_creator_agent_id = agent_id.to_string();
        state = state.with_agent_project_creator(Arc::new(move |slug, name, description| {
            let session_store = SandAgentSessionStore::new(Arc::clone(&project_creator_sessions));
            match session_store.create_agent_project(
                &project_creator_agent_id,
                &slug,
                &name,
                description.as_deref(),
            ) {
                Ok(true) => StateWriteResult::success(format!(
                    "Created and joined project \"{name}\" (folder {slug})."
                )),
                Ok(false) => StateWriteResult::success(format!(
                    "Joined existing project \"{slug}\"."
                )),
                Err(error) => StateWriteResult::failure(format!(
                    "could not create or join project \"{slug}\": {error}"
                )),
            }
        }));
        let avatar_sessions = Arc::clone(&sessions);
        let avatar_agent_id = agent_id.to_string();
        state = state.with_agent_avatar_mutator(Arc::new(move |bytes| {
            let session_store = SandAgentSessionStore::new(Arc::clone(&avatar_sessions));
            match session_store.set_agent_avatar_bytes_by_id(&avatar_agent_id, bytes.as_deref()) {
                Ok(_) => StateWriteResult::success("Updated your picture."),
                Err(error) => StateWriteResult::failure(format!(
                    "could not update your picture: {error}"
                )),
            }
        }));
        let state_writer: Arc<dyn SandStateWriter> = Arc::new(state);
        let multitask_todo_state = if multitask_enabled {
            Some(Arc::new(ProductionMultitaskTodoState::new(
                Arc::clone(&sessions),
                agent_id,
            )) as Arc<dyn MultitaskTodoState>)
        } else {
            None
        };
        Ok(ProductionTurnStateSurfaces {
            state_writer: Some(state_writer),
            multitask_todo_state,
        })
    }

    /// Compose the production transcript/checkpoint boundary for one Runner session.
    ///
    /// Session storage and transcript-mirror implementations retain their own state;
    /// HostRunnerComposition owns only the one-time wiring between those owners.
    pub fn compose_production_checkpoint_sink(
        &self,
        sessions: &ProductionSessionWorkers,
        data_dir: &Path,
        agent_id: &str,
        group_member_turn: bool,
        report_outcome: JournalOutcomeReporter,
        is_journal_enabled: JournalEnabledReader,
    ) -> Result<Option<Arc<dyn AgentStateCheckpointSink>>, String> {
        if group_member_turn {
            return Ok(None);
        }
        let agent_store = sessions.open_agent_store_owner(agent_id)?;
        let blob_store = Arc::new(sessions.create_agent_blob_store(agent_id)?);
        let prior_state_bytes = agent_store.latest_checkpoint_bytes().unwrap_or_default();
        let transcripts_dir = data_dir.join("transcripts");
        let transcript_provider = {
            let mut providers = self
                .transcript_providers
                .lock()
                .map_err(|_| "production transcript provider registry is poisoned".to_string())?;
            Arc::clone(
                providers
                    .entry(transcripts_dir.clone())
                    .or_insert_with(|| {
                        Arc::new(ProductionTranscriptMirrorProvider::with_reporter(
                            transcripts_dir,
                            GeneratedTranscriptOccurrenceCodec::new(
                                CanonicalGeneratedToolJsonProjection,
                            ),
                            Arc::clone(&report_outcome),
                        ))
                    }),
            )
        };
        let transcript_mirror = Arc::new(transcript_provider.route_for_session(
            Arc::clone(&blob_store),
            &prior_state_bytes,
            is_journal_enabled,
        )?);
        let sink = ProductionAgentStateCheckpointSink::new(
            agent_id.to_string(),
            agent_store,
            blob_store,
            transcript_mirror,
            prior_state_bytes,
            true,
        )?;
        Ok(Some(Arc::new(sink)))
    }

    /// Resolve the frozen confirmed-user watermark from the canonical generated
    /// Agent ConversationStateStructure and content-addressed BlobStore.
    ///
    /// Missing or unreadable referenced blobs fail closed as "a user turn
    /// exists but its identity is unknown", preventing queued-message replay.
    pub fn confirmed_user_turn_watermark(
        &self,
        sessions: &ProductionSessionWorkers,
        agent_id: &str,
    ) -> Result<WatermarkResult, String> {
        let agent_store = sessions.open_agent_store_owner(agent_id)?;
        let state_bytes = agent_store.latest_checkpoint_bytes().unwrap_or_default();
        if state_bytes.is_empty() {
            return Ok(WatermarkResult {
                last_user_message_id: None,
                has_user_turn: false,
            });
        }
        let state = match decode_conversation_state_recovery_fields(&state_bytes) {
            Ok(state) => state,
            Err(_) => {
                return Ok(WatermarkResult {
                    last_user_message_id: None,
                    has_user_turn: true,
                });
            }
        };
        let blob_store = sessions.create_agent_blob_store(agent_id)?;
        let mut turns = Vec::with_capacity(state.turns.len());
        for turn_ref in state.turns {
            if turn_ref.is_empty() {
                continue;
            }
            let kind = match blob_store.get_blob_blocking(&turn_ref) {
                Ok(Some(turn_blob)) => match decode_conversation_turn_structure_fields(&turn_blob) {
                    Ok(Some(ConversationTurnStructureFields::Shell { .. })) | Ok(None) => {
                        MaterializedTurnKind::NonAgent
                    }
                    Ok(Some(ConversationTurnStructureFields::Agent { user_message, .. })) => {
                        if user_message.is_empty() {
                            MaterializedTurnKind::Agent { user_message: None }
                        } else {
                            match blob_store.get_blob_blocking(&user_message) {
                                Ok(Some(user_blob)) => {
                                    match decode_user_message_identity_fields(&user_blob) {
                                        Ok(user) => MaterializedTurnKind::Agent {
                                            user_message: Some(MaterializedUserMessage {
                                                text: user.text,
                                                message_id: user.message_id,
                                                rich_text: None,
                                            }),
                                        },
                                        Err(_) => MaterializedTurnKind::Unreadable,
                                    }
                                }
                                _ => MaterializedTurnKind::Unreadable,
                            }
                        }
                    }
                    Err(_) => MaterializedTurnKind::Unreadable,
                },
                _ => MaterializedTurnKind::Unreadable,
            };
            turns.push(MaterializedTurn { turn_ref, kind });
        }
        Ok(find_confirmed_user_turn_watermark(&turns, None).result)
    }

    /// Read the durable conversation compaction epoch used by dynamic prompt projection.
    pub fn prompt_compaction_epoch(
        &self,
        sessions: &ProductionSessionWorkers,
        agent_id: &str,
    ) -> Result<u64, String> {
        let agent_store = sessions.open_agent_store_owner(agent_id)?;
        let state = agent_store.latest_checkpoint_bytes().unwrap_or_default();
        conversation_compaction_epoch(&state).map_err(|error| {
            format!("could not decode production conversation compaction epoch for {agent_id}: {error}")
        })
    }

    /// Resolve this turn's automation reminder through the long-lived per-Agent collector state.
    pub fn automation_status_reminder_for_turn(
        &self,
        agent_id: &str,
        rendered: Option<&str>,
        compaction_epoch: u64,
    ) -> Option<String> {
        self.prompt_automation_reminders
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(agent_id.to_string())
            .or_default()
            .reminder_for_turn(rendered, compaction_epoch)
    }

    /// Commit a reminder only after the prompt collector actually injected it.
    pub fn note_automation_status_reminder(
        &self,
        agent_id: &str,
        reminder: Option<&str>,
        compaction_epoch: u64,
    ) {
        self.prompt_automation_reminders
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(agent_id.to_string())
            .or_default()
            .note_reminder(reminder, compaction_epoch);
    }

    /// Construct the shipping Runner facade from the composed turn and lifecycle owners.
    pub fn compose_production_runner(
        &self,
        composition: TurnAgentComposition,
        build_bindings: ProductionTurnAgentBuildBindings,
        lifecycle_bindings: ProductionTurnAgentLifecycleBindings,
        checkpoint_sink: Option<Arc<dyn AgentStateCheckpointSink>>,
        upgrade_quiesce_signal: Arc<AtomicBool>,
        generated_agent_runtime: Arc<Mutex<SubagentRuntime>>,
        observation: Option<TurnObservationHandle>,
        cloud_agent_watches: Option<Arc<RunnerCloudAgentWatches>>,
        background_shell_watches: Option<Arc<RunnerBackgroundShellWatches>>,
        computer_use: Option<Arc<Mutex<ComputerUseCoordination>>>,
    ) -> SandAgentRunner {
        create_production_runner(
            composition,
            build_bindings,
            lifecycle_bindings,
            checkpoint_sink,
            upgrade_quiesce_signal,
            generated_agent_runtime,
            observation,
            cloud_agent_watches,
            background_shell_watches,
            computer_use,
        )
    }

    /// Build one production turn through the canonical Host -> Runner composition owner.
    ///
    /// The entrypoint supplies concrete turn-scoped services; this method owns the
    /// composition order so app/main.rs cannot grow a second Runner assembly path.
    pub fn compose_production_turn(
        &self,
        input: ProductionRunnerCompositionInput,
        hooks: ProductionTurnCompositionHooks,
    ) -> TurnAgentComposition {
        create_production_runner_composition_with_hooks(input, hooks)
    }

    pub fn bind_local_permission_surface(&self, agent_id: &str) {
        self.unbind_local_permission_surface(agent_id);
        let wanted = agent_id.to_string();
        let sink = Arc::clone(&self.sink);
        let subscription = self.controller.subscribe(Arc::new(move |event| {
            if event.request.agent_id == wanted {
                sink(&event);
            }
        }));
        self.surfaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(agent_id.to_string(), subscription);
    }

    pub fn unbind_local_permission_surface(&self, agent_id: &str) {
        self.surfaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(agent_id);
    }

    pub fn can_ask_local_tool_permission(&self, agent_id: &str) -> bool {
        self.surfaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(agent_id)
    }

    pub fn forget_local_tool_permission(&self, agent_id: &str) {
        self.unbind_local_permission_surface(agent_id);
        self.controller.forget_agent(agent_id);
    }

    pub fn active_permission_surface_count(&self) -> usize {
        self.surfaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Settle HostRunnerComposition-owned shutdown state after the canonical
    /// Runner registry has interrupted active turns. Runner cancellation itself
    /// remains owned by TranscriptRunnerRegistry; this owner only drops its live
    /// local-permission subscriptions, matching the frozen composition boundary.
    pub fn dispose(&self) {
        self.surfaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.prompt_automation_reminders
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    pub fn computer_use_coordination(&self) -> Arc<Mutex<ComputerUseCoordination>> {
        Arc::clone(&self.computer_use)
    }

    pub fn begin_computer_use_preparation(&self, agent_id: &str) -> Option<ComputerControlLease> {
        let mut owner = self
            .computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lease = owner.acquire_control_lease(agent_id);
        if lease.is_some() {
            owner.begin_preparation(agent_id);
        } else {
            owner.mark_preparation_failed(
                agent_id,
                ComputerUsePrewarmStage::Box,
                "computer_window_busy",
            );
        }
        lease
    }

    pub fn mark_computer_use_preparation_ready(&self, agent_id: &str) {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_preparation_ready(agent_id);
    }

    pub fn mark_computer_use_preparation_failed(
        &self,
        agent_id: &str,
        stage: ComputerUsePrewarmStage,
        error_class: impl Into<String>,
    ) {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .mark_preparation_failed(agent_id, stage, error_class);
    }

    pub fn owns_computer_control_lease(&self, lease: &ComputerControlLease) -> bool {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .owns_control_lease(lease)
    }

    pub fn record_computer_audit_intent(&self, action_case: &str) {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_audit_intent(action_case);
    }

    pub fn capture_computer_navigation_baseline(&self, stdout: &str) {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .capture_navigation_baseline(stdout);
    }

    pub fn request_computer_navigation_probe(&self) -> Option<u64> {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .request_navigation_probe()
    }

    pub fn computer_navigation_probe_wait_ms(&self, generation: u64, now_ms: u64) -> Option<u64> {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .navigation_probe_wait_ms(generation, now_ms)
    }

    pub fn complete_computer_navigation_probe(
        &self,
        generation: u64,
        stdout: &str,
        agent_id: &str,
        turn_id: Option<&str>,
        occurred_at_ms: u64,
    ) -> Vec<ActionAuditRecord> {
        self.computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .complete_navigation_probe(generation, stdout, agent_id, turn_id, occurred_at_ms)
    }

    pub fn finish_computer_use_turn(
        &self,
        agent_id: &str,
        lease: Option<&ComputerControlLease>,
        model_id: Option<&str>,
        usage: Option<TurnUsage>,
    ) {
        let mut owner = self
            .computer_use
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(model_id) = model_id {
            owner.record_model_id(model_id);
        }
        owner.record_turn_ended(usage);
        if let Some(lease) = lease {
            let _ = owner.release_control_lease(lease);
        } else {
            owner.free_window(agent_id);
        }
    }
}

fn persist_local_permission_event(
    sessions: &ProductionSessionWorkers,
    event: &SandLocalToolControllerEvent,
) -> Result<(), String> {
    match event.kind {
        SandLocalToolControllerEventKind::Created => {
            let mut ask = json!({
                "requestId": event.request.id,
                "action": event.request.action,
                "target": event.request.target,
                "status": "pending",
            });
            if let Some(description) = event.request.description.as_deref() {
                ask["description"] = Value::String(description.to_string());
            }
            let mut entry = json!({
                "id": event.request.id,
                "kind": "send-message",
                "message": {
                    "type": "local-tool-permission",
                    "ask": ask,
                },
                "timestampMs": now_ms(),
            });
            let session_key = match event.request.session_key.as_deref() {
                Some(session_key) => Some(session_key.to_string()),
                None => sessions
                    .read_session_work_admission(&event.request.agent_id)?
                    .map(|record| record.session_key),
            };
            if let Some(session_key) = session_key.as_deref() {
                entry["sessionKey"] = Value::String(session_key.to_string());
            }
            if let Some(session_key) = session_key.as_deref() {
                sessions
                    .append_agent_transcript_entries_if_session_admitted(
                        &event.request.agent_id,
                        session_key,
                        &[entry],
                    )
                    .map(|_| ())
            } else {
                sessions
                    .append_agent_transcript_entries(&event.request.agent_id, &[entry])
                    .map(|_| ())
            }
        }
        SandLocalToolControllerEventKind::Settled => {
            let entry_id = event.request.id.as_str();
            let Some(mut entry) = sessions
                .read_agent_transcript_entries(&event.request.agent_id)?
                .into_iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(entry_id))
            else {
                return Ok(());
            };
            if let Some(status) = entry
                .get_mut("message")
                .and_then(Value::as_object_mut)
                .and_then(|message| message.get_mut("ask"))
                .and_then(Value::as_object_mut)
            {
                status.insert(
                    "status".into(),
                    Value::String(permission_status(event.request.status).to_string()),
                );
            }
            let session_key = event
                .request
                .session_key
                .as_deref()
                .map(ToOwned::to_owned)
                .or_else(|| {
                    entry
                        .get("sessionKey")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                });
            if let Some(session_key) = session_key.as_deref() {
                sessions
                    .update_agent_transcript_entry_if_session_admitted(
                        &event.request.agent_id,
                        session_key,
                        entry_id,
                        &entry,
                    )
                    .map(|_| ())
            } else {
                sessions
                    .update_agent_transcript_entry(&event.request.agent_id, entry_id, &entry)
                    .map(|_| ())
            }
        }
    }
}

fn permission_status(status: SandLocalToolRequestStatus) -> &'static str {
    match status {
        SandLocalToolRequestStatus::Pending => "pending",
        SandLocalToolRequestStatus::Allowed => "allowed",
        SandLocalToolRequestStatus::Denied => "denied",
        SandLocalToolRequestStatus::Always => "always",
        SandLocalToolRequestStatus::Never => "never",
        SandLocalToolRequestStatus::Expired => "expired",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
