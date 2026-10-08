//! Mahayana-owned coding Agent engine.
//!
//! The engine owns the Agent loop, session state, policy/approval boundary,
//! workspace tools, checkpoints, memory, workflows, prompt queue, and
//! subagents. Model inference is injected through `mahayana-model`; Codex and
//! Grok Build are not protocol dependencies of this crate.

use async_trait::async_trait;
use mahayana_kernel::supervisor::{
    ApprovalLedger, ApprovalOutcome, ApprovalRecord, LoopDisposition, LoopPolicy, LoopState,
    PermissionDecision, PermissionKey, PermissionLedger, PermissionMemory,
};
use mahayana_kernel::telemetry::{
    RuntimeMetricsSnapshot, RuntimeTelemetry, SessionPersistenceOperation,
};
use mahayana_kernel::{
    ApprovalResolution, BackendDescriptor, Capability, CapabilitySet, EngineBackend,
    ExecutionPolicy, KernelError, KernelEvent, OpenSessionRequest, OperationId,
    ResumeOperationRequest, RiskLevel, RunRequest, SessionId,
    SessionSnapshot as KernelSessionSnapshot, SharedKernelEventSink, SuspendOperationRequest,
};
use mahayana_model::{
    ModelError, ModelEvent, ModelEventSink, ModelProviderMode, ModelRequest, ModelRuntime,
    ModelUsage, SharedModelEventSink,
};
use mahayana_orchestrator::{
    HookEffect, HookPoint, HookRegistry, MemoryStore, PromptEntry, PromptPriority, PromptQueue,
    SubagentScheduler, Workflow,
};
use mahayana_workspace_engine::WorkspaceEngine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};
use uuid::Uuid;

mod web_research;
use web_research::{WebResearchClient, WebResearchConfig};

const MAIN_ASSISTANT_CONVERSATION_ID: &str = "mahayana-ai:agent:assistant";
const CONVERSATION_FAST_LANE_MAX_CHARS: usize = 280;
const CONVERSATION_FAST_LANE_INSTRUCTION: &str = "CHAT-013 direct conversation fast lane: answer this simple conversational turn directly in plain text. Do not attempt any tool call. The native Host streams provider text into the canonical user-visible transcript and records delivery through the Host path.";
const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_MODEL_TURNS: usize = 16;
const MAX_REPLY_NUDGES: usize = 3;
const REPLY_NUDGE_PROMPT: &str = "Your previous turn left the user without the result they're waiting on — you never called send_message that turn, or every send_message you tried failed to deliver. Deliver the result now by actually invoking the send_message tool. Plain assistant text is not user-visible for this turn; only a successful send_message satisfies delivery.";
const CLOSING_SEND_NUDGE_PROMPT: &str = "Your previous turn already sent an acknowledgement, then continued with tool work, but ended without a follow-up send_message. The user can only see the earlier acknowledgement. If the work produced the result they are waiting on, deliver it now with a real send_message tool call. If work is genuinely unfinished, continue it and send the result when ready.";
const DEFAULT_APPROVAL_TIMEOUT_MS: u64 = 120_000;

#[derive(Debug, Clone, Default)]
pub enum ProcessExecution {
    #[default]
    Host,
    LocalDocker {
        docker_path: PathBuf,
        image: String,
    },
}

#[derive(Debug, Clone)]
pub struct NativeEngineConfig {
    pub model: String,
    pub system_instructions: String,
    pub max_model_turns: usize,
    pub enable_process_tools: bool,
    pub approval_timeout_ms: u64,
    pub process_execution: ProcessExecution,
    pub session_state_path: Option<PathBuf>,
}

impl NativeEngineConfig {
    pub fn desktop(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system_instructions: default_system_instructions(),
            max_model_turns: DEFAULT_MAX_MODEL_TURNS,
            enable_process_tools: true,
            approval_timeout_ms: DEFAULT_APPROVAL_TIMEOUT_MS,
            process_execution: ProcessExecution::Host,
            session_state_path: None,
        }
    }

    pub fn embedded(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system_instructions: default_system_instructions(),
            max_model_turns: DEFAULT_MAX_MODEL_TURNS,
            enable_process_tools: false,
            approval_timeout_ms: DEFAULT_APPROVAL_TIMEOUT_MS,
            process_execution: ProcessExecution::Host,
            session_state_path: None,
        }
    }

    fn validate(&self) -> Result<(), KernelError> {
        if self.model.trim().is_empty() {
            return Err(KernelError::BackendUnavailable(
                "Mahayana native engine model must not be empty".into(),
            ));
        }
        if self.max_model_turns == 0 {
            return Err(KernelError::BackendUnavailable(
                "Mahayana native engine max_model_turns must be at least one".into(),
            ));
        }
        if self.approval_timeout_ms == 0 {
            return Err(KernelError::BackendUnavailable(
                "Mahayana native engine approval timeout must be positive".into(),
            ));
        }
        if let ProcessExecution::LocalDocker { docker_path, image } = &self.process_execution {
            if docker_path.as_os_str().is_empty() {
                return Err(KernelError::BackendUnavailable(
                    "Local Docker executable path must not be empty".into(),
                ));
            }
            if !is_pinned_container_image(image) {
                return Err(KernelError::BackendUnavailable(
                    "Local Docker image must be pinned by sha256 digest".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OperationAttemptState {
    Running,
    Suspended,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationAttempt {
    id: String,
    operation_id: String,
    prompt_id: String,
    started_at_ms: i64,
    finished_at_ms: Option<i64>,
    state: OperationAttemptState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InflightToolCheckpoint {
    operation_id: String,
    call_id: String,
    tool: String,
    arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompletedToolCheckpoint {
    operation_id: String,
    call_id: String,
    tool: String,
    output: Value,
    success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NativeSession {
    workspace_root: Option<PathBuf>,
    history: Vec<Value>,
    prompt_queue: PromptQueue,
    #[serde(default)]
    active_prompt: Option<PromptEntry>,
    #[serde(default)]
    permissions: PermissionLedger,
    #[serde(default)]
    approvals: ApprovalLedger,
    #[serde(default)]
    loop_state: LoopState,
    #[serde(default)]
    attempts: Vec<OperationAttempt>,
    #[serde(default)]
    inflight_tool: Option<InflightToolCheckpoint>,
    #[serde(default)]
    completed_tool_results: BTreeMap<String, CompletedToolCheckpoint>,
    #[serde(default)]
    completed_outputs: BTreeMap<String, String>,
    #[serde(default)]
    updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NativeSnapshotState {
    session: NativeSession,
    memory: MemoryStore,
    workflows: HashMap<String, Workflow>,
    subagents: SubagentScheduler,
    hooks: HookRegistry,
}

struct ApprovalWaiter {
    operation_id: String,
    sender: oneshot::Sender<ApprovalResolution>,
}

#[derive(Debug, Default)]
struct OperationControl {
    interrupted: AtomicBool,
    suspended: AtomicBool,
    suspension_persisted: AtomicBool,
    suspension_settled: Notify,
}

pub struct NativeEngine {
    model: Arc<dyn ModelRuntime>,
    config: NativeEngineConfig,
    sessions: Mutex<HashMap<String, Arc<AsyncMutex<NativeSession>>>>,
    active_operations: Mutex<HashMap<String, Arc<OperationControl>>>,
    approvals: Mutex<HashMap<String, ApprovalWaiter>>,
    memory: Mutex<MemoryStore>,
    workflows: Mutex<HashMap<String, Workflow>>,
    subagents: Mutex<SubagentScheduler>,
    hooks: Mutex<HookRegistry>,
    telemetry: Arc<RuntimeTelemetry>,
    persisted_sessions: Mutex<HashMap<String, PathBuf>>,
    web_research: Option<WebResearchClient>,
}

impl NativeEngine {
    pub fn new(
        model: Arc<dyn ModelRuntime>,
        config: NativeEngineConfig,
    ) -> Result<Self, KernelError> {
        let web_config = WebResearchConfig::tinyfish_from_env();
        Self::new_with_web_config(model, config, web_config)
    }

    fn new_with_web_config(
        model: Arc<dyn ModelRuntime>,
        config: NativeEngineConfig,
        web_config: Option<WebResearchConfig>,
    ) -> Result<Self, KernelError> {
        config.validate()?;
        let web_research = web_config.map(WebResearchClient::new).transpose()?;
        Ok(Self {
            model,
            config,
            sessions: Mutex::new(HashMap::new()),
            active_operations: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            memory: Mutex::new(MemoryStore::default()),
            workflows: Mutex::new(HashMap::new()),
            subagents: Mutex::new(
                SubagentScheduler::new(4)
                    .map_err(|error| KernelError::Backend(error.to_string()))?,
            ),
            hooks: Mutex::new(HookRegistry::default()),
            telemetry: Arc::new(RuntimeTelemetry::default()),
            persisted_sessions: Mutex::new(HashMap::new()),
            web_research,
        })
    }

    pub fn metrics_snapshot(&self) -> RuntimeMetricsSnapshot {
        self.telemetry.snapshot()
    }

    /// Clears all account-bound native Agent state. The active operation flags
    /// are set before the registries are cleared so in-flight work exits at
    /// its next cancellation checkpoint instead of crossing an account
    /// boundary.
    pub fn reset_session(&self) -> Result<(), KernelError> {
        {
            let operations = self
                .active_operations
                .lock()
                .map_err(|_| KernelError::Backend("native operation registry poisoned".into()))?;
            for control in operations.values() {
                control.interrupted.store(true, Ordering::SeqCst);
            }
        }
        self.active_operations
            .lock()
            .map_err(|_| KernelError::Backend("native operation registry poisoned".into()))?
            .clear();
        self.approvals
            .lock()
            .map_err(|_| KernelError::Backend("approval registry poisoned".into()))?
            .clear();
        self.sessions
            .lock()
            .map_err(|_| KernelError::Backend("native session registry poisoned".into()))?
            .clear();
        let persisted_session_paths = self
            .persisted_sessions
            .lock()
            .map_err(|_| KernelError::Backend("persisted session registry poisoned".into()))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        self.persisted_sessions
            .lock()
            .map_err(|_| KernelError::Backend("persisted session registry poisoned".into()))?
            .clear();
        *self
            .memory
            .lock()
            .map_err(|_| KernelError::Backend("memory store poisoned".into()))? =
            MemoryStore::default();
        self.workflows
            .lock()
            .map_err(|_| KernelError::Backend("workflow store poisoned".into()))?
            .clear();
        *self
            .subagents
            .lock()
            .map_err(|_| KernelError::Backend("subagent scheduler poisoned".into()))? =
            SubagentScheduler::default();
        for path in persisted_session_paths {
            remove_session_state_file(&path)?;
        }
        if let Some(path) = self.config.session_state_path.as_deref() {
            remove_all_conversation_session_state_files(path)?;
        }
        Ok(())
    }

    pub fn register_hook(
        &self,
        point: HookPoint,
        priority: i32,
        matcher: Option<String>,
        effect: HookEffect,
    ) -> Result<String, KernelError> {
        Ok(self
            .hooks
            .lock()
            .map_err(|_| KernelError::Backend("hook registry poisoned".into()))?
            .register(point, priority, matcher, effect))
    }

    pub async fn set_permission_memory(
        &self,
        session_id: &SessionId,
        key: PermissionKey,
        memory: PermissionMemory,
    ) -> Result<(), KernelError> {
        let session = self.session(session_id)?;
        session.lock().await.permissions.remember(key, memory);
        Ok(())
    }

    fn capabilities(&self) -> CapabilitySet {
        let mut capabilities = vec![
            Capability::Model,
            Capability::FilesystemRead,
            Capability::FilesystemWrite,
            Capability::Workspace,
            Capability::Checkpoint,
            Capability::Worktree,
            Capability::CodebaseGraph,
            Capability::Memory,
            Capability::Workflow,
            Capability::PromptQueue,
            Capability::Subagent,
            Capability::Hooks,
            Capability::ToolProtocol,
        ];
        if !self.model.is_local() || self.web_research.is_some() {
            capabilities.push(Capability::Network);
        }
        if self.web_research.is_some() {
            capabilities.push(Capability::WebSearch);
        }
        if self.config.enable_process_tools {
            capabilities.push(Capability::Process);
            capabilities.push(Capability::Git);
        }
        CapabilitySet::new(capabilities)
    }

    fn session(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<AsyncMutex<NativeSession>>, KernelError> {
        self.sessions
            .lock()
            .map_err(|_| KernelError::Backend("native session registry poisoned".into()))?
            .get(session_id.as_str())
            .cloned()
            .ok_or_else(|| KernelError::SessionNotFound(session_id.as_str().to_string()))
    }

    fn register_operation(
        &self,
        operation_id: &OperationId,
    ) -> Result<Arc<OperationControl>, KernelError> {
        let control = Arc::new(OperationControl::default());
        self.active_operations
            .lock()
            .map_err(|_| KernelError::Backend("native operation registry poisoned".into()))?
            .insert(operation_id.as_str().to_string(), Arc::clone(&control));
        Ok(control)
    }

    fn finish_operation(&self, operation_id: &OperationId) -> Result<(), KernelError> {
        self.active_operations
            .lock()
            .map_err(|_| KernelError::Backend("native operation registry poisoned".into()))?
            .remove(operation_id.as_str());
        Ok(())
    }

    fn cancel_operation_approvals(&self, operation_id: &OperationId) -> Result<(), KernelError> {
        self.approvals
            .lock()
            .map_err(|_| KernelError::Backend("approval registry poisoned".into()))?
            .retain(|_, waiter| waiter.operation_id != operation_id.as_str());
        Ok(())
    }

    fn persist_session_state_if_configured(
        &self,
        session_id: &SessionId,
        session: &NativeSession,
    ) -> Result<(), KernelError> {
        let path = self
            .persisted_sessions
            .lock()
            .map_err(|_| KernelError::Backend("persisted session registry poisoned".into()))?
            .get(session_id.as_str())
            .cloned();
        let Some(path) = path else {
            return Ok(());
        };

        let started = Instant::now();
        let mut byte_count = 0_u64;
        let result = (|| -> Result<(), KernelError> {
            let state = NativeSnapshotState {
                session: session.clone(),
                memory: self
                    .memory
                    .lock()
                    .map_err(|_| KernelError::Backend("memory store poisoned".into()))?
                    .clone(),
                workflows: self
                    .workflows
                    .lock()
                    .map_err(|_| KernelError::Backend("workflow store poisoned".into()))?
                    .clone(),
                subagents: self
                    .subagents
                    .lock()
                    .map_err(|_| KernelError::Backend("subagent scheduler poisoned".into()))?
                    .clone(),
                hooks: self
                    .hooks
                    .lock()
                    .map_err(|_| KernelError::Backend("hook registry poisoned".into()))?
                    .clone(),
            };
            let snapshot = KernelSessionSnapshot {
                session_id: session_id.clone(),
                backend_id: "mahayana-native".into(),
                state: serde_json::to_value(state)
                    .map_err(|error| KernelError::Backend(error.to_string()))?,
                metadata: json!({"snapshotVersion": 1, "updatedAtMs": session.updated_at_ms}),
            };
            let bytes = serde_json::to_vec(&snapshot)
                .map_err(|error| KernelError::Backend(error.to_string()))?;
            byte_count = bytes.len().min(u64::MAX as usize) as u64;
            let parent = path.parent().ok_or_else(|| {
                KernelError::Backend("persisted session path has no parent".into())
            })?;
            std::fs::create_dir_all(parent)
                .map_err(|error| KernelError::Backend(error.to_string()))?;
            let temporary = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
            write_private_file(&temporary, &bytes)?;
            replace_file(&temporary, &path)?;
            Ok(())
        })();

        self.telemetry.session_persistence_finished(
            SessionPersistenceOperation::Checkpoint,
            byte_count,
            started.elapsed(),
            result.is_ok(),
        );
        result
    }

    async fn persist_session_if_configured(
        &self,
        session_id: &SessionId,
    ) -> Result<(), KernelError> {
        let session = self.session(session_id)?;
        let session = session.lock().await.clone();
        self.persist_session_state_if_configured(session_id, &session)
    }

    fn apply_hooks(
        &self,
        point: HookPoint,
        subject: &str,
        mut session: Option<&mut NativeSession>,
        operation_id: &OperationId,
        events: &SharedKernelEventSink,
    ) -> Result<(), KernelError> {
        let hooks = self
            .hooks
            .lock()
            .map_err(|_| KernelError::Backend("hook registry poisoned".into()))?
            .dispatch(point, subject);
        for hook in hooks {
            events.emit(KernelEvent::Activity {
                operation_id: operation_id.clone(),
                kind: "hook".into(),
                title: format!("Mahayana hook {}", hook.id),
                detail: hook.effect.block_reason.clone(),
                metadata: json!({"point": format!("{point:?}"), "hookId": hook.id}),
            })?;
            if let Some(reason) = hook.effect.block_reason {
                return Err(KernelError::PolicyDenied(format!(
                    "hook blocked {subject}: {reason}"
                )));
            }
            if hook.effect.require_approval {
                return Err(KernelError::PolicyDenied(format!(
                    "hook requires explicit approval before {subject}"
                )));
            }
            if let Some(context) = hook.effect.inject_context
                && let Some(active_session) = session.as_deref_mut()
            {
                active_session
                    .history
                    .push(json!({"role":"system", "content": context, "source":"mahayana_hook"}));
            }
        }
        Ok(())
    }

    async fn run_prompt(
        &self,
        session_id: &SessionId,
        session: &mut NativeSession,
        operation_id: &OperationId,
        prompt: String,
        prompt_metadata: &Value,
        append_user_prompt: bool,
        policy: &ExecutionPolicy,
        control: &OperationControl,
        events: SharedKernelEventSink,
    ) -> Result<String, KernelError> {
        if append_user_prompt {
            session.history.push(json!({
                "role": "user",
                "content": model_user_content(&prompt, prompt_metadata),
            }));
        }

        // Some OpenAI-compatible providers silently turn an explicit tool
        // request into prose even when tools are present in the request. Keep
        // the execution contract on the Agent side: when the user names
        // concrete Mahayana tools, derive a small ordered plan and only use
        // it when the model omitted the corresponding function call. The plan
        // still goes through authorization, execute_tool, loop protection,
        // and function_call_output history, so the UI reflects real work.
        let mut explicit_tool_plan = explicit_tool_request_plan(&prompt);
        let mut last_workflow_id: Option<String> = None;
        let visible_user_turn =
            prompt_metadata.get("hidden").and_then(Value::as_bool) == Some(false);
        let turn_started = Instant::now();
        let mut delivered_message = false;
        let mut reply_nudge_attempts = 0usize;
        let mut tool_call_count = 0u64;
        let mut stream_output_produced = false;
        let mut tool_work_after_delivery = false;
        let mut closing_send_nudge_attempted =
            has_closing_send_nudge_for_operation(&session.history, operation_id);

        for turn in 0..self.config.max_model_turns {
            ensure_operation_active(control)?;
            if !self.model.is_local() && !policy.allow_network {
                return Err(KernelError::PolicyDenied(
                    "remote model inference is disabled by Mahayana policy".into(),
                ));
            }

            self.apply_hooks(
                HookPoint::BeforeModel,
                &self.config.model,
                Some(session),
                operation_id,
                &events,
            )?;
            events.emit(KernelEvent::Activity {
                operation_id: operation_id.clone(),
                kind: "model".into(),
                title: format!("Mahayana reasoning turn {}", turn + 1),
                detail: None,
                metadata: json!({"engine": "mahayana-native"}),
            })?;

            let full_declared_tools =
                tool_definitions(self.config.enable_process_tools, self.web_research.is_some());
            let direct_conversation_fast_lane = visible_user_turn
                && self.model.provider_mode() == ModelProviderMode::FirstPartyDacheng
                && is_conversation_fast_lane(&session.history);
            let declared_tools = model_turn_tools(
                self.model.provider_mode(),
                visible_user_turn,
                &session.history,
                &full_declared_tools,
            );
            let mut model_instructions = self.config.system_instructions.clone();
            if let Some(reaction_context) =
                reaction_message_reference_context(prompt_metadata)
            {
                if !model_instructions.trim().is_empty() {
                    model_instructions.push_str("\n\n");
                }
                model_instructions.push_str(&reaction_context);
            }
            if direct_conversation_fast_lane {
                if !model_instructions.trim().is_empty() {
                    model_instructions.push_str("\n\n");
                }
                model_instructions.push_str(CONVERSATION_FAST_LANE_INSTRUCTION);
            }
            let collector = Arc::new(if visible_user_turn && !direct_conversation_fast_lane {
                ModelCollector::buffered()
            } else {
                ModelCollector::streaming(Arc::clone(&events), operation_id.clone())
            });
            let sink: SharedModelEventSink = collector.clone();
            let started = Instant::now();
            let inference = self
                .model
                .infer(
                    ModelRequest {
                        model: self.config.model.clone(),
                        input: Value::Array(session.history.clone()),
                        metadata: json!({
                            "instructions": model_instructions,
                            "tools": declared_tools.clone(),
                            "tool_choice": "auto",
                            "parallel_tool_calls": false,
                        }),
                    },
                    sink,
                )
                .await;
            self.telemetry
                .model_finished(started.elapsed(), inference.is_ok());
            inference.map_err(model_error)?;
            ensure_operation_active(control)?;
            self.apply_hooks(
                HookPoint::AfterModel,
                &self.config.model,
                Some(session),
                operation_id,
                &events,
            )?;

            if let Some(usage) = collector.usage()? {
                events.emit(KernelEvent::UsageUpdated {
                    operation_id: operation_id.clone(),
                    total_tokens: usage.total_tokens,
                    input_tokens: usage.input_tokens,
                    cached_input_tokens: usage.cached_input_tokens,
                    output_tokens: usage.output_tokens,
                    reasoning_output_tokens: usage.reasoning_output_tokens,
                })?;
            }
            let payload = collector.output()?.ok_or_else(|| {
                KernelError::Backend("model runtime completed without a payload".into())
            })?;
            let step_text = collector.text()?;
            let mut calls = extract_function_calls(&payload)?;
            let mut normalized_dsml = false;
            if calls.is_empty() {
                let payload_text = if step_text.trim().is_empty() {
                    mahayana_model::responses::extract_output_text(&payload)
                } else {
                    None
                };
                let compatibility_text = if step_text.trim().is_empty() {
                    payload_text.as_deref().unwrap_or_default()
                } else {
                    step_text.as_str()
                };
                if let Some(dsml_calls) =
                    dsml_compat_function_calls(compatibility_text, &declared_tools, turn)
                {
                    // Do not append all normalized calls up front. Each executable
                    // call is recorded immediately before its own result so the next
                    // model turn always sees balanced call/result pairs.
                    calls = dsml_calls;
                    normalized_dsml = true;
                }
            }
            if !normalized_dsml {
                append_model_output(&mut session.history, &payload);
            }
            if calls.is_empty()
                && let Some(call) = explicit_tool_plan.first().cloned()
            {
                explicit_tool_plan.remove(0);
                calls.push(call);
            }
            if !calls.is_empty() {
                let called_names = calls
                    .iter()
                    .map(|call| call.name.as_str())
                    .collect::<std::collections::HashSet<_>>();
                explicit_tool_plan.retain(|planned| !called_names.contains(planned.name.as_str()));
            }
            if calls.is_empty() {
                let text = mahayana_model::responses::extract_output_text(&payload)
                    .or_else(|| (!step_text.is_empty()).then(|| step_text.clone()))
                    .ok_or_else(|| {
                        KernelError::Backend(
                            "model completed without assistant text or tool calls".into(),
                        )
                    })?;
                stream_output_produced |= !text.is_empty();
                if direct_conversation_fast_lane {
                    events.emit(KernelEvent::MessageCompleted {
                        operation_id: operation_id.clone(),
                        text: text.clone(),
                    })?;
                    return Ok(text);
                }
                if visible_user_turn
                    && should_attempt_reply_nudge(delivered_message, reply_nudge_attempts, control)
                {
                    reply_nudge_attempts = reply_nudge_attempts.saturating_add(1);
                    session.history.push(json!({
                        "role": "user",
                        "content": shape_hidden_nudge_content(REPLY_NUDGE_PROMPT, &prompt),
                        "source": "mahayana_reply_nudge",
                    }));
                    continue;
                }
                if visible_user_turn
                    && should_attempt_closing_send_nudge(
                        delivered_message,
                        tool_work_after_delivery,
                        closing_send_nudge_attempted,
                        control,
                    )
                {
                    closing_send_nudge_attempted = true;
                    self.telemetry.closing_send_nudge();
                    session.history.push(json!({
                        "role": "user",
                        "content": shape_hidden_nudge_content(CLOSING_SEND_NUDGE_PROMPT, &prompt),
                        "source": "mahayana_closing_send_nudge",
                        "operationId": operation_id.as_str(),
                    }));
                    continue;
                }
                if visible_user_turn && !delivered_message {
                    self.telemetry.turn_empty_delivery(
                        reply_nudge_attempts as u64,
                        tool_call_count,
                        stream_output_produced,
                        turn_started.elapsed(),
                    );
                }
                if !visible_user_turn {
                    events.emit(KernelEvent::MessageDelta {
                        operation_id: operation_id.clone(),
                        delta: text.clone(),
                    })?;
                    events.emit(KernelEvent::MessageCompleted {
                        operation_id: operation_id.clone(),
                        text: text.clone(),
                    })?;
                }
                return Ok(text);
            }

            for call in calls {
                ensure_operation_active(control)?;
                let mut call = call;
                tool_call_count = tool_call_count.saturating_add(1);
                if delivered_message && !is_delivery_tool(&call.name) {
                    tool_work_after_delivery = true;
                }
                if call.name == "workflow_status"
                    && call.arguments.get("workflow_id").and_then(Value::as_str)
                        == Some("$last_workflow_id")
                    && let Some(workflow_id) = last_workflow_id.clone()
                {
                    call.arguments["workflow_id"] = Value::String(workflow_id);
                }
                let fingerprint = tool_fingerprint(&call);
                match session
                    .loop_state
                    .observe(&fingerprint, LoopPolicy::default())
                {
                    LoopDisposition::Allow => {}
                    LoopDisposition::Warn => {
                        events.emit(KernelEvent::Activity {
                            operation_id: operation_id.clone(),
                            kind: "loop_warning".into(),
                            title: format!("Repeated tool action: {}", call.name),
                            detail: Some("Mahayana detected a repeated action and will interrupt if it continues".into()),
                            metadata: json!({"fingerprint": fingerprint}),
                        })?;
                    }
                    LoopDisposition::Interrupt => {
                        return Err(KernelError::PolicyDenied(format!(
                            "Mahayana loop protection interrupted repeated tool action {}",
                            call.name
                        )));
                    }
                }

                // Record only the call that is actually about to execute. Keeping
                // this adjacent to its result prevents parallel provider output from
                // becoming [call1, call2, result1, result2] in replay history.
                append_function_call_history(&mut session.history, &call);
                session.inflight_tool = Some(InflightToolCheckpoint {
                    operation_id: operation_id.as_str().to_string(),
                    call_id: call.call_id.clone(),
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                });
                session.updated_at_ms = now_ms();
                self.persist_session_state_if_configured(session_id, session)?;
                self.apply_hooks(
                    HookPoint::BeforeTool,
                    &call.name,
                    Some(session),
                    operation_id,
                    &events,
                )?;
                self.telemetry.tool_started();
                events.emit(KernelEvent::ToolStarted {
                    operation_id: operation_id.clone(),
                    tool_call_id: call.call_id.clone(),
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                })?;
                let output = self
                    .execute_tool(
                        session,
                        operation_id,
                        &call,
                        policy,
                        control,
                        Arc::clone(&events),
                    )
                    .await;
                match output {
                    Ok(mut output) => {
                        if is_delivery_tool(&call.name)
                            && output
                                .get("delivered")
                                .and_then(Value::as_bool)
                                .unwrap_or(false)
                        {
                            delivered_message = true;
                            tool_work_after_delivery = false;
                            if closing_send_nudge_attempted {
                                output["syntheticClosingSendNudge"] = Value::Bool(true);
                            } else if reply_nudge_attempts > 0 {
                                output["syntheticReplyNudge"] = Value::Bool(true);
                            }
                        }
                        let waiting_user = call.name == "request_box_help";
                        if call.name == "workflow_create" {
                            last_workflow_id = output
                                .get("workflow_id")
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                        }
                        let completion_key = tool_completion_key(operation_id, &call.call_id);
                        if tool_completion_revival_supported(&call.name) {
                            session.completed_tool_results.insert(
                                completion_key.clone(),
                                CompletedToolCheckpoint {
                                    operation_id: operation_id.as_str().to_string(),
                                    call_id: call.call_id.clone(),
                                    tool: call.name.clone(),
                                    output: output.clone(),
                                    success: true,
                                },
                            );
                            while session.completed_tool_results.len() > 128 {
                                if let Some(first) = session.completed_tool_results.keys().next().cloned() {
                                    session.completed_tool_results.remove(&first);
                                } else {
                                    break;
                                }
                            }
                            session.updated_at_ms = now_ms();
                            self.persist_session_state_if_configured(session_id, session)?;
                        }
                        self.telemetry.tool_completed(true);
                        events.emit(KernelEvent::ToolCompleted {
                            operation_id: operation_id.clone(),
                            tool_call_id: call.call_id.clone(),
                            tool: call.name.clone(),
                            output: output.clone(),
                            success: true,
                        })?;
                        append_function_call_output_history(
                            &mut session.history,
                            &call.call_id,
                            &output,
                        );
                        session.inflight_tool = None;
                        session.completed_tool_results.remove(&completion_key);
                        session.updated_at_ms = now_ms();
                        self.persist_session_state_if_configured(session_id, session)?;
                        if waiting_user {
                            return Ok("waiting_user".into());
                        }
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let failure_output = json!({"error": message.clone()});
                        let completion_key = tool_completion_key(operation_id, &call.call_id);
                        if tool_completion_revival_supported(&call.name) {
                            session.completed_tool_results.insert(
                                completion_key.clone(),
                                CompletedToolCheckpoint {
                                    operation_id: operation_id.as_str().to_string(),
                                    call_id: call.call_id.clone(),
                                    tool: call.name.clone(),
                                    output: failure_output.clone(),
                                    success: false,
                                },
                            );
                            while session.completed_tool_results.len() > 128 {
                                if let Some(first) = session.completed_tool_results.keys().next().cloned() {
                                    session.completed_tool_results.remove(&first);
                                } else {
                                    break;
                                }
                            }
                            session.updated_at_ms = now_ms();
                            self.persist_session_state_if_configured(session_id, session)?;
                        }
                        self.telemetry.tool_completed(false);
                        events.emit(KernelEvent::ToolCompleted {
                            operation_id: operation_id.clone(),
                            tool_call_id: call.call_id.clone(),
                            tool: call.name.clone(),
                            output: failure_output.clone(),
                            success: false,
                        })?;
                        append_function_call_output_history(
                            &mut session.history,
                            &call.call_id,
                            &failure_output,
                        );
                        session.inflight_tool = None;
                        session.completed_tool_results.remove(&completion_key);
                        session.updated_at_ms = now_ms();
                        self.persist_session_state_if_configured(session_id, session)?;
                    }
                }
                self.apply_hooks(
                    HookPoint::AfterTool,
                    &call.name,
                    Some(session),
                    operation_id,
                    &events,
                )?;
            }
        }

        Err(KernelError::Backend(format!(
            "native Agent exceeded {} model turns",
            self.config.max_model_turns
        )))
    }

    fn execute_tool<'a>(
        &'a self,
        session: &'a mut NativeSession,
        operation_id: &'a OperationId,
        call: &'a FunctionCall,
        policy: &'a ExecutionPolicy,
        control: &'a OperationControl,
        events: SharedKernelEventSink,
    ) -> Pin<Box<dyn Future<Output = Result<Value, KernelError>> + Send + 'a>> {
        Box::pin(async move {
            let risk = tool_risk(&call.name);
            self.authorize_tool(
                session,
                operation_id,
                call,
                risk,
                policy,
                Arc::clone(&events),
            )
            .await?;
            ensure_operation_active(control)?;

            match call.name.as_str() {
                "send_message" => {
                    let message = call
                        .arguments
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    let attachment = call
                        .arguments
                        .get("attachment")
                        .and_then(Value::as_object)
                        .cloned()
                        .map(Value::Object);
                    if message.is_none() && attachment.is_none() {
                        return Err(KernelError::Backend(
                            "send_message requires a non-empty message or attachment".into(),
                        ));
                    }
                    let reply_to_message_id = call
                        .arguments
                        .get("reply_to_message_id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    Ok(json!({
                        "delivered": true,
                        "characters": message.map(|value| value.chars().count()).unwrap_or_default(),
                        "generatedMessage": message,
                        "generatedAttachment": attachment,
                        "toolCallId": call.call_id.clone(),
                        "replyToMessageId": reply_to_message_id,
                    }))
                }
                "react_to_message" => {
                    let message_id = string_arg(&call.arguments, "message_id")?.trim();
                    if message_id.is_empty() {
                        return Err(KernelError::Backend(
                            "react_to_message requires a non-empty message_id".into(),
                        ));
                    }
                    let emoji = string_arg(&call.arguments, "emoji")?.trim();
                    if emoji.is_empty() || emoji.encode_utf16().count() > 16 {
                        return Err(KernelError::Backend(
                            "react_to_message emoji must contain 1-16 UTF-16 code units".into(),
                        ));
                    }
                    let is_live_target = session
                        .active_prompt
                        .as_ref()
                        .is_some_and(|prompt| {
                            reaction_message_ref_exists(&prompt.metadata, message_id)
                        });
                    if !is_live_target {
                        return Err(KernelError::Backend(format!(
                            "react_to_message target is not a canonical user message in this turn: {message_id}"
                        )));
                    }
                    Ok(json!({
                        "delivered": true,
                        "applied": true,
                        "messageId": message_id,
                        "emoji": emoji,
                        "toolCallId": call.call_id.clone(),
                    }))
                }
                "request_box_help" => {
                    let instruction = call
                        .arguments
                        .get("instruction")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            KernelError::Backend(
                                "request_box_help requires a non-empty instruction".into(),
                            )
                        })?;
                    let reason = call.arguments.get("reason").cloned().unwrap_or(Value::Null);
                    let domain = call.arguments.get("domain").cloned().unwrap_or(Value::Null);
                    let idp_domain = call
                        .arguments
                        .get("idp_domain")
                        .cloned()
                        .unwrap_or(Value::Null);
                    events.emit(KernelEvent::Activity {
                        operation_id: operation_id.clone(),
                        kind: "box_handoff_request".into(),
                        title: "Waiting for user help".into(),
                        detail: Some(instruction.to_string()),
                        metadata: json!({
                            "stepId": format!("box-handoff:{}", call.call_id),
                            "status": "completed",
                            "provider": {
                                "toolCallId": call.call_id,
                                "instruction": instruction,
                                "reason": reason,
                                "domain": domain,
                                "idp_domain": idp_domain,
                            }
                        }),
                    })?;
                    Ok(json!({
                        "status": "awaiting_user",
                        "instruction": instruction,
                    }))
                }
                "workspace_read" => {
                    let root = workspace_root(session)?;
                    let path = string_arg(&call.arguments, "path")?;
                    let path = safe_join(root, Path::new(path))?;
                    let content = std::fs::read_to_string(&path)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    Ok(json!({"path": relative_display(root, &path), "content": content}))
                }
                "workspace_write" => {
                    if !policy.allow_workspace_writes {
                        return Err(KernelError::PolicyDenied(
                            "workspace writes are disabled by Mahayana policy".into(),
                        ));
                    }
                    self.apply_hooks(
                        HookPoint::BeforeCheckpoint,
                        "workspace_write",
                        None,
                        operation_id,
                        &events,
                    )?;
                    let root = workspace_root(session)?;
                    let relative = string_arg(&call.arguments, "path")?;
                    let content = string_arg(&call.arguments, "content")?;
                    let engine = WorkspaceEngine::open(root)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    let checkpoint = engine
                        .create_checkpoint(Some(format!("before write {relative}")))
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    events.emit(KernelEvent::CheckpointCreated {
                        operation_id: operation_id.clone(),
                        checkpoint_id: checkpoint.id,
                        label: checkpoint.label,
                    })?;
                    self.apply_hooks(
                        HookPoint::AfterCheckpoint,
                        "workspace_write",
                        None,
                        operation_id,
                        &events,
                    )?;
                    let path = safe_join(root, Path::new(relative))?;
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|error| KernelError::Backend(error.to_string()))?;
                    }
                    std::fs::write(&path, content)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    Ok(json!({"path": relative_display(root, &path), "bytes": content.len()}))
                }
                "workspace_search" => {
                    let root = workspace_root(session)?;
                    let query = string_arg(&call.arguments, "query")?;
                    let limit = call
                        .arguments
                        .get("limit")
                        .and_then(Value::as_u64)
                        .unwrap_or(50)
                        .clamp(1, 200) as usize;
                    let matches = search_workspace(root, query, limit)?;
                    Ok(json!({"query": query, "matches": matches}))
                }
                "workspace_checkpoint" => {
                    self.apply_hooks(
                        HookPoint::BeforeCheckpoint,
                        "workspace_checkpoint",
                        None,
                        operation_id,
                        &events,
                    )?;
                    let root = workspace_root(session)?;
                    let label = call
                        .arguments
                        .get("label")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let checkpoint = WorkspaceEngine::open(root)
                        .and_then(|engine| engine.create_checkpoint(label))
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    events.emit(KernelEvent::CheckpointCreated {
                        operation_id: operation_id.clone(),
                        checkpoint_id: checkpoint.id.clone(),
                        label: checkpoint.label.clone(),
                    })?;
                    self.apply_hooks(
                        HookPoint::AfterCheckpoint,
                        "workspace_checkpoint",
                        None,
                        operation_id,
                        &events,
                    )?;
                    serde_json::to_value(checkpoint)
                        .map_err(|error| KernelError::Backend(error.to_string()))
                }
                "workspace_restore" => {
                    if !policy.allow_workspace_writes {
                        return Err(KernelError::PolicyDenied(
                            "workspace writes are disabled by Mahayana policy".into(),
                        ));
                    }
                    self.apply_hooks(
                        HookPoint::BeforeCheckpoint,
                        "workspace_restore",
                        None,
                        operation_id,
                        &events,
                    )?;
                    let root = workspace_root(session)?;
                    let checkpoint_id = string_arg(&call.arguments, "checkpoint_id")?;
                    let engine = WorkspaceEngine::open(root)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    let safety = engine
                        .create_checkpoint(Some(format!("before restore {checkpoint_id}")))
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    events.emit(KernelEvent::CheckpointCreated {
                        operation_id: operation_id.clone(),
                        checkpoint_id: safety.id,
                        label: safety.label,
                    })?;
                    self.apply_hooks(
                        HookPoint::AfterCheckpoint,
                        "workspace_restore",
                        None,
                        operation_id,
                        &events,
                    )?;
                    let restored = engine
                        .restore_checkpoint(checkpoint_id)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    Ok(json!({"restored": restored.id, "files": restored.files.len()}))
                }
                "workspace_worktree" => {
                    let root = workspace_root(session)?;
                    let checkpoint_id = call.arguments.get("checkpoint_id").and_then(Value::as_str);
                    let worktree = WorkspaceEngine::open(root)
                        .and_then(|engine| engine.create_worktree(checkpoint_id))
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    serde_json::to_value(worktree)
                        .map_err(|error| KernelError::Backend(error.to_string()))
                }
                "codebase_graph" => {
                    let root = workspace_root(session)?;
                    let graph = WorkspaceEngine::open(root)
                        .and_then(|engine| engine.build_codebase_graph())
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    serde_json::to_value(graph)
                        .map_err(|error| KernelError::Backend(error.to_string()))
                }
                "code_symbols" => {
                    let root = workspace_root(session)?;
                    let symbols = WorkspaceEngine::open(root)
                        .and_then(|engine| engine.index_symbols())
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    serde_json::to_value(symbols)
                        .map_err(|error| KernelError::Backend(error.to_string()))
                }
                "memory_put" => {
                    let namespace = string_arg(&call.arguments, "namespace")?;
                    let key = string_arg(&call.arguments, "key")?;
                    let value = call.arguments.get("value").cloned().unwrap_or(Value::Null);
                    let tags = call
                        .arguments
                        .get("tags")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let record = self
                        .memory
                        .lock()
                        .map_err(|_| KernelError::Backend("memory store poisoned".into()))?
                        .upsert(namespace, key, value, tags, None)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    serde_json::to_value(record)
                        .map_err(|error| KernelError::Backend(error.to_string()))
                }
                "memory_get" => {
                    let namespace = string_arg(&call.arguments, "namespace")?;
                    let key = string_arg(&call.arguments, "key")?;
                    let record = self
                        .memory
                        .lock()
                        .map_err(|_| KernelError::Backend("memory store poisoned".into()))?
                        .get(namespace, key);
                    Ok(json!({"record": record}))
                }
                "memory_search" => {
                    let query = call.arguments.get("query").and_then(Value::as_str);
                    let namespace = call.arguments.get("namespace").and_then(Value::as_str);
                    let tags = call
                        .arguments
                        .get("tags")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let records = self
                        .memory
                        .lock()
                        .map_err(|_| KernelError::Backend("memory store poisoned".into()))?
                        .search(namespace, query, &tags, 50);
                    Ok(json!({"records": records}))
                }
                "workflow_create" => {
                    let title = string_arg(&call.arguments, "title")?;
                    let mut workflow = Workflow::new(title)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    if let Some(tasks) = call.arguments.get("tasks").and_then(Value::as_array) {
                        for task in tasks {
                            let id = task.get("id").and_then(Value::as_str).ok_or_else(|| {
                                KernelError::Backend("workflow task id is required".into())
                            })?;
                            let task_title =
                                task.get("title").and_then(Value::as_str).unwrap_or(id);
                            let dependencies = task
                                .get("depends_on")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect::<Vec<_>>();
                            workflow
                                .add_task(id, task_title, dependencies, Value::Null)
                                .map_err(|error| KernelError::Backend(error.to_string()))?;
                        }
                    }
                    let id = workflow.id.clone();
                    let snapshot = serde_json::to_value(&workflow)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    self.workflows
                        .lock()
                        .map_err(|_| KernelError::Backend("workflow store poisoned".into()))?
                        .insert(id.clone(), workflow);
                    Ok(json!({"workflow_id": id, "workflow": snapshot}))
                }
                "workflow_status" => {
                    let id = string_arg(&call.arguments, "workflow_id")?;
                    let workflows = self
                        .workflows
                        .lock()
                        .map_err(|_| KernelError::Backend("workflow store poisoned".into()))?;
                    let workflow = workflows
                        .get(id)
                        .ok_or_else(|| KernelError::Backend(format!("workflow not found: {id}")))?;
                    serde_json::to_value(workflow)
                        .map_err(|error| KernelError::Backend(error.to_string()))
                }
                "subagent_run" => {
                    let name = call
                        .arguments
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("subagent");
                    let goal = string_arg(&call.arguments, "goal")?;
                    let task_id = {
                        let mut scheduler = self.subagents.lock().map_err(|_| {
                            KernelError::Backend("subagent scheduler poisoned".into())
                        })?;
                        let task_id = scheduler
                            .spawn(
                                None,
                                name,
                                goal,
                                CapabilitySet::new([Capability::Model]),
                                Value::Null,
                            )
                            .map_err(|error| KernelError::Backend(error.to_string()))?;
                        scheduler
                            .start(&task_id)
                            .map_err(|error| KernelError::Backend(error.to_string()))?;
                        task_id
                    };
                    let result = self
                        .run_subagent(goal, control, Arc::clone(&events), operation_id)
                        .await;
                    match result {
                        Ok(text) => {
                            self.subagents
                                .lock()
                                .map_err(|_| {
                                    KernelError::Backend("subagent scheduler poisoned".into())
                                })?
                                .complete(&task_id, json!({"text": text}))
                                .map_err(|error| KernelError::Backend(error.to_string()))?;
                            Ok(json!({"task_id": task_id, "text": text}))
                        }
                        Err(error) => {
                            let message = error.to_string();
                            let _ = self.subagents.lock().ok().and_then(|mut scheduler| {
                                scheduler.fail(&task_id, message.clone()).ok()
                            });
                            Err(error)
                        }
                    }
                }
                "web_search" => {
                    if !policy.allow_network {
                        return Err(KernelError::PolicyDenied(
                            "web search is disabled by Mahayana network policy".into(),
                        ));
                    }
                    let query = string_arg(&call.arguments, "query")?;
                    let limit = call
                        .arguments
                        .get("limit")
                        .and_then(Value::as_u64)
                        .unwrap_or(10)
                        .clamp(1, 10) as usize;
                    self.web_research
                        .as_ref()
                        .ok_or_else(|| {
                            KernelError::CapabilityUnavailable(
                                "web research provider is not configured".into(),
                            )
                        })?
                        .search(query, limit)
                        .await
                }
                "web_fetch" => {
                    if !policy.allow_network {
                        return Err(KernelError::PolicyDenied(
                            "web fetch is disabled by Mahayana network policy".into(),
                        ));
                    }
                    let urls = call
                        .arguments
                        .get("urls")
                        .and_then(Value::as_array)
                        .ok_or_else(|| KernelError::Backend("web_fetch urls are required".into()))?
                        .iter()
                        .map(|value| {
                            value.as_str().map(str::to_owned).ok_or_else(|| {
                                KernelError::Backend(
                                    "web_fetch urls must contain only strings".into(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let format = call
                        .arguments
                        .get("format")
                        .and_then(Value::as_str)
                        .unwrap_or("markdown");
                    self.web_research
                        .as_ref()
                        .ok_or_else(|| {
                            KernelError::CapabilityUnavailable(
                                "web research provider is not configured".into(),
                            )
                        })?
                        .fetch(&urls, format)
                        .await
                }
                "process_exec" => {
                    if !self.config.enable_process_tools || !policy.allow_process {
                        return Err(KernelError::PolicyDenied(
                            "process execution is disabled by Mahayana policy".into(),
                        ));
                    }
                    let root = workspace_root(session)?;
                    let program = string_arg(&call.arguments, "program")?;
                    let args = call
                        .arguments
                        .get("args")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    run_process(&self.config.process_execution, root, program, &args)
                }
                "git_status" => {
                    if !self.config.enable_process_tools || !policy.allow_process {
                        return Err(KernelError::PolicyDenied(
                            "Git process execution is disabled by Mahayana policy".into(),
                        ));
                    }
                    run_process(
                        &self.config.process_execution,
                        workspace_root(session)?,
                        "git",
                        &["status".into(), "--short".into()],
                    )
                }
                "git_diff" => {
                    if !self.config.enable_process_tools || !policy.allow_process {
                        return Err(KernelError::PolicyDenied(
                            "Git process execution is disabled by Mahayana policy".into(),
                        ));
                    }
                    run_process(
                        &self.config.process_execution,
                        workspace_root(session)?,
                        "git",
                        &["diff".into(), "--".into()],
                    )
                }
                other => Err(KernelError::CapabilityUnavailable(format!(
                    "native tool {other} is not registered"
                ))),
            }
        })
    }

    async fn run_subagent(
        &self,
        goal: &str,
        control: &OperationControl,
        events: SharedKernelEventSink,
        operation_id: &OperationId,
    ) -> Result<String, KernelError> {
        ensure_operation_active(control)?;
        let collector = Arc::new(ModelCollector::default());
        let sink: SharedModelEventSink = collector.clone();
        let started = Instant::now();
        let inference = self
            .model
            .infer(
                ModelRequest {
                    model: self.config.model.clone(),
                    input: json!([{"role":"user", "content": goal}]),
                    metadata: json!({
                        "instructions": "You are a focused Mahayana subagent. Solve only the delegated goal and return a concise result. Do not claim tools you were not given."
                    }),
                },
                sink,
            )
            .await;
        self.telemetry
            .model_finished(started.elapsed(), inference.is_ok());
        inference.map_err(model_error)?;
        ensure_operation_active(control)?;
        if let Some(usage) = collector.usage()? {
            events.emit(KernelEvent::UsageUpdated {
                operation_id: operation_id.clone(),
                total_tokens: usage.total_tokens,
                input_tokens: usage.input_tokens,
                cached_input_tokens: usage.cached_input_tokens,
                output_tokens: usage.output_tokens,
                reasoning_output_tokens: usage.reasoning_output_tokens,
            })?;
        }
        let payload = collector
            .output()?
            .ok_or_else(|| KernelError::Backend("subagent returned no model payload".into()))?;
        mahayana_model::responses::extract_output_text(&payload)
            .or_else(|| collector.text().ok().filter(|text| !text.is_empty()))
            .ok_or_else(|| KernelError::Backend("subagent returned no output text".into()))
    }

    async fn authorize_tool(
        &self,
        session: &mut NativeSession,
        operation_id: &OperationId,
        call: &FunctionCall,
        risk: RiskLevel,
        policy: &ExecutionPolicy,
        events: SharedKernelEventSink,
    ) -> Result<(), KernelError> {
        let key = PermissionKey::new(tool_capability(&call.name), permission_target(call))
            .map_err(|error| KernelError::Backend(error.to_string()))?;
        match session.permissions.evaluate(policy, &key, risk) {
            PermissionDecision::Allow => return Ok(()),
            PermissionDecision::Deny => {
                return Err(KernelError::PolicyDenied(format!(
                    "{} is denied for {}",
                    call.name, key.target
                )));
            }
            PermissionDecision::Ask => {}
        }

        let approval_id = format!("approval:{}", Uuid::new_v4());
        let requested_at_ms = now_ms();
        let (sender, receiver) = oneshot::channel();
        self.approvals
            .lock()
            .map_err(|_| KernelError::Backend("approval registry poisoned".into()))?
            .insert(
                approval_id.clone(),
                ApprovalWaiter {
                    operation_id: operation_id.as_str().to_owned(),
                    sender,
                },
            );
        self.telemetry.approval_requested();
        events.emit(KernelEvent::ApprovalRequested {
            operation_id: operation_id.clone(),
            approval_id: approval_id.clone(),
            title: format!("Allow {}", call.name),
            risk,
            details: json!({
                "tool": call.name,
                "capability": format!("{:?}", key.capability),
                "target": key.target,
                "engine": "mahayana-native"
            }),
        })?;

        let result = tokio::time::timeout(
            Duration::from_millis(self.config.approval_timeout_ms),
            receiver,
        )
        .await;
        let resolved_at_ms = now_ms();
        let (outcome, memory) = match result {
            Ok(Ok(resolution)) => {
                let memory = permission_memory_from_metadata(&resolution.metadata)?;
                if resolution.approved {
                    self.telemetry.approval_approved();
                    (ApprovalOutcome::Approved, memory)
                } else {
                    self.telemetry.approval_rejected();
                    (ApprovalOutcome::Rejected, memory)
                }
            }
            Ok(Err(_)) => {
                self.telemetry.approval_interrupted();
                (ApprovalOutcome::Interrupted, None)
            }
            Err(_) => {
                self.approvals
                    .lock()
                    .map_err(|_| KernelError::Backend("approval registry poisoned".into()))?
                    .remove(&approval_id);
                self.telemetry.approval_timed_out();
                (ApprovalOutcome::TimedOut, None)
            }
        };

        let record = ApprovalRecord::new(
            approval_id,
            key,
            risk,
            requested_at_ms,
            resolved_at_ms,
            outcome,
            memory,
        )
        .map_err(|error| KernelError::Backend(error.to_string()))?;
        let decision = {
            let NativeSession {
                approvals,
                permissions,
                ..
            } = session;
            approvals
                .record(record, permissions)
                .map_err(|error| KernelError::Backend(error.to_string()))?
        };
        if decision == PermissionDecision::Allow {
            Ok(())
        } else {
            Err(KernelError::PolicyDenied(format!(
                "approval did not allow {}",
                call.name
            )))
        }
    }

    fn start_attempt(
        session: &mut NativeSession,
        operation_id: &OperationId,
        prompt_id: &str,
    ) -> String {
        let id = format!("native-attempt:{}", Uuid::new_v4());
        session.attempts.push(OperationAttempt {
            id: id.clone(),
            operation_id: operation_id.as_str().to_owned(),
            prompt_id: prompt_id.to_owned(),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            state: OperationAttemptState::Running,
        });
        id
    }

    fn finish_attempt(session: &mut NativeSession, id: &str, state: OperationAttemptState) {
        if let Some(attempt) = session.attempts.iter_mut().find(|attempt| attempt.id == id) {
            attempt.finished_at_ms = Some(now_ms());
            attempt.state = state;
        }
    }

    async fn execute_active_prompt(
        &self,
        session_id: &SessionId,
        session: &mut NativeSession,
        operation_id: &OperationId,
        prompt: PromptEntry,
        attempt_id: Option<String>,
        append_user_prompt: bool,
        policy: &ExecutionPolicy,
        control: &OperationControl,
        events: SharedKernelEventSink,
    ) -> Result<(), KernelError> {
        let attempt_id =
            attempt_id.unwrap_or_else(|| Self::start_attempt(session, operation_id, &prompt.id));
        let result = self
            .run_prompt(
                session_id,
                session,
                operation_id,
                prompt.text.clone(),
                &prompt.metadata,
                append_user_prompt,
                policy,
                control,
                Arc::clone(&events),
            )
            .await;
        if control.suspended.load(Ordering::SeqCst) {
            Self::finish_attempt(session, &attempt_id, OperationAttemptState::Suspended);
            self.telemetry.operation_suspended();
            events.emit(KernelEvent::Activity {
                operation_id: operation_id.clone(),
                kind: "operation_suspended".into(),
                title: "Mahayana operation suspended".into(),
                detail: None,
                metadata: json!({"promptId": prompt.id}),
            })?;
            return Ok(());
        }
        match result {
            Ok(output) => {
                if !output.trim().is_empty() && output != "waiting_user" {
                    session.completed_outputs.insert(operation_id.as_str().to_string(), output);
                    while session.completed_outputs.len() > 128 {
                        if let Some(first) = session.completed_outputs.keys().next().cloned() {
                            session.completed_outputs.remove(&first);
                        } else {
                            break;
                        }
                    }
                }
                session
                    .prompt_queue
                    .complete(&prompt.id)
                    .map_err(|error| KernelError::Backend(error.to_string()))?;
                session.active_prompt = None;
                Self::finish_attempt(session, &attempt_id, OperationAttemptState::Completed);
                self.telemetry.operation_completed();
                events.emit(KernelEvent::OperationCompleted {
                    operation_id: operation_id.clone(),
                })?;
                Ok(())
            }
            Err(error) => {
                session
                    .prompt_queue
                    .cancel(&prompt.id)
                    .map_err(|queue_error| KernelError::Backend(queue_error.to_string()))?;
                session.active_prompt = None;
                let interrupted = control.interrupted.load(Ordering::SeqCst);
                Self::finish_attempt(
                    session,
                    &attempt_id,
                    if interrupted {
                        OperationAttemptState::Interrupted
                    } else {
                        OperationAttemptState::Failed
                    },
                );
                self.telemetry.operation_failed();
                events.emit(KernelEvent::OperationFailed {
                    operation_id: operation_id.clone(),
                    message: error.to_string(),
                    retryable: interrupted,
                })?;
                Err(error)
            }
        }
    }
}

#[async_trait]
impl EngineBackend for NativeEngine {
    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            id: "mahayana-native".into(),
            display_name: "Mahayana Native Engine".into(),
            native: true,
            capabilities: self.capabilities(),
        }
    }

    fn reset_session(&self) -> Result<(), KernelError> {
        NativeEngine::reset_session(self)
    }

    async fn open_session(&self, request: OpenSessionRequest) -> Result<SessionId, KernelError> {
        let persisted_path = request
            .metadata
            .get("conversationId")
            .and_then(Value::as_str)
            .and_then(|conversation_id| {
                self.config
                    .session_state_path
                    .as_deref()
                    .map(|base| session_state_path_for_conversation(base, conversation_id))
            });
        if let Some(path) = persisted_path.as_ref()
            && path.exists()
        {
            let started = Instant::now();
            match std::fs::read(path) {
                Ok(bytes) => match serde_json::from_slice::<KernelSessionSnapshot>(&bytes) {
                    Ok(snapshot) => {
                        let snapshot_updated_at_ms = snapshot
                            .metadata
                            .get("updatedAtMs")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        let transcript_updated_at_ms = request
                            .metadata
                            .get("transcriptUpdatedAtMs")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        if snapshot_updated_at_ms >= transcript_updated_at_ms {
                            let byte_count = bytes.len().min(u64::MAX as usize) as u64;
                            match self.restore_session(snapshot).await {
                                Ok(session_id) => {
                                    self.telemetry.session_persistence_finished(
                                        SessionPersistenceOperation::Replay,
                                        byte_count,
                                        started.elapsed(),
                                        true,
                                    );
                                    self.persisted_sessions
                                        .lock()
                                        .map_err(|_| {
                                            KernelError::Backend(
                                                "persisted session registry poisoned".into(),
                                            )
                                        })?
                                        .insert(session_id.as_str().to_owned(), path.clone());
                                    self.telemetry.session_opened();
                                    return Ok(session_id);
                                }
                                Err(error) => {
                                    self.telemetry.session_persistence_finished(
                                        SessionPersistenceOperation::Replay,
                                        byte_count,
                                        started.elapsed(),
                                        false,
                                    );
                                    return Err(error);
                                }
                            }
                        }
                    }
                    Err(_) => {
                        self.telemetry.session_persistence_finished(
                            SessionPersistenceOperation::Replay,
                            bytes.len().min(u64::MAX as usize) as u64,
                            started.elapsed(),
                            false,
                        );
                    }
                },
                Err(_) => {
                    self.telemetry.session_persistence_finished(
                        SessionPersistenceOperation::Replay,
                        0,
                        started.elapsed(),
                        false,
                    );
                }
            }
        }
        let workspace_root = request
            .workspace_root
            .as_deref()
            .map(PathBuf::from)
            .map(|path| {
                path.canonicalize()
                    .map_err(|error| KernelError::Backend(error.to_string()))
            })
            .transpose()?;
        if let Some(root) = workspace_root.as_deref()
            && !root.is_dir()
        {
            return Err(KernelError::BackendUnavailable(format!(
                "workspace root is not a directory: {}",
                root.display()
            )));
        }
        let session_id = SessionId::new();
        self.sessions
            .lock()
            .map_err(|_| KernelError::Backend("native session registry poisoned".into()))?
            .insert(
                session_id.as_str().to_string(),
                Arc::new(AsyncMutex::new(NativeSession {
                    workspace_root,
                    history: native_bootstrap_history(&request.metadata),
                    prompt_queue: PromptQueue::default(),
                    active_prompt: None,
                    permissions: PermissionLedger::default(),
                    approvals: ApprovalLedger::default(),
                    loop_state: LoopState::default(),
                    attempts: Vec::new(),
                    inflight_tool: None,
                    completed_tool_results: BTreeMap::new(),
                    completed_outputs: BTreeMap::new(),
                    updated_at_ms: request
                        .metadata
                        .get("transcriptUpdatedAtMs")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                })),
            );
        self.telemetry.session_opened();
        if let Some(path) = persisted_path {
            self.persisted_sessions
                .lock()
                .map_err(|_| KernelError::Backend("persisted session registry poisoned".into()))?
                .insert(session_id.as_str().to_owned(), path);
        }
        Ok(session_id)
    }

    async fn run(
        &self,
        request: RunRequest,
        events: SharedKernelEventSink,
    ) -> Result<(), KernelError> {
        if !self
            .capabilities()
            .supports_all(&request.required_capabilities)
        {
            return Err(KernelError::CapabilityUnavailable(
                "native engine does not satisfy the requested capability set".into(),
            ));
        }
        let session = self.session(&request.session_id)?;
        let control = self.register_operation(&request.operation_id)?;
        self.telemetry.operation_started();
        let result = async {
            let mut session = session.lock().await;
            if session.active_prompt.is_some() {
                return Err(KernelError::Backend(
                    "session already has a suspended or running prompt; resume it before enqueuing another user prompt".into(),
                ));
            }
            let prompt_id = session
                .prompt_queue
                .enqueue(
                    request.input,
                    PromptPriority::UserBlocking,
                    request
                        .metadata
                        .get("clientMessageId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    request.metadata,
                )
                .map_err(|error| KernelError::Backend(error.to_string()))?;
            let prompt = session
                .prompt_queue
                .take_next()
                .ok_or_else(|| KernelError::Backend("prompt queue unexpectedly empty".into()))?;
            if prompt.id != prompt_id {
                return Err(KernelError::Backend(
                    "prompt queue selected a different blocking prompt".into(),
                ));
            }
            session.active_prompt = Some(prompt.clone());
            let attempt_id =
                Self::start_attempt(&mut session, &request.operation_id, &prompt.id);
            session.updated_at_ms = now_ms();
            self.persist_session_state_if_configured(&request.session_id, &session)?;
            self.execute_active_prompt(
                &request.session_id,
                &mut session,
                &request.operation_id,
                prompt,
                Some(attempt_id),
                true,
                &request.policy,
                control.as_ref(),
                events,
            )
            .await
        }
        .await;
        self.session(&request.session_id)?
            .lock()
            .await
            .updated_at_ms = now_ms();
        let persist_result = self.persist_session_if_configured(&request.session_id).await;
        if control.suspended.load(Ordering::SeqCst) && result.is_ok() {
            if persist_result.is_ok() {
                control.suspension_persisted.store(true, Ordering::SeqCst);
            }
            control.suspension_settled.notify_one();
        }
        self.finish_operation(&request.operation_id)?;
        persist_result?;
        result
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), KernelError> {
        let control = self
            .active_operations
            .lock()
            .map_err(|_| KernelError::Backend("native operation registry poisoned".into()))?
            .get(operation_id.as_str())
            .cloned()
            .ok_or_else(|| KernelError::OperationNotFound(operation_id.as_str().to_string()))?;
        control.interrupted.store(true, Ordering::SeqCst);
        self.cancel_operation_approvals(operation_id)?;
        Ok(())
    }

    async fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), KernelError> {
        let waiter = self
            .approvals
            .lock()
            .map_err(|_| KernelError::Backend("approval registry poisoned".into()))?
            .remove(&resolution.approval_id)
            .ok_or_else(|| KernelError::ApprovalNotFound(resolution.approval_id.clone()))?;
        waiter
            .sender
            .send(resolution)
            .map_err(|resolution| KernelError::ApprovalNotFound(resolution.approval_id))
    }

    async fn snapshot_session(
        &self,
        session_id: &SessionId,
    ) -> Result<KernelSessionSnapshot, KernelError> {
        let session = self.session(session_id)?;
        let session = session.lock().await.clone();
        let updated_at_ms = session.updated_at_ms;
        let state = NativeSnapshotState {
            session,
            memory: self
                .memory
                .lock()
                .map_err(|_| KernelError::Backend("memory store poisoned".into()))?
                .clone(),
            workflows: self
                .workflows
                .lock()
                .map_err(|_| KernelError::Backend("workflow store poisoned".into()))?
                .clone(),
            subagents: self
                .subagents
                .lock()
                .map_err(|_| KernelError::Backend("subagent scheduler poisoned".into()))?
                .clone(),
            hooks: self
                .hooks
                .lock()
                .map_err(|_| KernelError::Backend("hook registry poisoned".into()))?
                .clone(),
        };
        Ok(KernelSessionSnapshot {
            session_id: session_id.clone(),
            backend_id: "mahayana-native".into(),
            state: serde_json::to_value(state)
                .map_err(|error| KernelError::Backend(error.to_string()))?,
            metadata: json!({"snapshotVersion": 1, "updatedAtMs": updated_at_ms}),
        })
    }

    async fn restore_session(
        &self,
        snapshot: KernelSessionSnapshot,
    ) -> Result<SessionId, KernelError> {
        if snapshot.backend_id != "mahayana-native" {
            return Err(KernelError::BackendUnavailable(format!(
                "snapshot belongs to backend {}",
                snapshot.backend_id
            )));
        }
        let state: NativeSnapshotState = serde_json::from_value(snapshot.state)
            .map_err(|error| KernelError::Backend(format!("invalid native snapshot: {error}")))?;
        *self
            .memory
            .lock()
            .map_err(|_| KernelError::Backend("memory store poisoned".into()))? = state.memory;
        *self
            .workflows
            .lock()
            .map_err(|_| KernelError::Backend("workflow store poisoned".into()))? = state.workflows;
        *self
            .subagents
            .lock()
            .map_err(|_| KernelError::Backend("subagent scheduler poisoned".into()))? =
            state.subagents;
        *self
            .hooks
            .lock()
            .map_err(|_| KernelError::Backend("hook registry poisoned".into()))? = state.hooks;
        self.sessions
            .lock()
            .map_err(|_| KernelError::Backend("native session registry poisoned".into()))?
            .insert(
                snapshot.session_id.as_str().to_owned(),
                Arc::new(AsyncMutex::new(state.session)),
            );
        Ok(snapshot.session_id)
    }

    async fn suspend_operation(&self, request: SuspendOperationRequest) -> Result<(), KernelError> {
        let control = self
            .active_operations
            .lock()
            .map_err(|_| KernelError::Backend("native operation registry poisoned".into()))?
            .get(request.operation_id.as_str())
            .cloned()
            .ok_or_else(|| KernelError::OperationNotFound(request.operation_id.as_str().into()))?;
        let has_running_descendants = self
            .subagents
            .lock()
            .map_err(|_| KernelError::Backend("subagent scheduler poisoned".into()))?
            .running_count()
            > 0;
        let cascade = request
            .metadata
            .get("cascade")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if has_running_descendants && !cascade {
            return Err(KernelError::PolicyDenied(
                "cannot suspend operation while live subagents exist without cascade=true".into(),
            ));
        }
        control.suspended.store(true, Ordering::SeqCst);
        self.cancel_operation_approvals(&request.operation_id)?;
        if control.suspension_persisted.load(Ordering::SeqCst) {
            return Ok(());
        }
        let settled = control.suspension_settled.notified();
        if control.suspension_persisted.load(Ordering::SeqCst) {
            return Ok(());
        }
        tokio::time::timeout(Duration::from_secs(30), settled)
            .await
            .map_err(|_| KernelError::Backend("operation suspension safe-point timed out".into()))?;
        if !control.suspension_persisted.load(Ordering::SeqCst) {
            return Err(KernelError::Backend(
                "operation suspension did not persist a safe execution checkpoint".into(),
            ));
        }
        Ok(())
    }

    async fn resume_operation(
        &self,
        request: ResumeOperationRequest,
        events: SharedKernelEventSink,
    ) -> Result<(), KernelError> {
        if !self
            .capabilities()
            .supports_all(&request.required_capabilities)
        {
            return Err(KernelError::CapabilityUnavailable(
                "native engine does not satisfy the requested capability set".into(),
            ));
        }
        let session = self.session(&request.session_id)?;
        let control = self.register_operation(&request.operation_id)?;
        self.telemetry.operation_started();
        self.telemetry.operation_resumed();
        let result = async {
            let mut session = session.lock().await;
            if let Some(inflight) = session.inflight_tool.clone() {
                if inflight.operation_id != request.operation_id.as_str() {
                    return Err(KernelError::Backend(format!(
                        "session has in-flight tool {} owned by another operation",
                        inflight.call_id
                    )));
                }
                let completion_key = tool_completion_key(&request.operation_id, &inflight.call_id);
                let durable_completion = session
                    .completed_tool_results
                    .get(&completion_key)
                    .cloned()
                    .filter(|completion| {
                        completion.operation_id == inflight.operation_id
                            && completion.call_id == inflight.call_id
                            && completion.tool == inflight.tool
                    });
                let (output, success) = if let Some(completion) = durable_completion {
                    (completion.output, completion.success)
                } else {
                    (
                        json!({
                            "error": "tool execution was interrupted by Host recreation; external side effects are unknown and the tool will not be replayed",
                            "recovery": "interrupted",
                            "unknownSideEffects": true,
                            "tool": inflight.tool,
                        }),
                        false,
                    )
                };
                events.emit(KernelEvent::ToolCompleted {
                    operation_id: request.operation_id.clone(),
                    tool_call_id: inflight.call_id.clone(),
                    tool: inflight.tool.clone(),
                    output: output.clone(),
                    success,
                })?;
                session.history.push(json!({
                    "type": "function_call_output",
                    "call_id": inflight.call_id,
                    "output": serde_json::to_string(&output).unwrap_or_else(|_| "null".into()),
                }));
                session.inflight_tool = None;
                session.completed_tool_results.remove(&completion_key);
                session.updated_at_ms = now_ms();
                self.persist_session_state_if_configured(&request.session_id, &session)?;
            }
            let prompt = match session.active_prompt.clone() {
                Some(prompt) => prompt,
                None => {
                    let terminal = session
                        .attempts
                        .iter()
                        .rev()
                        .find(|attempt| attempt.operation_id == request.operation_id.as_str())
                        .map(|attempt| attempt.state);
                    match terminal {
                        Some(OperationAttemptState::Completed) => {
                            if let Some(output) = session.completed_outputs.get(request.operation_id.as_str()).cloned()
                                && !output.trim().is_empty()
                            {
                                events.emit(KernelEvent::MessageCompleted {
                                    operation_id: request.operation_id.clone(),
                                    text: output,
                                })?;
                            }
                            return Ok(());
                        },
                        Some(OperationAttemptState::Failed) => {
                            return Err(KernelError::Backend(format!(
                                "{} previously failed before Host settlement",
                                request.operation_id.as_str()
                            )));
                        }
                        Some(OperationAttemptState::Interrupted) => {
                            return Err(KernelError::Backend(format!(
                                "{} was interrupted before Host settlement",
                                request.operation_id.as_str()
                            )));
                        }
                        _ => {
                            return Err(KernelError::OperationNotFound(format!(
                                "{} has no suspended prompt in session {}",
                                request.operation_id.as_str(),
                                request.session_id.as_str()
                            )));
                        }
                    }
                }
            };
            events.emit(KernelEvent::Activity {
                operation_id: request.operation_id.clone(),
                kind: "operation_resumed".into(),
                title: "Mahayana operation resumed".into(),
                detail: None,
                metadata: json!({"promptId": prompt.id}),
            })?;
            self.execute_active_prompt(
                &request.session_id,
                &mut session,
                &request.operation_id,
                prompt,
                None,
                false,
                &request.policy,
                control.as_ref(),
                events,
            )
            .await
        }
        .await;
        self.session(&request.session_id)?
            .lock()
            .await
            .updated_at_ms = now_ms();
        let persist_result = self.persist_session_if_configured(&request.session_id).await;
        if control.suspended.load(Ordering::SeqCst) && result.is_ok() {
            if persist_result.is_ok() {
                control.suspension_persisted.store(true, Ordering::SeqCst);
            }
            control.suspension_settled.notify_one();
        }
        self.finish_operation(&request.operation_id)?;
        persist_result?;
        result
    }
}

#[derive(Debug, Clone)]
struct FunctionCall {
    call_id: String,
    name: String,
    arguments: Value,
}

fn explicit_tool_request_plan(prompt: &str) -> Vec<FunctionCall> {
    const TOOL_NAMES: [&str; 13] = [
        "workspace_read",
        "workspace_write",
        "workspace_search",
        "workspace_checkpoint",
        "workspace_restore",
        "workspace_worktree",
        "codebase_graph",
        "code_symbols",
        "memory_put",
        "memory_get",
        "memory_search",
        "workflow_create",
        "workflow_status",
    ];

    let mut requested = TOOL_NAMES
        .into_iter()
        .filter_map(|name| prompt.find(name).map(|position| (position, name)))
        .collect::<Vec<_>>();
    if requested.is_empty() {
        return Vec::new();
    }
    requested.sort_by_key(|(position, _)| *position);

    requested
        .into_iter()
        .enumerate()
        .map(|(index, (_, name))| FunctionCall {
            call_id: format!("planned-call:{}", index + 1),
            name: name.to_string(),
            arguments: explicit_tool_arguments(prompt, name),
        })
        .collect()
}

fn explicit_tool_arguments(prompt: &str, tool: &str) -> Value {
    match tool {
        "workspace_read" => json!({
            "path": explicit_path(prompt).unwrap_or_else(|| "README.md".to_string()),
        }),
        "workspace_search" => json!({
            "query": quoted_value_after(prompt, "workspace_search")
                .unwrap_or_else(|| "Mahayana".to_string()),
            "limit": number_after(prompt, &["limit", "限制返回"]).unwrap_or(50),
        }),
        "workflow_create" => json!({
            "title": quoted_value_after(prompt, "workflow_create")
                .unwrap_or_else(|| "Mahayana workflow".to_string()),
            "tasks": explicit_workflow_tasks(prompt),
        }),
        "workflow_status" => json!({"workflow_id": "$last_workflow_id"}),
        _ => json!({}),
    }
}

fn explicit_path(prompt: &str) -> Option<String> {
    ["package.json", "Cargo.toml", "README.md"]
        .iter()
        .find(|candidate| prompt.contains(*candidate))
        .map(|candidate| (*candidate).to_string())
}

fn quoted_value_after(prompt: &str, marker: &str) -> Option<String> {
    let start = prompt.find(marker)? + marker.len();
    let tail = &prompt[start..];
    for (open, close) in [('“', '”'), ('"', '"'), ('`', '`'), ('\'', '\'')] {
        let Some(open_index) = tail.find(open) else {
            continue;
        };
        let value_start = open_index + open.len_utf8();
        let Some(close_index) = tail[value_start..].find(close) else {
            continue;
        };
        let value = tail[value_start..value_start + close_index].trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn number_after(prompt: &str, markers: &[&str]) -> Option<u64> {
    markers.iter().find_map(|marker| {
        let start = prompt.find(marker)? + marker.len();
        prompt[start..]
            .chars()
            .skip_while(|character| !character.is_ascii_digit())
            .take_while(|character| character.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    })
}

fn explicit_workflow_tasks(prompt: &str) -> Vec<Value> {
    let known = ["verify-read", "verify-search"];
    let mut task_ids = known
        .iter()
        .filter(|task_id| prompt.contains(*task_id))
        .copied()
        .collect::<Vec<_>>();
    if task_ids.is_empty() {
        return Vec::new();
    }
    task_ids.dedup();
    task_ids
        .into_iter()
        .map(|task_id| {
            json!({
                "id": task_id,
                "title": task_id,
                "depends_on": if task_id == "verify-search"
                    && prompt.contains("verify-read")
                {
                    json!(["verify-read"])
                } else {
                    json!([])
                },
            })
        })
        .collect()
}

fn parse_dsml_compat_calls(text: &str) -> Option<Vec<(String, Value)>> {
    const CALLS_OPEN: &str = "<｜｜DSML｜｜ calls>";
    const CALLS_CLOSE: &str = "</｜｜DSML｜｜ calls>";
    const INVOKE_OPEN: &str = "<｜｜DSML｜｜ invoke name=\"";
    const INVOKE_CLOSE: &str = "</｜｜DSML｜｜ invoke>";
    const PARAM_OPEN: &str = "<｜｜DSML｜｜ parameter name=\"";
    const PARAM_CLOSE: &str = "</｜｜DSML｜｜ parameter>";

    let trimmed = text.trim();
    let mut rest = trimmed
        .strip_prefix(CALLS_OPEN)?
        .strip_suffix(CALLS_CLOSE)?
        .trim();
    let mut calls = Vec::new();

    while !rest.is_empty() {
        let after_open = rest.strip_prefix(INVOKE_OPEN)?;
        let name_end = after_open.find("\">")?;
        let name = &after_open[..name_end];
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return None;
        }

        let body = &after_open[name_end + 2..];
        let invoke_end = body.find(INVOKE_CLOSE)?;
        let mut params = body[..invoke_end].trim();
        let mut arguments = serde_json::Map::new();

        while !params.is_empty() {
            let after_param_open = params.strip_prefix(PARAM_OPEN)?;
            let param_name_end = after_param_open.find('"')?;
            let param_name = &after_param_open[..param_name_end];
            if param_name.is_empty()
                || !param_name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
                || arguments.contains_key(param_name)
            {
                return None;
            }

            let after_name = &after_param_open[param_name_end + 1..];
            let tag_end = after_name.find('>')?;
            let attributes = after_name[..tag_end].trim();
            if !attributes.is_empty() && attributes != "string=\"true\"" {
                return None;
            }

            let value_and_tail = &after_name[tag_end + 1..];
            let value_end = value_and_tail.find(PARAM_CLOSE)?;
            let value = &value_and_tail[..value_end];
            arguments.insert(param_name.to_string(), Value::String(value.to_string()));
            params = value_and_tail[value_end + PARAM_CLOSE.len()..].trim();
        }

        calls.push((name.to_string(), Value::Object(arguments)));
        rest = body[invoke_end + INVOKE_CLOSE.len()..].trim();
    }

    (!calls.is_empty()).then_some(calls)
}

fn dsml_compat_function_calls(
    text: &str,
    tools: &[Value],
    step: usize,
) -> Option<Vec<FunctionCall>> {
    let parsed = parse_dsml_compat_calls(text)?;
    if parsed.iter().any(|(name, _)| {
        !tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name.as_str()))
    }) {
        return None;
    }
    Some(
        parsed
            .into_iter()
            .enumerate()
            .map(|(index, (name, arguments))| FunctionCall {
                call_id: format!("dsml-step-{step}-call-{index}"),
                name,
                arguments,
            })
            .collect(),
    )
}

fn function_call_history_item(call: &FunctionCall) -> Value {
    json!({
        "type": "function_call",
        "name": call.name,
        "call_id": call.call_id,
        "arguments": serde_json::to_string(&call.arguments).unwrap_or_else(|_| "{}".into()),
    })
}

fn append_function_call_history(history: &mut Vec<Value>, call: &FunctionCall) {
    history.push(function_call_history_item(call));
}

fn append_function_call_output_history(history: &mut Vec<Value>, call_id: &str, output: &Value) {
    history.push(json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": serde_json::to_string(output).unwrap_or_else(|_| "null".into()),
    }));
}

fn extract_function_calls(payload: &Value) -> Result<Vec<FunctionCall>, KernelError> {
    let mut calls = Vec::new();
    for item in payload
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if !matches!(item_type, "function_call" | "tool_call") {
            continue;
        }
        let name = item
            .get("name")
            .or_else(|| item.pointer("/function/name"))
            .and_then(Value::as_str)
            .ok_or_else(|| KernelError::Backend("tool call is missing a name".into()))?;
        let Some(call_id) = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
        else {
            // A provider function_call without a stable id cannot be paired with
            // a function_call_output on replay. Treat it as incomplete output
            // instead of inventing an id and executing an orphan tool request.
            continue;
        };
        let arguments = match item
            .get("arguments")
            .or_else(|| item.pointer("/function/arguments"))
        {
            Some(Value::String(arguments)) => serde_json::from_str(arguments).map_err(|error| {
                KernelError::Backend(format!("invalid tool arguments for {name}: {error}"))
            })?,
            Some(arguments) => arguments.clone(),
            None => json!({}),
        };
        calls.push(FunctionCall {
            call_id,
            name: name.to_string(),
            arguments,
        });
    }
    Ok(calls)
}

fn model_user_content(prompt: &str, metadata: &Value) -> Value {
    let images = metadata
        .get("selectedImageDataUrls")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| value.starts_with("data:image/"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if images.is_empty() {
        return Value::String(prompt.to_string());
    }
    let mut content = vec![json!({"type": "input_text", "text": prompt})];
    content.extend(
        images
            .into_iter()
            .map(|image_url| json!({"type": "input_image", "image_url": image_url})),
    );
    Value::Array(content)
}

fn reaction_message_ref_exists(metadata: &Value, message_id: &str) -> bool {
    metadata
        .get("reactionMessageRefs")
        .and_then(Value::as_array)
        .is_some_and(|rows| {
            rows.iter().any(|row| {
                row.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|candidate| candidate == message_id)
            })
        })
}

fn reaction_message_reference_context(metadata: &Value) -> Option<String> {
    let rows = metadata.get("reactionMessageRefs")?.as_array()?;
    let mut references = Vec::new();
    for row in rows.iter().take(32) {
        let Some(message_id) = row
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let excerpt = row
            .get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        references.push(format!("- message_id={message_id}: {excerpt}"));
    }
    if references.is_empty() {
        return None;
    }
    Some(format!(
        "[Canonical reaction targets]\nYou may call react_to_message when a reaction alone is an appropriate response. Only use one of these message_id values; never invent an id.\n{}",
        references.join("\n")
    ))
}

fn append_model_output(history: &mut Vec<Value>, payload: &Value) {
    if let Some(output) = payload.get("output").and_then(Value::as_array) {
        history.extend(output.iter().filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
            (!matches!(item_type, "function_call" | "tool_call")).then(|| item.clone())
        }));
    } else if let Some(text) = mahayana_model::responses::extract_output_text(payload) {
        history.push(json!({"role": "assistant", "content": text}));
    }
}

#[derive(Default)]
struct ModelCollector {
    output: Mutex<Option<Value>>,
    text: Mutex<String>,
    usage: Mutex<Option<ModelUsage>>,
    streaming_events: Option<(SharedKernelEventSink, OperationId)>,
}

impl ModelCollector {
    fn buffered() -> Self {
        Self::default()
    }

    fn streaming(events: SharedKernelEventSink, operation_id: OperationId) -> Self {
        Self {
            streaming_events: Some((events, operation_id)),
            ..Self::default()
        }
    }

    fn output(&self) -> Result<Option<Value>, KernelError> {
        self.output
            .lock()
            .map(|output| output.clone())
            .map_err(|_| KernelError::Backend("model output collector poisoned".into()))
    }

    fn text(&self) -> Result<String, KernelError> {
        self.text
            .lock()
            .map(|text| text.clone())
            .map_err(|_| KernelError::Backend("model text collector poisoned".into()))
    }

    fn usage(&self) -> Result<Option<ModelUsage>, KernelError> {
        self.usage
            .lock()
            .map(|usage| usage.clone())
            .map_err(|_| KernelError::Backend("model usage collector poisoned".into()))
    }
}

impl ModelEventSink for ModelCollector {
    fn emit(&self, event: ModelEvent) -> Result<(), ModelError> {
        match event {
            ModelEvent::OutputTextDelta(delta) => {
                if let Some((events, operation_id)) = self.streaming_events.as_ref() {
                    events
                        .emit(KernelEvent::MessageDelta {
                            operation_id: operation_id.clone(),
                            delta: delta.clone(),
                        })
                        .map_err(|error| ModelError::Inference(error.to_string()))?;
                }
                self.text
                    .lock()
                    .map_err(|_| ModelError::EventConsumerClosed)?
                    .push_str(&delta);
            }
            ModelEvent::Usage(usage) => {
                *self
                    .usage
                    .lock()
                    .map_err(|_| ModelError::EventConsumerClosed)? = Some(usage);
            }
            ModelEvent::Completed { output } => {
                *self
                    .output
                    .lock()
                    .map_err(|_| ModelError::EventConsumerClosed)? = Some(output);
            }
            ModelEvent::Failed { code, message } => {
                return Err(ModelError::Inference(format!("{code}: {message}")));
            }
        }
        Ok(())
    }
}

fn workspace_root(session: &NativeSession) -> Result<&Path, KernelError> {
    session.workspace_root.as_deref().ok_or_else(|| {
        KernelError::CapabilityUnavailable("this session has no workspace root".into())
    })
}

fn safe_join(root: &Path, relative: &Path) -> Result<PathBuf, KernelError> {
    if relative.is_absolute() {
        return Err(KernelError::PolicyDenied(
            "absolute workspace paths are not allowed".into(),
        ));
    }
    let canonical_root = root
        .canonicalize()
        .map_err(|error| KernelError::Backend(error.to_string()))?;
    let mut safe = canonical_root.clone();
    for component in relative.components() {
        match component {
            Component::Normal(segment) => {
                safe.push(segment);
                if safe.exists() {
                    let metadata = std::fs::symlink_metadata(&safe)
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    if metadata.file_type().is_symlink() {
                        return Err(KernelError::PolicyDenied(format!(
                            "workspace path crosses a symbolic link: {}",
                            safe.display()
                        )));
                    }
                    let canonical = safe
                        .canonicalize()
                        .map_err(|error| KernelError::Backend(error.to_string()))?;
                    if !canonical.starts_with(&canonical_root) {
                        return Err(KernelError::PolicyDenied(
                            "workspace path escapes the active root".into(),
                        ));
                    }
                }
            }
            Component::CurDir => {}
            _ => {
                return Err(KernelError::PolicyDenied(
                    "workspace path traversal is not allowed".into(),
                ));
            }
        }
    }
    Ok(safe)
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn search_workspace(root: &Path, query: &str, limit: usize) -> Result<Vec<Value>, KernelError> {
    if query.is_empty() {
        return Err(KernelError::Backend(
            "search query must not be empty".into(),
        ));
    }
    let mut matches = Vec::new();
    search_directory(root, root, query, limit, &mut matches)?;
    Ok(matches)
}

fn search_directory(
    root: &Path,
    directory: &Path,
    query: &str,
    limit: usize,
    matches: &mut Vec<Value>,
) -> Result<(), KernelError> {
    if matches.len() >= limit {
        return Ok(());
    }
    for entry in
        std::fs::read_dir(directory).map_err(|error| KernelError::Backend(error.to_string()))?
    {
        let entry = entry.map_err(|error| KernelError::Backend(error.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| KernelError::Backend(error.to_string()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some(".git" | ".mahayana" | "target" | "node_modules" | "dist" | "build")
            ) {
                continue;
            }
            search_directory(root, &path, query, limit, matches)?;
        } else if file_type.is_file() {
            let metadata = entry
                .metadata()
                .map_err(|error| KernelError::Backend(error.to_string()))?;
            if metadata.len() > 2 * 1024 * 1024 {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(_) => continue,
            };
            for (index, line) in content.lines().enumerate() {
                if line.contains(query) {
                    matches.push(json!({
                        "path": relative_display(root, &path),
                        "line": index + 1,
                        "text": line,
                    }));
                    if matches.len() >= limit {
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}

fn native_bootstrap_history(metadata: &Value) -> Vec<Value> {
    metadata
        .get("bootstrapHistory")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .rev()
        .take(200)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter_map(|message| {
            let role = message.get("role").and_then(Value::as_str)?;
            if !matches!(role, "user" | "assistant") {
                return None;
            }
            let content = message.get("content").and_then(Value::as_str)?;
            Some(json!({"role": role, "content": content}))
        })
        .collect()
}

fn run_process(
    execution: &ProcessExecution,
    root: &Path,
    program: &str,
    args: &[String],
) -> Result<Value, KernelError> {
    if program.trim().is_empty() || program.contains(['\r', '\n']) {
        return Err(KernelError::PolicyDenied("invalid process program".into()));
    }
    let output = match execution {
        ProcessExecution::Host => Command::new(program).args(args).current_dir(root).output(),
        ProcessExecution::LocalDocker { docker_path, image } => {
            let canonical_root = root
                .canonicalize()
                .map_err(|error| KernelError::Backend(error.to_string()))?;
            let mount = format!("{}:/workspace:rw", canonical_root.display());
            let temporary = format!(
                "type=tmpfs,destination=/tmp,tmpfs-size={}",
                256 * 1024 * 1024
            );
            Command::new(docker_path)
                .args([
                    "run",
                    "--rm",
                    "--network",
                    "none",
                    "--read-only",
                    "--cap-drop",
                    "ALL",
                    "--security-opt",
                    "no-new-privileges",
                    "--pids-limit",
                    "256",
                    "--memory",
                    "1g",
                    "--cpus",
                    "2",
                    "--label",
                    "com.fabushi.owner=mahayana-native-engine",
                    "--mount",
                    &temporary,
                    "--volume",
                    &mount,
                    "--workdir",
                    "/workspace",
                    image,
                    program,
                ])
                .args(args)
                .output()
        }
    }
    .map_err(|error| KernelError::Backend(error.to_string()))?;
    let stdout = truncate_bytes(&output.stdout);
    let stderr = truncate_bytes(&output.stderr);
    Ok(json!({
        "success": output.status.success(),
        "code": output.status.code(),
        "stdout": stdout,
        "stderr": stderr,
    }))
}

fn is_pinned_container_image(image: &str) -> bool {
    let Some((name, digest)) = image.rsplit_once("@sha256:") else {
        return false;
    };
    !name.trim().is_empty()
        && digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), KernelError> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| KernelError::Backend(error.to_string()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| KernelError::Backend(error.to_string()))
}

fn replace_file(temporary: &Path, destination: &Path) -> Result<(), KernelError> {
    match std::fs::rename(temporary, destination) {
        Ok(()) => Ok(()),
        Err(_error) if destination.exists() => {
            std::fs::remove_file(destination)
                .map_err(|remove_error| KernelError::Backend(remove_error.to_string()))?;
            std::fs::rename(temporary, destination)
                .map_err(|rename_error| KernelError::Backend(rename_error.to_string()))
        }
        Err(error) => Err(KernelError::Backend(error.to_string())),
    }
}

fn truncate_bytes(bytes: &[u8]) -> String {
    let end = bytes.len().min(MAX_TOOL_OUTPUT_BYTES);
    let mut text = String::from_utf8_lossy(&bytes[..end]).to_string();
    if bytes.len() > end {
        text.push_str("\n...[truncated]");
    }
    text
}

fn string_arg<'a>(arguments: &'a Value, key: &str) -> Result<&'a str, KernelError> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| KernelError::Backend(format!("tool argument {key} is required")))
}

fn is_delivery_tool(tool: &str) -> bool {
    matches!(tool, "send_message" | "react_to_message")
}

fn tool_risk(tool: &str) -> RiskLevel {
    match tool {
        "workspace_write" | "workspace_restore" => RiskLevel::WorkspaceWrite,
        "process_exec" => RiskLevel::SystemWrite,
        _ => RiskLevel::ReadOnly,
    }
}

fn tool_capability(tool: &str) -> Capability {
    match tool {
        "workspace_read" | "workspace_search" | "codebase_graph" | "code_symbols" => {
            Capability::FilesystemRead
        }
        "workspace_write" | "workspace_restore" => Capability::FilesystemWrite,
        "workspace_checkpoint" => Capability::Checkpoint,
        "workspace_worktree" => Capability::Worktree,
        "memory_put" | "memory_get" | "memory_search" => Capability::Memory,
        "workflow_create" | "workflow_status" => Capability::Workflow,
        "subagent_run" => Capability::Subagent,
        "web_search" | "web_fetch" => Capability::WebSearch,
        "process_exec" => Capability::Process,
        "git_status" | "git_diff" => Capability::Git,
        _ => Capability::ToolProtocol,
    }
}

fn permission_target(call: &FunctionCall) -> String {
    let target = match call.name.as_str() {
        "workspace_read" | "workspace_write" => call.arguments.get("path"),
        "workspace_restore" | "workspace_worktree" => call.arguments.get("checkpoint_id"),
        "memory_get" | "memory_put" => call.arguments.get("key"),
        "workflow_status" => call.arguments.get("workflow_id"),
        "process_exec" => call.arguments.get("program"),
        "web_search" => call.arguments.get("query"),
        "web_fetch" => call
            .arguments
            .get("urls")
            .and_then(Value::as_array)
            .and_then(|urls| urls.first()),
        _ => None,
    };
    target
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("{}:{value}", call.name))
        .unwrap_or_else(|| call.name.clone())
}

fn tool_fingerprint(call: &FunctionCall) -> String {
    format!("{}:{}", call.name, call.arguments)
}

fn tool_completion_key(operation_id: &OperationId, call_id: &str) -> String {
    format!("{}:{call_id}", operation_id.as_str())
}

fn tool_completion_revival_supported(tool: &str) -> bool {
    matches!(tool, "subagent_run" | "process_exec" | "git_status" | "git_diff")
}

fn permission_memory_from_metadata(
    metadata: &Value,
) -> Result<Option<PermissionMemory>, KernelError> {
    let Some(value) = metadata
        .get("permissionMemory")
        .or_else(|| metadata.get("permission_memory"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    match value {
        "allow_for_session" => Ok(Some(PermissionMemory::AllowForSession)),
        "deny_permanently" => Ok(Some(PermissionMemory::DenyPermanently)),
        "clear" => Ok(Some(PermissionMemory::Clear)),
        other => Err(KernelError::Backend(format!(
            "unknown permission memory directive: {other}"
        ))),
    }
}

fn shape_hidden_nudge_content(nudge_prompt: &str, pending_user_request: &str) -> String {
    let pending_user_request = pending_user_request.trim();
    if pending_user_request.is_empty() {
        return nudge_prompt.to_string();
    }
    format!(
        "{nudge_prompt}\n\nThe user request still pending from this same turn is reproduced verbatim below. Answer this exact request when you invoke send_message; preserve every explicit marker, constraint, and requested output detail.\n\n--- BEGIN PENDING USER REQUEST ---\n{pending_user_request}\n--- END PENDING USER REQUEST ---"
    )
}

fn has_closing_send_nudge_for_operation(
    history: &[Value],
    operation_id: &OperationId,
) -> bool {
    history.iter().any(|item| {
        item.get("source").and_then(Value::as_str) == Some("mahayana_closing_send_nudge")
            && item.get("operationId").and_then(Value::as_str) == Some(operation_id.as_str())
    })
}

fn should_attempt_closing_send_nudge(
    delivered_message: bool,
    tool_work_after_delivery: bool,
    already_attempted: bool,
    control: &OperationControl,
) -> bool {
    delivered_message
        && tool_work_after_delivery
        && !already_attempted
        && !control.suspended.load(Ordering::SeqCst)
        && !control.interrupted.load(Ordering::SeqCst)
}

fn should_attempt_reply_nudge(
    delivered_message: bool,
    attempts: usize,
    control: &OperationControl,
) -> bool {
    !delivered_message
        && attempts < MAX_REPLY_NUDGES
        && !control.suspended.load(Ordering::SeqCst)
        && !control.interrupted.load(Ordering::SeqCst)
}

fn ensure_operation_active(control: &OperationControl) -> Result<(), KernelError> {
    if control.suspended.load(Ordering::SeqCst) {
        return Err(KernelError::Backend("operation suspended".into()));
    }
    if control.interrupted.load(Ordering::SeqCst) {
        return Err(KernelError::Backend("operation interrupted".into()));
    }
    Ok(())
}

fn remove_session_state_file(path: &Path) -> Result<(), KernelError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(KernelError::Backend(error.to_string())),
    }
}

fn remove_all_conversation_session_state_files(base: &Path) -> Result<(), KernelError> {
    remove_session_state_file(base)?;
    let Some(parent) = base.parent() else { return Ok(()); };
    let Some(file_name) = base.file_name().and_then(|name| name.to_str()) else { return Ok(()); };
    let prefix = format!("{file_name}.conversation-");
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(KernelError::Backend(error.to_string())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| KernelError::Backend(error.to_string()))?;
        if entry.file_name().to_str().is_some_and(|name| name.starts_with(&prefix)) {
            remove_session_state_file(&entry.path())?;
        }
    }
    Ok(())
}

fn session_state_path_for_conversation(base: &Path, conversation_id: &str) -> PathBuf {
    if conversation_id == MAIN_ASSISTANT_CONVERSATION_ID {
        return base.to_path_buf();
    }
    // Stable FNV-1a suffix keeps arbitrary conversation identifiers out of the
    // filesystem path while providing one durable native session per canonical
    // conversation.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in conversation_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let file_name = base
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mahayana-session.json");
    base.with_file_name(format!("{file_name}.conversation-{hash:016x}"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn model_error(error: ModelError) -> KernelError {
    KernelError::Backend(error.to_string())
}

fn latest_plain_user_text(history: &[Value]) -> Option<&str> {
    for item in history.iter().rev() {
        if item.get("role").and_then(Value::as_str) == Some("user") {
            return item.get("content").and_then(Value::as_str);
        }
    }
    None
}

fn is_conversation_fast_lane(history: &[Value]) -> bool {
    let Some(text) = latest_plain_user_text(history) else {
        return false;
    };
    let text = text.trim();
    if text.is_empty()
        || text.chars().count() > CONVERSATION_FAST_LANE_MAX_CHARS
        || text.contains('\n')
        || text.contains('`')
        || text.contains("http://")
        || text.contains("https://")
    {
        return false;
    }

    let lower = text.to_lowercase();
    const ACTION_MARKERS: &[&str] = &[
        " search ",
        " browse ",
        " open ",
        " create ",
        " build ",
        " edit ",
        " modify ",
        " delete ",
        " remove ",
        " install ",
        " download ",
        " upload ",
        " run ",
        " execute ",
        " send ",
        " email ",
        " calendar ",
        " github ",
        " slack ",
        " terminal ",
        " shell ",
        " file ",
        " folder ",
        " website ",
        " webpage ",
        " script ",
        " code ",
        "搜索",
        "查找",
        "浏览",
        "打开",
        "创建",
        "新建",
        "构建",
        "编辑",
        "修改",
        "删除",
        "安装",
        "下载",
        "上传",
        "运行",
        "执行",
        "发送",
        "邮件",
        "日历",
        "文件",
        "文件夹",
        "终端",
        "脚本",
        "代码",
        "网站",
        "网页",
    ];
    let padded = format!(" {lower} ");
    !ACTION_MARKERS.iter().any(|marker| padded.contains(marker))
}

fn conversation_fast_lane_tools(history: &[Value], _tools: &[Value]) -> Option<Vec<Value>> {
    is_conversation_fast_lane(history).then(Vec::new)
}

fn model_turn_tools(
    provider_mode: ModelProviderMode,
    visible_user_turn: bool,
    history: &[Value],
    tools: &[Value],
) -> Vec<Value> {
    if visible_user_turn && provider_mode == ModelProviderMode::FirstPartyDacheng {
        if let Some(reduced) = conversation_fast_lane_tools(history, tools) {
            return reduced;
        }
    }
    tools.to_vec()
}

fn tool_definitions(enable_process_tools: bool, enable_web_research: bool) -> Vec<Value> {
    let mut tools = vec![
        function_tool(
            "send_message",
            "Send a concise user-visible progress update or answer as a separate message bubble. Use this for meaningful milestones, confirmations, and the final answer in a multi-step task. Do not invent progress; only report work that has happened or is about to happen.",
            json!({"type":"object","properties":{"message":{"type":"string","description":"Optional concise text to show the user."},"attachment":{"type":"object","properties":{"url":{"type":"string"},"file_name":{"type":"string"},"alt":{"type":"string"},"channel":{"type":"string"},"width":{"type":"integer"},"height":{"type":"integer"}},"required":["url"],"additionalProperties":false},"reply_to_message_id":{"type":"string","description":"Optional live transcript message id to reply to. Invalid or stale ids are ignored by the transcript owner."}},"anyOf":[{"required":["message"]},{"required":["attachment"]}],"additionalProperties":false}),
        ),
        function_tool(
            "react_to_message",
            "React to one canonical user message. A successful reaction is itself a user-visible delivery, so do not also call send_message unless the turn needs additional text.",
            json!({"type":"object","properties":{"message_id":{"type":"string","minLength":1,"maxLength":256,"description":"Canonical message_id from the reaction targets in the current turn context."},"emoji":{"type":"string","minLength":1,"maxLength":16,"description":"Reaction emoji; runtime enforces at most 16 UTF-16 code units."}},"required":["message_id","emoji"],"additionalProperties":false}),
        ),
        function_tool(
            "request_box_help",
            "Hand control to the user for a protected or manual step only they can safely complete, such as login, SSO, passkey, 2FA, captcha, or payment confirmation. This turn ends after the request and resumes after the user returns control.",
            json!({"type":"object","properties":{"instruction":{"type":"string","minLength":1,"maxLength":1000},"reason":{"type":"string","enum":["auth","captcha","payment","other"]},"domain":{"type":"string","maxLength":128},"idp_domain":{"type":"string","maxLength":128}},"required":["instruction"],"additionalProperties":false}),
        ),
        function_tool(
            "workspace_read",
            "Read a UTF-8 text file inside the active workspace.",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        ),
        function_tool(
            "workspace_write",
            "Write a UTF-8 text file inside the workspace. Mahayana automatically checkpoints before writing.",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
        ),
        function_tool(
            "workspace_search",
            "Search text across the workspace while skipping generated and dependency directories.",
            json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":200}},"required":["query"],"additionalProperties":false}),
        ),
        function_tool(
            "workspace_checkpoint",
            "Create a restorable Mahayana workspace checkpoint.",
            json!({"type":"object","properties":{"label":{"type":"string"}},"additionalProperties":false}),
        ),
        function_tool(
            "workspace_restore",
            "Restore a Mahayana workspace checkpoint. A safety checkpoint is created first.",
            json!({"type":"object","properties":{"checkpoint_id":{"type":"string"}},"required":["checkpoint_id"],"additionalProperties":false}),
        ),
        function_tool(
            "workspace_worktree",
            "Create an isolated Mahayana logical worktree from the workspace or a checkpoint.",
            json!({"type":"object","properties":{"checkpoint_id":{"type":"string"}},"additionalProperties":false}),
        ),
        function_tool(
            "codebase_graph",
            "Build the Mahayana cross-language codebase reference graph.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        function_tool(
            "code_symbols",
            "Index symbols in common workspace programming languages.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
        ),
        function_tool(
            "memory_put",
            "Store durable structured Mahayana memory.",
            json!({"type":"object","properties":{"namespace":{"type":"string"},"key":{"type":"string"},"value":{},"tags":{"type":"array","items":{"type":"string"}}},"required":["namespace","key","value"],"additionalProperties":false}),
        ),
        function_tool(
            "memory_get",
            "Read one durable Mahayana memory record.",
            json!({"type":"object","properties":{"namespace":{"type":"string"},"key":{"type":"string"}},"required":["namespace","key"],"additionalProperties":false}),
        ),
        function_tool(
            "memory_search",
            "Search durable Mahayana memory by namespace, text, and tags.",
            json!({"type":"object","properties":{"namespace":{"type":"string"},"query":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}}},"additionalProperties":false}),
        ),
        function_tool(
            "workflow_create",
            "Create a dependency-validated Mahayana workflow DAG.",
            json!({"type":"object","properties":{"title":{"type":"string"},"tasks":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"title":{"type":"string"},"depends_on":{"type":"array","items":{"type":"string"}}},"required":["id"],"additionalProperties":false}}},"required":["title"],"additionalProperties":false}),
        ),
        function_tool(
            "workflow_status",
            "Read the current state of a Mahayana workflow.",
            json!({"type":"object","properties":{"workflow_id":{"type":"string"}},"required":["workflow_id"],"additionalProperties":false}),
        ),
        function_tool(
            "subagent_run",
            "Delegate a focused reasoning task to an isolated Mahayana subagent.",
            json!({"type":"object","properties":{"name":{"type":"string"},"goal":{"type":"string"}},"required":["goal"],"additionalProperties":false}),
        ),
    ];
    if enable_web_research {
        tools.extend([
            function_tool(
                "web_search",
                "Search the live public web for current or external information. Use this when the task depends on information outside the workspace or may have changed since model training.",
                json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":10}},"required":["query"],"additionalProperties":false}),
            ),
            function_tool(
                "web_fetch",
                "Fetch and extract readable content from up to 10 public HTTP(S) URLs returned by web search or provided by the user. Prefer markdown for research and verify important claims from source content rather than snippets alone.",
                json!({"type":"object","properties":{"urls":{"type":"array","minItems":1,"maxItems":10,"items":{"type":"string"}},"format":{"type":"string","enum":["markdown","text"]}},"required":["urls"],"additionalProperties":false}),
            ),
        ]);
    }
    if enable_process_tools {
        tools.extend([
            function_tool(
                "process_exec",
                "Run an explicitly approved process in the active workspace.",
                json!({"type":"object","properties":{"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["program"],"additionalProperties":false}),
            ),
            function_tool(
                "git_status",
                "Read git status for the active workspace.",
                json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
            function_tool(
                "git_diff",
                "Read the current git diff for the active workspace.",
                json!({"type":"object","properties":{},"additionalProperties":false}),
            ),
        ]);
    }
    tools
}

fn function_tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": parameters,
    })
}

fn default_system_instructions() -> String {
    "You are Mahayana, a product-owned coding and automation Agent. Inspect before editing; prefer minimal, reversible changes; use checkpoints before risky workspace mutations; use workflows for dependent tasks; delegate focused analysis to subagents; use web_search when live or external information is needed and web_fetch to inspect strong sources before drawing conclusions. When the user names an available tool or requests a verifiable multi-step operation, make the actual function call, wait for its result, and continue the Agent loop until the requested work is complete; do not replace an executable tool call with a prose claim. For a multi-step task, use send_message to publish short, human-readable milestone updates and the final answer as separate user-visible messages; keep internal reasoning private, never fabricate progress, and do not merge all milestones into one long response. Never claim a tool succeeded unless its result says so; respect Mahayana approval and platform policy."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mahayana_model::ModelProviderMode;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    #[test]
    fn selected_images_project_into_the_current_user_turn() {
        let content = model_user_content(
            "inspect",
            &json!({
                "selectedImageDataUrls": [
                    "data:image/png;base64,AQID",
                    "https://example.test/not-local.png"
                ]
            }),
        );
        let parts = content.as_array().expect("multimodal user content");
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "inspect");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["image_url"], "data:image/png;base64,AQID");
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn conversation_fast_lane_accepts_simple_chinese_and_english_questions() {
        for prompt in [
            "用一句话解释为什么海水有咸味。",
            "In one sentence, explain why the daytime sky appears blue.",
            "用一句话说明声音为什么不能在真空中传播。",
            "In one sentence, explain what HTTPS protects.",
        ] {
            let history = vec![json!({"role":"user","content":prompt})];
            assert!(
                is_conversation_fast_lane(&history),
                "expected simple Q&A to use the conversation fast lane: {prompt}",
            );
        }
    }

    #[test]
    fn conversation_fast_lane_rejects_action_or_external_resource_turns() {
        for prompt in [
            "Please search GitHub for the latest release.",
            "Create a small app and write the files.",
            "Open https://example.com and summarize it.",
            "请搜索网页并下载文件。",
            "修改这个代码文件并运行测试。",
            "第一行\n第二行",
        ] {
            let history = vec![json!({"role":"user","content":prompt})];
            assert!(
                !is_conversation_fast_lane(&history),
                "action-oriented turn must remain on the full tool path: {prompt}",
            );
        }

        let multimodal = vec![json!({
            "role":"user",
            "content":[
                {"type":"input_text","text":"what is in this image?"},
                {"type":"input_image","image_url":"data:image/png;base64,AQID"}
            ]
        })];
        assert!(
            !is_conversation_fast_lane(&multimodal),
            "non-plain/multimodal user content must keep the full tool path",
        );
    }

    #[test]
    fn conversation_fast_lane_exposes_zero_tools() {
        let history = vec![json!({
            "role":"user",
            "content":"In one sentence, explain what a database index is for."
        })];
        let tools = vec![
            function_tool(
                "send_message",
                "visible reply",
                json!({"type":"object","properties":{}}),
            ),
            function_tool(
                "react_to_message",
                "reaction delivery",
                json!({"type":"object","properties":{}}),
            ),
            function_tool(
                "workspace_read",
                "read workspace",
                json!({"type":"object","properties":{}}),
            ),
        ];
        let reduced =
            conversation_fast_lane_tools(&history, &tools).expect("simple turn fast lane");
        assert!(
            reduced.is_empty(),
            "simple first-party conversation must expose zero provider tools",
        );
        assert!(
            model_turn_tools(ModelProviderMode::FirstPartyDacheng, true, &history, &tools)
                .is_empty(),
            "visible first-party fast lane must expose zero provider tools",
        );
        assert_eq!(
            model_turn_tools(ModelProviderMode::FirstPartyDacheng, false, &history, &tools).len(),
            3,
            "hidden/recovery turns must keep the full tool schema",
        );
        assert_eq!(
            model_turn_tools(ModelProviderMode::UserConfiguredRemote, true, &history, &tools).len(),
            3,
            "non-first-party providers must keep the full tool schema",
        );
    }

    #[tokio::test]
    async fn reaction_tool_is_terminal_delivery_without_forced_send_message() {
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([
                json!({
                    "output": [{
                        "type":"function_call",
                        "call_id":"call-react",
                        "name":"react_to_message",
                        "arguments":"{\"message_id\":\"user-message-1\",\"emoji\":\"👍\"}"
                    }]
                }),
                json!({
                    "output": [{
                        "type":"message",
                        "content":[{"type":"output_text","text":"reaction complete"}]
                    }]
                }),
            ])),
        });
        let engine =
            NativeEngine::new(model, NativeEngineConfig::embedded("model")).expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "thanks".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({
                        "hidden": false,
                        "reactionMessageRefs": [{
                            "id":"user-message-1",
                            "text":"thanks"
                        }]
                    }),
                },
                events.clone(),
            )
            .await
            .expect("reaction-only delivery turn");
        let events = events.0.lock().expect("events");
        assert!(events.iter().any(|event| matches!(
            event,
            KernelEvent::ToolCompleted { tool, output, success: true, .. }
                if tool == "react_to_message"
                    && output.get("delivered").and_then(Value::as_bool) == Some(true)
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            KernelEvent::ToolCompleted { tool, success: true, .. } if tool == "send_message"
        )));
    }

    #[test]
    fn web_research_capability_and_tools_follow_provider_configuration() {
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::new()),
        });
        let without_web = NativeEngine::new_with_web_config(
            model.clone(),
            NativeEngineConfig::embedded("model"),
            None,
        )
        .expect("create engine without web");
        assert!(!without_web.capabilities().contains(Capability::WebSearch));
        assert!(!tool_definitions(false, false).iter().any(|tool| matches!(
            tool.get("name").and_then(Value::as_str),
            Some("web_search" | "web_fetch")
        )));

        let with_web = NativeEngine::new_with_web_config(
            model,
            NativeEngineConfig::embedded("model"),
            Some(WebResearchConfig::for_test(
                "http://127.0.0.1:9/search",
                "http://127.0.0.1:9/fetch",
                "MAHAYANA_WEB_RESEARCH_TEST_KEY",
            )),
        )
        .expect("create engine with web");
        assert!(with_web.capabilities().contains(Capability::WebSearch));
        assert!(with_web.capabilities().contains(Capability::Network));
        let names = tool_definitions(false, true)
            .into_iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_owned))
            .collect::<Vec<_>>();
        assert!(names.contains(&"web_search".to_string()));
        assert!(names.contains(&"web_fetch".to_string()));
        assert_eq!(tool_capability("web_search"), Capability::WebSearch);
        assert_eq!(tool_capability("web_fetch"), Capability::WebSearch);
    }

    #[test]
    fn strict_dsml_tool_calls_normalize_into_declared_native_tools() {
        let tools = tool_definitions(false, false);
        let dsml = concat!(
            "<｜｜DSML｜｜ calls>\n",
            "<｜｜DSML｜｜ invoke name=\"send_message\">\n",
            "<｜｜DSML｜｜ parameter name=\"message\" string=\"true\">",
            "FABUSHI-IOS-DSML-7421",
            "</｜｜DSML｜｜ parameter>\n",
            "</｜｜DSML｜｜ invoke>\n",
            "</｜｜DSML｜｜ calls>"
        );
        let calls = dsml_compat_function_calls(dsml, &tools, 0)
            .expect("strict declared DSML should normalize");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "send_message");
        assert_eq!(calls[0].call_id, "dsml-step-0-call-0");
        assert_eq!(
            calls[0].arguments,
            json!({"message":"FABUSHI-IOS-DSML-7421"})
        );
        let history = function_call_history_item(&calls[0]);
        assert_eq!(history["type"], "function_call");
        assert_eq!(history["name"], "send_message");
        assert_eq!(history["call_id"], "dsml-step-0-call-0");
    }

    #[test]
    fn dsml_compatibility_fails_closed_for_mixed_or_undeclared_text() {
        let tools = tool_definitions(false, false);
        for text in [
            "prefix <｜｜DSML｜｜ calls><｜｜DSML｜｜ invoke name=\"send_message\"></｜｜DSML｜｜ invoke></｜｜DSML｜｜ calls>",
            "<｜｜DSML｜｜ calls><｜｜DSML｜｜ invoke name=\"unknown_tool\"></｜｜DSML｜｜ invoke></｜｜DSML｜｜ calls>",
        ] {
            assert!(
                dsml_compat_function_calls(text, &tools, 0).is_none(),
                "mixed or undeclared DSML must remain ordinary model text"
            );
        }
    }

    #[test]
    fn provider_tool_calls_require_stable_ids_and_are_not_replayed_up_front() {
        let payload = json!({
            "output": [
                {
                    "type": "reasoning",
                    "encrypted_content": "opaque"
                },
                {
                    "type": "function_call",
                    "name": "read_file",
                    "arguments": "{}"
                },
                {
                    "type": "function_call",
                    "call_id": "call-complete",
                    "name": "read_file",
                    "arguments": "{}"
                }
            ]
        });

        let calls = extract_function_calls(&payload).expect("extract provider calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call_id, "call-complete");

        let mut history = Vec::new();
        append_model_output(&mut history, &payload);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["type"], "reasoning");
        assert!(
            history.iter().all(|item| {
                !matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call" | "tool_call")
                )
            }),
            "provider tool calls must be recorded only when they actually execute"
        );
    }

    #[test]
    fn parallel_tool_replay_history_keeps_each_call_adjacent_to_its_result() {
        let calls = [
            FunctionCall {
                call_id: "call-a".into(),
                name: "read_file".into(),
                arguments: json!({"path":"a.txt"}),
            },
            FunctionCall {
                call_id: "call-b".into(),
                name: "read_file".into(),
                arguments: json!({"path":"b.txt"}),
            },
        ];
        let mut history = vec![json!({"type":"reasoning","encrypted_content":"opaque"})];
        for call in &calls {
            append_function_call_history(&mut history, call);
            append_function_call_output_history(
                &mut history,
                &call.call_id,
                &json!({"ok": true, "path": call.arguments["path"]}),
            );
        }

        let replay = history
            .iter()
            .map(|item| {
                (
                    item.get("type").and_then(Value::as_str),
                    item.get("call_id").and_then(Value::as_str),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            replay,
            vec![
                (Some("reasoning"), None),
                (Some("function_call"), Some("call-a")),
                (Some("function_call_output"), Some("call-a")),
                (Some("function_call"), Some("call-b")),
                (Some("function_call_output"), Some("call-b")),
            ]
        );
    }

    #[test]
    fn local_docker_requires_an_immutable_image_digest() {
        assert!(!is_pinned_container_image("example.test/fabushi:latest"));
        assert!(is_pinned_container_image(&format!(
            "example.test/fabushi@sha256:{}",
            "a".repeat(64)
        )));

        let mut config = NativeEngineConfig::desktop("model");
        config.process_execution = ProcessExecution::LocalDocker {
            docker_path: PathBuf::from("docker"),
            image: "example.test/fabushi:latest".into(),
        };
        assert!(config.validate().is_err());
    }

    struct FakeModel {
        outputs: Mutex<VecDeque<Value>>,
    }

    #[async_trait]
    impl ModelRuntime for FakeModel {
        async fn infer(
            &self,
            _request: ModelRequest,
            events: SharedModelEventSink,
        ) -> Result<(), ModelError> {
            let output = self
                .outputs
                .lock()
                .map_err(|_| ModelError::Inference("fake model poisoned".into()))?
                .pop_front()
                .ok_or_else(|| ModelError::Inference("fake model exhausted".into()))?;
            events.emit(ModelEvent::Usage(ModelUsage {
                total_tokens: 3,
                input_tokens: 2,
                cached_input_tokens: 0,
                output_tokens: 1,
                reasoning_output_tokens: 0,
            }))?;
            events.emit(ModelEvent::Completed { output })
        }

        fn provider_mode(&self) -> ModelProviderMode {
            ModelProviderMode::LocalModel
        }
    }

    struct BlockingModel {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        calls: AtomicUsize,
        output: Value,
    }

    #[async_trait]
    impl ModelRuntime for BlockingModel {
        async fn infer(
            &self,
            _request: ModelRequest,
            events: SharedModelEventSink,
        ) -> Result<(), ModelError> {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            events.emit(ModelEvent::Completed {
                output: self.output.clone(),
            })
        }

        fn provider_mode(&self) -> ModelProviderMode {
            ModelProviderMode::LocalModel
        }
    }

    struct FirstPartyStreamingModel {
        requests: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl ModelRuntime for FirstPartyStreamingModel {
        async fn infer(
            &self,
            request: ModelRequest,
            events: SharedModelEventSink,
        ) -> Result<(), ModelError> {
            self.requests
                .lock()
                .map_err(|_| ModelError::Inference("request capture poisoned".into()))?
                .push(request.metadata);
            events.emit(ModelEvent::OutputTextDelta("般若".into()))?;
            events.emit(ModelEvent::OutputTextDelta("波罗蜜".into()))?;
            events.emit(ModelEvent::Completed {
                output: json!({
                    "output": [{
                        "type": "message",
                        "content": [{"type": "output_text", "text": "般若波罗蜜"}]
                    }]
                }),
            })
        }

        fn provider_mode(&self) -> ModelProviderMode {
            ModelProviderMode::FirstPartyDacheng
        }
    }

    struct DsmlStreamingModel {
        inference_calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelRuntime for DsmlStreamingModel {
        async fn infer(
            &self,
            _request: ModelRequest,
            events: SharedModelEventSink,
        ) -> Result<(), ModelError> {
            let call = self
                .inference_calls
                .fetch_add(1, AtomicOrdering::SeqCst);
            if call == 0 {
                let dsml = concat!(
                    "<｜｜DSML｜｜ calls>\n",
                    "<｜｜DSML｜｜ invoke name=\"send_message\">\n",
                    "<｜｜DSML｜｜ parameter name=\"message\" string=\"true\">",
                    "FABUSHI-IOS-STREAM-7421",
                    "</｜｜DSML｜｜ parameter>\n",
                    "</｜｜DSML｜｜ invoke>\n",
                    "</｜｜DSML｜｜ calls>"
                );
                events.emit(ModelEvent::OutputTextDelta(dsml.to_string()))?;
                return events.emit(ModelEvent::Completed {
                    output: json!({"id":"resp-dsml-1","output":[]}),
                });
            }
            events.emit(ModelEvent::Completed {
                output: json!({
                    "id":"resp-dsml-2",
                    "output":[{
                        "type":"message",
                        "content":[{"type":"output_text","text":"internal-after-dsml"}]
                    }]
                }),
            })
        }

        fn provider_mode(&self) -> ModelProviderMode {
            ModelProviderMode::LocalModel
        }
    }

    struct SnapshotCheckingModel {
        state_path: PathBuf,
        expected_operation_id: String,
        observed: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ModelRuntime for SnapshotCheckingModel {
        async fn infer(
            &self,
            _request: ModelRequest,
            events: SharedModelEventSink,
        ) -> Result<(), ModelError> {
            let bytes = std::fs::read(&self.state_path)
                .map_err(|error| ModelError::Inference(format!("missing pre-inference snapshot: {error}")))?;
            let snapshot: KernelSessionSnapshot = serde_json::from_slice(&bytes)
                .map_err(|error| ModelError::Inference(format!("invalid pre-inference snapshot: {error}")))?;
            let state: NativeSnapshotState = serde_json::from_value(snapshot.state)
                .map_err(|error| ModelError::Inference(format!("invalid native snapshot state: {error}")))?;
            let active_prompt = state
                .session
                .active_prompt
                .as_ref()
                .ok_or_else(|| ModelError::Inference("pre-inference snapshot has no active prompt".into()))?;
            let has_running_attempt = state.session.attempts.iter().any(|attempt| {
                attempt.operation_id == self.expected_operation_id
                    && attempt.prompt_id == active_prompt.id
                    && attempt.state == OperationAttemptState::Running
                    && attempt.finished_at_ms.is_none()
            });
            if !has_running_attempt {
                return Err(ModelError::Inference(
                    "pre-inference snapshot has no matching running operation attempt".into(),
                ));
            }
            self.observed.store(true, Ordering::SeqCst);
            events.emit(ModelEvent::Completed {
                output: json!({
                    "output": [{"type":"message", "content":[{"type":"output_text", "text":"checkpoint observed"}]}]
                }),
            })
        }

        fn provider_mode(&self) -> ModelProviderMode {
            ModelProviderMode::LocalModel
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<KernelEvent>>);

    impl mahayana_kernel::KernelEventSink for Events {
        fn emit(&self, event: KernelEvent) -> Result<(), KernelError> {
            self.0
                .lock()
                .map_err(|_| KernelError::EventConsumerClosed)?
                .push(event);
            Ok(())
        }
    }

    fn temp_workspace() -> PathBuf {
        let root = std::env::temp_dir().join(format!("mahayana-native-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp workspace");
        root
    }

    #[test]
    fn explicit_tool_request_plan_preserves_order_and_dependencies() {
        let plan = explicit_tool_request_plan(
            "请按顺序调用 workspace_read 读取 package.json，workspace_search 搜索 “Mahayana” 限制返回 3 个结果，workflow_create 创建名为“CI 多步骤验证”的工作流并包含 verify-read、verify-search，最后 workflow_status 查询刚刚创建的 workflow_id。",
        );
        assert_eq!(
            plan.iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "workspace_read",
                "workspace_search",
                "workflow_create",
                "workflow_status"
            ]
        );
        assert_eq!(plan[0].arguments["path"], "package.json");
        assert_eq!(plan[1].arguments["query"], "Mahayana");
        assert_eq!(plan[1].arguments["limit"], 3);
        assert_eq!(plan[2].arguments["title"], "CI 多步骤验证");
        assert_eq!(
            plan[2].arguments["tasks"][1]["depends_on"],
            json!(["verify-read"])
        );
        assert_eq!(plan[3].arguments["workflow_id"], "$last_workflow_id");
    }

    #[tokio::test]
    async fn completes_direct_model_response_and_records_telemetry() {
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([json!({
                "output": [{"type":"message", "content":[{"type":"output_text", "text":"done"}]}]
            })])),
        });
        let engine =
            NativeEngine::new(model, NativeEngineConfig::embedded("model")).expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "hello".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: Value::Null,
                },
                events.clone(),
            )
            .await
            .expect("run engine");
        assert!(
            events
                .0
                .lock()
                .expect("events")
                .iter()
                .any(|event| matches!(
                    event,
                    KernelEvent::MessageCompleted { text, .. } if text == "done"
                ))
        );
        let metrics = engine.metrics_snapshot();
        assert_eq!(metrics.sessions_opened, 1);
        assert_eq!(metrics.operations_started, 1);
        assert_eq!(metrics.operations_completed, 1);
        assert_eq!(metrics.model_calls, 1);
    }

    #[tokio::test]
    async fn direct_conversation_fast_lane_streams_canonical_text_without_send_message_tool() {
        let model = Arc::new(FirstPartyStreamingModel {
            requests: Mutex::new(Vec::new()),
        });
        let engine = NativeEngine::new(model.clone(), NativeEngineConfig::embedded("model"))
            .expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "In one sentence, explain why the sky is blue.".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"hidden": false}),
                },
                events.clone(),
            )
            .await
            .expect("run direct conversation fast lane");

        let requests = model.requests.lock().expect("requests");
        assert_eq!(requests.len(), 1, "fast lane must not enter reply-nudge inference");
        assert_eq!(requests[0]["tools"], json!([]));
        assert!(
            requests[0]["instructions"]
                .as_str()
                .is_some_and(|value| value.contains("CHAT-013 direct conversation fast lane")),
        );
        assert!(
            requests[0]["instructions"]
                .as_str()
                .is_some_and(|value| !value.contains("react_to_message")),
            "fast-lane instruction must not advertise a reaction tool that is absent from the schema",
        );
        drop(requests);

        let events = events.0.lock().expect("events");
        let deltas = events
            .iter()
            .filter_map(|event| match event {
                KernelEvent::MessageDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(deltas, vec!["般若", "波罗蜜"]);
        assert!(events.iter().any(|event| matches!(
            event,
            KernelEvent::MessageCompleted { text, .. } if text == "般若波罗蜜"
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            KernelEvent::ToolStarted { tool, .. } | KernelEvent::ToolCompleted { tool, .. }
                if tool == "send_message"
        )));
    }

    #[tokio::test]
    async fn strict_streamed_dsml_executes_declared_send_message_and_continues() {
        let model = Arc::new(DsmlStreamingModel {
            inference_calls: AtomicUsize::new(0),
        });
        let engine = NativeEngine::new(model.clone(), NativeEngineConfig::embedded("model"))
            .expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "deliver the result".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"hidden": false}),
                },
                events.clone(),
            )
            .await
            .expect("run DSML-compatible visible turn");

        let events = events.0.lock().expect("events");
        let delivered = events
            .iter()
            .find_map(|event| match event {
                KernelEvent::ToolCompleted {
                    tool,
                    output,
                    success: true,
                    ..
                } if tool == "send_message" => Some(output),
                _ => None,
            })
            .expect("DSML send_message completion");
        assert_eq!(
            delivered["generatedMessage"],
            "FABUSHI-IOS-STREAM-7421"
        );
        assert_eq!(
            delivered["toolCallId"],
            "dsml-step-0-call-0"
        );
        assert!(!events.iter().any(|event| matches!(
            event,
            KernelEvent::MessageDelta { delta, .. } if delta.contains("DSML")
        )));
        assert_eq!(
            model.inference_calls.load(AtomicOrdering::SeqCst),
            2,
            "function_call_output must continue the model loop"
        );
    }

    #[tokio::test]
    async fn visible_user_turn_retries_plain_text_until_send_message_delivers() {
        let prose = |text| {
            json!({
                "output": [{"type":"message", "content":[{"type":"output_text", "text":text}]}]
            })
        };
        let send = json!({
            "output": [{
                "type":"function_call",
                "call_id":"call-send-final",
                "name":"send_message",
                "arguments":"{\"message\":\"final delivered result\"}"
            }]
        });
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([
                prose("plain-0"),
                prose("plain-1"),
                prose("plain-2"),
                send,
                prose("internal-after-send"),
            ])),
        });
        let engine = NativeEngine::new(model.clone(), NativeEngineConfig::embedded("model"))
            .expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session.clone(),
                    operation_id: OperationId::new(),
                    input: "finish the task exactly; preserve marker FABUSHI-MARKER-7421 and output detail OUTPUT=full".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"hidden": false}),
                },
                events.clone(),
            )
            .await
            .expect("run visible turn");
        let events = events.0.lock().expect("events");
        let delivered = events
            .iter()
            .find_map(|event| match event {
                KernelEvent::ToolCompleted {
                    tool,
                    output,
                    success: true,
                    ..
                } if tool == "send_message" => Some(output),
                _ => None,
            })
            .expect("reply nudge send_message completion");
        assert_eq!(delivered["syntheticReplyNudge"], true);
        assert!(!events.iter().any(|event| matches!(
            event,
            KernelEvent::MessageDelta { .. } | KernelEvent::MessageCompleted { .. }
        )));
        assert_eq!(model.outputs.lock().expect("outputs").len(), 0);

        drop(events);
        let session_state = engine.session(&session).expect("session state");
        let session_state = session_state.lock().await;
        let reply_nudges = session_state
            .history
            .iter()
            .filter(|item| {
                item.get("source").and_then(Value::as_str) == Some("mahayana_reply_nudge")
            })
            .map(|item| {
                item.get("content")
                    .and_then(Value::as_str)
                    .expect("reply nudge content")
            })
            .collect::<Vec<_>>();
        assert_eq!(reply_nudges.len(), 3);
        for nudge in reply_nudges {
            assert!(nudge.starts_with(REPLY_NUDGE_PROMPT));
            assert!(nudge.contains("--- BEGIN PENDING USER REQUEST ---"));
            assert!(nudge.contains("FABUSHI-MARKER-7421"));
            assert!(nudge.contains("OUTPUT=full"));
            assert!(nudge.contains("--- END PENDING USER REQUEST ---"));
        }
    }

    #[tokio::test]
    async fn visible_ack_then_tool_work_requires_one_durable_closing_send_nudge() {
        let send_ack = json!({
            "output": [{"type":"function_call","call_id":"call-send-ack","name":"send_message",
                "arguments":"{\"message\":\"I will check that.\"}"}]
        });
        let memory_get = json!({
            "output": [{"type":"function_call","call_id":"call-memory","name":"memory_get",
                "arguments":"{\"namespace\":\"test\",\"key\":\"missing\"}"}]
        });
        let prose = |text| json!({
            "output": [{"type":"message", "content":[{"type":"output_text", "text":text}]}]
        });
        let send_final = json!({
            "output": [{"type":"function_call","call_id":"call-send-final","name":"send_message",
                "arguments":"{\"message\":\"The check is complete.\"}"}]
        });
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([
                send_ack,
                memory_get,
                prose("internal result after tool work"),
                send_final,
                prose("internal terminal text"),
            ])),
        });
        let engine = NativeEngine::new(model.clone(), NativeEngineConfig::embedded("model"))
            .expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let operation_id = OperationId::new();
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session.clone(),
                    operation_id: operation_id.clone(),
                    input: "check memory and report back; preserve marker CLOSING-MARKER-5937 and output detail FORMAT=complete".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"hidden": false}),
                },
                events.clone(),
            )
            .await
            .expect("run visible turn");

        let events = events.0.lock().expect("events");
        let sends = events
            .iter()
            .filter_map(|event| match event {
                KernelEvent::ToolCompleted {
                    tool,
                    output,
                    success: true,
                    ..
                } if tool == "send_message" => Some(output),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(sends.len(), 2);
        assert_eq!(sends[1]["syntheticClosingSendNudge"], true);
        drop(events);

        let session_state = engine.session(&session).expect("session state");
        let session_state = session_state.lock().await;
        let markers = session_state
            .history
            .iter()
            .filter(|item| {
                item.get("source").and_then(Value::as_str)
                    == Some("mahayana_closing_send_nudge")
                    && item.get("operationId").and_then(Value::as_str)
                        == Some(operation_id.as_str())
            })
            .count();
        assert_eq!(markers, 1);
        let closing_nudge = session_state
            .history
            .iter()
            .find(|item| {
                item.get("source").and_then(Value::as_str)
                    == Some("mahayana_closing_send_nudge")
                    && item.get("operationId").and_then(Value::as_str)
                        == Some(operation_id.as_str())
            })
            .and_then(|item| item.get("content"))
            .and_then(Value::as_str)
            .expect("closing nudge content");
        assert!(closing_nudge.starts_with(CLOSING_SEND_NUDGE_PROMPT));
        assert!(closing_nudge.contains("--- BEGIN PENDING USER REQUEST ---"));
        assert!(closing_nudge.contains("CLOSING-MARKER-5937"));
        assert!(closing_nudge.contains("FORMAT=complete"));
        assert!(closing_nudge.contains("--- END PENDING USER REQUEST ---"));
        drop(session_state);
        let metrics = engine.metrics_snapshot();
        assert_eq!(metrics.closing_send_nudges, 1);
        assert_eq!(metrics.turn_empty_deliveries, 0);
        assert_eq!(model.outputs.lock().expect("outputs").len(), 0);
    }

    #[tokio::test]
    async fn visible_turn_reports_empty_delivery_after_bounded_reply_nudges() {
        let prose = |text| json!({
            "output": [{"type":"message", "content":[{"type":"output_text", "text":text}]}]
        });
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([
                prose("plain-0"),
                prose("plain-1"),
                prose("plain-2"),
                prose("plain-3"),
            ])),
        });
        let engine = NativeEngine::new(model, NativeEngineConfig::embedded("model"))
            .expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "produce a delivered answer".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"hidden": false}),
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("run exhausted reply-nudge turn");
        let metrics = engine.metrics_snapshot();
        assert_eq!(metrics.turn_empty_deliveries, 1);
        assert_eq!(metrics.empty_delivery_reply_nudge_attempts_total, 3);
        assert_eq!(metrics.empty_delivery_tool_calls_total, 0);
        assert_eq!(metrics.empty_delivery_stream_output_turns, 1);
    }

    #[test]
    fn closing_send_guard_is_one_shot_and_cancel_fenced() {
        let control = OperationControl::default();
        assert!(should_attempt_closing_send_nudge(true, true, false, &control));
        assert!(!should_attempt_closing_send_nudge(false, true, false, &control));
        assert!(!should_attempt_closing_send_nudge(true, false, false, &control));
        assert!(!should_attempt_closing_send_nudge(true, true, true, &control));
        control.interrupted.store(true, Ordering::SeqCst);
        assert!(!should_attempt_closing_send_nudge(true, true, false, &control));
    }

    #[tokio::test]
    async fn request_box_help_ends_visible_turn_without_reply_nudge() {
        let request_help = json!({
            "output": [{
                "type":"function_call",
                "call_id":"call-handoff",
                "name":"request_box_help",
                "arguments":"{\"instruction\":\"Complete sign-in\",\"reason\":\"auth\",\"domain\":\"example.test\"}"
            }]
        });
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([
                request_help,
                json!({"output": [{"type":"message", "content":[{"type":"output_text", "text":"must-not-run"}]}]}),
            ])),
        });
        let engine = NativeEngine::new(model.clone(), NativeEngineConfig::embedded("model"))
            .expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "sign me in".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"hidden": false}),
                },
                events.clone(),
            )
            .await
            .expect("run handoff turn");
        let events = events.0.lock().expect("events");
        assert!(events.iter().any(|event| matches!(
            event,
            KernelEvent::Activity { kind, detail: Some(detail), .. }
                if kind == "box_handoff_request" && detail == "Complete sign-in"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            KernelEvent::ToolCompleted { tool, success: true, .. } if tool == "request_box_help"
        )));
        assert_eq!(model.outputs.lock().expect("outputs").len(), 1);
    }

    #[test]
    fn reply_nudge_guard_is_bounded_and_cancel_fenced() {
        let control = OperationControl::default();
        assert!(should_attempt_reply_nudge(false, 0, &control));
        assert!(should_attempt_reply_nudge(false, 2, &control));
        assert!(!should_attempt_reply_nudge(
            false,
            MAX_REPLY_NUDGES,
            &control
        ));
        assert!(!should_attempt_reply_nudge(true, 0, &control));
        control.interrupted.store(true, Ordering::SeqCst);
        assert!(!should_attempt_reply_nudge(false, 0, &control));
    }

    #[tokio::test]
    async fn executes_explicit_multi_step_tool_request_when_model_returns_prose() {
        let workspace = temp_workspace();
        std::fs::write(
            workspace.join("package.json"),
            r#"{"name":"mahayana-test"}"#,
        )
        .expect("seed package manifest");
        let prose = |text| {
            json!({
                "output": [{"type":"message", "content":[{"type":"output_text", "text":text}]}]
            })
        };
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([
                prose("我会执行这些步骤。"),
                prose("已读取，继续搜索。"),
                prose("已搜索，继续创建工作流。"),
                prose("已创建，继续查询状态。"),
                prose("全部完成。"),
            ])),
        });
        let engine =
            NativeEngine::new(model, NativeEngineConfig::desktop("model")).expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::DesktopFull,
                workspace_root: Some(workspace.to_string_lossy().to_string()),
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "请严格按顺序实际调用 workspace_read 读取 package.json，workspace_search 搜索 “Mahayana” 限制返回 3 个结果，workflow_create 创建名为“CI 多步骤验证”的工作流并包含 verify-read、verify-search，最后 workflow_status 查询刚刚创建的 workflow_id。".into(),
                    policy: ExecutionPolicy::interactive_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: Value::Null,
                },
                events.clone(),
            )
            .await
            .expect("run explicit tool request");
        let completed_tools = events
            .0
            .lock()
            .expect("events")
            .iter()
            .filter_map(|event| match event {
                KernelEvent::ToolCompleted { tool, success, .. } if *success => Some(tool.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            completed_tools,
            vec![
                "workspace_read".to_string(),
                "workspace_search".to_string(),
                "workflow_create".to_string(),
                "workflow_status".to_string()
            ]
        );
        assert!(
            events
                .0
                .lock()
                .expect("events")
                .iter()
                .any(|event| matches!(
                    event,
                    KernelEvent::MessageCompleted { text, .. } if text == "全部完成。"
                ))
        );
        std::fs::remove_dir_all(workspace).expect("cleanup");
    }

    #[tokio::test]
    async fn executes_workspace_write_with_checkpoint() {
        let workspace = temp_workspace();
        std::fs::write(workspace.join("existing.txt"), "before").expect("seed workspace");
        let call = json!({
            "output": [{
                "type": "function_call",
                "call_id": "call-1",
                "name": "workspace_write",
                "arguments": "{\"path\":\"new.txt\",\"content\":\"hello\"}"
            }]
        });
        let done = json!({
            "output": [{"type":"message", "content":[{"type":"output_text", "text":"written"}]}]
        });
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([call, done])),
        });
        let engine =
            NativeEngine::new(model, NativeEngineConfig::desktop("model")).expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::DesktopFull,
                workspace_root: Some(workspace.to_string_lossy().to_string()),
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        let mut policy = ExecutionPolicy::interactive_default();
        policy.max_unattended_risk = RiskLevel::WorkspaceWrite;
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "write the file".into(),
                    policy,
                    required_capabilities: CapabilitySet::new([
                        Capability::Model,
                        Capability::FilesystemWrite,
                    ]),
                    metadata: Value::Null,
                },
                events.clone(),
            )
            .await
            .expect("run engine");
        assert_eq!(
            std::fs::read_to_string(workspace.join("new.txt")).expect("read result"),
            "hello"
        );
        assert!(
            events
                .0
                .lock()
                .expect("events")
                .iter()
                .any(|event| matches!(event, KernelEvent::CheckpointCreated { .. }))
        );
        std::fs::remove_dir_all(workspace).expect("cleanup");
    }

    #[tokio::test]
    async fn network_policy_blocks_web_search_before_provider_request() {
        let call = json!({
            "output": [{
                "type":"function_call",
                "call_id":"call-web-denied",
                "name":"web_search",
                "arguments":"{\"query\":\"current topic\"}"
            }]
        });
        let done = json!({
            "output": [{"type":"message", "content":[{"type":"output_text", "text":"network denied"}]}]
        });
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([call, done])),
        });
        let engine = NativeEngine::new_with_web_config(
            model,
            NativeEngineConfig::embedded("model"),
            Some(WebResearchConfig::for_test(
                "http://127.0.0.1:9/search",
                "http://127.0.0.1:9/fetch",
                "MAHAYANA_WEB_POLICY_TEST_KEY",
            )),
        )
        .expect("create web engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        let events = Arc::new(Events::default());
        let mut policy = ExecutionPolicy::mobile_default();
        policy.allow_network = false;
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "search the web".into(),
                    policy,
                    required_capabilities: CapabilitySet::new([Capability::WebSearch]),
                    metadata: Value::Null,
                },
                events.clone(),
            )
            .await
            .expect("model recovers from denied search");
        assert!(
            events
                .0
                .lock()
                .expect("events")
                .iter()
                .any(|event| matches!(
                    event,
                    KernelEvent::ToolCompleted { tool, output, success: false, .. }
                        if tool == "web_search" && output.to_string().contains("denied")
                ))
        );
    }

    #[tokio::test]
    async fn approval_timeout_is_fail_closed() {
        let workspace = temp_workspace();
        let call = json!({
            "output": [{
                "type":"function_call",
                "call_id":"call-timeout",
                "name":"workspace_write",
                "arguments":"{\"path\":\"blocked.txt\",\"content\":\"no\"}"
            }]
        });
        let done = json!({
            "output": [{"type":"message", "content":[{"type":"output_text", "text":"denied"}]}]
        });
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([call, done])),
        });
        let mut config = NativeEngineConfig::desktop("model");
        config.approval_timeout_ms = 1;
        let engine = NativeEngine::new(model, config).expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::DesktopFull,
                workspace_root: Some(workspace.to_string_lossy().to_string()),
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "try write".into(),
                    policy: ExecutionPolicy::interactive_default(),
                    required_capabilities: CapabilitySet::new([Capability::FilesystemWrite]),
                    metadata: Value::Null,
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("model can recover from denied tool");
        assert!(!workspace.join("blocked.txt").exists());
        assert_eq!(engine.metrics_snapshot().approvals_timed_out, 1);
        std::fs::remove_dir_all(workspace).expect("cleanup");
    }

    #[test]
    fn session_reset_cleanup_removes_every_conversation_snapshot_only() {
        let root = std::env::temp_dir().join(format!(
            "mahayana-native-session-cleanup-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp root");
        let base = root.join("session.json");
        let agent = session_state_path_for_conversation(&base, "mahayana-ai:agent:alpha");
        let other = session_state_path_for_conversation(&base, "mahayana-ai:agent:beta");
        std::fs::write(&base, b"main").expect("write main");
        std::fs::write(&agent, b"agent").expect("write agent");
        std::fs::write(&other, b"other").expect("write other");
        std::fs::write(root.join("unrelated.json"), b"keep").expect("write unrelated");

        remove_all_conversation_session_state_files(&base).expect("cleanup snapshots");

        assert!(!base.exists());
        assert!(!agent.exists());
        assert!(!other.exists());
        assert!(root.join("unrelated.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn suspend_ack_waits_for_durable_prompt_and_resume_clears_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "mahayana-native-suspend-resume-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp root");
        let base = root.join("session.json");
        let conversation_id = "mahayana-ai:agent:resume-test";
        let persisted = session_state_path_for_conversation(&base, conversation_id);
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let blocking = Arc::new(BlockingModel {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            calls: AtomicUsize::new(0),
            output: json!({
                "output": [{"type":"message","content":[{"type":"output_text","text":"first pass"}]}]
            }),
        });
        let mut config = NativeEngineConfig::embedded("model");
        config.session_state_path = Some(base.clone());
        let engine = Arc::new(NativeEngine::new(blocking, config.clone()).expect("create engine"));
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::Headless,
                workspace_root: None,
                model: None,
                metadata: json!({"conversationId": conversation_id}),
            })
            .await
            .expect("open persisted session");
        let operation_id = OperationId::from_string("resume-same-operation");
        let run_engine = Arc::clone(&engine);
        let run_session = session.clone();
        let run_operation = operation_id.clone();
        let run = tokio::spawn(async move {
            run_engine
                .run(
                    RunRequest {
                        session_id: run_session,
                        operation_id: run_operation,
                        input: "continue durable work".into(),
                        policy: ExecutionPolicy::mobile_default(),
                        required_capabilities: CapabilitySet::new([Capability::Model]),
                        metadata: json!({"conversationId": conversation_id, "hidden": true}),
                    },
                    Arc::new(Events::default()),
                )
                .await
        });
        entered.notified().await;

        let suspend_engine = Arc::clone(&engine);
        let suspend_operation = operation_id.clone();
        let mut suspend = tokio::spawn(async move {
            suspend_engine
                .suspend_operation(SuspendOperationRequest {
                    operation_id: suspend_operation,
                    reason: Some("lifecycle".into()),
                    metadata: json!({"cascade": true}),
                })
                .await
        });
        loop {
            let suspended = engine
                .active_operations
                .lock()
                .expect("operation registry")
                .get(operation_id.as_str())
                .is_some_and(|control| control.suspended.load(Ordering::SeqCst));
            if suspended {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut suspend)
                .await
                .is_err(),
            "suspend must not acknowledge an unsafe pre-inference snapshot while inference is still active"
        );
        release.notify_waiters();
        suspend.await.expect("suspend task").expect("durable safe-point suspend");
        run.await.expect("run task").expect("suspended run returns cleanly");

        let bytes = std::fs::read(&persisted).expect("read suspended snapshot");
        let snapshot: KernelSessionSnapshot =
            serde_json::from_slice(&bytes).expect("decode suspended snapshot");
        let state: NativeSnapshotState =
            serde_json::from_value(snapshot.state).expect("decode suspended state");
        assert_eq!(
            state.session.active_prompt.as_ref().map(|prompt| prompt.text.as_str()),
            Some("continue durable work")
        );

        drop(engine);
        let resumed_model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([json!({
                "output": [{"type":"message","content":[{"type":"output_text","text":"resumed once"}]}]
            })])),
        });
        let resumed = NativeEngine::new(resumed_model, config).expect("recreate engine");
        let restored_session = resumed
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::Headless,
                workspace_root: None,
                model: None,
                metadata: json!({"conversationId": conversation_id}),
            })
            .await
            .expect("restore session");
        resumed
            .resume_operation(
                ResumeOperationRequest {
                    session_id: restored_session,
                    operation_id: operation_id.clone(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"resumedBy": "test"}),
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("resume same operation");

        let bytes = std::fs::read(&persisted).expect("read completed snapshot");
        let snapshot: KernelSessionSnapshot =
            serde_json::from_slice(&bytes).expect("decode completed snapshot");
        let state: NativeSnapshotState =
            serde_json::from_value(snapshot.state).expect("decode completed state");
        assert!(state.session.active_prompt.is_none());
        assert!(state.session.attempts.iter().any(|attempt| {
            attempt.operation_id == operation_id.as_str()
                && attempt.state == OperationAttemptState::Completed
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn recreate_converts_inflight_tool_to_interrupted_output_without_replay() {
        let root = std::env::temp_dir().join(format!("mahayana-inflight-tool-recovery-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp root");
        let base = root.join("session.json");
        let conversation_id = "mahayana-ai:agent:tool-recovery";
        let mut config = NativeEngineConfig::embedded("model");
        config.session_state_path = Some(base.clone());
        let first = NativeEngine::new(
            Arc::new(FakeModel { outputs: Mutex::new(VecDeque::new()) }),
            config.clone(),
        ).expect("first engine");
        let session_id = first.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::Headless,
            workspace_root: None,
            model: None,
            metadata: json!({"conversationId": conversation_id}),
        }).await.expect("open session");
        let operation_id = OperationId::from_string("recover-inflight-tool");
        {
            let session = first.session(&session_id).expect("session");
            let mut session = session.lock().await;
            let prompt_id = session
                .prompt_queue
                .enqueue(
                    "continue after interrupted subordinate",
                    PromptPriority::Background,
                    None,
                    json!({"hidden": true}),
                )
                .expect("enqueue recoverable parent prompt");
            let prompt = session.prompt_queue.take_next().expect("activate recoverable parent prompt");
            assert_eq!(prompt.id, prompt_id);
            session.active_prompt = Some(prompt.clone());
            session.history.push(json!({
                "type": "function_call",
                "call_id": "call-interrupted",
                "name": "subagent_run",
                "arguments": "{\"prompt\":\"child work\"}"
            }));
            session.inflight_tool = Some(InflightToolCheckpoint {
                operation_id: operation_id.as_str().to_string(),
                call_id: "call-interrupted".into(),
                tool: "subagent_run".into(),
                arguments: json!({"prompt":"child work"}),
            });
            session.attempts.push(OperationAttempt {
                id: "attempt:recover-inflight-tool".into(),
                operation_id: operation_id.as_str().to_string(),
                prompt_id: prompt.id,
                started_at_ms: now_ms(),
                finished_at_ms: None,
                state: OperationAttemptState::Running,
            });
            session.updated_at_ms = now_ms();
            first.persist_session_state_if_configured(&session_id, &session).expect("persist inflight checkpoint");
        }
        drop(first);

        let second = NativeEngine::new(
            Arc::new(FakeModel {
                outputs: Mutex::new(VecDeque::from([json!({
                    "output": [{"type":"message","content":[{"type":"output_text","text":"parent continued safely"}]}]
                })])),
            }),
            config,
        ).expect("second engine");
        let restored = second.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::Headless,
            workspace_root: None,
            model: None,
            metadata: json!({"conversationId": conversation_id}),
        }).await.expect("restore session");
        let events = Arc::new(Events::default());
        second.resume_operation(
            ResumeOperationRequest {
                session_id: restored,
                operation_id: operation_id.clone(),
                policy: ExecutionPolicy::mobile_default(),
                required_capabilities: CapabilitySet::new([Capability::Model]),
                metadata: json!({"resumedBy":"test"}),
            },
            events.clone(),
        ).await.expect("resume parent after interrupted tool");

        let bytes = std::fs::read(session_state_path_for_conversation(&base, conversation_id))
            .expect("read restored snapshot");
        let snapshot: KernelSessionSnapshot = serde_json::from_slice(&bytes).expect("decode snapshot");
        let state: NativeSnapshotState = serde_json::from_value(snapshot.state).expect("decode state");
        assert!(state.session.inflight_tool.is_none());
        assert!(state.session.history.iter().any(|item| {
            item.to_string().contains("unknownSideEffects")
                && item.to_string().contains("call-interrupted")
        }));
        assert!(events.0.lock().expect("events").iter().any(|event| matches!(
            event,
            KernelEvent::ToolCompleted { tool, success: false, output, .. }
                if tool == "subagent_run"
                    && output.get("unknownSideEffects").and_then(Value::as_bool) == Some(true)
        )));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn durable_tool_completion_revival_covers_subordinate_and_process_owners_only() {
        for tool in ["subagent_run", "process_exec", "git_status", "git_diff"] {
            assert!(
                tool_completion_revival_supported(tool),
                "{tool} completion must be eligible for durable revival"
            );
        }
        for tool in ["send_message", "workspace_write", "request_box_help", "web_fetch"] {
            assert!(
                !tool_completion_revival_supported(tool),
                "{tool} must not be replay-classified as a durable subordinate/process completion"
            );
        }
    }

    #[tokio::test]
    async fn recreate_revives_durable_subordinate_completion_without_reexecution() {
        let root = std::env::temp_dir().join(format!(
            "mahayana-durable-tool-completion-recovery-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create temp root");
        let base = root.join("session.json");
        let conversation_id = "mahayana-ai:agent:durable-tool-recovery";
        let mut config = NativeEngineConfig::embedded("model");
        config.session_state_path = Some(base.clone());
        let first = NativeEngine::new(
            Arc::new(FakeModel { outputs: Mutex::new(VecDeque::new()) }),
            config.clone(),
        ).expect("first engine");
        let session_id = first.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::Headless,
            workspace_root: None,
            model: None,
            metadata: json!({"conversationId": conversation_id}),
        }).await.expect("open session");
        let operation_id = OperationId::from_string("recover-durable-subagent-completion");
        {
            let session = first.session(&session_id).expect("session");
            let mut session = session.lock().await;
            let prompt_id = session
                .prompt_queue
                .enqueue(
                    "continue after durable subordinate result",
                    PromptPriority::Background,
                    None,
                    json!({"hidden": true}),
                )
                .expect("enqueue recoverable parent prompt");
            let prompt = session.prompt_queue.take_next().expect("activate recoverable parent prompt");
            assert_eq!(prompt.id, prompt_id);
            session.active_prompt = Some(prompt.clone());
            session.history.push(json!({
                "type": "function_call",
                "call_id": "call-durable-subagent",
                "name": "subagent_run",
                "arguments": "{\"goal\":\"child work\"}"
            }));
            session.inflight_tool = Some(InflightToolCheckpoint {
                operation_id: operation_id.as_str().to_string(),
                call_id: "call-durable-subagent".into(),
                tool: "subagent_run".into(),
                arguments: json!({"goal":"child work"}),
            });
            session.completed_tool_results.insert(
                tool_completion_key(&operation_id, "call-durable-subagent"),
                CompletedToolCheckpoint {
                    operation_id: operation_id.as_str().to_string(),
                    call_id: "call-durable-subagent".into(),
                    tool: "subagent_run".into(),
                    output: json!({"task_id":"child:stable","text":"durable child result"}),
                    success: true,
                },
            );
            session.attempts.push(OperationAttempt {
                id: "attempt:recover-durable-subagent-completion".into(),
                operation_id: operation_id.as_str().to_string(),
                prompt_id: prompt.id,
                started_at_ms: now_ms(),
                finished_at_ms: None,
                state: OperationAttemptState::Running,
            });
            session.updated_at_ms = now_ms();
            first.persist_session_state_if_configured(&session_id, &session)
                .expect("persist durable tool completion");
        }
        drop(first);

        let second = NativeEngine::new(
            Arc::new(FakeModel {
                outputs: Mutex::new(VecDeque::from([json!({
                    "output": [{"type":"message","content":[{"type":"output_text","text":"parent continued from durable child"}]}]
                })])),
            }),
            config,
        ).expect("second engine");
        let restored = second.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::Headless,
            workspace_root: None,
            model: None,
            metadata: json!({"conversationId": conversation_id}),
        }).await.expect("restore session");
        let events = Arc::new(Events::default());
        second.resume_operation(
            ResumeOperationRequest {
                session_id: restored,
                operation_id: operation_id.clone(),
                policy: ExecutionPolicy::mobile_default(),
                required_capabilities: CapabilitySet::new([Capability::Model]),
                metadata: json!({"resumedBy":"test"}),
            },
            events.clone(),
        ).await.expect("resume parent from durable subordinate completion");

        let bytes = std::fs::read(session_state_path_for_conversation(&base, conversation_id))
            .expect("read restored snapshot");
        let snapshot: KernelSessionSnapshot = serde_json::from_slice(&bytes).expect("decode snapshot");
        let state: NativeSnapshotState = serde_json::from_value(snapshot.state).expect("decode state");
        assert!(state.session.inflight_tool.is_none());
        assert!(state.session.completed_tool_results.is_empty());
        assert!(state.session.history.iter().any(|item| {
            item.to_string().contains("call-durable-subagent")
                && item.to_string().contains("durable child result")
                && !item.to_string().contains("unknownSideEffects")
        }));
        assert!(events.0.lock().expect("events").iter().any(|event| matches!(
            event,
            KernelEvent::ToolCompleted { tool, success: true, output, .. }
                if tool == "subagent_run"
                    && output.get("text").and_then(Value::as_str) == Some("durable child result")
        )));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn recreate_replays_completed_output_for_unsettled_host_terminal() {
        let root = std::env::temp_dir().join(format!("mahayana-terminal-revival-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp root");
        let base = root.join("session.json");
        let conversation_id = "mahayana-ai:agent:terminal-revival";
        let mut config = NativeEngineConfig::embedded("model");
        config.session_state_path = Some(base.clone());
        let first = NativeEngine::new(
            Arc::new(FakeModel { outputs: Mutex::new(VecDeque::new()) }),
            config.clone(),
        ).expect("first engine");
        let session_id = first.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::Headless,
            workspace_root: None,
            model: None,
            metadata: json!({"conversationId": conversation_id}),
        }).await.expect("open session");
        let operation_id = OperationId::from_string("recover-completed-terminal");
        {
            let session = first.session(&session_id).expect("session");
            let mut session = session.lock().await;
            session.attempts.push(OperationAttempt {
                id: "attempt:recover-completed-terminal".into(),
                operation_id: operation_id.as_str().to_string(),
                prompt_id: "prompt:done".into(),
                started_at_ms: now_ms(),
                finished_at_ms: Some(now_ms()),
                state: OperationAttemptState::Completed,
            });
            session.completed_outputs.insert(
                operation_id.as_str().to_string(),
                "durable final background result".into(),
            );
            session.updated_at_ms = now_ms();
            first.persist_session_state_if_configured(&session_id, &session).expect("persist completed terminal");
        }
        drop(first);

        let second = NativeEngine::new(
            Arc::new(FakeModel { outputs: Mutex::new(VecDeque::new()) }),
            config,
        ).expect("second engine");
        let restored = second.open_session(OpenSessionRequest {
            profile: mahayana_kernel::RuntimeProfile::Headless,
            workspace_root: None,
            model: None,
            metadata: json!({"conversationId": conversation_id}),
        }).await.expect("restore session");
        let events = Arc::new(Events::default());
        second.resume_operation(
            ResumeOperationRequest {
                session_id: restored,
                operation_id,
                policy: ExecutionPolicy::mobile_default(),
                required_capabilities: CapabilitySet::new([Capability::Model]),
                metadata: json!({"resumedBy":"test"}),
            },
            events.clone(),
        ).await.expect("revive completed terminal");
        assert!(events.0.lock().expect("events").iter().any(|event| matches!(
            event,
            KernelEvent::MessageCompleted { text, .. }
                if text == "durable final background result"
        )));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn snapshot_round_trip_preserves_native_orchestration_state() {
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([json!({
                "output": [{"type":"message", "content":[{"type":"output_text", "text":"remembered"}]}]
            })])),
        });
        let engine =
            NativeEngine::new(model, NativeEngineConfig::embedded("model")).expect("create engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::Headless,
                workspace_root: None,
                model: None,
                metadata: Value::Null,
            })
            .await
            .expect("open session");
        engine
            .run(
                RunRequest {
                    session_id: session.clone(),
                    operation_id: OperationId::new(),
                    input: "persist me".into(),
                    policy: ExecutionPolicy::default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: Value::Null,
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("run");
        let snapshot = engine
            .snapshot_session(&session)
            .await
            .expect("snapshot native session");
        let history = snapshot
            .state
            .pointer("/session/history")
            .and_then(Value::as_array)
            .expect("history in snapshot");
        assert!(
            history
                .iter()
                .any(|item| item.to_string().contains("persist me"))
        );
        let restored = engine
            .restore_session(snapshot)
            .await
            .expect("restore snapshot");
        assert_eq!(restored, session);
    }

    #[tokio::test]
    async fn persisted_main_session_checkpoints_active_operation_before_inference() {
        let root =
            std::env::temp_dir().join(format!("mahayana-pre-inference-checkpoint-{}", Uuid::new_v4()));
        let state_path = root.join("assistant.json");
        let operation_id = OperationId::new();
        let observed = Arc::new(AtomicBool::new(false));
        let model = Arc::new(SnapshotCheckingModel {
            state_path: state_path.clone(),
            expected_operation_id: operation_id.as_str().to_owned(),
            observed: Arc::clone(&observed),
        });
        let mut config = NativeEngineConfig::desktop("checkpoint-model");
        config.session_state_path = Some(state_path.clone());
        let engine = NativeEngine::new(model, config).expect("create checkpoint engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::DesktopFull,
                workspace_root: None,
                model: None,
                metadata: json!({"conversationId": MAIN_ASSISTANT_CONVERSATION_ID}),
            })
            .await
            .expect("open persisted main session");

        engine
            .run(
                RunRequest {
                    session_id: session,
                    operation_id,
                    input: "persist identity before inference".into(),
                    policy: ExecutionPolicy::interactive_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: Value::Null,
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("run with durable pre-inference checkpoint");

        assert!(
            observed.load(Ordering::SeqCst),
            "model inference must observe the active prompt and matching running operation attempt already persisted"
        );
        let metrics = engine.metrics_snapshot();
        assert!(metrics.session_checkpoints_succeeded >= 1);
        assert_eq!(metrics.session_checkpoints_failed, 0);
        assert!(metrics.session_checkpoint_bytes_total > 0);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test]
    async fn group_member_turn_runs_without_private_agent_checkpoint_binding() {
        let root = std::env::temp_dir().join(format!(
            "mahayana-group-member-no-private-checkpoint-{}",
            Uuid::new_v4()
        ));
        let state_path = root.join("assistant.json");
        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([json!({
                "output": [{"type":"message", "content":[{"type":"output_text", "text":"group member completed"}]}]
            })])),
        });
        let mut config = NativeEngineConfig::embedded("group-model");
        config.session_state_path = Some(state_path.clone());
        let engine = NativeEngine::new(model, config).expect("create group engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::MobileEmbedded,
                workspace_root: None,
                model: None,
                metadata: json!({"conversationId": "codex:agent:group-member"}),
            })
            .await
            .expect("open group-member session");

        engine
            .run(
                RunRequest {
                    session_id: session.clone(),
                    operation_id: OperationId::new(),
                    input: "[MAHAYANA_HIDDEN_CONTEXT] group turn".into(),
                    policy: ExecutionPolicy::mobile_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"hidden": true}),
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("group member generated lifecycle");

        let snapshot = engine
            .snapshot_session(&session)
            .await
            .expect("group member remains a normal generated session in memory");
        assert!(snapshot.state.to_string().contains("group member completed"));
        assert!(
            !state_path.exists(),
            "group-member conversation must not bind the private main-Agent checkpoint path"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn main_assistant_history_survives_provider_engine_recreation() {
        let root =
            std::env::temp_dir().join(format!("mahayana-provider-session-{}", Uuid::new_v4()));
        let state_path = root.join("assistant.json");
        let first_model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::from([json!({
                "output": [{"type":"message", "content":[{"type":"output_text", "text":"first provider reply"}]}]
            })])),
        });
        let mut first_config = NativeEngineConfig::desktop("first-model");
        first_config.session_state_path = Some(state_path.clone());
        let first = NativeEngine::new(first_model, first_config).expect("create first engine");
        let session = first
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::DesktopFull,
                workspace_root: None,
                model: None,
                metadata: json!({"conversationId": MAIN_ASSISTANT_CONVERSATION_ID}),
            })
            .await
            .expect("open first session");
        first
            .run(
                RunRequest {
                    session_id: session,
                    operation_id: OperationId::new(),
                    input: "remember across providers".into(),
                    policy: ExecutionPolicy::interactive_default(),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: Value::Null,
                },
                Arc::new(Events::default()),
            )
            .await
            .expect("run first provider");
        assert!(state_path.is_file());

        let second_model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::new()),
        });
        let mut second_config = NativeEngineConfig::desktop("second-model");
        second_config.session_state_path = Some(state_path);
        let second = NativeEngine::new(second_model, second_config).expect("create second engine");
        let restored = second
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::DesktopFull,
                workspace_root: None,
                model: None,
                metadata: json!({"conversationId": MAIN_ASSISTANT_CONVERSATION_ID}),
            })
            .await
            .expect("restore provider-neutral session");
        let snapshot = second
            .snapshot_session(&restored)
            .await
            .expect("snapshot restored session");
        assert!(
            snapshot
                .state
                .to_string()
                .contains("remember across providers")
        );
        assert!(snapshot.state.to_string().contains("first provider reply"));
        let replay_metrics = second.metrics_snapshot();
        assert_eq!(replay_metrics.session_replays_succeeded, 1);
        assert_eq!(replay_metrics.session_replays_failed, 0);
        assert!(replay_metrics.session_replay_bytes_total > 0);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[tokio::test]
    async fn invalid_persisted_session_records_replay_failure_without_changing_fallback() {
        let root =
            std::env::temp_dir().join(format!("mahayana-invalid-session-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create temp root");
        let state_path = root.join("assistant.json");
        std::fs::write(&state_path, b"{not-valid-json").expect("seed invalid persisted state");

        let model = Arc::new(FakeModel {
            outputs: Mutex::new(VecDeque::new()),
        });
        let mut config = NativeEngineConfig::desktop("fallback-model");
        config.session_state_path = Some(state_path);
        let engine = NativeEngine::new(model, config).expect("create fallback engine");
        let session = engine
            .open_session(OpenSessionRequest {
                profile: mahayana_kernel::RuntimeProfile::DesktopFull,
                workspace_root: None,
                model: None,
                metadata: json!({"conversationId": MAIN_ASSISTANT_CONVERSATION_ID}),
            })
            .await
            .expect("invalid persisted state must keep existing fresh-session fallback");

        assert!(engine.session(&session).is_ok());
        let metrics = engine.metrics_snapshot();
        assert_eq!(metrics.session_replays_succeeded, 0);
        assert_eq!(metrics.session_replays_failed, 1);
        assert!(metrics.session_replay_bytes_total > 0);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
