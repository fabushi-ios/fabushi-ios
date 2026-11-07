use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use crate::automations::automation_store::FileAutomationStore;
use crate::extensions::session::agent_db_transcript_pages::{
    TranscriptPage, TranscriptWindow, TranscriptWindowQuery,
};
use crate::extensions::transcript::agent_lifecycle::{
    AgentDeletionRuntimeDeps, AgentKickstartHook, AgentLifecycleGatewayError,
    dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes,
};
use crate::extensions::attachments::attachments_service::AttachmentsService;
use crate::extensions::content_search::extension::ProductionContentSearchExtension;
use crate::extensions::memory::extension::HostMemoryExtension;
use crate::extensions::memory::memory_service::MemoryKind;
use crate::extensions::session::box_handoff_service::{
    BoxHandoffService, HandoffDecision, HandoffRequest, HandoffStartResult, HandoffTrigger,
    PendingHandoff, decide_box_hand_back,
};
use crate::extensions::session::gateway::{
    SessionGatewayError,
    dispatch_production_session_gateway_call_with_content_search_and_group_chat,
};
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::telemetry::analytics_service::AutomationRunAnalyticsTelemetry;
use crate::extensions::telemetry::host_telemetry_service::{
    HostProductAnalytics, HostTelemetryApi,
};
use crate::extensions::trays::extension::HostTraysExtension;
use crate::extensions::trays::trays_service::PushErrorOptions;
use crate::extensions::turn_execution::turn_execution_service::{
    TurnExecutionError, TurnExecutionRegistry, TurnExecutor,
};

use super::ack_obligations::AckObligations;
use super::automation_runtime::{AutomationRuntime, automation_records_value};
use super::background_wakes::BackgroundWakes;
use super::box_handoff_resume::settle_box_handoff_state_with_sink;
use super::client_side_tool_v2_producer::{
    ClientSideToolV2ProducedValue, ClientSideToolV2Producer, ClientSideToolV2TransportEvent,
};
use super::group_chat_glue::GroupChatGlue;
use super::async_task_union::AsyncTask;
use super::pending_wake_rearm::{PendingWakeRearm, PendingWakeRuntimePort};
use super::sand_pending_wake_store::DurablePendingWakeMarker;
use super::production_runtime::{
    AgentRunLifecycleObserver, ProductionSendError, ProductionTranscriptRuntime,
};
use super::runner_registry::TranscriptRunnerRegistry;
use super::roster_emit::ProductionRosterEmit;
use super::roster_search::RosterContentSearch;
use super::sand_upgrade_resume_store::UpgradeResumeMarker;
use super::shared_rooms::SharedRooms;
use super::upgrade_recreate_resume::UpgradeQuiesceSummary;
use super::widget_responses::WidgetResponses;
use super::workflow_commands::WorkflowCommands;

#[derive(Clone)]
pub struct TranscriptTurnExecutionPort {
    registry: Arc<Mutex<TurnExecutionRegistry>>,
}

impl TranscriptTurnExecutionPort {
    pub fn new(registry: Arc<Mutex<TurnExecutionRegistry>>) -> Self {
        Self { registry }
    }

    fn executor_handle(&self) -> Result<Arc<dyn TurnExecutor>, TurnExecutionError> {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .executor_handle()
            .ok_or(TurnExecutionError::Unbound)
    }

    pub fn can_execute(&self) -> bool {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .can_execute()
    }

    pub fn can_execute_group_member(&self) -> bool {
        self.can_execute()
    }

    pub async fn is_run_ready(&self) -> bool {
        let Ok(executor) = self.executor_handle() else {
            return false;
        };
        executor.is_inference_ready().await
    }

    pub fn create_runner(&self, session: Value, hooks: Value) -> Result<Value, TurnExecutionError> {
        Ok(self.executor_handle()?.create_runner(session, hooks))
    }

    pub fn create_group_member_runner(
        &self,
        session: Value,
        hooks: Value,
        overrides: Value,
    ) -> Result<Value, TurnExecutionError> {
        Ok(self
            .executor_handle()?
            .create_group_member_runner(session, hooks, overrides))
    }
}

#[derive(Clone)]
pub struct TranscriptManagerServices {
    pub telemetry: Arc<AutomationRunAnalyticsTelemetry>,
    pub product_analytics: HostProductAnalytics,
    pub trace_flusher: HostTelemetryApi,
    pub memory: HostMemoryExtension,
    pub content_search: Arc<ProductionContentSearchExtension>,
    pub attachments: Arc<AttachmentsService>,
    pub trays: Arc<HostTraysExtension>,
}

/// Production composition root for the Grok Transcript extension.
///
/// Frozen Grok builds TranscriptManager as the owner/delegation facade for
/// Session, send-pipeline, run-lifecycle and Runner registry state. The Rust
/// port keeps those owners independent, but constructs them exactly once here
/// so Host main does not create parallel runtime/ack/runner registries.
pub type AutomationConfigChangedObserver = Arc<dyn Fn() + Send + Sync>;
pub type ChannelConfigChangedObserver = Arc<dyn Fn() + Send + Sync>;
pub type ListenerConnectObserver = Arc<dyn Fn(&str, &str) + Send + Sync>;

pub struct TranscriptManager {
    session_workers: Arc<ProductionSessionWorkers>,
    transcript_runtime: Arc<ProductionTranscriptRuntime>,
    runner_registry: Arc<TranscriptRunnerRegistry>,
    ack_obligations: Arc<AckObligations>,
    automation_runtime: Arc<AutomationRuntime>,
    background_wakes: Arc<Mutex<BackgroundWakes<Value>>>,
    client_side_tool_v2: Mutex<ClientSideToolV2Producer>,
    group_chat: Arc<GroupChatGlue>,
    widget_responses: Arc<WidgetResponses>,
    roster_emit: Mutex<Option<Arc<ProductionRosterEmit>>>,
    workflow_commands: Arc<WorkflowCommands>,
    pending_wakes: Mutex<Option<PendingWakeRearm>>,
    watched_automation_store: Mutex<Option<FileAutomationStore>>,
    handoff_service: Mutex<Option<BoxHandoffService>>,
    shared_rooms: Arc<SharedRooms>,
    services: Mutex<Option<TranscriptManagerServices>>,
    automation_config_changed_observer: Mutex<Option<AutomationConfigChangedObserver>>,
    channel_config_changed_observer: Arc<Mutex<Option<ChannelConfigChangedObserver>>>,
    listener_connect_observer: Mutex<Option<ListenerConnectObserver>>,
    disposed: AtomicBool,
}

impl TranscriptManager {
    pub fn new(
        root_dir: impl AsRef<Path>,
        session_workers: Arc<ProductionSessionWorkers>,
    ) -> Self {
        let root_dir = root_dir.as_ref();
        let automation_runtime = Arc::new(AutomationRuntime::new(Arc::clone(&session_workers)));
        let transcript_runtime = Arc::new(ProductionTranscriptRuntime::new(Some(root_dir)));
        let shared_rooms = Arc::new(SharedRooms::new(Arc::clone(&session_workers)));
        let group_chat = Arc::new(GroupChatGlue::new(Arc::clone(&session_workers)));
        let channel_config_changed_observer: Arc<Mutex<Option<ChannelConfigChangedObserver>>> =
            Arc::new(Mutex::new(None));
        let widget_responses = Arc::new(WidgetResponses::with_runtime(
            Arc::clone(&session_workers),
            Arc::clone(&automation_runtime),
            Arc::clone(&transcript_runtime),
        ));
        let channel_config_changed_proxy = Arc::clone(&channel_config_changed_observer);
        widget_responses
            .bind_channel_config_changed(Arc::new(move || {
                let observer = channel_config_changed_proxy
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                if let Some(observer) = observer {
                    observer();
                }
            }))
            .expect("TranscriptManager must own the WidgetResponses channel-config signal");
        let workflow_commands = Arc::new(WorkflowCommands::new(
            Arc::clone(&session_workers),
            Arc::clone(&automation_runtime),
        ));
        Self {
            session_workers,
            transcript_runtime,
            runner_registry: Arc::new(TranscriptRunnerRegistry::default()),
            ack_obligations: Arc::new(AckObligations::new(root_dir)),
            automation_runtime,
            background_wakes: Arc::new(Mutex::new(BackgroundWakes::default())),
            client_side_tool_v2: Mutex::new(ClientSideToolV2Producer::new()),
            group_chat,
            widget_responses,
            roster_emit: Mutex::new(None),
            workflow_commands,
            pending_wakes: Mutex::new(None),
            watched_automation_store: Mutex::new(None),
            handoff_service: Mutex::new(None),
            shared_rooms,
            services: Mutex::new(None),
            automation_config_changed_observer: Mutex::new(None),
            channel_config_changed_observer,
            listener_connect_observer: Mutex::new(None),
            disposed: AtomicBool::new(false),
        }
    }

    pub fn session_workers(&self) -> Arc<ProductionSessionWorkers> {
        Arc::clone(&self.session_workers)
    }

    pub fn transcript_runtime(&self) -> Arc<ProductionTranscriptRuntime> {
        Arc::clone(&self.transcript_runtime)
    }

    pub fn runner_registry(&self) -> Arc<TranscriptRunnerRegistry> {
        Arc::clone(&self.runner_registry)
    }

    pub fn ack_obligations(&self) -> Arc<AckObligations> {
        Arc::clone(&self.ack_obligations)
    }

    pub fn automation_runtime(&self) -> Arc<AutomationRuntime> {
        Arc::clone(&self.automation_runtime)
    }

    pub fn background_wakes(&self) -> Arc<Mutex<BackgroundWakes<Value>>> {
        Arc::clone(&self.background_wakes)
    }

    pub fn shared_rooms(&self) -> Arc<SharedRooms> {
        Arc::clone(&self.shared_rooms)
    }

    pub fn publish_client_side_tool_v2(
        &self,
        agent_id: &str,
        produced: ClientSideToolV2ProducedValue,
    ) -> Option<ClientSideToolV2TransportEvent> {
        self.client_side_tool_v2
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .publish(agent_id, produced)
    }

    pub fn reset_client_side_tool_v2(
        &self,
        agent_id: &str,
    ) -> Option<ClientSideToolV2TransportEvent> {
        self.client_side_tool_v2
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reset(agent_id)
    }

    pub fn group_chat(&self) -> Arc<GroupChatGlue> {
        Arc::clone(&self.group_chat)
    }

    pub fn widget_responses(&self) -> Arc<WidgetResponses> {
        Arc::clone(&self.widget_responses)
    }

    pub fn bind_roster_emit(&self, roster: Arc<ProductionRosterEmit>) -> Result<(), String> {
        let mut slot = self
            .roster_emit
            .lock()
            .map_err(|_| "transcript roster mutex poisoned".to_string())?;
        if slot.is_some() {
            return Err("transcript roster already configured".into());
        }
        self.widget_responses.bind_roster(Arc::clone(&roster))?;
        *slot = Some(roster);
        Ok(())
    }

    pub fn roster_emit(&self) -> Option<Arc<ProductionRosterEmit>> {
        self.roster_emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn emit_agent_update(&self, agent_id: &str) -> Result<bool, String> {
        let Some(roster) = self.roster_emit() else {
            return Ok(false);
        };
        roster.emit_agent_update(agent_id)?;
        Ok(true)
    }

    pub fn emit_automations(&self, agent_id: &str) -> Result<bool, String> {
        let Some(roster) = self.roster_emit() else {
            return Ok(false);
        };
        let automations =
            automation_records_value(self.automation_runtime.get_agent_automations(agent_id)?);
        roster.emit_automations(agent_id, &automations)
    }

    pub fn respond_to_widget_with<Send>(
        &self,
        entry_id: &str,
        value: &str,
        agent_id: &str,
        now_ms: f64,
        send: Send,
    ) -> Result<bool, String>
    where
        Send: FnOnce(Value) -> Result<(), String>,
    {
        self.switch_agent(agent_id, now_ms)?;
        self.widget_responses
            .respond_to_widget_with(entry_id, value, agent_id, now_ms, send)
    }

    pub fn settle_stale_auto_review_card(
        &self,
        agent_id: &str,
        entry_id: &str,
        request_id: &str,
    ) -> Result<bool, String> {
        self.widget_responses
            .settle_stale_auto_review_card(agent_id, entry_id, request_id)
    }

    pub fn expire_all_pending_auto_review_approval_cards(&self) -> Result<usize, String> {
        self.widget_responses
            .expire_all_pending_auto_review_approval_cards()
    }

    pub fn dismiss_widget(
        &self,
        entry_id: &str,
        agent_id: &str,
        now_ms: f64,
    ) -> Result<bool, String> {
        self.switch_agent(agent_id, now_ms)?;
        self.widget_responses.dismiss_widget(entry_id, agent_id)
    }

    pub fn submit_secret_with<Resume>(
        &self,
        entry_id: &str,
        value: &str,
        agent_id: &str,
        now_ms: f64,
        resume: Resume,
    ) -> Result<bool, String>
    where
        Resume: FnOnce(String) -> Result<(), String>,
    {
        self.switch_agent(agent_id, now_ms)?;
        let prompt = match self.widget_responses.submit_secret(entry_id, value, agent_id) {
            Ok(Some(prompt)) => prompt,
            Ok(None) => return Ok(false),
            Err(_) => {
                if let Some(services) = self.production_services() {
                    services.trays.push_error(PushErrorOptions {
                        agent_id: Some(agent_id.to_string()),
                        title: "Could not store the secret".into(),
                        detail: "The secure input could not write the credential to its store.".into(),
                        dedupe_key: Some(format!("transcript-secret-store:{agent_id}:{entry_id}")),
                        ..PushErrorOptions::default()
                    });
                }
                return Ok(false);
            }
        };
        resume(prompt)?;
        Ok(true)
    }

    pub fn react_to_message_with<Resume>(
        &self,
        entry_id: &str,
        emoji: &str,
        agent_id: &str,
        now_ms: f64,
        resume: Resume,
    ) -> Result<bool, String>
    where
        Resume: FnOnce(String) -> Result<(), String>,
    {
        self.switch_agent(agent_id, now_ms)?;
        let prompt = self
            .widget_responses
            .react_to_message(entry_id, emoji, agent_id)?;
        if let Some(prompt) = prompt {
            resume(prompt)?;
        }
        Ok(true)
    }

    pub fn workflow_commands(&self) -> Arc<WorkflowCommands> {
        Arc::clone(&self.workflow_commands)
    }


    pub fn bind_pending_wake_runtime(
        &self,
        runtime: Arc<dyn PendingWakeRuntimePort>,
    ) -> Result<(), String> {
        let mut slot = self
            .pending_wakes
            .lock()
            .map_err(|_| "transcript pending-wake mutex poisoned".to_string())?;
        if slot.is_some() {
            return Err("transcript pending-wake runtime already configured".into());
        }
        *slot = Some(PendingWakeRearm::new(
            self.transcript_runtime.pending_wake_store().cloned(),
            runtime,
        ));
        Ok(())
    }

    pub fn rearm_pending_wakes(&self) -> Result<(), String> {
        let owner = self
            .pending_wakes
            .lock()
            .map_err(|_| "transcript pending-wake mutex poisoned".to_string())?
            .clone()
            .ok_or_else(|| "transcript pending-wake runtime is not configured".to_string())?;
        owner.rearm_pending_wakes();
        Ok(())
    }

    pub fn restore_recreate_pending_wakes(&self, carried: &[Value]) -> Result<usize, String> {
        let owner = self
            .pending_wakes
            .lock()
            .map_err(|_| "transcript pending-wake mutex poisoned".to_string())?
            .clone()
            .ok_or_else(|| "transcript pending-wake runtime is not configured".to_string())?;
        Ok(owner.restore_recreate_carried_pending_wakes(carried))
    }

    pub fn set_handoff_service(&self, handoff: BoxHandoffService) -> Result<(), String> {
        let mut slot = self
            .handoff_service
            .lock()
            .map_err(|_| "transcript handoff mutex poisoned".to_string())?;
        if slot.is_some() {
            return Err("transcript handoff service already configured".into());
        }
        *slot = Some(handoff);
        Ok(())
    }

    fn handoff_service(&self) -> Result<BoxHandoffService, String> {
        self.handoff_service
            .lock()
            .map_err(|_| "transcript handoff mutex poisoned".to_string())?
            .clone()
            .ok_or_else(|| "transcript handoff service is not configured".to_string())
    }

    pub fn box_handoff(&self, agent_id: &str) -> Option<PendingHandoff> {
        self.handoff_service().ok()?.get(agent_id)
    }

    pub fn start_box_handoff(
        &self,
        request: HandoffRequest,
    ) -> Result<HandoffStartResult, String> {
        Ok(self.handoff_service()?.start(request))
    }

    pub fn forget_box_handoff(&self, agent_id: &str) -> Result<(), String> {
        self.handoff_service()?.forget(agent_id);
        Ok(())
    }

    pub fn hand_back_forever_box(
        &self,
        agent_id: &str,
        trigger: HandoffTrigger,
    ) -> Result<HandoffDecision, String> {
        let handoff = self.handoff_service()?;
        let decision = decide_box_hand_back(handoff.get(agent_id).as_ref(), &trigger);
        let HandoffDecision::End(end) = &decision else {
            return Ok(decision);
        };
        if !handoff.end(agent_id, trigger)? {
            return Ok(HandoffDecision::None);
        }

        let _ = self
            .transcript_runtime
            .resolve_box_request_tracking(&end.request_id);
        let roster = self
            .roster_emit()
            .ok_or_else(|| "transcript roster is not configured".to_string())?;
        settle_box_handoff_state_with_sink(
            &self.session_workers,
            roster.as_ref(),
            agent_id,
            &end.request_id,
            &end.resolution,
        )?;
        Ok(decision)
    }

    pub fn set_production_services(&self, services: TranscriptManagerServices) -> Result<(), String> {
        let mut slot = self
            .services
            .lock()
            .map_err(|_| "transcript service composition mutex poisoned".to_string())?;
        if slot.is_some() {
            return Err("transcript production services already configured".into());
        }
        *slot = Some(services);
        Ok(())
    }

    pub fn production_services(&self) -> Option<TranscriptManagerServices> {
        self.services
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_turn_execution(&self, execution: TranscriptTurnExecutionPort) {
        self.runner_registry.set_turn_execution(execution);
    }

    pub fn set_agent_run_lifecycle_observer(
        &self,
        observer: Option<AgentRunLifecycleObserver>,
    ) {
        self.transcript_runtime
            .set_agent_run_lifecycle_observer(observer);
    }

    pub fn set_automation_config_changed_observer(
        &self,
        observer: Option<AutomationConfigChangedObserver>,
    ) {
        *self
            .automation_config_changed_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = observer;
    }

    pub fn emit_automation_config_changed(&self) {
        let observer = self
            .automation_config_changed_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(observer) = observer {
            observer();
        }
    }

    pub fn set_channel_config_changed_observer(
        &self,
        observer: Option<ChannelConfigChangedObserver>,
    ) {
        *self
            .channel_config_changed_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = observer;
    }

    pub fn emit_channel_config_changed(&self) {
        let observer = self
            .channel_config_changed_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(observer) = observer {
            observer();
        }
    }

    pub fn set_listener_connect_observer(&self, observer: Option<ListenerConnectObserver>) {
        *self
            .listener_connect_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = observer;
    }

    pub fn emit_listener_connect_card(&self, agent_id: &str, platform: &str) {
        let observer = self
            .listener_connect_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(observer) = observer {
            observer(agent_id, platform);
        }
    }

    pub fn turn_execution(&self) -> Option<TranscriptTurnExecutionPort> {
        self.runner_registry.turn_execution()
    }

    pub fn can_execute(&self) -> bool {
        self.runner_registry.can_execute()
    }

    pub fn can_execute_group_member(&self) -> bool {
        self.runner_registry.can_execute_group_member()
    }

    pub async fn is_run_ready(&self) -> bool {
        self.runner_registry.is_run_ready().await
    }

    pub fn create_runner(
        &self,
        session: Value,
        hooks: Value,
    ) -> Result<Value, TurnExecutionError> {
        self.runner_registry.create_runner(session, hooks)
    }

    pub fn create_group_member_runner(
        &self,
        session: Value,
        hooks: Value,
        overrides: Value,
    ) -> Result<Value, TurnExecutionError> {
        self.runner_registry
            .create_group_member_runner(session, hooks, overrides)
    }

    pub fn prompt_acceptance_status(
        &self,
        args: &Value,
    ) -> Result<Value, ProductionSendError> {
        self.transcript_runtime.prompt_acceptance_status(args)
    }

    pub fn dispatch_agent_lifecycle_gateway_call(
        &self,
        deletion_runtime: &AgentDeletionRuntimeDeps,
        kickstart_created_agent: Option<AgentKickstartHook>,
        method: &str,
        args: &Value,
    ) -> Option<Result<Value, AgentLifecycleGatewayError>> {
        let result = dispatch_production_agent_lifecycle_gateway_call_with_all_runtimes(
            &self.session_workers,
            deletion_runtime,
            self.roster_emit(),
            kickstart_created_agent,
            method,
            args,
        );
        if matches!(result.as_ref(), Some(Ok(_))) {
            self.sync_memory_active_agent();
        }
        result
    }

    pub fn dispatch_session_gateway_call(
        &self,
        method: &str,
        args: &Value,
    ) -> Option<Result<Value, SessionGatewayError>> {
        let services = self.production_services();
        let content_search = services
            .as_ref()
            .map(|services| services.content_search.as_ref() as &dyn RosterContentSearch);
        let result = dispatch_production_session_gateway_call_with_content_search_and_group_chat(
            &self.session_workers,
            content_search,
            self.group_chat.as_ref(),
            method,
            args,
        );
        if matches!(result.as_ref(), Some(Ok(_))) {
            self.sync_memory_active_agent();
        }
        result
    }

    fn sync_memory_active_agent(&self) {
        self.session_workers
            .memory_service()
            .set_active_agent(self.active_agent_id());
    }

    pub fn dispatch_memory_gateway_call(
        &self,
        method: &str,
        args: &Value,
    ) -> Option<Result<Value, String>> {
        let required = |name: &str| {
            args.get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{method} requires {name}"))
        };
        match method {
            "getAgentMemories" => Some(required("id").map(|agent_id| {
                Value::Array(
                    self.session_workers
                        .memory_service()
                        .list(agent_id)
                        .into_iter()
                        .map(|memory| {
                            json!({
                                "id": memory.id,
                                "content": memory.content,
                                "createdAt": memory.created_at,
                                "kind": match memory.kind {
                                    MemoryKind::Profile => "profile",
                                    MemoryKind::Log => "log",
                                },
                            })
                        })
                        .collect(),
                )
            })),
            "deleteAgentMemory" => Some(required("id").and_then(|agent_id| {
                let memory_id = required("memoryId")?;
                let removed = self.session_workers.remove_agent_memory(agent_id, memory_id)?;
                Ok(Value::Bool(removed))
            })),
            "clearAgentMemories" => Some(required("id").and_then(|agent_id| {
                self.session_workers.clear_agent_memories(agent_id)?;
                Ok(Value::Null)
            })),
            _ => None,
        }
    }

    pub fn active_agent_id(&self) -> Option<String> {
        self.transcript_runtime.active_agent_id(&self.session_workers)
    }

    fn prepare_live_bounded_open(
        &self,
        agent_id: &str,
        now_ms: f64,
    ) -> Result<bool, String> {
        if self.active_agent_id().as_deref() == Some(agent_id) {
            return Ok(true);
        }
        if self
            .transcript_runtime
            .session_runtime()
            .is_live_session(agent_id)
        {
            self.switch_agent(agent_id, now_ms)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn open_agent_tail(
        &self,
        agent_id: &str,
        query: TranscriptWindowQuery,
        now_ms: f64,
    ) -> Result<TranscriptPage, String> {
        if self.prepare_live_bounded_open(agent_id, now_ms)? {
            return self.session_workers.read_agent_transcript_tail(agent_id, query);
        }
        let page = self.session_workers.read_agent_transcript_tail(agent_id, query)?;
        self.session_workers.mark_agent_viewed(agent_id, now_ms, false)?;
        Ok(page)
    }

    pub fn open_agent_windowed(
        &self,
        agent_id: &str,
        query: TranscriptWindowQuery,
        now_ms: f64,
    ) -> Result<TranscriptWindow<BTreeMap<String, usize>>, String> {
        if self.prepare_live_bounded_open(agent_id, now_ms)? {
            return self.session_workers.read_agent_transcript_window(agent_id, query);
        }
        let window = self.session_workers.read_agent_transcript_window(agent_id, query)?;
        self.session_workers.mark_agent_viewed(agent_id, now_ms, false)?;
        Ok(window)
    }

    pub fn switch_agent(
        &self,
        agent_id: &str,
        now_ms: f64,
    ) -> Result<Vec<Value>, String> {
        let transcript = self
            .transcript_runtime
            .switch_agent(&self.session_workers, agent_id, now_ms)?;
        self.sync_memory_active_agent();
        let next = self.automation_runtime.watch_agent_automations(agent_id)?;
        let previous = self
            .watched_automation_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(next);
        if let Some(previous) = previous {
            previous.set_on_change(None);
        }
        self.workflow_commands.watch_agent_workflows(agent_id)?;
        Ok(transcript)
    }

    pub fn set_window_focused(
        &self,
        is_focused: bool,
        now_ms: f64,
    ) -> Result<bool, String> {
        self.transcript_runtime
            .session_runtime()
            .set_window_focused(&self.session_workers, is_focused, now_ms)
    }

    pub fn window_focused_at_ms(&self) -> Option<f64> {
        self.transcript_runtime.session_runtime().window_focused_at_ms()
    }

    pub fn get_async_tasks(
        &self,
        agent_id: &str,
        live_tasks: &[AsyncTask],
    ) -> Vec<AsyncTask> {
        self.transcript_runtime.get_async_tasks(agent_id, live_tasks)
    }

    pub fn decorate_agent_summaries(&self, value: &mut Value) {
        self.transcript_runtime.decorate_agent_summaries(value);
    }

    pub fn clear_agent_durable_recovery(&self, agent_id: &str) {
        self.transcript_runtime.clear_agent_durable_recovery(agent_id);
    }

    pub fn live_running_agent_ids(&self) -> Vec<String> {
        self.transcript_runtime.live_running_agent_ids()
    }

    pub fn active_turn_source(&self, agent_id: &str) -> Option<String> {
        self.transcript_runtime.active_turn_source(agent_id)
    }

    pub fn has_carryable_pending_wake(&self) -> bool {
        self.transcript_runtime.has_carryable_pending_wake()
    }

    pub fn recreate_carry_pending_wakes(&self) -> Vec<DurablePendingWakeMarker> {
        self.transcript_runtime.recreate_carry_pending_wakes()
    }

    pub fn quiesce_for_upgrade(&self) -> UpgradeQuiesceSummary {
        if let Some(services) = self.production_services() {
            services.trace_flusher.flush_tracing();
        }
        self.runner_registry.request_quiesce_for_upgrade();
        self.transcript_runtime.quiesce_for_upgrade()
    }

    pub fn is_quiescing_for_upgrade(&self) -> bool {
        self.transcript_runtime.is_quiescing_for_upgrade()
    }

    pub fn mark_upgrade_resume_pending(&self, marker: UpgradeResumeMarker) {
        if let Some(store) = self.transcript_runtime.upgrade_resume_store() {
            store.mark_pending(marker);
        }
    }

    pub fn resume_after_recreate(&self) {
        self.runner_registry.cancel_quiesce_for_upgrade();
        self.transcript_runtime.resume_after_recreate();
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }

    /// Mirrors the frozen manager's process-stop ownership without deleting
    /// durable recovery/ack state that must survive restart.
    pub fn dispose(&self) {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return;
        }

        self.set_agent_run_lifecycle_observer(None);
        self.set_automation_config_changed_observer(None);
        self.set_channel_config_changed_observer(None);
        self.set_listener_connect_observer(None);
        self.session_workers.memory_service().set_active_agent(None);
        if let Some(roster) = self
            .roster_emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            roster.stop_outline_stream_coalescing();
        }
        self.transcript_runtime.dispose();

        if let Some(handoff) = self
            .handoff_service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            handoff.clear_all();
        }

        self.ack_obligations.dispose();
        self.automation_runtime.dispose();
        self.workflow_commands.dispose();
        self.pending_wakes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        *self
            .background_wakes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = BackgroundWakes::default();
        *self
            .client_side_tool_v2
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            ClientSideToolV2Producer::new();

        if let Some(store) = self
            .watched_automation_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            store.set_on_change(None);
        }

        self.runner_registry
            .cancel_all("TranscriptManager disposed");
        self.session_workers.shutdown_with_checkpoint();
    }
}
