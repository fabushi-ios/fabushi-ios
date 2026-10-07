use async_trait::async_trait;
use mahayana_conversation::select_history_window;
use mahayana_conversation::{
    ConversationError, ConversationProvider, MAHAYANA_AI_PROVIDER_KEY,
    ResolveApprovalRequest, ResumeConversationOperationRequest, SendMessageRequest,
    SharedConversationEventSink, SuspendConversationOperationRequest,
};
use mahayana_core::{
    ApprovalDecision, ApprovalId, BuildProfile, Conversation, ConversationId, Message, MessageId,
    MessageRole, ModelTokenUsage, ModelTokenUsageSnapshot, OperationId, RuntimeActivityStatus,
    RuntimeEvent,
};
use mahayana_kernel::{
    ApprovalResolution, Capability, CapabilitySet, EngineBackend, ExecutionPolicy, KernelError,
    KernelEvent, KernelEventSink, OpenSessionRequest, OperationId as KernelOperationId,
    ResumeOperationRequest, RunRequest, RuntimeProfile, SessionId, SharedKernelEventSink,
    SuspendOperationRequest,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex as AsyncMutex, Notify};

// FeatureHost's explicit conversation.open contract requests 200 messages. The
// background search/index path asks for 2,000 and Runtime clamps that request to
// 500. Keep read acknowledgement tied to the explicit-open request so a
// background history scan can never clear a real unread assistant reply.
const OPEN_CONVERSATION_HISTORY_LIMIT: u32 = 200;

struct ConversationState {
    history: Vec<Message>,
    read_through_by_conversation: BTreeMap<String, usize>,
}

impl ConversationState {
    fn new(history: Vec<Message>) -> Self {
        let mut read_through_by_conversation = BTreeMap::new();
        for message in &history {
            *read_through_by_conversation
                .entry(message.conversation_id.as_str().to_string())
                .or_insert(0) += 1;
        }
        Self {
            history,
            read_through_by_conversation,
        }
    }

    fn unread_count(&self, conversation_id: &ConversationId) -> u32 {
        let read_through = self
            .read_through_by_conversation
            .get(conversation_id.as_str())
            .copied()
            .unwrap_or_default();
        self.history
            .iter()
            .filter(|message| &message.conversation_id == conversation_id)
            .skip(read_through)
            .filter(|message| message.role == MessageRole::Assistant)
            .count()
            .try_into()
            .unwrap_or(u32::MAX)
    }

    fn mark_read(&mut self, conversation_id: &ConversationId) {
        let visible_message_count = self
            .history
            .iter()
            .filter(|message| &message.conversation_id == conversation_id)
            .count();
        self.read_through_by_conversation
            .insert(conversation_id.as_str().to_string(), visible_message_count);
    }

    fn clear(&mut self) {
        self.history.clear();
        self.read_through_by_conversation.clear();
    }

    fn record_assistant_completion(&mut self, message: Message, hidden: bool) -> bool {
        if hidden {
            return false;
        }
        self.history.push(message);
        true
    }

    fn upsert_assistant_stream(&mut self, message: Message, hidden: bool) -> bool {
        if hidden {
            return false;
        }
        if let Some(existing) = self.history.iter_mut().find(|existing| {
            existing.conversation_id == message.conversation_id && existing.id == message.id
        }) {
            *existing = message;
        } else {
            self.history.push(message);
        }
        true
    }
}

fn history_request_marks_read(limit: u32) -> bool {
    limit == OPEN_CONVERSATION_HISTORY_LIMIT
}

#[derive(Debug, Default)]
struct DirectOperationGate {
    dispatched: bool,
    cancellation: Option<String>,
}

#[derive(Clone, Debug)]
struct DirectOperationState {
    operation_id: KernelOperationId,
    recovery_shaped: bool,
    gate: Arc<Mutex<DirectOperationGate>>,
    changed: Arc<Notify>,
}

impl DirectOperationState {
    fn new(operation_id: KernelOperationId, recovery_shaped: bool) -> Self {
        Self {
            operation_id,
            recovery_shaped,
            gate: Arc::new(Mutex::new(DirectOperationGate::default())),
            changed: Arc::new(Notify::new()),
        }
    }

    fn snapshot(&self) -> Result<(bool, Option<String>), ConversationError> {
        let gate = self.gate.lock().map_err(|_| {
            ConversationError::Provider("direct operation gate mutex poisoned".into())
        })?;
        Ok((gate.dispatched, gate.cancellation.clone()))
    }

    fn pre_dispatch_supersede_allowed(
        &self,
        carries_recovery: bool,
    ) -> Result<bool, ConversationError> {
        let gate = self.gate.lock().map_err(|_| {
            ConversationError::Provider("direct operation gate mutex poisoned".into())
        })?;
        Ok(!gate.dispatched
            && gate.cancellation.is_none()
            && carries_recovery
            && self.recovery_shaped)
    }

    fn cancel_before_dispatch(&self, reason: &str) -> Result<bool, ConversationError> {
        let mut gate = self.gate.lock().map_err(|_| {
            ConversationError::Provider("direct operation gate mutex poisoned".into())
        })?;
        if gate.dispatched {
            return Ok(false);
        }
        if gate.cancellation.is_none() {
            gate.cancellation = Some(reason.to_string());
        }
        drop(gate);
        self.changed.notify_waiters();
        Ok(true)
    }

    fn mark_dispatched_or_cancelled(&self) -> Result<Option<String>, ConversationError> {
        let mut gate = self.gate.lock().map_err(|_| {
            ConversationError::Provider("direct operation gate mutex poisoned".into())
        })?;
        if let Some(reason) = gate.cancellation.clone() {
            return Ok(Some(reason));
        }
        gate.dispatched = true;
        drop(gate);
        self.changed.notify_waiters();
        Ok(None)
    }

    fn finish(&self) {
        self.changed.notify_waiters();
    }
}

pub struct KernelConversationProvider {
    backend: Arc<dyn EngineBackend>,
    profile: BuildProfile,
    workspace_root: Option<String>,
    model: Option<String>,
    session_ids: AsyncMutex<BTreeMap<String, SessionId>>,
    state: Arc<Mutex<ConversationState>>,
    history_path: Option<PathBuf>,
    direct_operations: AsyncMutex<BTreeMap<String, DirectOperationState>>,
    interrupt_reasons: AsyncMutex<BTreeMap<String, String>>,
}

impl KernelConversationProvider {
    pub fn new(
        backend: Arc<dyn EngineBackend>,
        profile: BuildProfile,
        workspace_root: Option<String>,
        model: Option<String>,
        history_path: Option<PathBuf>,
    ) -> Self {
        let history = history_path
            .as_deref()
            .map(load_history)
            .unwrap_or_default();
        Self {
            backend,
            profile,
            workspace_root,
            model,
            session_ids: AsyncMutex::new(BTreeMap::new()),
            state: Arc::new(Mutex::new(ConversationState::new(history))),
            history_path,
            direct_operations: AsyncMutex::new(BTreeMap::new()),
            interrupt_reasons: AsyncMutex::new(BTreeMap::new()),
        }
    }

    async fn admit_direct_operation(
        &self,
        conversation_key: &str,
        current: DirectOperationState,
        carries_recovery: bool,
    ) -> Result<(), ConversationError> {
        const SUPERSEDE_REASON: &str = "superseded by a new user message";
        loop {
            let mut operations = self.direct_operations.lock().await;
            let Some(previous) = operations.get(conversation_key).cloned() else {
                operations.insert(conversation_key.to_string(), current.clone());
                return Ok(());
            };
            if previous.operation_id == current.operation_id {
                return Ok(());
            }

            let changed = Arc::clone(&previous.changed);
            let notified = changed.notified();
            let (dispatched, cancellation) = previous.snapshot()?;
            if cancellation.is_some() {
                drop(operations);
                notified.await;
                continue;
            }
            if !dispatched {
                if previous.pre_dispatch_supersede_allowed(carries_recovery)? {
                    if previous.cancel_before_dispatch(SUPERSEDE_REASON)? {
                        operations.insert(conversation_key.to_string(), current.clone());
                        return Ok(());
                    }
                    continue;
                }
                drop(operations);
                notified.await;
                continue;
            }

            self.interrupt_reasons.lock().await.insert(
                previous.operation_id.as_str().to_string(),
                SUPERSEDE_REASON.to_string(),
            );
            match self.backend.interrupt(&previous.operation_id).await {
                Ok(()) => {
                    operations.insert(conversation_key.to_string(), current.clone());
                    return Ok(());
                }
                Err(KernelError::OperationNotFound(_)) => {
                    self.interrupt_reasons
                        .lock()
                        .await
                        .remove(previous.operation_id.as_str());
                    operations.insert(conversation_key.to_string(), current.clone());
                    return Ok(());
                }
                Err(error) => {
                    self.interrupt_reasons
                        .lock()
                        .await
                        .remove(previous.operation_id.as_str());
                    return Err(kernel_error(error));
                }
            }
        }
    }

    async fn session_id(
        &self,
        conversation_id: &ConversationId,
    ) -> Result<SessionId, ConversationError> {
        let conversation_key = conversation_id.as_str().to_string();
        let mut session_ids = self.session_ids.lock().await;
        if let Some(session_id) = session_ids.get(&conversation_key) {
            return Ok(session_id.clone());
        }
        let history = self
            .state
            .lock()
            .map_err(|_| ConversationError::Provider("kernel conversation state mutex poisoned".into()))?
            .history
            .iter()
            .filter(|message| &message.conversation_id == conversation_id)
            .map(|message| json!({
                "id": message.id.as_str(),
                "role": match &message.role { MessageRole::Assistant => "assistant", _ => "user" },
                "content": message.text.as_str(),
                "createdAtMs": message.created_at_ms,
            }))
            .collect::<Vec<_>>();
        let transcript_updated_at_ms = history
            .last()
            .and_then(|message| message.get("createdAtMs"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let created = self
            .backend
            .open_session(OpenSessionRequest {
                profile: runtime_profile(self.profile),
                workspace_root: self.workspace_root.clone(),
                model: self.model.clone(),
                metadata: json!({
                    "conversationId": conversation_id.as_str(),
                    "bootstrapHistory": history,
                    "transcriptUpdatedAtMs": transcript_updated_at_ms,
                }),
            })
            .await
            .map_err(kernel_error)?;
        session_ids.insert(conversation_key, created.clone());
        Ok(created)
    }
}

#[async_trait]
impl ConversationProvider for KernelConversationProvider {
    fn key(&self) -> &'static str {
        MAHAYANA_AI_PROVIDER_KEY
    }

    async fn list_conversations(&self) -> Result<Vec<Conversation>, ConversationError> {
        let conversation_id =
            ConversationId(mahayana_core::MAHAYANA_AI_CONVERSATION_ID.to_string());
        let unread_count = self
            .state
            .lock()
            .map_err(|_| {
                ConversationError::Provider("kernel conversation state mutex poisoned".into())
            })?
            .unread_count(&conversation_id);
        let mut conversation = Conversation::mahayana_assistant();
        conversation.unread_count = unread_count;
        Ok(vec![conversation])
    }

    async fn history(
        &self,
        conversation_id: &ConversationId,
        limit: u32,
    ) -> Result<Vec<Message>, ConversationError> {
        let mut state = self.state.lock().map_err(|_| {
            ConversationError::Provider("kernel conversation state mutex poisoned".into())
        })?;
        let matching = state
            .history
            .iter()
            .filter(|message| &message.conversation_id == conversation_id)
            .cloned()
            .collect::<Vec<_>>();
        let start = matching.len().saturating_sub(limit as usize);
        let messages = matching[start..].to_vec();
        if history_request_marks_read(limit) {
            state.mark_read(conversation_id);
        }
        Ok(messages)
    }

    async fn history_window(
        &self,
        conversation_id: &ConversationId,
        before_message_id: Option<&str>,
        after_message_id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<Message>, ConversationError> {
        let state = self.state.lock().map_err(|_| {
            ConversationError::Provider("kernel conversation state mutex poisoned".into())
        })?;
        let matching = state
            .history
            .iter()
            .filter(|message| &message.conversation_id == conversation_id)
            .cloned()
            .collect::<Vec<_>>();
        select_history_window(&matching, before_message_id, after_message_id, limit)
    }

    async fn replace_message(
        &self,
        conversation_id: &ConversationId,
        message: Message,
    ) -> Result<bool, ConversationError> {
        if &message.conversation_id != conversation_id {
            return Ok(false);
        }
        let mut state = self.state.lock().map_err(|_| {
            ConversationError::Provider("kernel conversation state mutex poisoned".into())
        })?;
        let Some(index) = state.history.iter().position(|existing| {
            &existing.conversation_id == conversation_id && existing.id == message.id
        }) else {
            return Ok(false);
        };
        state.history[index] = message;
        persist_history(&self.state, self.history_path.as_deref()).map_err(kernel_error)?;
        Ok(true)
    }

    async fn warmup(&self, conversation_id: &ConversationId) -> Result<(), ConversationError> {
        self.session_id(conversation_id).await.map(|_| ())
    }

    async fn send_message(
        &self,
        request: SendMessageRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError> {
        let session_id = self.session_id(&request.conversation_id).await?;
        let visible_text = visible_user_text(&request);
        let user_message = Message {
            id: request
                .client_message_id
                .as_deref()
                .and_then(|id| MessageId::new(id).ok())
                .unwrap_or_else(|| MessageId::generated("message")),
            conversation_id: request.conversation_id.clone(),
            role: MessageRole::User,
            text: visible_text,
            created_at_ms: now_ms(),
            metadata: json!({"runtime": "mahayana-kernel"}),
        };
        if !request.hidden {
            self.state
                .lock()
                .map_err(|_| {
                    ConversationError::Provider("kernel conversation state mutex poisoned".into())
                })?
                .history
                .push(user_message);
            persist_history(&self.state, self.history_path.as_deref()).map_err(kernel_error)?;
        }

        let kernel_operation_id = KernelOperationId::from_string(request.operation_id.as_str());
        let conversation_key = request.conversation_id.as_str().to_string();
        let carries_recovery = !request.hidden
            && request
                .client_message_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty());
        let recovery_shaped = carries_recovery
            && request.recovery_eligible
            && request.selected_image_data_urls.is_empty();
        let direct_operation =
            DirectOperationState::new(kernel_operation_id.clone(), recovery_shaped);
        if !request.hidden {
            self.admit_direct_operation(
                &conversation_key,
                direct_operation.clone(),
                carries_recovery,
            )
            .await?;
            if let Some(reason) = direct_operation.mark_dispatched_or_cancelled()? {
                direct_operation.finish();
                clear_current_direct_operation(
                    &self.direct_operations,
                    &conversation_key,
                    &kernel_operation_id,
                )
                .await;
                return Err(ConversationError::Interrupted(reason));
            }
        }

        let turn_attachment_batch_id = request
            .attachment_batch_id
            .clone()
            .unwrap_or_else(|| format!("attachment-batch:{}", request.operation_id.as_str()));
        let suspended = Arc::new(AtomicBool::new(false));
        let bridge = Arc::new(RuntimeKernelEventBridge {
            conversation_id: request.conversation_id,
            operation_id: request.operation_id,
            events,
            state: Arc::clone(&self.state),
            history_path: self.history_path.clone(),
            hide_assistant_history: request.hidden,
            suppress_assistant_events: request.hidden && !request.show_assistant_output,
            reply_to_message_id: request.reply_to_message_id.clone(),
            is_fork: request.is_fork,
            attachment_batch_id: Some(turn_attachment_batch_id),
            streaming_assistant: Mutex::new(None),
            suspended: Arc::clone(&suspended),
        });
        let sink: SharedKernelEventSink = bridge;
        let result = self
            .backend
            .run(
                RunRequest {
                    session_id,
                    operation_id: kernel_operation_id.clone(),
                    input: request.text,
                    policy: execution_policy(self.profile),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({
                        "clientMessageId": request.client_message_id,
                        "replyToMessageId": request.reply_to_message_id,
                        "isFork": request.is_fork,
                        "attachmentBatchId": request.attachment_batch_id,
                        "selectedImageDataUrls": request.selected_image_data_urls,
                        "hidden": request.hidden,
                        "showAssistantOutput": request.show_assistant_output,
                    }),
                },
                sink,
            )
            .await;
        if !request.hidden {
            direct_operation.finish();
            clear_current_direct_operation(
                &self.direct_operations,
                &conversation_key,
                &kernel_operation_id,
            )
            .await;
        }
        let interrupt_reason = self
            .interrupt_reasons
            .lock()
            .await
            .remove(kernel_operation_id.as_str());
        match result {
            Err(KernelError::Backend(message)) if message == "operation interrupted" => {
                Err(ConversationError::Interrupted(
                    interrupt_reason.unwrap_or_else(|| "operation interrupted".to_string()),
                ))
            }
            Err(error) => Err(kernel_error(error)),
            Ok(()) if suspended.load(Ordering::SeqCst) => Err(ConversationError::Suspended),
            Ok(()) => Ok(()),
        }
    }

    async fn interrupt(&self, operation_id: &OperationId) -> Result<(), ConversationError> {
        let kernel_operation_id = KernelOperationId::from_string(operation_id.as_str());
        self.interrupt_reasons.lock().await.insert(
            operation_id.as_str().to_string(),
            "interrupted by user".to_string(),
        );
        match self.backend.interrupt(&kernel_operation_id).await {
            Ok(()) => Ok(()),
            Err(error) => {
                self.interrupt_reasons
                    .lock()
                    .await
                    .remove(operation_id.as_str());
                Err(kernel_error(error))
            }
        }
    }

    async fn suspend_operation(
        &self,
        request: SuspendConversationOperationRequest,
    ) -> Result<(), ConversationError> {
        self.backend
            .suspend_operation(SuspendOperationRequest {
                operation_id: KernelOperationId::from_string(request.operation_id.as_str()),
                reason: request.reason,
                metadata: json!({"cascade": request.cascade}),
            })
            .await
            .map_err(kernel_error)
    }

    async fn resume_operation(
        &self,
        request: ResumeConversationOperationRequest,
        events: SharedConversationEventSink,
    ) -> Result<(), ConversationError> {
        let session_id = self.session_id(&request.conversation_id).await?;
        let kernel_operation_id = KernelOperationId::from_string(request.operation_id.as_str());
        let suspended = Arc::new(AtomicBool::new(false));
        let bridge = Arc::new(RuntimeKernelEventBridge {
            conversation_id: request.conversation_id,
            operation_id: request.operation_id,
            events,
            state: Arc::clone(&self.state),
            history_path: self.history_path.clone(),
            hide_assistant_history: request.hidden,
            suppress_assistant_events: request.hidden && !request.show_assistant_output,
            reply_to_message_id: request.reply_to_message_id,
            is_fork: request.is_fork,
            attachment_batch_id: request.attachment_batch_id,
            streaming_assistant: Mutex::new(None),
            suspended: Arc::clone(&suspended),
        });
        let sink: SharedKernelEventSink = bridge;
        let result = self
            .backend
            .resume_operation(
                ResumeOperationRequest {
                    session_id,
                    operation_id: kernel_operation_id,
                    policy: execution_policy(self.profile),
                    required_capabilities: CapabilitySet::new([Capability::Model]),
                    metadata: json!({"resumedBy": "conversation-provider"}),
                },
                sink,
            )
            .await;
        match result {
            Err(error) => Err(kernel_error(error)),
            Ok(()) if suspended.load(Ordering::SeqCst) => Err(ConversationError::Suspended),
            Ok(()) => Ok(()),
        }
    }

    async fn reset_session(&self) -> Result<(), ConversationError> {
        self.backend.reset_session().map_err(kernel_error)?;
        self.session_ids.lock().await.clear();
        self.direct_operations.lock().await.clear();
        self.interrupt_reasons.lock().await.clear();
        {
            let mut state = self.state.lock().map_err(|_| {
                ConversationError::Provider("kernel conversation state mutex poisoned".into())
            })?;
            state.clear();
        }
        persist_history(&self.state, self.history_path.as_deref()).map_err(kernel_error)
    }

    async fn resolve_approval(
        &self,
        request: ResolveApprovalRequest,
    ) -> Result<(), ConversationError> {
        self.backend
            .resolve_approval(ApprovalResolution {
                approval_id: request.approval_id.to_string(),
                approved: matches!(
                    request.decision,
                    ApprovalDecision::Accept | ApprovalDecision::AcceptForSession
                ),
                metadata: request.payload,
            })
            .await
            .map_err(kernel_error)
    }
}

fn visible_user_text(request: &SendMessageRequest) -> String {
    request
        .display_text
        .clone()
        .unwrap_or_else(|| request.text.clone())
}

async fn clear_current_direct_operation(
    operations: &AsyncMutex<BTreeMap<String, DirectOperationState>>,
    conversation_key: &str,
    operation_id: &KernelOperationId,
) {
    let mut operations = operations.lock().await;
    if operations
        .get(conversation_key)
        .is_some_and(|current| &current.operation_id == operation_id)
    {
        operations.remove(conversation_key);
    }
}

struct RuntimeKernelEventBridge {
    conversation_id: ConversationId,
    operation_id: OperationId,
    events: SharedConversationEventSink,
    state: Arc<Mutex<ConversationState>>,
    history_path: Option<PathBuf>,
    hide_assistant_history: bool,
    suppress_assistant_events: bool,
    reply_to_message_id: Option<String>,
    is_fork: bool,
    attachment_batch_id: Option<String>,
    streaming_assistant: Mutex<Option<Message>>,
    suspended: Arc<AtomicBool>,
}

impl RuntimeKernelEventBridge {
    fn emit_runtime(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        self.events
            .emit(event)
            .map_err(|error| KernelError::Backend(error.to_string()))
    }

    fn provider_stream_message(&self, text: String, state: &ConversationState) -> Message {
        let reply_to = self
            .reply_to_message_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .filter(|candidate| {
                state.history.iter().any(|message| {
                    message.conversation_id == self.conversation_id
                        && message.id.as_str() == *candidate
                })
            });
        let mut metadata = json!({
            "runtime": "mahayana-kernel",
            "providerStream": true,
        });
        if let Some(object) = metadata.as_object_mut()
            && let Some(reply_to) = reply_to
        {
            object.insert(
                "replyToMessageId".into(),
                Value::String(reply_to.to_string()),
            );
            if self.is_fork {
                object.insert("branched".into(), Value::Bool(true));
            }
        }
        Message {
            id: MessageId::generated("provider-stream"),
            conversation_id: self.conversation_id.clone(),
            role: MessageRole::Assistant,
            text,
            created_at_ms: now_ms(),
            metadata,
        }
    }

    fn activity(
        &self,
        step_id: String,
        kind: String,
        title: String,
        detail: Option<String>,
        status: RuntimeActivityStatus,
        metadata: Option<Value>,
    ) -> Result<(), KernelError> {
        self.emit_runtime(RuntimeEvent::AgentActivity {
            operation_id: self.operation_id.clone(),
            step_id,
            kind,
            title,
            detail,
            status,
            metadata,
        })
    }
}

impl KernelEventSink for RuntimeKernelEventBridge {
    fn emit(&self, event: KernelEvent) -> Result<(), KernelError> {
        match event {
            KernelEvent::MessageDelta { delta, .. } => {
                if self.suppress_assistant_events {
                    return Ok(());
                }
                let mut stream = self.streaming_assistant.lock().map_err(|_| {
                    KernelError::Backend("provider stream mutex poisoned".into())
                })?;
                let mut state = self.state.lock().map_err(|_| {
                    KernelError::Backend("kernel conversation state mutex poisoned".into())
                })?;
                let message = stream
                    .get_or_insert_with(|| self.provider_stream_message(String::new(), &state));
                message.text.push_str(&delta);
                let should_persist =
                    state.upsert_assistant_stream(message.clone(), self.hide_assistant_history);
                drop(state);
                drop(stream);
                if should_persist {
                    persist_history(&self.state, self.history_path.as_deref())?;
                }
                self.emit_runtime(RuntimeEvent::MessageDelta {
                    operation_id: self.operation_id.clone(),
                    conversation_id: self.conversation_id.clone(),
                    delta,
                })
            }
            KernelEvent::MessageCompleted { text, .. } => {
                let mut stream = self.streaming_assistant.lock().map_err(|_| {
                    KernelError::Backend("provider stream mutex poisoned".into())
                })?;
                let mut state = self.state.lock().map_err(|_| {
                    KernelError::Backend("kernel conversation state mutex poisoned".into())
                })?;
                let message = if let Some(mut message) = stream.take() {
                    message.text = text;
                    message
                } else {
                    self.provider_stream_message(text, &state)
                };
                let should_persist =
                    state.upsert_assistant_stream(message.clone(), self.hide_assistant_history);
                drop(state);
                drop(stream);
                if should_persist {
                    persist_history(&self.state, self.history_path.as_deref())?;
                }
                if self.suppress_assistant_events {
                    return Ok(());
                }
                self.emit_runtime(RuntimeEvent::MessageCompleted {
                    operation_id: self.operation_id.clone(),
                    message,
                })
            }
            KernelEvent::UsageUpdated {
                total_tokens,
                input_tokens,
                cached_input_tokens,
                output_tokens,
                reasoning_output_tokens,
                ..
            } => self.emit_runtime(RuntimeEvent::ModelUsageUpdated {
                operation_id: self.operation_id.clone(),
                usage: ModelTokenUsageSnapshot {
                    total: None,
                    last: ModelTokenUsage {
                        total_tokens: i64::try_from(total_tokens).unwrap_or(i64::MAX),
                        input_tokens: i64::try_from(input_tokens).unwrap_or(i64::MAX),
                        cached_input_tokens: i64::try_from(cached_input_tokens).unwrap_or(i64::MAX),
                        output_tokens: i64::try_from(output_tokens).unwrap_or(i64::MAX),
                        reasoning_output_tokens: i64::try_from(reasoning_output_tokens)
                            .unwrap_or(i64::MAX),
                    },
                    model_context_window: None,
                },
            }),
            KernelEvent::Activity {
                kind,
                title,
                detail,
                metadata,
                ..
            } => {
                if kind == "operation_suspended" {
                    self.suspended.store(true, Ordering::SeqCst);
                }
                let step_id = metadata
                    .get("stepId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("kernel-step:{}", self.operation_id));
                let status = metadata
                    .get("status")
                    .and_then(Value::as_str)
                    .map(runtime_activity_status)
                    .unwrap_or(RuntimeActivityStatus::Running);
                self.activity(step_id, kind, title, detail, status, Some(metadata))
            }
            KernelEvent::ToolStarted {
                tool_call_id,
                tool,
                arguments,
                ..
            } => self.activity(
                format!("tool:{tool_call_id}"),
                "tool".into(),
                format!("Running {tool}"),
                None,
                RuntimeActivityStatus::Running,
                Some(json!({
                    "tool": tool,
                    "toolCallId": tool_call_id,
                    "arguments": arguments
                })),
            ),
            KernelEvent::ToolCompleted {
                tool_call_id,
                tool,
                output,
                success,
                ..
            } => {
                if tool == "send_message" && success {
                    let mut state = self.state.lock().map_err(|_| {
                        KernelError::Backend("kernel conversation state mutex poisoned".into())
                    })?;
                    let message = generated_send_message(
                        &self.conversation_id,
                        &output,
                        &state.history,
                        self.reply_to_message_id.as_deref(),
                        self.is_fork,
                        self.attachment_batch_id.as_deref(),
                    )
                    .ok_or_else(|| {
                        KernelError::Backend(
                            "send_message tool completed without canonical generated payload"
                                .into(),
                        )
                    })?;
                    let should_persist = state
                        .record_assistant_completion(message.clone(), self.hide_assistant_history);
                    drop(state);
                    if should_persist {
                        persist_history(&self.state, self.history_path.as_deref())?;
                    }
                    self.emit_runtime(RuntimeEvent::MessageCompleted {
                        operation_id: self.operation_id.clone(),
                        message,
                    })?;
                }
                self.activity(
                    format!("tool:{tool_call_id}"),
                    "tool".into(),
                    format!("Completed {tool}"),
                    None,
                    if success {
                        RuntimeActivityStatus::Completed
                    } else {
                        RuntimeActivityStatus::Failed
                    },
                    Some(json!({
                        "tool": tool,
                        "toolCallId": tool_call_id,
                        "output": output,
                        "success": success
                    })),
                )
            }
            KernelEvent::ApprovalRequested {
                approval_id,
                title,
                risk,
                details,
                ..
            } => {
                let approval_id = ApprovalId::new(approval_id)
                    .map_err(|error| KernelError::Backend(error.to_string()))?;
                self.emit_runtime(RuntimeEvent::ApprovalRequested {
                    operation_id: self.operation_id.clone(),
                    approval_id,
                    title,
                    details: json!({"risk": risk, "details": details}),
                })
            }
            KernelEvent::CheckpointCreated {
                checkpoint_id,
                label,
                ..
            } => self.activity(
                format!("checkpoint:{checkpoint_id}"),
                "checkpoint".into(),
                label.unwrap_or_else(|| "Workspace checkpoint".into()),
                Some(checkpoint_id.clone()),
                RuntimeActivityStatus::Completed,
                Some(json!({"checkpointId": checkpoint_id})),
            ),
            KernelEvent::OperationCompleted { .. } => Ok(()),
            KernelEvent::OperationFailed {
                message, retryable, ..
            } => self.activity(
                format!("operation:{}", self.operation_id),
                "operation".into(),
                "Operation failed".into(),
                Some(message),
                RuntimeActivityStatus::Failed,
                Some(json!({"retryable": retryable})),
            ),
        }
    }
}

fn runtime_profile(profile: BuildProfile) -> RuntimeProfile {
    match profile {
        BuildProfile::DesktopFull => RuntimeProfile::DesktopFull,
        BuildProfile::MobileEmbedded => RuntimeProfile::MobileEmbedded,
        BuildProfile::WebWasm => RuntimeProfile::WebWasm,
    }
}

fn execution_policy(profile: BuildProfile) -> ExecutionPolicy {
    match profile {
        BuildProfile::DesktopFull => ExecutionPolicy::interactive_default(),
        BuildProfile::MobileEmbedded | BuildProfile::WebWasm => ExecutionPolicy::mobile_default(),
    }
}

fn generated_send_message(
    conversation_id: &ConversationId,
    output: &Value,
    history: &[Message],
    reply_thread_target: Option<&str>,
    is_fork: bool,
    attachment_batch_id: Option<&str>,
) -> Option<Message> {
    let text = output
        .get("generatedMessage")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let attachment = output
        .get("generatedAttachment")
        .filter(|value| value.is_object())
        .cloned();
    if text.is_empty() && attachment.is_none() {
        return None;
    }
    let tool_call_id = output
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())?;
    let message_id = MessageId::generated("generated-send");
    let live_target = |candidate: &str| {
        history.iter().any(|message| {
            &message.conversation_id == conversation_id && message.id.as_str() == candidate
        })
    };
    let synthetic_reply_nudge = output
        .get("syntheticReplyNudge")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let explicit_reply = output
        .get("replyToMessageId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| *value != message_id.as_str() && live_target(value));
    let inherited_reply_thread_target = if synthetic_reply_nudge {
        None
    } else {
        reply_thread_target
    };
    let reply_to = explicit_reply.or_else(|| {
        inherited_reply_thread_target
            .map(str::trim)
            .filter(|value| !value.is_empty() && live_target(value))
    });
    let branched =
        !synthetic_reply_nudge && is_fork && inherited_reply_thread_target.is_some_and(live_target);
    let mut metadata = json!({
        "runtime": "mahayana-kernel",
        "generatedSend": true,
        "toolCallId": tool_call_id,
    });
    if let Some(object) = metadata.as_object_mut() {
        if let Some(reply_to) = reply_to {
            object.insert(
                "replyToMessageId".into(),
                Value::String(reply_to.to_string()),
            );
        }
        if branched {
            object.insert("branched".into(), Value::Bool(true));
        }
        if let Some(attachment) = attachment {
            object.insert("generatedAttachment".into(), attachment);
            if !synthetic_reply_nudge
                && let Some(batch_id) = attachment_batch_id
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            {
                object.insert(
                    "attachmentBatchId".into(),
                    Value::String(batch_id.to_string()),
                );
            }
        }
    }
    Some(Message {
        id: message_id,
        conversation_id: conversation_id.clone(),
        role: MessageRole::Assistant,
        text: text.to_string(),
        created_at_ms: now_ms(),
        metadata,
    })
}

fn runtime_activity_status(value: &str) -> RuntimeActivityStatus {
    match value {
        "completed" => RuntimeActivityStatus::Completed,
        "failed" => RuntimeActivityStatus::Failed,
        _ => RuntimeActivityStatus::Running,
    }
}

fn kernel_error(error: KernelError) -> ConversationError {
    match error {
        KernelError::OperationNotFound(id) => ConversationError::OperationNotFound(
            OperationId::new(id).unwrap_or_else(|_| OperationId::generated("operation")),
        ),
        KernelError::ApprovalNotFound(id) => ConversationError::ApprovalNotFound(
            ApprovalId::new(id).unwrap_or_else(|_| ApprovalId::generated("approval")),
        ),
        other => ConversationError::Provider(other.to_string()),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn load_history(path: &Path) -> Vec<Message> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    serde_json::from_slice::<Vec<Message>>(&bytes).unwrap_or_default()
}

fn persist_history(
    state: &Arc<Mutex<ConversationState>>,
    path: Option<&Path>,
) -> Result<(), KernelError> {
    let Some(path) = path else {
        return Ok(());
    };
    let bytes = {
        let state = state
            .lock()
            .map_err(|_| KernelError::Backend("kernel conversation state mutex poisoned".into()))?;
        let start = state.history.len().saturating_sub(1_000);
        serde_json::to_vec(&state.history[start..])
            .map_err(|error| KernelError::Backend(error.to_string()))?
    };
    let parent = path
        .parent()
        .ok_or_else(|| KernelError::Backend("kernel history path has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|error| KernelError::Backend(error.to_string()))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temporary = path.with_extension(format!("json.{}.{nonce}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| KernelError::Backend(error.to_string()))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| KernelError::Backend(error.to_string()))?;
    replace_file(&temporary, path)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(id: &str) -> ConversationId {
        ConversationId(id.to_string())
    }

    fn message(conversation_id: &ConversationId, role: MessageRole, text: &str) -> Message {
        Message {
            id: MessageId::generated("test-message"),
            conversation_id: conversation_id.clone(),
            role,
            text: text.to_string(),
            created_at_ms: 1,
            metadata: Value::Null,
        }
    }

    #[derive(Default)]
    struct CapturedRuntimeEvents(Mutex<Vec<RuntimeEvent>>);

    impl mahayana_conversation::ConversationEventSink for CapturedRuntimeEvents {
        fn emit(&self, event: RuntimeEvent) -> Result<(), ConversationError> {
            self.0
                .lock()
                .map_err(|_| ConversationError::Provider("event capture poisoned".into()))?
                .push(event);
            Ok(())
        }
    }

    #[test]
    fn hidden_background_can_project_completion_without_persisting_chat_history() {
        let conversation_id = conversation("mahayana-ai:agent:background");
        let state = Arc::new(Mutex::new(ConversationState::default()));
        let events = Arc::new(CapturedRuntimeEvents::default());
        let bridge = RuntimeKernelEventBridge {
            conversation_id: conversation_id.clone(),
            operation_id: OperationId::generated("background-operation"),
            events: events.clone(),
            state: state.clone(),
            history_path: None,
            hide_assistant_history: true,
            suppress_assistant_events: false,
            reply_to_message_id: None,
            is_fork: false,
            attachment_batch_id: None,
            streaming_assistant: Mutex::new(None),
            suspended: Arc::new(AtomicBool::new(false)),
        };
        bridge.emit(KernelEvent::MessageCompleted {
            operation_id: KernelOperationId::from_string("background-operation"),
            text: "durable background result".into(),
        }).expect("project hidden completion");

        assert!(
            state.lock().expect("state").history.iter().all(|message| message.role != MessageRole::Assistant),
            "hidden background completion must not enter visible conversation history"
        );
        assert!(events.0.lock().expect("events").iter().any(|event| matches!(
            event,
            RuntimeEvent::MessageCompleted { message, .. }
                if message.text == "durable background result"
                    && message.metadata.get("providerStream").and_then(Value::as_bool) == Some(true)
        )));
    }

    #[test]
    fn provider_stream_reuses_one_canonical_message_and_preserves_reply_fork_identity() {
        let conversation_id = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let reply = message(&conversation_id, MessageRole::User, "reply target");
        let reply_id = reply.id.as_str().to_string();
        let state = Arc::new(Mutex::new(ConversationState::new(vec![reply])));
        let events = Arc::new(CapturedRuntimeEvents::default());
        let bridge = RuntimeKernelEventBridge {
            conversation_id: conversation_id.clone(),
            operation_id: OperationId::generated("operation"),
            events: events.clone(),
            state: state.clone(),
            history_path: None,
            hide_assistant_history: false,
            suppress_assistant_events: false,
            reply_to_message_id: Some(reply_id.clone()),
            is_fork: true,
            attachment_batch_id: None,
            streaming_assistant: Mutex::new(None),
            suspended: Arc::new(AtomicBool::new(false)),
        };
        let kernel_operation_id = KernelOperationId::from_string("kernel-fast-lane");

        bridge
            .emit(KernelEvent::MessageDelta {
                operation_id: kernel_operation_id.clone(),
                delta: "般若".into(),
            })
            .expect("first provider delta");
        bridge
            .emit(KernelEvent::MessageDelta {
                operation_id: kernel_operation_id.clone(),
                delta: "波罗蜜".into(),
            })
            .expect("second provider delta");
        bridge
            .emit(KernelEvent::MessageCompleted {
                operation_id: kernel_operation_id,
                text: "般若波罗蜜".into(),
            })
            .expect("provider stream completion");

        let state = state.lock().expect("state");
        let assistant = state
            .history
            .iter()
            .filter(|message| message.role == MessageRole::Assistant)
            .collect::<Vec<_>>();
        assert_eq!(assistant.len(), 1);
        assert_eq!(assistant[0].text, "般若波罗蜜");
        assert_eq!(assistant[0].metadata["providerStream"], true);
        assert_eq!(assistant[0].metadata["replyToMessageId"], reply_id);
        assert_eq!(assistant[0].metadata["branched"], true);
        let canonical_id = assistant[0].id.clone();
        drop(state);

        let events = events.0.lock().expect("events");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::MessageDelta { .. }))
                .count(),
            2
        );
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::MessageCompleted { message, .. }
                if message.id == canonical_id && message.text == "般若波罗蜜"
        )));
    }

    #[test]
    fn generated_send_tool_output_becomes_canonical_transcript_message() {
        let conversation_id = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let history = vec![message(&conversation_id, MessageRole::User, "reply target")];
        let reply_target = history[0].id.as_str().to_string();
        let message = generated_send_message(
            &conversation_id,
            &json!({
                "generatedMessage": "milestone complete",
                "toolCallId": "call-17",
                "replyToMessageId": reply_target,
            }),
            &history,
            None,
            false,
            None,
        )
        .expect("generated send message");

        assert_eq!(message.role, MessageRole::Assistant);
        assert_eq!(message.text, "milestone complete");
        assert_eq!(message.conversation_id, conversation_id);
        assert_eq!(message.metadata["generatedSend"], true);
        assert_eq!(message.metadata["toolCallId"], "call-17");
        assert!(message.id.as_str().starts_with("generated-send"));
    }

    #[test]
    fn generated_send_validates_reply_then_falls_back_to_live_thread_and_stamps_fork_batch() {
        let conversation_id = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let other_conversation = conversation("mahayana-ai:agent:other");
        let live = Message {
            id: MessageId("live-reply".into()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::User,
            text: "live".into(),
            created_at_ms: 1,
            metadata: Value::Null,
        };
        let wrong_conversation = Message {
            id: MessageId("wrong-conversation".into()),
            conversation_id: other_conversation,
            role: MessageRole::User,
            text: "wrong".into(),
            created_at_ms: 1,
            metadata: Value::Null,
        };
        let history = vec![live, wrong_conversation];
        let generated = generated_send_message(
            &conversation_id,
            &json!({
                "generatedMessage":"reply",
                "generatedAttachment": {
                    "url": "file:///tmp/report.pdf",
                    "file_name": "report.pdf",
                    "alt": "report"
                },
                "toolCallId":"call-thread",
                "replyToMessageId":"stale-id"
            }),
            &history,
            Some("live-reply"),
            true,
            Some("batch-7"),
        )
        .expect("generated send");
        assert_eq!(generated.metadata["replyToMessageId"], "live-reply");
        assert_eq!(generated.metadata["branched"], true);
        assert_eq!(generated.metadata["attachmentBatchId"], "batch-7");

        let stale = generated_send_message(
            &conversation_id,
            &json!({
                "generatedMessage":"no thread",
                "toolCallId":"call-stale",
                "replyToMessageId":"wrong-conversation"
            }),
            &history,
            Some("also-stale"),
            true,
            None,
        )
        .expect("generated stale send");
        assert!(stale.metadata.get("replyToMessageId").is_none());
        assert!(stale.metadata.get("branched").is_none());
        assert!(stale.metadata.get("attachmentBatchId").is_none());

        let attachment_only = generated_send_message(
            &conversation_id,
            &json!({
                "generatedAttachment": {
                    "url": "file:///tmp/image.png",
                    "file_name": "image.png"
                },
                "toolCallId":"call-attachment"
            }),
            &history,
            Some("live-reply"),
            false,
            Some("batch-attachment"),
        )
        .expect("attachment-only generated send");
        assert!(attachment_only.text.is_empty());
        assert_eq!(
            attachment_only.metadata["attachmentBatchId"],
            "batch-attachment"
        );
        assert_eq!(
            attachment_only.metadata["generatedAttachment"]["file_name"],
            "image.png"
        );
    }

    #[test]
    fn reply_nudge_send_drops_inherited_reply_fork_and_attachment_identity() {
        let conversation_id = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let history = vec![Message {
            id: MessageId("live-reply".into()),
            conversation_id: conversation_id.clone(),
            role: MessageRole::User,
            text: "original".into(),
            created_at_ms: 1,
            metadata: Value::Null,
        }];
        let generated = generated_send_message(
            &conversation_id,
            &json!({
                "generatedMessage": "delivered after nudge",
                "generatedAttachment": {
                    "url": "file:///tmp/result.pdf",
                    "file_name": "result.pdf"
                },
                "toolCallId": "call-nudge",
                "syntheticReplyNudge": true
            }),
            &history,
            Some("live-reply"),
            true,
            Some("original-attachment-batch"),
        )
        .expect("reply nudge generated send");

        assert!(generated.metadata.get("replyToMessageId").is_none());
        assert!(generated.metadata.get("branched").is_none());
        assert!(generated.metadata.get("attachmentBatchId").is_none());
        assert_eq!(
            generated.metadata["generatedAttachment"]["file_name"],
            "result.pdf"
        );

        let explicit_reply = generated_send_message(
            &conversation_id,
            &json!({
                "generatedMessage": "explicit reply after nudge",
                "toolCallId": "call-nudge-explicit",
                "replyToMessageId": "live-reply",
                "syntheticReplyNudge": true
            }),
            &history,
            Some("ignored-inherited-target"),
            true,
            Some("ignored-batch"),
        )
        .expect("explicit reply nudge send");
        assert_eq!(explicit_reply.metadata["replyToMessageId"], "live-reply");
        assert!(explicit_reply.metadata.get("branched").is_none());
    }

    #[test]
    fn unread_counts_only_assistant_messages_after_per_conversation_read_boundary() {
        let assistant = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let research = conversation("codex:agent:research");
        let mut state = ConversationState::new(Vec::new());

        state
            .history
            .push(message(&assistant, MessageRole::User, "hello"));
        state
            .history
            .push(message(&assistant, MessageRole::Assistant, "reply one"));
        state
            .history
            .push(message(&research, MessageRole::Assistant, "research reply"));
        state
            .history
            .push(message(&assistant, MessageRole::Assistant, "reply two"));

        assert_eq!(state.unread_count(&assistant), 2);
        assert_eq!(state.unread_count(&research), 1);

        state.mark_read(&research);
        assert_eq!(state.unread_count(&assistant), 2);
        assert_eq!(state.unread_count(&research), 0);

        state.mark_read(&assistant);
        assert_eq!(state.unread_count(&assistant), 0);
    }

    #[test]
    fn persisted_history_starts_read_for_every_existing_conversation() {
        let assistant = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let research = conversation("codex:agent:research");
        let mut state = ConversationState::new(vec![
            message(&assistant, MessageRole::Assistant, "persisted assistant"),
            message(&research, MessageRole::Assistant, "persisted research"),
        ]);
        assert_eq!(state.unread_count(&assistant), 0);
        assert_eq!(state.unread_count(&research), 0);

        state
            .history
            .push(message(&assistant, MessageRole::User, "new prompt"));
        state
            .history
            .push(message(&assistant, MessageRole::Assistant, "fresh reply"));
        assert_eq!(state.unread_count(&assistant), 1);
        assert_eq!(state.unread_count(&research), 0);
    }

    #[test]
    fn hidden_assistant_completion_stays_out_of_visible_history_and_unread() {
        let assistant = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let mut state = ConversationState::new(Vec::new());

        assert!(!state.record_assistant_completion(
            message(&assistant, MessageRole::Assistant, "hidden reply"),
            true,
        ));
        assert!(state.history.is_empty());
        assert_eq!(state.unread_count(&assistant), 0);

        assert!(state.record_assistant_completion(
            message(&assistant, MessageRole::Assistant, "visible reply"),
            false,
        ));
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.unread_count(&assistant), 1);
    }

    #[test]
    fn pre_dispatch_supersession_requires_recovery_on_both_turns() {
        let ordinary = DirectOperationState::new(KernelOperationId::from_string("ordinary"), false);
        assert!(
            !ordinary
                .pre_dispatch_supersede_allowed(false)
                .expect("ordinary gate")
        );
        assert!(
            !ordinary
                .pre_dispatch_supersede_allowed(true)
                .expect("ordinary gate with incoming recovery")
        );

        let recovery = DirectOperationState::new(KernelOperationId::from_string("recovery"), true);
        assert!(
            !recovery
                .pre_dispatch_supersede_allowed(false)
                .expect("recovery gate without incoming recovery")
        );
        assert!(
            recovery
                .pre_dispatch_supersede_allowed(true)
                .expect("recovery gate")
        );
        assert!(
            recovery
                .cancel_before_dispatch("superseded by a new user message")
                .expect("cancel recovery before dispatch")
        );
        assert_eq!(
            recovery
                .mark_dispatched_or_cancelled()
                .expect("observe pre-dispatch cancellation")
                .as_deref(),
            Some("superseded by a new user message")
        );
    }

    #[test]
    fn dispatched_direct_operation_leaves_pre_dispatch_fence() {
        let recovery =
            DirectOperationState::new(KernelOperationId::from_string("dispatched"), true);
        assert_eq!(
            recovery
                .mark_dispatched_or_cancelled()
                .expect("mark dispatched"),
            None
        );
        assert!(
            !recovery
                .pre_dispatch_supersede_allowed(true)
                .expect("dispatched gate")
        );
        assert!(
            !recovery
                .cancel_before_dispatch("too late")
                .expect("dispatch fence blocks local cancellation")
        );
    }

    #[test]
    fn visible_user_text_is_separate_from_expanded_provider_input() {
        let request = SendMessageRequest {
            conversation_id: ConversationId("mahayana-ai:agent:workflow".into()),
            operation_id: OperationId("operation-workflow".into()),
            text: "[Persistent agent memory]\nexpanded provider input".into(),
            display_text: Some("visible user turn".into()),
            client_message_id: Some("visible-message-1".into()),
            hidden: false,
            show_assistant_output: false,
            recovery_eligible: true,
            reply_to_message_id: None,
            is_fork: false,
            attachment_batch_id: None,
            selected_image_data_urls: Vec::new(),
        };
        assert_eq!(visible_user_text(&request), "visible user turn");
        assert!(request.text.contains("expanded provider input"));
    }

    #[tokio::test]
    async fn direct_operation_identity_fences_stale_settlement() {
        let operations = AsyncMutex::new(BTreeMap::new());
        let first = DirectOperationState::new(KernelOperationId::from_string("first"), true);
        let second = DirectOperationState::new(KernelOperationId::from_string("second"), true);
        operations
            .lock()
            .await
            .insert("conversation-a".into(), first.clone());
        let previous = operations
            .lock()
            .await
            .insert("conversation-a".into(), second.clone());
        assert_eq!(
            previous.as_ref().map(|state| state.operation_id.as_str()),
            Some(first.operation_id.as_str())
        );

        clear_current_direct_operation(&operations, "conversation-a", &first.operation_id).await;
        assert_eq!(
            operations
                .lock()
                .await
                .get("conversation-a")
                .map(|state| state.operation_id.as_str()),
            Some(second.operation_id.as_str())
        );

        clear_current_direct_operation(&operations, "conversation-a", &second.operation_id).await;
        assert!(!operations.lock().await.contains_key("conversation-a"));
    }

    #[test]
    fn only_explicit_open_history_contract_marks_read() {
        assert!(history_request_marks_read(OPEN_CONVERSATION_HISTORY_LIMIT));
        assert!(!history_request_marks_read(500));
    }

    #[test]
    fn reset_clears_history_and_all_conversation_boundaries() {
        let assistant = conversation(mahayana_core::MAHAYANA_AI_CONVERSATION_ID);
        let research = conversation("codex:agent:research");
        let mut state = ConversationState::new(vec![
            message(&assistant, MessageRole::Assistant, "persisted assistant"),
            message(&research, MessageRole::Assistant, "persisted research"),
        ]);
        state.clear();
        assert!(state.history.is_empty());
        assert!(state.read_through_by_conversation.is_empty());
        assert_eq!(state.unread_count(&assistant), 0);
        assert_eq!(state.unread_count(&research), 0);
    }
}
