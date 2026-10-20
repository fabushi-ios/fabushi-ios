//! Product-owned `AgentBackend` implementation for Mahayana.
//!
//! This adapter preserves the existing MiniApp/host contract while replacing
//! the default Codex implementation with the sovereign native engine and MCP
//! transport. Codex can remain behind an explicit compatibility feature.

pub mod interaction;

use async_trait::async_trait;
use interaction::{
    CancellationFencedListener, ContextInjectionState, InteractionError, InteractionListener,
    InteractionListenerContext, InteractionQuery, InteractionQueryKind, InteractionResponseKind,
    InteractionUpdate,
    ResponseComparisonDisplayOrder, ResponseComparisonEvent, ResponseComparisonSkipReason,
    ResponseComparisonStarted, ResponseComparisonUpdate,
    NoopInteractionListener, PrivacyMode, RedactedDelegateAdapter, UnredactedDelegateAdapter,
    redacted_update_for_kernel_event,
};
use mahayana_agent::{
    AgentActivity, AgentActivityStatus, AgentBackend, AgentError, AgentEvent, AgentMessageRequest,
    ApprovalResolution, McpAppSession, OpenMcpAppRequest, SharedAgentEventSink, StartThreadRequest,
};
use mahayana_core::{
    AgentThreadId, ApprovalDecision, ContextInjectionOutcome, ContextInjectionRequest,
    ConversationId, Message, MessageId, MessageRole, ModelTokenUsage, ModelTokenUsageSnapshot,
    OperationId,
};
use mahayana_kernel::{
    ApprovalResolution as KernelApprovalResolution, Capability, CapabilitySet, EngineBackend,
    ExecutionPolicy, KernelError, KernelEvent, KernelEventSink, OpenSessionRequest,
    OperationId as KernelOperationId, RunRequest, RuntimeProfile, SessionId, SharedKernelEventSink,
};
use mahayana_mcp_runtime::{NativeMcpRegistry, ResolvedMcpPlugin};
use mahayana_native_engine::{
    DEFAULT_RESPONSE_COMPARISON_MAX_RESPONSE_CHARS,
    DEFAULT_RESPONSE_COMPARISON_MIN_CHARACTER_DIFF,
    NativeEngine, NativeResponseComparisonDisplayOrder, NativeResponseComparisonEvent,
    NativeResponseComparisonSkipReason, ResponseComparisonControl, ResponseComparisonEventSink,
    ResponseComparisonRequest, SharedResponseComparisonEventSink, SharedToolDecisionGate,
    ToolContextSignal, ToolDecisionGate, RESPONSE_COMPARISON_TIMEOUT,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct NativeAgentConfig {
    pub profile: RuntimeProfile,
    pub workspace_root: Option<PathBuf>,
    pub mcp_registry: NativeMcpRegistry,
    /// True only when the owning Host installed a reviewed process execution
    /// backend. MobileEmbedded never infers this from its platform profile.
    pub allow_process: bool,
}

#[derive(Clone)]
struct NativeThread {
    session_id: SessionId,
    conversation_id: ConversationId,
    scope_generation: u64,
}

#[derive(Clone)]
struct NativeMcpSession {
    plugin: ResolvedMcpPlugin,
    platform: mahayana_platform_core::HostPlatform,
    tools: Vec<Value>,
}

#[derive(Clone)]
struct NativeMcpServerState {
    plugin_id: String,
    status: String,
    status_detail: Option<String>,
    tools: Vec<Value>,
    generation: u64,
}

#[derive(Default)]
struct NativeMcpServerStateStore {
    by_server: BTreeMap<String, NativeMcpServerState>,
    next_generation: u64,
}

impl NativeMcpServerStateStore {
    fn begin(&mut self, server_identifier: &str, plugin_id: &str) -> Option<u64> {
        let generation = self.next_generation.checked_add(1)?;
        self.next_generation = generation;
        self.by_server.insert(
            server_identifier.to_string(),
            NativeMcpServerState {
                plugin_id: plugin_id.to_string(),
                status: "loading".into(),
                status_detail: None,
                tools: Vec::new(),
                generation,
            },
        );
        Some(generation)
    }

    fn is_current(&self, server_identifier: &str, generation: u64) -> bool {
        self.by_server
            .get(server_identifier)
            .is_some_and(|state| state.generation == generation)
    }

    fn settle(
        &mut self,
        server_identifier: &str,
        generation: u64,
        status: &str,
        status_detail: Option<String>,
        tools: Vec<Value>,
    ) -> bool {
        let Some(state) = self.by_server.get_mut(server_identifier) else {
            return false;
        };
        if state.generation != generation {
            return false;
        }
        state.status = status.to_string();
        state.status_detail = status_detail.filter(|detail| !detail.trim().is_empty());
        state.tools = tools;
        true
    }

    fn remove(&mut self, server_identifier: &str) -> bool {
        self.by_server.remove(server_identifier).is_some()
    }

    fn projected(&self) -> Vec<Value> {
        self.by_server
            .iter()
            .map(|(server_identifier, state)| {
                project_mcp_server_state(
                    server_identifier,
                    &state.plugin_id,
                    &state.status,
                    state.status_detail.as_deref(),
                    &state.tools,
                )
            })
            .collect()
    }

    fn clear(&mut self) {
        self.by_server.clear();
    }
}


#[derive(Clone, Debug)]
struct PendingContextInjection {
    request: ContextInjectionRequest,
    scope_generation: u64,
}

#[derive(Clone, Debug)]
struct ClaimedContextInjection {
    pending: PendingContextInjection,
    position: usize,
}

#[derive(Clone, Debug)]
struct ContextInjectionAdmission {
    outcome: ContextInjectionOutcome,
    state: ContextInjectionState,
    emit_update: bool,
    queued_new: bool,
}

#[derive(Default)]
struct ContextInjectionWorkflowControlStore {
    pending: Vec<PendingContextInjection>,
    claimed: HashMap<String, ClaimedContextInjection>,
    delivery_attempts: HashMap<String, u8>,
    completed_injection_ids: HashSet<String>,
    completed_delivery_batch_ids: HashSet<String>,
    terminal_states: HashMap<String, ContextInjectionState>,
    active_runs: HashMap<String, String>,
    last_ended_runs: HashMap<String, String>,
}

impl ContextInjectionWorkflowControlStore {
    fn begin_run(&mut self, conversation_id: &ConversationId, operation_id: &OperationId) {
        let conversation_key = conversation_id.as_str().to_string();
        self.active_runs
            .insert(conversation_key.clone(), operation_id.as_str().to_string());
        self.last_ended_runs.remove(&conversation_key);
    }

    fn outcome_for_state(state: &ContextInjectionState) -> ContextInjectionOutcome {
        match state {
            ContextInjectionState::Queued | ContextInjectionState::Delivered { .. } => {
                ContextInjectionOutcome::Queued
            }
            ContextInjectionState::QueuedForNextTurn => ContextInjectionOutcome::QueuedForNextTurn,
            ContextInjectionState::Cancelled => ContextInjectionOutcome::Rejected {
                reason: json!("context injection was cancelled"),
            },
            ContextInjectionState::Rejected { reason } => ContextInjectionOutcome::Rejected {
                reason: reason.clone(),
            },
        }
    }

    fn record_terminal(&mut self, injection_id: &str, state: ContextInjectionState) {
        self.completed_injection_ids.insert(injection_id.to_string());
        self.terminal_states.insert(injection_id.to_string(), state);
    }

    fn reject(&mut self, injection_id: &str, reason: Value) -> ContextInjectionAdmission {
        let state = ContextInjectionState::Rejected {
            reason: reason.clone(),
        };
        self.record_terminal(injection_id, state.clone());
        ContextInjectionAdmission {
            outcome: ContextInjectionOutcome::Rejected { reason },
            state,
            emit_update: true,
            queued_new: false,
        }
    }

    fn admit(
        &mut self,
        request: ContextInjectionRequest,
        scope_generation: u64,
    ) -> ContextInjectionAdmission {
        if let Some(state) = self.terminal_states.get(&request.injection_id).cloned() {
            return ContextInjectionAdmission {
                outcome: Self::outcome_for_state(&state),
                state,
                emit_update: false,
                queued_new: false,
            };
        }
        if self
            .pending
            .iter()
            .any(|pending| pending.request.injection_id == request.injection_id)
        {
            return ContextInjectionAdmission {
                outcome: ContextInjectionOutcome::Queued,
                state: ContextInjectionState::Queued,
                emit_update: false,
                queued_new: false,
            };
        }
        if request.injection_id.trim().is_empty() {
            return self.reject(
                &request.injection_id,
                json!({"code":"INVALID_INJECTION_ID","message":"injection_id is required"}),
            );
        }
        if request.expected_run_id.trim().is_empty() {
            return self.reject(
                &request.injection_id,
                json!({"code":"INVALID_EXPECTED_RUN_ID","message":"expected_run_id is required"}),
            );
        }
        if request
            .delivery_batch_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return self.reject(
                &request.injection_id,
                json!({"code":"INVALID_DELIVERY_BATCH_ID","message":"delivery_batch_id must not be empty"}),
            );
        }
        if let Some(delivery_batch_id) = request.delivery_batch_id.as_deref() {
            if self.completed_delivery_batch_ids.contains(delivery_batch_id)
                || self.pending.iter().any(|pending| {
                    pending.request.delivery_batch_id.as_deref() == Some(delivery_batch_id)
                })
            {
                return self.reject(
                    &request.injection_id,
                    json!({
                        "code":"DELIVERY_BATCH_ALREADY_ADMITTED",
                        "deliveryBatchId":delivery_batch_id
                    }),
                );
            }
        }

        let conversation_key = request.conversation_id.as_str().to_string();
        let active_run = self.active_runs.get(&conversation_key).cloned();
        let ended_same_run = self
            .last_ended_runs
            .get(&conversation_key)
            .is_some_and(|run| run == &request.expected_run_id);
        match active_run {
            Some(active_run) if active_run == request.expected_run_id => {
                self.pending.push(PendingContextInjection {
                    request,
                    scope_generation,
                });
                ContextInjectionAdmission {
                    outcome: ContextInjectionOutcome::Queued,
                    state: ContextInjectionState::Queued,
                    emit_update: true,
                    queued_new: true,
                }
            }
            Some(active_run) => self.reject(
                &request.injection_id,
                json!({
                    "code":"EXPECTED_RUN_MISMATCH",
                    "expectedRunId":request.expected_run_id,
                    "activeRunId":active_run
                }),
            ),
            None if ended_same_run => {
                let state = ContextInjectionState::QueuedForNextTurn;
                self.record_terminal(&request.injection_id, state.clone());
                ContextInjectionAdmission {
                    outcome: ContextInjectionOutcome::QueuedForNextTurn,
                    state,
                    emit_update: true,
                    queued_new: false,
                }
            }
            None => self.reject(
                &request.injection_id,
                json!({
                    "code":"EXPECTED_RUN_NOT_ACTIVE",
                    "expectedRunId":request.expected_run_id
                }),
            ),
        }
    }

    fn rollback_queued_admission(
        &mut self,
        injection_id: &str,
        scope_generation: u64,
    ) -> bool {
        let Some(index) = self.pending.iter().position(|pending| {
            pending.request.injection_id == injection_id
                && pending.scope_generation == scope_generation
        }) else {
            return false;
        };
        self.pending.remove(index);
        true
    }

    fn claim_next(
        &mut self,
        conversation_id: &ConversationId,
        operation_id: &str,
        scope_generation: u64,
    ) -> Option<ClaimedContextInjection> {
        if self
            .active_runs
            .get(conversation_id.as_str())
            .map_or(true, |active_run| active_run != operation_id)
        {
            return None;
        }
        let position = self.pending.iter().position(|pending| {
            pending.request.conversation_id == *conversation_id
                && pending.request.expected_run_id == operation_id
                && pending.scope_generation == scope_generation
        })?;
        let injection_id = self.pending[position].request.injection_id.clone();
        let attempts = self.delivery_attempts.get(&injection_id).copied().unwrap_or(0);
        if attempts >= 2 { return None; }
        self.delivery_attempts.insert(injection_id.clone(), attempts.saturating_add(1));
        let pending = self.pending.remove(position);
        let claim = ClaimedContextInjection { pending, position };
        self.claimed.insert(injection_id, claim.clone());
        Some(claim)
    }

    fn effective_delivery_batch_id(claim: &ClaimedContextInjection) -> String {
        claim
            .pending
            .request
            .delivery_batch_id
            .clone()
            .unwrap_or_else(|| {
                format!("context-injection:{}", claim.pending.request.injection_id)
            })
    }

    fn commit_delivery(
        &mut self,
        claim: &ClaimedContextInjection,
        current_scope_generation: u64,
        step: Value,
        delivered_at_ms: u64,
    ) -> Result<ContextInjectionState, String> {
        let request = &claim.pending.request;
        if claim.pending.scope_generation != current_scope_generation {
            return Err("context injection scope became stale before delivery commit".into());
        }
        if !self.claimed.contains_key(&request.injection_id) {
            return Err("context injection is not the current claimed signal".into());
        }
        if self
            .active_runs
            .get(request.conversation_id.as_str())
            .map_or(true, |active_run| active_run != &request.expected_run_id)
        {
            return Err("context injection expected run ended before delivery commit".into());
        }
        if self.completed_injection_ids.contains(&request.injection_id) {
            return Err("context injection was already completed".into());
        }
        let delivery_batch_id = Self::effective_delivery_batch_id(claim);
        if self.completed_delivery_batch_ids.contains(&delivery_batch_id) {
            return Err("context injection delivery batch was already completed".into());
        }
        let state = ContextInjectionState::Delivered {
            step,
            delivery_batch_id: Value::String(delivery_batch_id.clone()),
            delivered_at_ms,
        };
        self.completed_delivery_batch_ids.insert(delivery_batch_id);
        self.record_terminal(&request.injection_id, state.clone());
        self.claimed.remove(&request.injection_id);
        self.delivery_attempts.remove(&request.injection_id);
        Ok(state)
    }

    fn commit_claimed_delivery(&mut self,injection_id:&str,current_scope_generation:u64,step:Value,delivered_at_ms:u64)->Result<ContextInjectionState,String>{
        let claim=self.claimed.get(injection_id).cloned().ok_or_else(||"context injection claimed signal was not found".to_string())?;
        self.commit_delivery(&claim,current_scope_generation,step,delivered_at_ms)
    }

    fn rollback_claimed_delivery(&mut self,injection_id:&str,current_scope_generation:u64)->bool{
        let Some(claim)=self.claimed.get(injection_id).cloned() else { return false; };
        self.rollback_delivery(claim,current_scope_generation)
    }

    fn rollback_delivery(&mut self,claim:ClaimedContextInjection,current_scope_generation:u64)->bool{
        let conversation_id=claim.pending.request.conversation_id.clone();
        let expected_run_id=claim.pending.request.expected_run_id.clone();
        let injection_id=claim.pending.request.injection_id.clone();
        self.claimed.remove(&injection_id);
        let attempts=self.delivery_attempts.get(&injection_id).copied().unwrap_or_default();
        if claim.pending.scope_generation!=current_scope_generation {
            self.delivery_attempts.remove(&injection_id); return false;
        }
        if self.active_runs.get(conversation_id.as_str()).is_some_and(|r|r==&expected_run_id) {
            if attempts<2 {
                let pos=claim.position.min(self.pending.len()); self.pending.insert(pos,claim.pending); return true;
            }
            self.delivery_attempts.remove(&injection_id);
            self.record_terminal(&injection_id,ContextInjectionState::QueuedForNextTurn);
            return false;
        }
        self.delivery_attempts.remove(&injection_id);
        if self.last_ended_runs.get(conversation_id.as_str()).is_some_and(|r|r==&expected_run_id) {
            self.record_terminal(&injection_id,ContextInjectionState::QueuedForNextTurn);
        }
        false
    }

    fn finish_run(
        &mut self,
        conversation_id: &ConversationId,
        operation_id: &OperationId,
    ) -> Vec<(String, ContextInjectionState)> {
        let conversation_key = conversation_id.as_str().to_string();
        if self
            .active_runs
            .get(&conversation_key)
            .map_or(true, |active_run| active_run != operation_id.as_str())
        {
            return Vec::new();
        }
        self.active_runs.remove(&conversation_key);
        self.last_ended_runs
            .insert(conversation_key.clone(), operation_id.as_str().to_string());

        let pending = std::mem::take(&mut self.pending);
        let mut retained = Vec::with_capacity(pending.len());
        let mut updates = Vec::new();
        for item in pending {
            if item.request.conversation_id.as_str() == conversation_key.as_str()
                && item.request.expected_run_id == operation_id.as_str()
            {
                let injection_id = item.request.injection_id.clone();
                let state = ContextInjectionState::QueuedForNextTurn;
                self.record_terminal(&injection_id, state.clone());
                updates.push((injection_id, state));
            } else {
                retained.push(item);
            }
        }
        self.pending = retained;
        let claimed=std::mem::take(&mut self.claimed);
        let mut keep=HashMap::new();
        for (id,claim) in claimed {
            if claim.pending.request.conversation_id.as_str()==conversation_key.as_str() && claim.pending.request.expected_run_id==operation_id.as_str() {
                self.delivery_attempts.remove(&id);
                let state=ContextInjectionState::QueuedForNextTurn; self.record_terminal(&id,state.clone()); updates.push((id,state));
            } else { keep.insert(id,claim); }
        }
        self.claimed=keep;
        updates
    }

    fn cancel_run(&mut self, operation_id: &OperationId) -> Vec<(String, ContextInjectionState)> {
        self.active_runs
            .retain(|_, active_run| active_run != operation_id.as_str());
        let pending = std::mem::take(&mut self.pending);
        let mut retained = Vec::with_capacity(pending.len());
        let mut updates = Vec::new();
        for item in pending {
            if item.request.expected_run_id == operation_id.as_str() {
                let injection_id = item.request.injection_id.clone();
                let state = ContextInjectionState::Cancelled;
                self.record_terminal(&injection_id, state.clone());
                updates.push((injection_id, state));
            } else {
                retained.push(item);
            }
        }
        self.pending = retained;
        let claimed=std::mem::take(&mut self.claimed);
        let mut keep=HashMap::new();
        for (id,claim) in claimed {
            if claim.pending.request.expected_run_id==operation_id.as_str() {
                self.delivery_attempts.remove(&id);
                let state=ContextInjectionState::Cancelled; self.record_terminal(&id,state.clone()); updates.push((id,state));
            } else { keep.insert(id,claim); }
        }
        self.claimed=keep;
        updates
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.claimed.clear();
        self.delivery_attempts.clear();
        self.completed_injection_ids.clear();
        self.completed_delivery_batch_ids.clear();
        self.terminal_states.clear();
        self.active_runs.clear();
        self.last_ended_runs.clear();
    }
}

pub struct NativeAgentBackend {
    engine: Arc<NativeEngine>,
    config: NativeAgentConfig,
    threads: Mutex<HashMap<String, NativeThread>>,
    mcp_sessions: Mutex<HashMap<String, NativeMcpSession>>,
    mcp_server_state: Mutex<NativeMcpServerStateStore>,
    disabled_tools: Mutex<HashMap<String, HashSet<String>>>,
    interaction_listener: Arc<dyn InteractionListener>,
    interaction_privacy_mode: PrivacyMode,
    scope_generation: Arc<AtomicU64>,
    cancelled_operations: Arc<Mutex<HashSet<String>>>,
    next_interaction_query_id: Arc<AtomicU64>,
    post_turn_flushes: Arc<AtomicU64>,
    response_comparison_controls: Arc<Mutex<HashMap<String, ResponseComparisonControl>>>,
    context_injections: Arc<Mutex<ContextInjectionWorkflowControlStore>>,
}

impl NativeAgentBackend {
    pub fn new(engine: Arc<NativeEngine>, config: NativeAgentConfig) -> Self {
        let privacy_mode = PrivacyMode::Unspecified;
        Self::new_with_interaction_listener(
            engine,
            config,
            privacy_mode,
            shipping_interaction_listener(privacy_mode),
        )
    }

    pub fn new_with_interaction_listener(
        engine: Arc<NativeEngine>,
        config: NativeAgentConfig,
        interaction_privacy_mode: PrivacyMode,
        interaction_listener: Arc<dyn InteractionListener>,
    ) -> Self {
        Self {
            engine,
            config,
            threads: Mutex::new(HashMap::new()),
            mcp_sessions: Mutex::new(HashMap::new()),
            mcp_server_state: Mutex::new(NativeMcpServerStateStore::default()),
            disabled_tools: Mutex::new(HashMap::new()),
            interaction_listener,
            interaction_privacy_mode,
            scope_generation: Arc::new(AtomicU64::new(1)),
            cancelled_operations: Arc::new(Mutex::new(HashSet::new())),
            next_interaction_query_id: Arc::new(AtomicU64::new(1)),
            post_turn_flushes: Arc::new(AtomicU64::new(0)),
            response_comparison_controls: Arc::new(Mutex::new(HashMap::new())),
            context_injections: Arc::new(Mutex::new(ContextInjectionWorkflowControlStore::default())),
        }
    }

    fn enqueue_response_comparison(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        expected_scope_generation: u64,
        capture: Arc<Mutex<ResponseComparisonParentCapture>>,
    ) {
        if self.interaction_privacy_mode != PrivacyMode::UsageCodebaseTrainingAllowed {
            return;
        }
        let capture = match capture.lock() {
            Ok(capture) => capture.clone(),
            Err(_) => return,
        };
        if capture.saw_non_delivery_tool_call {
            return;
        }
        let Some(parent_response) = capture
            .parent_response
            .filter(|response| !response.trim().is_empty())
        else {
            return;
        };
        if parent_response.chars().count() > DEFAULT_RESPONSE_COMPARISON_MAX_RESPONSE_CHARS {
            return;
        }

        let operation_key = operation_id.as_str().to_owned();
        let comparison_id = format!("response-comparison:{operation_key}");
        let alternate_invocation_id = format!("{operation_key}:alternate");
        let control = ResponseComparisonControl::default();
        {
            let Ok(mut controls) = self.response_comparison_controls.lock() else {
                return;
            };
            if let Some(previous) = controls.insert(operation_key.clone(), control.clone()) {
                previous.cancel();
            }
        }

        let engine = Arc::clone(&self.engine);
        let listener = Arc::clone(&self.interaction_listener);
        let scope_generation = Arc::clone(&self.scope_generation);
        let cancelled_operations = Arc::clone(&self.cancelled_operations);
        let controls = Arc::clone(&self.response_comparison_controls);
        let alternate_model_id = self.engine.response_comparison_model_name();
        let operation_key_for_cleanup = operation_key.clone();
        let enqueue_result = self.interaction_listener.enqueue_post_turn_ended_work(Box::new(move || {
            Box::pin(async move {
                let sink: SharedResponseComparisonEventSink =
                    Arc::new(NativeResponseComparisonInteractionSink {
                        comparison_id,
                        operation_id: operation_key.clone(),
                        listener,
                        scope_generation,
                        expected_scope_generation,
                        cancelled_operations,
                    });
                let request = ResponseComparisonRequest {
                    session_id,
                    parent_invocation_id: operation_key.clone(),
                    alternate_invocation_id,
                    parent_response,
                    comparison_config_id: "ios-native-response-comparison-v1".into(),
                    alternate_model_id,
                    display_order: NativeResponseComparisonDisplayOrder::ParentFirst,
                    max_response_chars: DEFAULT_RESPONSE_COMPARISON_MAX_RESPONSE_CHARS,
                    min_character_diff: DEFAULT_RESPONSE_COMPARISON_MIN_CHARACTER_DIFF,
                    timeout: RESPONSE_COMPARISON_TIMEOUT,
                };
                let _ = engine.run_response_comparison(request, control, sink).await;
                if let Ok(mut controls) = controls.lock() {
                    controls.remove(&operation_key);
                }
                Ok(())
            })
        }));
        if enqueue_result.is_err()
            && let Ok(mut controls) = self.response_comparison_controls.lock()
        {
            if let Some(control) = controls.remove(&operation_key_for_cleanup) {
                control.cancel();
            }
        }
    }

    async fn create_thread(
        &self,
        conversation_id: ConversationId,
    ) -> Result<(AgentThreadId, NativeThread), AgentError> {
        let session_id = self
            .engine
            .open_session(OpenSessionRequest {
                profile: self.config.profile,
                workspace_root: self
                    .config
                    .workspace_root
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                model: None,
                metadata: json!({"conversationId": conversation_id.as_str()}),
            })
            .await
            .map_err(kernel_error)?;
        let thread_id = AgentThreadId::generated("mahayana-native-thread");
        let thread = NativeThread {
            session_id,
            conversation_id,
            scope_generation: self.scope_generation.load(Ordering::SeqCst),
        };
        self.threads
            .lock()
            .map_err(|_| AgentError::Backend("native thread registry poisoned".into()))?
            .insert(thread_id.to_string(), thread.clone());
        Ok((thread_id, thread))
    }

    fn thread(&self, id: &AgentThreadId) -> Result<NativeThread, AgentError> {
        self.threads
            .lock()
            .map_err(|_| AgentError::Backend("native thread registry poisoned".into()))?
            .get(&id.to_string())
            .cloned()
            .ok_or_else(|| AgentError::ThreadNotFound(id.clone()))
    }

    fn ensure_thread_scope_current(&self, thread: &NativeThread) -> Result<(), AgentError> {
        if thread.scope_generation != self.scope_generation.load(Ordering::SeqCst) {
            return Err(AgentError::Unavailable(
                "native Agent result became stale because the account/session scope changed".into(),
            ));
        }
        Ok(())
    }

    fn operation_cancelled(&self, operation_id: &OperationId) -> Result<bool, AgentError> {
        Ok(self
            .cancelled_operations
            .lock()
            .map_err(|_| AgentError::Backend("interaction cancellation registry poisoned".into()))?
            .contains(operation_id.as_str()))
    }

    fn begin_context_injection_run(
        &self,
        conversation_id: &ConversationId,
        operation_id: &OperationId,
    ) -> Result<(), AgentError> {
        self.context_injections
            .lock()
            .map_err(|_| AgentError::Backend("context injection workflow-control store poisoned".into()))?
            .begin_run(conversation_id, operation_id);
        Ok(())
    }

    async fn emit_context_injection_state(
        &self,
        injection_id: String,
        state: ContextInjectionState,
    ) -> Result<(), AgentError> {
        self.interaction_listener
            .send_update(
                &InteractionListenerContext { canceled: false },
                InteractionUpdate::ContextInjectionState {
                    injection_id,
                    state,
                },
            )
            .await
            .map_err(interaction_error)
    }

    async fn finish_context_injection_run(
        &self,
        conversation_id: &ConversationId,
        operation_id: &OperationId,
    ) -> Result<(), AgentError> {
        let updates = self
            .context_injections
            .lock()
            .map_err(|_| AgentError::Backend("context injection workflow-control store poisoned".into()))?
            .finish_run(conversation_id, operation_id);
        for (injection_id, state) in updates {
            self.emit_context_injection_state(injection_id, state).await?;
        }
        Ok(())
    }

    async fn cancel_context_injection_run(
        &self,
        operation_id: &OperationId,
    ) -> Result<(), AgentError> {
        let updates = self
            .context_injections
            .lock()
            .map_err(|_| AgentError::Backend("context injection workflow-control store poisoned".into()))?
            .cancel_run(operation_id);
        for (injection_id, state) in updates {
            self.emit_context_injection_state(injection_id, state).await?;
        }
        Ok(())
    }

    fn mcp_session(&self, id: &AgentThreadId) -> Result<NativeMcpSession, AgentError> {
        self.mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .get(&id.to_string())
            .cloned()
            .ok_or_else(|| AgentError::ThreadNotFound(id.clone()))
    }

    fn is_tool_disabled(&self, server: &str, tool: &str) -> Result<bool, AgentError> {
        Ok(self
            .disabled_tools
            .lock()
            .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?
            .get(server)
            .is_some_and(|tools| tools.contains(tool)))
    }

    fn begin_mcp_server_attempt(&self, plugin: &ResolvedMcpPlugin) -> Result<u64, AgentError> {
        self.mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .begin(&plugin.server_name, &plugin.plugin_id)
            .ok_or_else(|| AgentError::Backend("native MCP server generation exhausted".into()))
    }

    fn settle_mcp_server_attempt(
        &self,
        server_identifier: &str,
        generation: u64,
        status: &str,
        status_detail: Option<String>,
        tools: Vec<Value>,
    ) -> Result<bool, AgentError> {
        Ok(self
            .mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .settle(
                server_identifier,
                generation,
                status,
                status_detail,
                tools,
            ))
    }

    async fn mcp_call(
        &self,
        session: NativeMcpSession,
        tool: String,
        arguments: Value,
    ) -> Result<Value, AgentError> {
        if self.is_tool_disabled(&session.plugin.server_name, &tool)? {
            return Err(AgentError::Unavailable(format!(
                "MCP tool `{tool}` is disabled by Mahayana policy"
            )));
        }
        tokio::task::spawn_blocking(move || session.plugin.client().call_tool(&tool, arguments))
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?
            .map_err(|error| AgentError::Backend(error.to_string()))
    }
}

fn shipping_interaction_listener(
    privacy_mode: PrivacyMode,
) -> Arc<dyn InteractionListener> {
    let fallback = NoopInteractionListener::default();
    let redacted = UnredactedDelegateAdapter::new(fallback, privacy_mode);
    let unredacted = RedactedDelegateAdapter::new(redacted, privacy_mode);
    Arc::new(CancellationFencedListener::new(unredacted))
}

fn context_injection_model_input(request:&ContextInjectionRequest,safe_boundary_tool:&str)->Value{
    let action_ref=&request.conversation_action_ref;
    let role=if action_ref.pointer("/action/value/payload/case").and_then(Value::as_str)==Some("systemContext"){"system"}else{"user"};
    let text=action_ref.pointer("/action/value/payload/value/content").and_then(Value::as_str)
        .or_else(||action_ref.pointer("/action/value/payload/value/userMessage/text").and_then(Value::as_str))
        .or_else(||action_ref.pointer("/action/value/payload/value/userMessage/content").and_then(Value::as_str))
        .or_else(||action_ref.pointer("/payload/text").and_then(Value::as_str))
        .or_else(||action_ref.get("text").and_then(Value::as_str))
        .map(str::trim).filter(|v|!v.is_empty());
    let canonical=serde_json::to_string(action_ref).unwrap_or_else(|_|"null".into());
    let content=match text { Some(v)=>format!("{v}\n\n[conversation_action_ref]\n{canonical}"),None=>format!("[conversation_action_ref]\n{canonical}") };
    json!({"role":role,"content":content,"source":"mahayana_context_injection","contextInjectionId":request.injection_id,"expectedRunId":request.expected_run_id,"safeBoundaryTool":safe_boundary_tool,"conversationActionRef":action_ref.clone()})
}

struct AgentInteractionDecisionGate {
    listener: Arc<dyn InteractionListener>,
    cancelled_operations: Arc<Mutex<HashSet<String>>>,
    scope_generation: Arc<AtomicU64>,
    expected_scope_generation: u64,
    next_query_id: Arc<AtomicU64>,
    conversation_id: ConversationId,
    context_injections: Arc<Mutex<ContextInjectionWorkflowControlStore>>,
}

impl AgentInteractionDecisionGate {
    fn is_cancelled(&self, operation_id: &KernelOperationId) -> Result<bool, KernelError> {
        Ok(self
            .cancelled_operations
            .lock()
            .map_err(|_| KernelError::Backend("interaction cancellation registry poisoned".into()))?
            .contains(operation_id.as_str()))
    }

    fn ensure_scope_current(&self) -> Result<(), KernelError> {
        if self.scope_generation.load(Ordering::SeqCst) != self.expected_scope_generation {
            return Err(KernelError::PolicyDenied(
                "Agent interaction response became stale after account/session scope change".into(),
            ));
        }
        Ok(())
    }

    fn next_query_id(&self) -> Result<u64, KernelError> {
        let id = self.next_query_id.fetch_add(1, Ordering::SeqCst);
        if id == u64::MAX {
            return Err(KernelError::Backend(
                "interaction query id space exhausted".into(),
            ));
        }
        Ok(id)
    }


}

#[async_trait]
impl ToolDecisionGate for AgentInteractionDecisionGate {
    async fn take_context_signal(&self,operation_id:&KernelOperationId,tool:&str)->Result<Option<ToolContextSignal>,KernelError>{
        self.ensure_scope_current()?;
        if self.is_cancelled(operation_id)? { return Err(KernelError::PolicyDenied("Agent interaction was cancelled before context signal claim".into())); }
        let claim=self.context_injections.lock().map_err(|_|KernelError::Backend("context injection workflow-control store poisoned".into()))?
            .claim_next(&self.conversation_id,operation_id.as_str(),self.expected_scope_generation);
        let Some(claim)=claim else { return Ok(None); };
        let signal_id=claim.pending.request.injection_id.clone();
        let input=context_injection_model_input(&claim.pending.request,tool);
        Ok(Some(ToolContextSignal{signal_id,input}))
    }

    async fn commit_context_signal(&self,operation_id:&KernelOperationId,signal_id:&str,step:Value)->Result<(),KernelError>{
        let current=self.scope_generation.load(Ordering::SeqCst);
        if current!=self.expected_scope_generation {
            if let Ok(mut s)=self.context_injections.lock(){s.rollback_claimed_delivery(signal_id,current);}
            return Err(KernelError::PolicyDenied("context signal became stale before Delivered commit".into()));
        }
        if self.is_cancelled(operation_id)? {
            if let Ok(mut s)=self.context_injections.lock(){s.rollback_claimed_delivery(signal_id,self.expected_scope_generation);}
            return Err(KernelError::PolicyDenied("context signal was cancelled before Delivered commit".into()));
        }
        let state=self.context_injections.lock().map_err(|_|KernelError::Backend("context injection workflow-control store poisoned".into()))?
            .commit_claimed_delivery(signal_id,self.expected_scope_generation,step,now_ms_u64()).map_err(KernelError::Backend)?;
        self.listener.send_update(&InteractionListenerContext{canceled:false},InteractionUpdate::ContextInjectionState{injection_id:signal_id.to_string(),state}).await
            .map_err(|e|KernelError::Backend(format!("context injection Delivered update failed after model consumption: {e}")))
    }

    async fn rollback_context_signal(&self,_:&KernelOperationId,signal_id:&str)->Result<bool,KernelError>{
        let current=self.scope_generation.load(Ordering::SeqCst);
        let (retry,terminal)={
            let mut s=self.context_injections.lock().map_err(|_|KernelError::Backend("context injection workflow-control store poisoned".into()))?;
            let retry=s.rollback_claimed_delivery(signal_id,current);
            let terminal=(!retry && current==self.expected_scope_generation).then(||s.terminal_states.get(signal_id).cloned()).flatten();
            (retry,terminal)
        };
        if let Some(state)=terminal {
            self.listener.send_update(&InteractionListenerContext{canceled:false},InteractionUpdate::ContextInjectionState{injection_id:signal_id.to_string(),state}).await
                .map_err(|e|KernelError::Backend(format!("context injection rollback terminal update failed: {e}")))?;
        }
        Ok(retry)
    }

    async fn before_tool(
        &self,
        operation_id: &KernelOperationId,
        tool: &str,
        arguments: &Value,
    ) -> Result<(), KernelError> {
        self.ensure_scope_current()?;
        let context = InteractionListenerContext {
            canceled: self.is_cancelled(operation_id)?,
        };
        if context.canceled {
            return Err(KernelError::PolicyDenied(
                "Agent interaction was cancelled before tool decision".into(),
            ));
        }

        let kind = match tool {
            "web_search" => InteractionQueryKind::WebSearch {
                args: arguments.clone(),
            },
            "web_fetch" => InteractionQueryKind::WebFetch {
                args: arguments.clone(),
                skip_approval: false,
                smart_mode_approval: None,
            },
            _ => return Ok(()),
        };
        let query = InteractionQuery {
            id: self.next_query_id()?,
            kind,
        };
        let response = self
            .listener
            .query(&context, query.clone())
            .await
            .map_err(|error| KernelError::PolicyDenied(error.to_string()))?;
        response
            .validate_for(&query)
            .map_err(|error| KernelError::PolicyDenied(error.to_string()))?;

        self.ensure_scope_current()?;
        if self.is_cancelled(operation_id)? {
            return Err(KernelError::PolicyDenied(
                "Agent interaction was cancelled before decision commit".into(),
            ));
        }

        match response.result {
            InteractionResponseKind::WebSearchApproved
            | InteractionResponseKind::WebFetchApproved => Ok(()),
            InteractionResponseKind::WebSearchRejected { reason }
            | InteractionResponseKind::WebFetchRejected { reason } => Err(KernelError::PolicyDenied(
                if reason.trim().is_empty() {
                    format!("{tool} rejected by Agent interaction listener")
                } else {
                    reason
                },
            )),
            other => Err(KernelError::PolicyDenied(format!(
                "unexpected Agent interaction response for {tool}: {}",
                other.name()
            ))),
        }
    }
}

#[derive(Clone, Default)]
struct ResponseComparisonParentCapture {
    parent_response: Option<String>,
    saw_non_delivery_tool_call: bool,
}

fn capture_response_comparison_parent(
    capture: &Arc<Mutex<ResponseComparisonParentCapture>>,
    event: &KernelEvent,
) -> Result<(), KernelError> {
    let mut capture = capture
        .lock()
        .map_err(|_| KernelError::Backend("response comparison capture poisoned".into()))?;
    match event {
        KernelEvent::ToolStarted { tool, .. } | KernelEvent::ToolCompleted { tool, .. }
            if tool != "send_message" =>
        {
            capture.saw_non_delivery_tool_call = true;
        }
        KernelEvent::ToolCompleted {
            tool,
            output,
            success: true,
            ..
        } if tool == "send_message" => {
            if let Some(text) = output
                .get("generatedMessage")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                capture.parent_response = Some(text.to_string());
            }
        }
        KernelEvent::MessageCompleted { text, .. } if !text.trim().is_empty() => {
            capture.parent_response = Some(text.clone());
        }
        _ => {}
    }
    Ok(())
}

struct NativeResponseComparisonInteractionSink {
    comparison_id: String,
    operation_id: String,
    listener: Arc<dyn InteractionListener>,
    scope_generation: Arc<AtomicU64>,
    expected_scope_generation: u64,
    cancelled_operations: Arc<Mutex<HashSet<String>>>,
}

#[async_trait]
impl ResponseComparisonEventSink for NativeResponseComparisonInteractionSink {
    async fn emit(&self, event: NativeResponseComparisonEvent) -> Result<(), String> {
        if self.scope_generation.load(Ordering::SeqCst) != self.expected_scope_generation {
            return Err("response comparison became stale after account/session scope change".into());
        }
        let cancelled = self
            .cancelled_operations
            .lock()
            .map_err(|_| "response comparison cancellation registry poisoned".to_string())?
            .contains(&self.operation_id);
        let is_cancel_terminal = matches!(
            &event,
            NativeResponseComparisonEvent::Skipped {
                reason: NativeResponseComparisonSkipReason::Cancelled
            }
        );
        if cancelled && !is_cancel_terminal {
            return Err("response comparison event became stale after cancellation".into());
        }

        let event = match event {
            NativeResponseComparisonEvent::Started {
                display_order,
                parent_invocation_id,
                alternate_invocation_id,
                parent_response,
                comparison_config_id,
                alternate_model_id,
            } => ResponseComparisonEvent::Started(ResponseComparisonStarted {
                display_order: match display_order {
                    NativeResponseComparisonDisplayOrder::ParentFirst => {
                        ResponseComparisonDisplayOrder::ParentFirst
                    }
                    NativeResponseComparisonDisplayOrder::AlternateFirst => {
                        ResponseComparisonDisplayOrder::AlternateFirst
                    }
                },
                parent_invocation_id,
                alternate_invocation_id,
                parent_response,
                comparison_config_id,
                alternate_model_id,
            }),
            NativeResponseComparisonEvent::TextDelta { text } => {
                ResponseComparisonEvent::TextDelta { text }
            }
            NativeResponseComparisonEvent::Completed => ResponseComparisonEvent::Completed,
            NativeResponseComparisonEvent::Skipped { reason } => ResponseComparisonEvent::Skipped {
                reason: match reason {
                    NativeResponseComparisonSkipReason::AlternateToolCall => {
                        ResponseComparisonSkipReason::AlternateToolCall
                    }
                    NativeResponseComparisonSkipReason::InferenceError => {
                        ResponseComparisonSkipReason::InferenceError
                    }
                    NativeResponseComparisonSkipReason::Timeout => {
                        ResponseComparisonSkipReason::Timeout
                    }
                    NativeResponseComparisonSkipReason::Cancelled => {
                        ResponseComparisonSkipReason::Cancelled
                    }
                },
            },
        };
        self.listener
            .send_update(
                &InteractionListenerContext { canceled: false },
                InteractionUpdate::ResponseComparison {
                    update: ResponseComparisonUpdate {
                        comparison_id: self.comparison_id.clone(),
                        event,
                    },
                },
            )
            .await
            .map_err(|error| error.to_string())
    }
}

struct NativeEventBridge {
    operation_id: OperationId,
    conversation_id: ConversationId,
    sink: SharedAgentEventSink,
    privacy_mode: PrivacyMode,
    response_comparison_parent: Option<Arc<Mutex<ResponseComparisonParentCapture>>>,
}

impl KernelEventSink for NativeEventBridge {
    fn emit(&self, event: KernelEvent) -> Result<(), KernelError> {
        if let Some(capture) = self.response_comparison_parent.as_ref() {
            capture_response_comparison_parent(capture, &event)?;
        }
        let interaction_projection =
            redacted_update_for_kernel_event(&event, self.privacy_mode);
        let mapped = match event {
            KernelEvent::MessageDelta { delta, .. } => {
                let delta = interaction_projection
                    .as_ref()
                    .and_then(|update| match &update.update {
                        InteractionUpdate::TextDelta { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or(delta);
                AgentEvent::MessageDelta { delta }
            },
            KernelEvent::MessageCompleted { text, .. } => AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("mahayana-native-message"),
                    conversation_id: self.conversation_id.clone(),
                    role: MessageRole::Assistant,
                    text,
                    created_at_ms: now_ms(),
                    metadata: json!({"engine":"mahayana-native"}),
                },
            },
            KernelEvent::UsageUpdated {
                total_tokens,
                input_tokens,
                cached_input_tokens,
                output_tokens,
                reasoning_output_tokens,
                ..
            } => AgentEvent::TokenUsageUpdated {
                usage: ModelTokenUsageSnapshot {
                    total: None,
                    last: ModelTokenUsage {
                        total_tokens: to_i64(
                            interaction_projection
                                .as_ref()
                                .and_then(|update| match &update.update {
                                    InteractionUpdate::TokenDelta { tokens } => Some(*tokens),
                                    _ => None,
                                })
                                .unwrap_or(total_tokens),
                        ),
                        input_tokens: to_i64(input_tokens),
                        cached_input_tokens: to_i64(cached_input_tokens),
                        output_tokens: to_i64(output_tokens),
                        reasoning_output_tokens: to_i64(reasoning_output_tokens),
                    },
                    model_context_window: None,
                },
            },
            KernelEvent::ApprovalRequested {
                approval_id,
                title,
                risk,
                details,
                ..
            } => AgentEvent::ApprovalRequested {
                approval_id: mahayana_core::ApprovalId::new(approval_id)
                    .map_err(|error| KernelError::Backend(error.to_string()))?,
                title,
                details: json!({"risk":risk,"details":details}),
            },
            KernelEvent::Activity {
                kind,
                title,
                detail,
                metadata,
                ..
            } => {
                let step_id = metadata
                    .get("stepId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("mahayana-native:{kind}:{title}"));
                AgentEvent::Activity {
                    activity: AgentActivity {
                        step_id,
                        kind,
                        title,
                        detail,
                        status: activity_status(&metadata),
                        metadata: Some(metadata),
                    },
                }
            }
            KernelEvent::ToolStarted { tool, .. } if tool == "send_message" => return Ok(()),
            KernelEvent::ToolStarted {
                tool, arguments, ..
            } => {
                let (title, detail) = tool_activity_copy(&tool, false, true);
                AgentEvent::Activity {
                    activity: AgentActivity {
                        step_id: format!("tool:{tool}"),
                        kind: "tool".into(),
                        title,
                        detail,
                        status: AgentActivityStatus::Running,
                        metadata: Some(json!({
                            "tool":tool,
                            "arguments":arguments,
                            "interaction":interaction_projection
                                .as_ref()
                                .map(|update| update.audit_metadata()),
                        })),
                    },
                }
            }
            KernelEvent::ToolCompleted {
                tool,
                output,
                success,
                ..
            } if tool == "send_message" => {
                if !success {
                    return Ok(());
                }
                let Some(event) =
                    native_send_message_event(&self.conversation_id, &output)
                else {
                    return Ok(());
                };
                event
            }
            KernelEvent::ToolCompleted {
                tool,
                output,
                success,
                ..
            } => {
                let (title, detail) = tool_activity_copy(&tool, true, success);
                AgentEvent::Activity {
                    activity: AgentActivity {
                        step_id: format!("tool:{tool}"),
                        kind: "tool".into(),
                        title,
                        detail,
                        status: if success {
                            AgentActivityStatus::Completed
                        } else {
                            AgentActivityStatus::Failed
                        },
                        metadata: Some(json!({
                            "tool":tool,
                            "output":output,
                            "success":success,
                            "interaction":interaction_projection
                                .as_ref()
                                .map(|update| update.audit_metadata()),
                        })),
                    },
                }
            }
            KernelEvent::CheckpointCreated {
                checkpoint_id,
                label,
                ..
            } => AgentEvent::Activity {
                activity: AgentActivity {
                    step_id: format!("checkpoint:{checkpoint_id}"),
                    kind: "checkpoint".into(),
                    title: label.unwrap_or_else(|| "Workspace checkpoint".into()),
                    detail: Some(checkpoint_id),
                    status: AgentActivityStatus::Completed,
                    metadata: None,
                },
            },
            KernelEvent::OperationFailed {
                message, retryable, ..
            } => AgentEvent::Activity {
                activity: AgentActivity {
                    step_id: format!("operation:{}", self.operation_id),
                    kind: "operation".into(),
                    title: "Operation failed".into(),
                    detail: Some(message),
                    status: AgentActivityStatus::Failed,
                    metadata: Some(json!({"retryable":retryable})),
                },
            },
            KernelEvent::OperationCompleted { .. } => return Ok(()),
        };
        self.sink
            .emit(mapped)
            .map_err(|error| KernelError::Backend(error.to_string()))
    }
}

#[async_trait]
impl AgentBackend for NativeAgentBackend {
    async fn start_thread(&self, request: StartThreadRequest) -> Result<AgentThreadId, AgentError> {
        self.create_thread(request.conversation_id)
            .await
            .map(|(thread_id, _)| thread_id)
    }

    async fn inject_context(
        &self,
        request: ContextInjectionRequest,
    ) -> Result<ContextInjectionOutcome, AgentError> {
        let scope_generation = self.scope_generation.load(Ordering::SeqCst);
        let injection_id = request.injection_id.clone();
        let admission = self
            .context_injections
            .lock()
            .map_err(|_| AgentError::Backend("context injection workflow-control store poisoned".into()))?
            .admit(request, scope_generation);
        if admission.emit_update {
            if let Err(error) = self
                .emit_context_injection_state(injection_id.clone(), admission.state.clone())
                .await
            {
                if admission.queued_new {
                    if let Ok(mut store) = self.context_injections.lock() {
                        store.rollback_queued_admission(&injection_id, scope_generation);
                    }
                }
                return Err(error);
            }
        }
        Ok(admission.outcome)
    }

    async fn send_message(
        &self,
        request: AgentMessageRequest,
        events: SharedAgentEventSink,
    ) -> Result<(), AgentError> {
        let thread = self.thread(&request.thread_id)?;
        self.ensure_thread_scope_current(&thread)?;
        if let Ok(mcp) = self.mcp_session(&request.thread_id)
            && mcp
                .tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some("chat"))
        {
            let result = self
                .mcp_call(
                    mcp,
                    "chat".into(),
                    json!({"message":request.text,"surface":"agent"}),
                )
                .await?;
            self.ensure_thread_scope_current(&thread)?;
            if self.operation_cancelled(&request.operation_id)? {
                return Err(AgentError::Unavailable(
                    "MCP chat result was cancelled before commit".into(),
                ));
            }
            return events.emit(AgentEvent::MessageCompleted {
                message: Message {
                    id: MessageId::generated("mcp-chat"),
                    conversation_id: request.conversation_id,
                    role: MessageRole::Assistant,
                    text: mcp_result_text(&result),
                    created_at_ms: now_ms(),
                    metadata: json!({"mcpResult":result}),
                },
            });
        }

        let kernel_operation_id = KernelOperationId::from_string(request.operation_id.as_str());
        let decision_gate: SharedToolDecisionGate = Arc::new(AgentInteractionDecisionGate {
            listener: Arc::clone(&self.interaction_listener),
            cancelled_operations: Arc::clone(&self.cancelled_operations),
            scope_generation: Arc::clone(&self.scope_generation),
            expected_scope_generation: thread.scope_generation,
            next_query_id: Arc::clone(&self.next_interaction_query_id),
            conversation_id: thread.conversation_id.clone(),
            context_injections: Arc::clone(&self.context_injections),
        });
        self.engine
            .install_tool_decision_gate(&kernel_operation_id, decision_gate)
            .map_err(kernel_error)?;
        let flush_counter = Arc::clone(&self.post_turn_flushes);
        self.interaction_listener
            .enqueue_post_turn_ended_work(Box::new(move || {
                Box::pin(async move {
                    flush_counter.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }))
            .map_err(interaction_error)?;
        self.begin_context_injection_run(
            &thread.conversation_id,
            &request.operation_id,
        )?;

        let response_comparison_parent =
            Arc::new(Mutex::new(ResponseComparisonParentCapture::default()));
        let response_comparison_session_id = thread.session_id.clone();
        let response_comparison_operation_id = request.operation_id.clone();
        let response_comparison_scope_generation = thread.scope_generation;
        let sink: SharedKernelEventSink = Arc::new(NativeEventBridge {
            operation_id: request.operation_id.clone(),
            conversation_id: thread.conversation_id.clone(),
            sink: events,
            privacy_mode: self.interaction_privacy_mode,
            response_comparison_parent: Some(Arc::clone(&response_comparison_parent)),
        });
        let run_result = self
            .engine
            .run(
                RunRequest {
                    session_id: thread.session_id.clone(),
                    operation_id: kernel_operation_id.clone(),
                    input: request.text,
                    policy: policy_for_profile(
                        self.config.profile,
                        self.config.allow_process,
                    ),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"clientMessageId":request.client_message_id}),
                },
                sink,
            )
            .await;
        let context_injection_finish_result = self
            .finish_context_injection_run(
                &thread.conversation_id,
                &request.operation_id,
            )
            .await;
        let remove_result = self.engine.remove_tool_decision_gate(&kernel_operation_id);
        if run_result.is_ok() {
            self.enqueue_response_comparison(
                response_comparison_session_id,
                response_comparison_operation_id,
                response_comparison_scope_generation,
                response_comparison_parent,
            );
        }
        let context = InteractionListenerContext {
            canceled: self.operation_cancelled(&request.operation_id)?,
        };
        let flush_result = self
            .interaction_listener
            .flush_post_turn_ended_work(&context)
            .await;
        self.ensure_thread_scope_current(&thread)?;
        context_injection_finish_result?;
        remove_result.map_err(kernel_error)?;
        flush_result.map_err(interaction_error)?;
        run_result.map_err(kernel_error)
    }

    async fn complete_async_ask_question(
        &self,
        request: mahayana_kernel::AsyncAskQuestionCompletionRequest,
        events: SharedAgentEventSink,
    ) -> Result<(), AgentError> {
        let bridge = Arc::new(NativeEventBridge {
            operation_id: OperationId(request.operation_id.as_str().to_string()),
            conversation_id: ConversationId(request.conversation_id.clone()),
            sink: events,
            privacy_mode: self.interaction_privacy_mode,
            response_comparison_parent: None,
        });
        self.engine
            .complete_async_ask_question(request, bridge)
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), AgentError> {
        self.cancelled_operations
            .lock()
            .map_err(|_| AgentError::Backend("interaction cancellation registry poisoned".into()))?
            .insert(operation_id.as_str().to_owned());
        self.cancel_context_injection_run(operation_id).await?;
        let comparison_control = self
            .response_comparison_controls
            .lock()
            .map_err(|_| AgentError::Backend("response comparison control registry poisoned".into()))?
            .get(operation_id.as_str())
            .cloned();
        if let Some(control) = comparison_control.as_ref() {
            control.cancel();
        }
        match self
            .engine
            .interrupt(&KernelOperationId::from_string(operation_id.as_str()))
            .await
        {
            Ok(()) => Ok(()),
            Err(KernelError::OperationNotFound(_)) if comparison_control.is_some() => Ok(()),
            Err(error) => Err(kernel_error(error)),
        }
    }

    async fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), AgentError> {
        self.engine
            .resolve_approval(KernelApprovalResolution {
                approval_id: resolution.approval_id.to_string(),
                approved: matches!(
                    resolution.decision,
                    ApprovalDecision::Accept | ApprovalDecision::AcceptForSession
                ),
                metadata: resolution.payload,
            })
            .await
            .map_err(kernel_error)
    }

    fn reset_session(&self) -> Result<(), AgentError> {
        self.scope_generation.fetch_add(1, Ordering::SeqCst);
        self.context_injections
            .lock()
            .map_err(|_| AgentError::Backend("context injection workflow-control store poisoned".into()))?
            .clear();
        {
            let mut controls = self
                .response_comparison_controls
                .lock()
                .map_err(|_| AgentError::Backend("response comparison control registry poisoned".into()))?;
            for control in controls.values() {
                control.cancel();
            }
            controls.clear();
        }
        NativeEngine::reset_session(&self.engine).map_err(kernel_error)?;
        self.cancelled_operations
            .lock()
            .map_err(|_| AgentError::Backend("interaction cancellation registry poisoned".into()))?
            .clear();
        self.threads
            .lock()
            .map_err(|_| AgentError::Backend("native thread registry poisoned".into()))?
            .clear();
        self.mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .clear();
        self.mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .clear();
        self.disabled_tools
            .lock()
            .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?
            .clear();
        Ok(())
    }

    async fn list_mcp_servers(&self) -> Result<Vec<Value>, AgentError> {
        Ok(self
            .mcp_server_state
            .lock()
            .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
            .projected())
    }

    async fn list_connector_apps(&self) -> Result<Vec<Value>, AgentError> {
        Ok(Vec::new())
    }

    async fn remove_mcp_server(&self, server: &str) -> Result<bool, AgentError> {
        let mut removed = false;
        {
            let mut sessions = self
                .mcp_sessions
                .lock()
                .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?;
            let before = sessions.len();
            sessions.retain(|_, session| session.plugin.server_name != server);
            removed |= sessions.len() != before;
        }
        {
            let mut state = self
                .mcp_server_state
                .lock()
                .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?;
            removed |= state.remove(server);
        }
        {
            let mut policies = self
                .disabled_tools
                .lock()
                .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?;
            removed |= policies.remove(server).is_some();
        }
        Ok(removed)
    }

    async fn mcp_custom_instructions(&self) -> Result<HashMap<String, String>, AgentError> {
        Ok(HashMap::new())
    }

    async fn set_mcp_tool_disabled(
        &self,
        server: &str,
        tool: &str,
        disabled: bool,
    ) -> Result<Vec<String>, AgentError> {
        let mut policies = self
            .disabled_tools
            .lock()
            .map_err(|_| AgentError::Backend("MCP tool policy registry poisoned".into()))?;
        let tools = policies.entry(server.to_string()).or_default();
        if disabled {
            tools.insert(tool.to_string());
        } else {
            tools.remove(tool);
        }
        let mut disabled = tools.iter().cloned().collect::<Vec<_>>();
        disabled.sort();
        Ok(disabled)
    }

    async fn refresh_mcp_servers(&self) -> Result<(), AgentError> {
        let live = self
            .mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .iter()
            .map(|(thread_id, session)| {
                (
                    thread_id.clone(),
                    session.plugin.plugin_id.clone(),
                    session.plugin.server_name.clone(),
                    session.platform,
                )
            })
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        for (_thread_id, plugin_id, server, platform) in live {
            if !seen.insert(server.clone()) {
                continue;
            }
            let generation = self
                .mcp_server_state
                .lock()
                .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?
                .begin(&server, &plugin_id)
                .ok_or_else(|| AgentError::Backend("native MCP server generation exhausted".into()))?;
            let registry = self.config.mcp_registry.clone();
            let plugin_id_for_resolve = plugin_id.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                registry.resolve_plugin(&plugin_id_for_resolve, platform)
            })
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?;
            let resolved = match resolved {
                Ok(resolved) if resolved.server_name == server => resolved,
                Ok(resolved) => {
                    let detail = format!(
                        "MCP refresh resolved {} instead of {}",
                        resolved.server_name,
                        server
                    );
                    let _ = self.settle_mcp_server_attempt(
                        &server,
                        generation,
                        "error",
                        Some(detail.clone()),
                        Vec::new(),
                    )?;
                    return Err(AgentError::Unavailable(detail));
                }
                Err(error) => {
                    let detail = error.to_string();
                    let _ = self.settle_mcp_server_attempt(
                        &server,
                        generation,
                        "error",
                        Some(detail.clone()),
                        Vec::new(),
                    )?;
                    return Err(AgentError::Unavailable(detail));
                }
            };
            let client = resolved.client();
            let tools = tokio::task::spawn_blocking(move || client.list_tools())
                .await
                .map_err(|error| AgentError::Backend(error.to_string()))?;
            let tools = match tools {
                Ok(tools) => tools,
                Err(error) => {
                    let detail = error.to_string();
                    let _ = self.settle_mcp_server_attempt(
                        &server,
                        generation,
                        "error",
                        Some(detail.clone()),
                        Vec::new(),
                    )?;
                    return Err(AgentError::Unavailable(detail));
                }
            };
            {
                let mut sessions = self
                    .mcp_sessions
                    .lock()
                    .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?;
                let state = self
                    .mcp_server_state
                    .lock()
                    .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?;
                if !state.is_current(&server, generation) {
                    continue;
                }
                for session in sessions.values_mut() {
                    if session.plugin.server_name == server {
                        session.plugin = resolved.clone();
                        session.platform = platform;
                        session.tools = tools.clone();
                    }
                }
            }
            let _ = self.settle_mcp_server_attempt(
                &server,
                generation,
                "connected",
                None,
                tools,
            )?;
        }
        Ok(())
    }

    async fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, AgentError> {
        let session = self
            .mcp_sessions
            .lock()
            .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?
            .values()
            .find(|session| session.plugin.server_name == server)
            .cloned()
            .ok_or_else(|| AgentError::Unavailable(format!("MCP server not open: {server}")))?;
        self.mcp_call(session, tool.to_string(), arguments).await
    }

    async fn open_mcp_app(&self, request: OpenMcpAppRequest) -> Result<McpAppSession, AgentError> {
        let platform = request.platform;
        let registry = self.config.mcp_registry.clone();
        let plugin_id = request.plugin_id.clone();
        let resolved =
            tokio::task::spawn_blocking(move || registry.resolve_plugin(&plugin_id, platform))
                .await
                .map_err(|error| AgentError::Backend(error.to_string()))?
                .map_err(|error| AgentError::Unavailable(error.to_string()))?;
        let server_identifier = resolved.server_name.clone();
        let generation = self.begin_mcp_server_attempt(&resolved)?;
        let client = resolved.client();
        let tools = match tokio::task::spawn_blocking({
            let client = client.clone();
            move || client.list_tools()
        })
        .await
        {
            Ok(Ok(tools)) => {
                let settled = self.settle_mcp_server_attempt(
                    &server_identifier,
                    generation,
                    "connected",
                    None,
                    tools.clone(),
                )?;
                if !settled {
                    return Err(AgentError::Unavailable(format!(
                        "MCP server open superseded by a newer attempt: {server_identifier}"
                    )));
                }
                tools
            }
            Ok(Err(error)) => {
                let detail = error.to_string();
                self.settle_mcp_server_attempt(
                    &server_identifier,
                    generation,
                    "error",
                    Some(detail.clone()),
                    Vec::new(),
                )?;
                return Err(AgentError::Backend(detail));
            }
            Err(error) => {
                let detail = error.to_string();
                self.settle_mcp_server_attempt(
                    &server_identifier,
                    generation,
                    "error",
                    Some(detail.clone()),
                    Vec::new(),
                )?;
                return Err(AgentError::Backend(detail));
            }
        };
        let resources = tokio::task::spawn_blocking({
            let client = client.clone();
            move || client.list_resources().unwrap_or_default()
        })
        .await
        .map_err(|error| AgentError::Backend(error.to_string()))?;
        let home_result = if tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some("home"))
        {
            tokio::task::spawn_blocking({
                let client = client.clone();
                move || client.call_tool("home", json!({}))
            })
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?
            .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let (thread_id, _) = self.create_thread(request.conversation_id).await?;
        {
            // Use the same lock order as reset_session (runnable sessions first,
            // canonical server state second) so a reset cannot clear state and
            // then be followed by an older in-flight open resurrecting a
            // runnable session.
            let mut sessions = self
                .mcp_sessions
                .lock()
                .map_err(|_| AgentError::Backend("native MCP session registry poisoned".into()))?;
            let state = self
                .mcp_server_state
                .lock()
                .map_err(|_| AgentError::Backend("native MCP server state poisoned".into()))?;
            if !state.is_current(&server_identifier, generation) {
                return Err(AgentError::Unavailable(format!(
                    "MCP server open superseded or reset before session commit: {server_identifier}"
                )));
            }
            sessions.insert(
                thread_id.to_string(),
                NativeMcpSession {
                    plugin: resolved.clone(),
                    platform,
                    tools: tools.clone(),
                },
            );
        }
        Ok(McpAppSession {
            thread_id,
            plugin_id: request.plugin_id,
            server: resolved.server_name,
            command_tools: command_tools(&tools),
            tool_gates: tool_gates(&tools),
            tools,
            home_result,
            ui_resources: resources,
        })
    }

    async fn list_mcp_app_tools(
        &self,
        thread_id: &AgentThreadId,
        server: &str,
    ) -> Result<Vec<Value>, AgentError> {
        let session = self.mcp_session(thread_id)?;
        if session.plugin.server_name != server {
            return Err(AgentError::Unavailable(format!(
                "MCP session is for `{}`, not `{server}`",
                session.plugin.server_name
            )));
        }
        Ok(session.tools)
    }

    async fn call_mcp_app_tool(
        &self,
        thread_id: &AgentThreadId,
        server: &str,
        tool: &str,
        arguments: Value,
    ) -> Result<Value, AgentError> {
        let session = self.mcp_session(thread_id)?;
        if session.plugin.server_name != server {
            return Err(AgentError::Unavailable(format!(
                "MCP session is for `{}`, not `{server}`",
                session.plugin.server_name
            )));
        }
        self.mcp_call(session, tool.to_string(), arguments).await
    }

    async fn read_mcp_app_resource(
        &self,
        thread_id: &AgentThreadId,
        server: &str,
        uri: &str,
    ) -> Result<Vec<Value>, AgentError> {
        let session = self.mcp_session(thread_id)?;
        if session.plugin.server_name != server {
            return Err(AgentError::Unavailable(format!(
                "MCP session is for `{}`, not `{server}`",
                session.plugin.server_name
            )));
        }
        let client = session.plugin.client();
        let uri = uri.to_string();
        tokio::task::spawn_blocking(move || client.read_resource(&uri))
            .await
            .map_err(|error| AgentError::Backend(error.to_string()))?
            .map_err(|error| AgentError::Backend(error.to_string()))
    }

    fn name(&self) -> &'static str {
        "mahayana-native"
    }
}

fn native_send_message_event(
    conversation_id: &ConversationId,
    output: &Value,
) -> Option<AgentEvent> {
    let text = output
        .get("generatedMessage")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let attachment = output
        .get("generatedAttachment")
        .cloned()
        .filter(|value| !value.is_null());
    let transcript_card = output
        .get("generatedTranscriptCard")
        .cloned()
        .filter(|value| !value.is_null());
    if text.is_empty() && attachment.is_none() && transcript_card.is_none() {
        return None;
    }
    let tool_call_id = output
        .get("toolCallId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("send-message");
    let reply_to_message_id = output
        .get("replyToMessageId")
        .cloned()
        .filter(|value| !value.is_null());
    Some(AgentEvent::MessageCompleted {
        message: Message {
            id: MessageId::generated(&format!("mahayana-native-send:{tool_call_id}")),
            conversation_id: conversation_id.clone(),
            role: MessageRole::Assistant,
            text: text.to_string(),
            created_at_ms: now_ms(),
            metadata: json!({
                "engine": "mahayana-native",
                "deliveryTool": "send_message",
                "toolCallId": tool_call_id,
                "generatedAttachment": attachment,
                "transcriptCard": transcript_card,
                "replyToMessageId": reply_to_message_id,
            }),
        },
    })
}

fn project_mcp_server_state(
    server_identifier: &str,
    plugin_id: &str,
    status: &str,
    status_detail: Option<&str>,
    tools: &[Value],
) -> Value {
    let tool_definitions = tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let input_schema = tool
                .get("inputSchema")
                .or_else(|| tool.get("input_schema"))
                .cloned()
                .unwrap_or(Value::Null);
            Some(json!({
                "name": name,
                "providerIdentifier": server_identifier,
                "toolName": name,
                "description": description,
                "inputSchema": input_schema,
            }))
        })
        .collect::<Vec<_>>();

    json!({
        "name": server_identifier,
        "serverIdentifier": server_identifier,
        "pluginId": plugin_id,
        "status": status,
        "statusDetail": status_detail.filter(|value| !value.trim().is_empty()),
        "runtime": "mahayana-native",
        "tools": tool_definitions,
    })
}

fn command_tools(tools: &[Value]) -> HashMap<String, String> {
    let mut commands = HashMap::new();
    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let command = tool
            .pointer("/annotations/command")
            .or_else(|| tool.pointer("/_meta/mahayana/command"))
            .and_then(Value::as_str);
        if let Some(command) = command {
            commands.insert(
                command.trim_start_matches('/').to_string(),
                name.to_string(),
            );
        }
    }
    commands
}

fn tool_gates(tools: &[Value]) -> HashMap<String, String> {
    tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?;
            let capability = tool
                .pointer("/annotations/requiresCapability")
                .or_else(|| tool.pointer("/_meta/mahayana/entitlement"))?
                .as_str()?;
            Some((name.to_string(), capability.to_string()))
        })
        .collect()
}

fn mcp_result_text(result: &Value) -> String {
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        let text = content
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return text;
        }
    }
    result
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            serde_json::to_string_pretty(result).unwrap_or_else(|_| "MCP result".into())
        })
}

fn policy_for_profile(profile: RuntimeProfile, allow_process: bool) -> ExecutionPolicy {
    let mut policy = match profile {
        RuntimeProfile::DesktopFull | RuntimeProfile::Headless => {
            ExecutionPolicy::interactive_default()
        }
        RuntimeProfile::MobileEmbedded | RuntimeProfile::WebWasm => {
            ExecutionPolicy::mobile_default()
        }
    };
    policy.allow_process = match profile {
        RuntimeProfile::DesktopFull | RuntimeProfile::Headless => true,
        RuntimeProfile::MobileEmbedded => allow_process,
        RuntimeProfile::WebWasm => false,
    };
    policy
}

#[cfg(test)]
mod platform_process_policy_tests {
    use super::*;

    #[test]
    fn mobile_process_policy_requires_explicit_host_backend() {
        assert!(!policy_for_profile(RuntimeProfile::MobileEmbedded, false).allow_process);
        assert!(policy_for_profile(RuntimeProfile::MobileEmbedded, true).allow_process);
        assert!(!policy_for_profile(RuntimeProfile::WebWasm, true).allow_process);
        assert!(policy_for_profile(RuntimeProfile::DesktopFull, false).allow_process);
    }
}

fn tool_activity_copy(tool: &str, completed: bool, success: bool) -> (String, Option<String>) {
    let (running, done, detail) = match tool {
        "subagent_run" => (
            "请独立子智能体复核第一版",
            "独立复核已完成",
            "检查完整性、可运行性和明显遗漏",
        ),
        "workspace_read" => ("读取工作区文件", "工作区文件已读取", "基于真实文件继续处理"),
        "workspace_write" => ("写入实现文件", "实现文件已写入", "把变更落到实际工作区"),
        "workspace_search" => ("搜索相关代码", "相关代码搜索完成", "定位需要处理的实现位置"),
        "workspace_checkpoint" => (
            "创建安全检查点",
            "安全检查点已创建",
            "为后续修改保留可恢复状态",
        ),
        "process_exec" => ("运行验证命令", "验证命令已完成", "使用真实执行结果检查实现"),
        "ExternalShell" => (
            "在已连接电脑运行命令",
            "已连接电脑命令执行完成",
            "使用受控远端执行结果继续处理",
        ),
        "ExternalRead" => (
            "读取已连接电脑文件",
            "已连接电脑文件读取完成",
            "使用受控远端文件结果继续处理",
        ),
        "git_status" => ("检查改动状态", "改动状态已检查", "确认当前工作区变更"),
        "git_diff" => ("检查代码差异", "代码差异已检查", "核对实际修改内容"),
        "web_search" => ("搜索外部资料", "外部资料搜索完成", "获取任务需要的最新信息"),
        "web_fetch" => ("读取外部资料", "外部资料读取完成", "核对来源内容"),
        "workflow_create" => ("建立执行步骤", "执行步骤已建立", "按依赖关系组织后续工作"),
        "workflow_status" => ("检查任务进度", "任务进度已更新", "确认各步骤当前状态"),
        _ => ("执行工具步骤", "工具步骤已完成", "继续推进实际任务"),
    };
    if completed {
        if success {
            (done.to_string(), Some(detail.to_string()))
        } else {
            (
                format!("{done}（失败）"),
                Some("这一步没有成功，Agent 会根据错误继续处理".into()),
            )
        }
    } else {
        (running.to_string(), Some(detail.to_string()))
    }
}

fn activity_status(metadata: &Value) -> AgentActivityStatus {
    match metadata.get("status").and_then(Value::as_str) {
        Some("completed") => AgentActivityStatus::Completed,
        Some("failed") => AgentActivityStatus::Failed,
        _ => AgentActivityStatus::Running,
    }
}

fn interaction_error(error: InteractionError) -> AgentError {
    AgentError::Backend(error.to_string())
}

fn kernel_error(error: KernelError) -> AgentError {
    match error {
        KernelError::SessionNotFound(id) => AgentError::Backend(format!("session not found: {id}")),
        KernelError::OperationNotFound(id) => AgentError::OperationNotFound(
            OperationId::new(id).unwrap_or_else(|_| OperationId::generated("operation")),
        ),
        KernelError::ApprovalNotFound(id) => AgentError::ApprovalNotFound(
            mahayana_core::ApprovalId::new(id)
                .unwrap_or_else(|_| mahayana_core::ApprovalId::generated("approval")),
        ),
        KernelError::BackendUnavailable(message) | KernelError::CapabilityUnavailable(message) => {
            AgentError::Unavailable(message)
        }
        KernelError::EventConsumerClosed => AgentError::EventConsumerClosed,
        other => AgentError::Backend(other.to_string()),
    }
}

fn now_ms_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_send_message_completion_becomes_the_canonical_visible_agent_message() {
        let conversation_id = ConversationId::new("mahayana-ai:agent:test")
            .expect("valid conversation id");
        let event = native_send_message_event(
            &conversation_id,
            &json!({
                "generatedMessage": "Final answer",
                "generatedAttachment": {"name":"report.pdf","path":"files/report.pdf"},
                "replyToMessageId": "user:42",
                "toolCallId": "call-send-42"
            }),
        )
        .expect("visible send_message event");
        let AgentEvent::MessageCompleted { message } = event else {
            panic!("send_message must become MessageCompleted");
        };
        assert_eq!(message.conversation_id, conversation_id);
        assert_eq!(message.text, "Final answer");
        assert_eq!(message.metadata["deliveryTool"], "send_message");
        assert_eq!(message.metadata["toolCallId"], "call-send-42");
        assert_eq!(message.metadata["generatedAttachment"]["name"], "report.pdf");
        assert_eq!(message.metadata["replyToMessageId"], "user:42");
    }

    #[test]
    fn native_send_message_projection_accepts_card_only_awaiting_user_delivery() {
        let event = native_send_message_event(
            &ConversationId::new("mahayana-ai:agent:test").expect("valid conversation id"),
            &json!({
                "generatedTranscriptCard": {
                    "kind": "secretRequest",
                    "requestId": "secret:call-1",
                    "label": "API key",
                    "provided": false
                },
                "toolCallId": "call-1"
            }),
        )
        .expect("card-only delivery");
        let AgentEvent::MessageCompleted { message } = event else {
            panic!("card-only send_message must become MessageCompleted");
        };
        assert_eq!(message.text, "");
        assert_eq!(message.metadata["transcriptCard"]["kind"], "secretRequest");
        assert_eq!(
            message.metadata["transcriptCard"]["requestId"],
            "secret:call-1"
        );
    }

    #[test]
    fn native_send_message_projection_rejects_empty_success_payloads() {
        assert!(
            native_send_message_event(
                &ConversationId::new("mahayana-ai:agent:test").expect("valid conversation id"),
                &json!({"generatedMessage":"   ","generatedAttachment":null})
            )
            .is_none()
        );
    }

    struct RejectingInteractionListener;

    #[async_trait]
    impl InteractionListener for RejectingInteractionListener {
        async fn send_update(
            &self,
            _context: &InteractionListenerContext,
            _update: InteractionUpdate,
        ) -> Result<(), InteractionError> {
            Ok(())
        }

        async fn query(
            &self,
            _context: &InteractionListenerContext,
            query: InteractionQuery,
        ) -> Result<interaction::InteractionResponse, InteractionError> {
            Ok(match query.kind {
                InteractionQueryKind::WebSearch { .. } => interaction::Responses::web_search_rejected(
                    query.id,
                    "focused production decision rejected",
                ),
                InteractionQueryKind::WebFetch { .. } => interaction::Responses::web_fetch_rejected(
                    query.id,
                    "focused production decision rejected",
                ),
                _ => return Err(InteractionError::StreamClosed {
                    message: "unexpected focused query".into(),
                }),
            })
        }

        fn enqueue_post_turn_ended_work(
            &self,
            _work: interaction::PostTurnWork,
        ) -> Result<(), InteractionError> {
            Ok(())
        }

        async fn flush_post_turn_ended_work(
            &self,
            _context: &InteractionListenerContext,
        ) -> Result<(), InteractionError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn shipping_interaction_gate_routes_typed_web_queries_and_fences_scope() {
        let scope_generation = Arc::new(AtomicU64::new(7));
        let gate = AgentInteractionDecisionGate {
            listener: shipping_interaction_listener(PrivacyMode::NoTraining),
            cancelled_operations: Arc::new(Mutex::new(HashSet::new())),
            scope_generation: Arc::clone(&scope_generation),
            expected_scope_generation: 7,
            next_query_id: Arc::new(AtomicU64::new(41)),
            conversation_id: ConversationId::new("mahayana-ai:agent:interaction-focus")
                .expect("conversation id"),
            context_injections: Arc::new(Mutex::new(
                ContextInjectionWorkflowControlStore::default(),
            )),
        };
        let operation_id = KernelOperationId::from_string("interaction-focus-op");

        gate.before_tool(
            &operation_id,
            "web_search",
            &json!({"query":"current Fabushi state"}),
        )
        .await
        .expect("shipping Noop fallback approves web search through typed listener");

        gate.before_tool(
            &operation_id,
            "web_fetch",
            &json!({"urls":["https://example.com"]}),
        )
        .await
        .expect("shipping Noop fallback approves web fetch through typed listener");

        scope_generation.store(8, Ordering::SeqCst);
        let error = gate
            .before_tool(&operation_id, "web_search", &json!({"query":"stale"}))
            .await
            .expect_err("scope change must fence stale interaction decisions");
        assert!(error.to_string().contains("stale"));
    }

    #[tokio::test]
    async fn production_interaction_gate_honors_listener_rejection_and_cancellation() {
        let cancelled_operations = Arc::new(Mutex::new(HashSet::new()));
        let gate = AgentInteractionDecisionGate {
            listener: Arc::new(CancellationFencedListener::new(RejectingInteractionListener)),
            cancelled_operations: Arc::clone(&cancelled_operations),
            scope_generation: Arc::new(AtomicU64::new(3)),
            expected_scope_generation: 3,
            next_query_id: Arc::new(AtomicU64::new(1)),
            conversation_id: ConversationId::new("mahayana-ai:agent:interaction-reject")
                .expect("conversation id"),
            context_injections: Arc::new(Mutex::new(
                ContextInjectionWorkflowControlStore::default(),
            )),
        };
        let operation_id = KernelOperationId::from_string("interaction-reject-op");

        let rejected = gate
            .before_tool(&operation_id, "web_search", &json!({"query":"denied"}))
            .await
            .expect_err("listener rejection must fail closed before tool execution");
        assert!(rejected.to_string().contains("focused production decision rejected"));

        cancelled_operations
            .lock()
            .expect("cancel registry")
            .insert(operation_id.as_str().to_owned());
        let cancelled = gate
            .before_tool(
                &operation_id,
                "web_fetch",
                &json!({"urls":["https://example.com"]}),
            )
            .await
            .expect_err("cancelled operation must fail closed before listener commit");
        assert!(cancelled.to_string().contains("cancelled"));
    }

    #[test]
    fn command_and_entitlement_metadata_are_product_owned() {
        let tools = vec![json!({
            "name":"publish",
            "annotations":{"command":"/ship","requiresCapability":"publish.pro"}
        })];
        assert_eq!(
            command_tools(&tools).get("ship").map(String::as_str),
            Some("publish")
        );
        assert_eq!(
            tool_gates(&tools).get("publish").map(String::as_str),
            Some("publish.pro")
        );
    }
}


#[cfg(test)]
mod context_injection_native_agent_backend_tests {
    use super::*;

    fn conversation() -> ConversationId {
        ConversationId::new("mahayana-ai:agent:context-injection")
            .expect("context injection conversation")
    }

    fn operation(id: &str) -> OperationId {
        OperationId::new(id).expect("operation id")
    }

    fn request(
        injection_id: &str,
        expected_run_id: &str,
        delivery_batch_id: Option<&str>,
    ) -> ContextInjectionRequest {
        ContextInjectionRequest {
            conversation_id: conversation(),
            injection_id: injection_id.to_string(),
            expected_run_id: expected_run_id.to_string(),
            created_at_ms: 42,
            source: "background-composer".into(),
            conversation_action_ref: json!({"type":"injectContext","payload":{"text":"new context"}}),
            delivery_batch_id: delivery_batch_id.map(str::to_string),
        }
    }

    #[test]
    fn duplicate_injection_id_is_idempotent_without_duplicate_queue_or_delivery() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let run = operation("run:duplicate");
        store.begin_run(&conversation(), &run);
        let first = store.admit(request("inj-1", run.as_str(), Some("batch-1")), 7);
        let duplicate = store.admit(request("inj-1", run.as_str(), Some("batch-1")), 7);
        assert_eq!(first.outcome, ContextInjectionOutcome::Queued);
        assert_eq!(duplicate.outcome, ContextInjectionOutcome::Queued);
        assert!(first.emit_update);
        assert!(!duplicate.emit_update);
        assert_eq!(store.pending.len(), 1);

        let claim = store
            .claim_next(&conversation(), run.as_str(), 7)
            .expect("single queued injection");
        store
            .commit_delivery(&claim, 7, json!({"step":"safe"}), 100)
            .expect("deliver once");
        assert!(store.claim_next(&conversation(), run.as_str(), 7).is_none());
        let completed_duplicate =
            store.admit(request("inj-1", run.as_str(), Some("batch-1")), 7);
        assert_eq!(completed_duplicate.outcome, ContextInjectionOutcome::Queued);
        assert!(!completed_duplicate.emit_update);
        assert_eq!(store.completed_injection_ids.len(), 1);
        assert_eq!(store.completed_delivery_batch_ids.len(), 1);
    }

    #[test]
    fn expected_run_mismatch_is_rejected_without_mutating_pending_queue() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let active = operation("run:active");
        store.begin_run(&conversation(), &active);
        let admission = store.admit(request("inj-mismatch", "run:stale", None), 3);
        assert!(matches!(
            admission.outcome,
            ContextInjectionOutcome::Rejected { .. }
        ));
        assert!(store.pending.is_empty());
        assert!(store.completed_injection_ids.contains("inj-mismatch"));
        assert!(matches!(
            store.terminal_states.get("inj-mismatch"),
            Some(ContextInjectionState::Rejected { .. })
        ));
    }

    #[test]
    fn active_run_claims_only_at_safe_boundary_and_commits_delivered_once() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let run = operation("run:safe-boundary");
        store.begin_run(&conversation(), &run);
        store.admit(request("inj-safe", run.as_str(), Some("batch-safe")), 9);
        let claim = store
            .claim_next(&conversation(), run.as_str(), 9)
            .expect("safe-boundary claim");
        let state = store
            .commit_delivery(
                &claim,
                9,
                json!({"kind":"safeToolBoundary","tool":"web_search"}),
                1234,
            )
            .expect("delivery commit");
        assert!(matches!(
            state,
            ContextInjectionState::Delivered {
                delivered_at_ms: 1234,
                ..
            }
        ));
        assert!(store.claim_next(&conversation(), run.as_str(), 9).is_none());
    }

    #[test]
    fn run_end_race_moves_pending_and_late_admission_to_queued_for_next_turn() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let run = operation("run:end-race");
        store.begin_run(&conversation(), &run);
        store.admit(request("inj-pending", run.as_str(), None), 4);
        let updates = store.finish_run(&conversation(), &run);
        assert_eq!(
            updates,
            vec![(
                "inj-pending".to_string(),
                ContextInjectionState::QueuedForNextTurn
            )]
        );
        assert!(store.pending.is_empty());
        let late = store.admit(request("inj-late", run.as_str(), None), 4);
        assert_eq!(late.outcome, ContextInjectionOutcome::QueuedForNextTurn);
        assert!(matches!(
            store.terminal_states.get("inj-late"),
            Some(ContextInjectionState::QueuedForNextTurn)
        ));
    }

    #[test]
    fn cancellation_before_claim_marks_pending_cancelled_and_blocks_stale_delivery() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let run = operation("run:cancel");
        store.begin_run(&conversation(), &run);
        store.admit(request("inj-cancel", run.as_str(), None), 5);
        let updates = store.cancel_run(&run);
        assert_eq!(
            updates,
            vec![(
                "inj-cancel".to_string(),
                ContextInjectionState::Cancelled
            )]
        );
        assert!(store.pending.is_empty());
        assert!(store.claim_next(&conversation(), run.as_str(), 5).is_none());
        assert!(store.completed_injection_ids.contains("inj-cancel"));
    }

    #[test]
    fn claimed_delivery_failure_rolls_back_pending_and_allows_exactly_one_retry() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let run = operation("run:rollback");
        store.begin_run(&conversation(), &run);
        store.admit(request("inj-rollback", run.as_str(), Some("batch-rb")), 6);
        let claim = store
            .claim_next(&conversation(), run.as_str(), 6)
            .expect("first claim");
        assert!(store.rollback_delivery(claim, 6));
        assert_eq!(store.pending.len(), 1);
        assert!(!store.completed_injection_ids.contains("inj-rollback"));
        assert!(!store.completed_delivery_batch_ids.contains("batch-rb"));

        let retry = store
            .claim_next(&conversation(), run.as_str(), 6)
            .expect("single retry claim");
        store
            .commit_delivery(&retry, 6, json!({"step":"retry"}), 11)
            .expect("retry commit");
        assert!(store.claim_next(&conversation(), run.as_str(), 6).is_none());
        assert_eq!(store.completed_injection_ids.len(), 1);
        assert_eq!(store.completed_delivery_batch_ids.len(), 1);
    }

    #[test]
    fn successful_delivery_atomically_updates_pending_injection_and_batch_completion_sets() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let run = operation("run:success");
        store.begin_run(&conversation(), &run);
        store.admit(request("inj-success", run.as_str(), Some("batch-success")), 8);
        let claim = store
            .claim_next(&conversation(), run.as_str(), 8)
            .expect("claim");
        let delivered = store
            .commit_delivery(&claim, 8, json!({"step":"commit"}), 77)
            .expect("commit");
        assert!(store.pending.is_empty());
        assert!(store.completed_injection_ids.contains("inj-success"));
        assert!(store.completed_delivery_batch_ids.contains("batch-success"));
        assert_eq!(store.terminal_states.get("inj-success"), Some(&delivered));
    }

    #[test]
    fn scope_generation_reset_fences_claimed_delivery_and_clears_old_account_state() {
        let mut store = ContextInjectionWorkflowControlStore::default();
        let run = operation("run:old-scope");
        store.begin_run(&conversation(), &run);
        store.admit(request("inj-old-scope", run.as_str(), Some("batch-old")), 11);
        let claim = store
            .claim_next(&conversation(), run.as_str(), 11)
            .expect("old-scope claim");
        store.clear();
        let new_run = operation("run:new-scope");
        store.begin_run(&conversation(), &new_run);
        let error = store
            .commit_delivery(&claim, 12, json!({"step":"stale"}), 99)
            .expect_err("old claim must be stale-fenced");
        assert!(error.contains("stale"));
        assert!(store.pending.is_empty());
        assert!(store.completed_injection_ids.is_empty());
        assert!(store.completed_delivery_batch_ids.is_empty());
        assert!(store.terminal_states.is_empty());
    }
}

#[cfg(test)]
mod mcp_state_projection_tests {
    use super::*;

    #[test]
    fn production_mcp_state_store_preserves_failure_detail_and_rejects_stale_settlement() {
        let mut store = NativeMcpServerStateStore::default();
        let first = store.begin("calendar", "calendar-plugin").expect("first generation");
        let second = store.begin("calendar", "calendar-plugin").expect("second generation");
        assert!(second > first);

        assert!(!store.settle(
            "calendar",
            first,
            "connected",
            None,
            vec![json!({"name": "stale"})],
        ));
        assert!(store.settle(
            "calendar",
            second,
            "error",
            Some("transport handshake failed".into()),
            Vec::new(),
        ));

        let projected = store.projected();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0]["status"], "error");
        assert_eq!(projected[0]["statusDetail"], "transport handshake failed");
        assert_eq!(projected[0]["tools"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn production_mcp_state_store_remove_invalidates_older_settlement() {
        let mut store = NativeMcpServerStateStore::default();
        let generation = store
            .begin("calendar", "calendar-plugin")
            .expect("generation");
        assert!(store.remove("calendar"));
        assert!(!store.remove("calendar"));
        assert!(!store.settle(
            "calendar",
            generation,
            "connected",
            None,
            vec![json!({"name": "stale-after-remove"})],
        ));
        assert!(store.projected().is_empty());
        let replacement = store
            .begin("calendar", "calendar-plugin")
            .expect("replacement generation");
        assert!(replacement > generation);
    }

    #[test]
    fn production_mcp_state_store_keeps_generation_monotonic_across_reset() {
        let mut store = NativeMcpServerStateStore::default();
        let before_reset = store
            .begin("calendar", "calendar-plugin")
            .expect("pre-reset generation");
        store.clear();
        let after_reset = store
            .begin("calendar", "calendar-plugin")
            .expect("post-reset generation");

        assert!(after_reset > before_reset);
        assert!(!store.settle(
            "calendar",
            before_reset,
            "connected",
            None,
            vec![json!({"name": "stale-after-reset"})],
        ));
        assert_eq!(store.projected()[0]["status"], "loading");
    }

    #[test]
    fn production_mcp_state_store_settles_tools_on_current_generation() {
        let mut store = NativeMcpServerStateStore::default();
        let generation = store.begin("calendar", "calendar-plugin").expect("generation");
        assert!(store.settle(
            "calendar",
            generation,
            "connected",
            None,
            vec![json!({
                "name": "search",
                "description": "Search calendar entries",
                "inputSchema": {"type": "object"}
            })],
        ));

        let projected = store.projected();
        assert_eq!(projected[0]["status"], "connected");
        assert!(projected[0]["statusDetail"].is_null());
        assert_eq!(projected[0]["tools"][0]["toolName"], "search");
        assert_eq!(projected[0]["tools"][0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn canonical_mcp_state_projection_preserves_tool_schema_and_status_detail() {
        let state = project_mcp_server_state(
            "calendar",
            "calendar-plugin",
            "connected",
            Some("healthy detail"),
            &[
                json!({
                    "name": "search",
                    "description": "Search calendar entries",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"query": {"type": "string"}}
                    }
                }),
                json!({"name": "create", "inputSchema": {"type": "object"}}),
            ],
        );

        assert_eq!(state["serverIdentifier"], "calendar");
        assert_eq!(state["pluginId"], "calendar-plugin");
        assert_eq!(state["status"], "connected");
        assert_eq!(state["statusDetail"], "healthy detail");
        assert_eq!(state["tools"].as_array().map(Vec::len), Some(2));
        assert_eq!(state["tools"][0]["providerIdentifier"], "calendar");
        assert_eq!(state["tools"][0]["toolName"], "search");
        assert_eq!(state["tools"][0]["description"], "Search calendar entries");
        assert_eq!(
            state["tools"][0]["inputSchema"]["properties"]["query"]["type"],
            "string"
        );
    }

    #[test]
    fn canonical_mcp_state_projection_filters_empty_detail_and_invalid_tools() {
        let state = project_mcp_server_state(
            "files",
            "files-plugin",
            "connected",
            Some("   "),
            &[json!({"description": "missing name"}), json!({"name": "   "})],
        );

        assert!(state["statusDetail"].is_null());
        assert_eq!(state["tools"].as_array().map(Vec::len), Some(0));
    }
    #[test]
    fn response_comparison_capture_uses_delivery_text_and_rejects_semantic_tool_turns() {
        let capture = Arc::new(Mutex::new(ResponseComparisonParentCapture::default()));
        capture_response_comparison_parent(
            &capture,
            &KernelEvent::ToolCompleted {
                operation_id: KernelOperationId::from_string("op-compare"),
                tool_call_id: "send-1".into(),
                tool: "send_message".into(),
                output: json!({"generatedMessage":"parent answer"}),
                success: true,
            },
        )
        .expect("capture send_message");
        {
            let captured = capture.lock().expect("capture");
            assert_eq!(captured.parent_response.as_deref(), Some("parent answer"));
            assert!(!captured.saw_non_delivery_tool_call);
        }
        capture_response_comparison_parent(
            &capture,
            &KernelEvent::ToolStarted {
                operation_id: KernelOperationId::from_string("op-compare"),
                tool_call_id: "tool-1".into(),
                tool: "workspace_read".into(),
                arguments: json!({"path":"README.md"}),
            },
        )
        .expect("capture semantic tool");
        assert!(capture
            .lock()
            .expect("capture")
            .saw_non_delivery_tool_call);
    }

}


#[cfg(test)]
mod context_injection_transient_signal_tests {
    use super::*;
    fn conversation()->ConversationId{ConversationId::new("mahayana-ai:agent:context-signal").unwrap()}
    fn operation(id:&str)->OperationId{OperationId::new(id).unwrap()}
    fn request(id:&str,run:&str)->ContextInjectionRequest{
        ContextInjectionRequest{conversation_id:conversation(),injection_id:id.into(),expected_run_id:run.into(),created_at_ms:1,source:"background-composer".into(),
            conversation_action_ref:json!({"action":{"case":"injectContextAction","value":{"injectionId":id,"expectedRunId":run,"payload":{"case":"systemContext","value":{"producer":"background-composer","content":"fresh canonical context"}}}}}),
            delivery_batch_id:Some(format!("batch-{id}"))}
    }
    fn gate(store:Arc<Mutex<ContextInjectionWorkflowControlStore>>,scope:Arc<AtomicU64>,cancelled:Arc<Mutex<HashSet<String>>>,expected:u64)->AgentInteractionDecisionGate{
        AgentInteractionDecisionGate{listener:shipping_interaction_listener(PrivacyMode::NoTraining),cancelled_operations:cancelled,scope_generation:scope,expected_scope_generation:expected,next_query_id:Arc::new(AtomicU64::new(1)),conversation_id:conversation(),context_injections:store}
    }

    #[test]
    fn context_injection_payload_preserves_exact_conversation_action_ref_for_model_input(){
        let r=request("inj-payload","run:payload"); let v=context_injection_model_input(&r,"workspace_read");
        assert_eq!(v["role"],"system"); assert_eq!(v["source"],"mahayana_context_injection"); assert_eq!(v["contextInjectionId"],"inj-payload");
        assert_eq!(v["conversationActionRef"],r.conversation_action_ref); assert!(v["content"].as_str().unwrap().contains("fresh canonical context"));
    }

    #[test]
    fn context_injection_signal_failure_can_retry_once_and_only_once(){
        let mut s=ContextInjectionWorkflowControlStore::default(); let run=operation("run:retry-once"); s.begin_run(&conversation(),&run); s.admit(request("inj-retry",run.as_str()),7);
        let first=s.claim_next(&conversation(),run.as_str(),7).unwrap(); assert!(s.rollback_delivery(first,7));
        let second=s.claim_next(&conversation(),run.as_str(),7).unwrap(); assert!(!s.rollback_delivery(second,7));
        assert!(s.claim_next(&conversation(),run.as_str(),7).is_none()); assert!(s.pending.is_empty()); assert!(s.claimed.is_empty());
        assert!(matches!(s.terminal_states.get("inj-retry"),Some(ContextInjectionState::QueuedForNextTurn)));
    }

    #[tokio::test]
    async fn cancel_after_signal_claim_never_commits_delivered(){
        let run=operation("run:cancel"); let store=Arc::new(Mutex::new(ContextInjectionWorkflowControlStore::default()));
        store.lock().unwrap().begin_run(&conversation(),&run); store.lock().unwrap().admit(request("inj-cancel",run.as_str()),9);
        let cancelled=Arc::new(Mutex::new(HashSet::new())); let g=gate(store.clone(),Arc::new(AtomicU64::new(9)),cancelled.clone(),9); let kop=KernelOperationId::from_string(run.as_str());
        g.take_context_signal(&kop,"workspace_read").await.unwrap().unwrap(); cancelled.lock().unwrap().insert(run.as_str().into());
        assert!(g.commit_context_signal(&kop,"inj-cancel",json!({"kind":"nextInnerStep"})).await.unwrap_err().to_string().contains("cancelled"));
        store.lock().unwrap().cancel_run(&run); let s=store.lock().unwrap();
        assert!(matches!(s.terminal_states.get("inj-cancel"),Some(ContextInjectionState::Cancelled))); assert!(!s.completed_delivery_batch_ids.contains("batch-inj-cancel"));
    }

    #[tokio::test]
    async fn scope_reset_after_signal_claim_never_commits_delivered(){
        let run=operation("run:scope"); let store=Arc::new(Mutex::new(ContextInjectionWorkflowControlStore::default()));
        store.lock().unwrap().begin_run(&conversation(),&run); store.lock().unwrap().admit(request("inj-scope",run.as_str()),11);
        let scope=Arc::new(AtomicU64::new(11)); let g=gate(store.clone(),scope.clone(),Arc::new(Mutex::new(HashSet::new())),11); let kop=KernelOperationId::from_string(run.as_str());
        g.take_context_signal(&kop,"workspace_read").await.unwrap().unwrap(); scope.store(12,Ordering::SeqCst);
        assert!(g.commit_context_signal(&kop,"inj-scope",json!({"kind":"nextInnerStep"})).await.unwrap_err().to_string().contains("stale"));
        let s=store.lock().unwrap(); assert!(s.claimed.is_empty()); assert!(!matches!(s.terminal_states.get("inj-scope"),Some(ContextInjectionState::Delivered{..})));
    }
}
