//! Typed Agent interaction, privacy, redaction, and post-turn contracts.
//!
//! This is an iOS-owned Rust adaptation of Desktop Agent Core interaction
//! contracts. It is composed by mahayana-native-agent rather than creating a
//! second Agent runtime.

use async_trait::async_trait;
use mahayana_kernel::KernelEvent;
use serde_json::{Value, json};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrivacyMode {
    Unspecified = 0,
    NoStorage = 1,
    NoTraining = 2,
    UsageDataTrainingAllowed = 3,
    UsageCodebaseTrainingAllowed = 4,
}

impl PrivacyMode {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(value: u8) -> Result<Self, InteractionError> {
        match value {
            0 => Ok(Self::Unspecified),
            1 => Ok(Self::NoStorage),
            2 => Ok(Self::NoTraining),
            3 => Ok(Self::UsageDataTrainingAllowed),
            4 => Ok(Self::UsageCodebaseTrainingAllowed),
            other => Err(InteractionError::InvalidPrivacyMode(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataClassification {
    Safe,
    Code,
    Credentials,
    Path,
    ProviderInfo,
    Unspecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyCapability {
    StorageForTraining,
    StorageForLogging,
    StorageForUsage,
    UnsafeAlwaysAllowed,
}

pub fn allowed_purpose(
    privacy_mode: PrivacyMode,
    purpose: PrivacyCapability,
    classification: DataClassification,
) -> bool {
    if classification == DataClassification::Safe {
        return true;
    }
    if purpose == PrivacyCapability::UnsafeAlwaysAllowed {
        return true;
    }
    if matches!(
        classification,
        DataClassification::Credentials | DataClassification::Unspecified
    ) {
        return false;
    }

    match privacy_mode {
        PrivacyMode::NoStorage | PrivacyMode::Unspecified => false,
        PrivacyMode::NoTraining => match purpose {
            PrivacyCapability::StorageForLogging => matches!(
                classification,
                DataClassification::Path | DataClassification::ProviderInfo
            ),
            PrivacyCapability::StorageForTraining => false,
            PrivacyCapability::StorageForUsage => true,
            PrivacyCapability::UnsafeAlwaysAllowed => true,
        },
        PrivacyMode::UsageDataTrainingAllowed | PrivacyMode::UsageCodebaseTrainingAllowed => true,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RedactedValue {
    pub privacy_mode: PrivacyMode,
    pub classification: DataClassification,
    value: Value,
}

impl RedactedValue {
    pub fn new(
        privacy_mode: PrivacyMode,
        classification: DataClassification,
        value: Value,
    ) -> Self {
        Self {
            privacy_mode,
            classification,
            value,
        }
    }

    pub fn expose(&self, purpose: PrivacyCapability) -> Option<&Value> {
        allowed_purpose(self.privacy_mode, purpose, self.classification)
            .then_some(&self.value)
    }

    pub fn expose_unsafe(&self) -> &Value {
        self.expose(PrivacyCapability::UnsafeAlwaysAllowed)
            .expect("unsafe capability must always expose the value")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InteractionListenerContext {
    pub canceled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractionQuery {
    pub id: u64,
    pub kind: InteractionQueryKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InteractionQueryKind {
    WebSearch { args: Value },
    WebFetch {
        args: Value,
        skip_approval: bool,
        smart_mode_approval: Option<Value>,
    },
    AskQuestion { args: Value },
    SwitchMode { args: Value },
    CreatePlan { args: Value },
    SetupVmEnvironment { args: Value },
    ReplaceEnv { args: Value },
    PrManagement { args: Value },
    McpAuth { args: Value },
    ConnectScm { args: Value },
    GenerateImage { description: Option<String> },
}

impl InteractionQueryKind {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::WebSearch { .. } => "webSearchRequestQuery",
            Self::WebFetch { .. } => "webFetchRequestQuery",
            Self::AskQuestion { .. } => "askQuestionInteractionQuery",
            Self::SwitchMode { .. } => "switchModeRequestQuery",
            Self::CreatePlan { .. } => "createPlanRequestQuery",
            Self::SetupVmEnvironment { .. } => "setupVmEnvironmentArgs",
            Self::ReplaceEnv { .. } => "replaceEnvArgs",
            Self::PrManagement { .. } => "prManagementRequestQuery",
            Self::McpAuth { .. } => "mcpAuthRequestQuery",
            Self::ConnectScm { .. } => "connectScmRequestQuery",
            Self::GenerateImage { .. } => "generateImageRequestQuery",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractionResponse {
    pub id: u64,
    pub result: InteractionResponseKind,
}

impl InteractionResponse {
    pub fn validate_for(&self, query: &InteractionQuery) -> Result<(), InteractionError> {
        if self.id != query.id {
            return Err(InteractionError::UnexpectedResponse {
                query_id: query.id,
                expected: query.kind.name(),
                actual: self.result.name(),
            });
        }
        if !self.result.matches_query(&query.kind) {
            return Err(InteractionError::UnexpectedResponse {
                query_id: query.id,
                expected: query.kind.name(),
                actual: self.result.name(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InteractionResponseKind {
    WebSearchApproved,
    WebSearchRejected { reason: String },
    WebFetchApproved,
    WebFetchRejected { reason: String },
    AskQuestionRejected { reason: String },
    SwitchModeApproved,
    SwitchModeRejected { reason: String },
    CreatePlanSuccess,
    SetupVmEnvironmentSuccess,
    ReplaceEnvFailure {
        error_message: String,
        setup_logs: String,
    },
    PrManagementError { error: String },
    McpAuthApproved,
    McpAuthRejected { reason: String },
    ConnectScmApproved,
    ConnectScmRejected { reason: String },
    ConnectScmFailed { error: String },
    GenerateImageApproved { description: String },
    GenerateImageRejected { reason: String },
}

impl InteractionResponseKind {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::WebSearchApproved | Self::WebSearchRejected { .. } => "webSearchRequestResponse",
            Self::WebFetchApproved | Self::WebFetchRejected { .. } => "webFetchRequestResponse",
            Self::AskQuestionRejected { .. } => "askQuestionInteractionResponse",
            Self::SwitchModeApproved | Self::SwitchModeRejected { .. } => "switchModeRequestResponse",
            Self::CreatePlanSuccess => "createPlanRequestResponse",
            Self::SetupVmEnvironmentSuccess => "setupVmEnvironmentResult",
            Self::ReplaceEnvFailure { .. } => "replaceEnvResult",
            Self::PrManagementError { .. } => "prManagementResult",
            Self::McpAuthApproved | Self::McpAuthRejected { .. } => "mcpAuthRequestResponse",
            Self::ConnectScmApproved
            | Self::ConnectScmRejected { .. }
            | Self::ConnectScmFailed { .. } => "connectScmRequestResponse",
            Self::GenerateImageApproved { .. } | Self::GenerateImageRejected { .. } => {
                "generateImageRequestResponse"
            }
        }
    }

    fn matches_query(&self, query: &InteractionQueryKind) -> bool {
        matches!(
            (query, self),
            (
                InteractionQueryKind::WebSearch { .. },
                Self::WebSearchApproved | Self::WebSearchRejected { .. }
            )
                | (
                    InteractionQueryKind::WebFetch { .. },
                    Self::WebFetchApproved | Self::WebFetchRejected { .. }
                )
                | (
                    InteractionQueryKind::AskQuestion { .. },
                    Self::AskQuestionRejected { .. }
                )
                | (
                    InteractionQueryKind::SwitchMode { .. },
                    Self::SwitchModeApproved | Self::SwitchModeRejected { .. }
                )
                | (InteractionQueryKind::CreatePlan { .. }, Self::CreatePlanSuccess)
                | (
                    InteractionQueryKind::SetupVmEnvironment { .. },
                    Self::SetupVmEnvironmentSuccess
                )
                | (
                    InteractionQueryKind::ReplaceEnv { .. },
                    Self::ReplaceEnvFailure { .. }
                )
                | (
                    InteractionQueryKind::PrManagement { .. },
                    Self::PrManagementError { .. }
                )
                | (
                    InteractionQueryKind::McpAuth { .. },
                    Self::McpAuthApproved | Self::McpAuthRejected { .. }
                )
                | (
                    InteractionQueryKind::ConnectScm { .. },
                    Self::ConnectScmApproved
                        | Self::ConnectScmRejected { .. }
                        | Self::ConnectScmFailed { .. }
                )
                | (
                    InteractionQueryKind::GenerateImage { .. },
                    Self::GenerateImageApproved { .. } | Self::GenerateImageRejected { .. }
                )
        )
    }
}

pub struct Responses;

impl Responses {
    pub fn web_search_approved(query_id: u64) -> InteractionResponse {
        InteractionResponse {
            id: query_id,
            result: InteractionResponseKind::WebSearchApproved,
        }
    }

    pub fn web_search_rejected(query_id: u64, reason: impl Into<String>) -> InteractionResponse {
        InteractionResponse {
            id: query_id,
            result: InteractionResponseKind::WebSearchRejected {
                reason: reason.into(),
            },
        }
    }

    pub fn web_fetch_approved(query_id: u64) -> InteractionResponse {
        InteractionResponse {
            id: query_id,
            result: InteractionResponseKind::WebFetchApproved,
        }
    }

    pub fn web_fetch_rejected(query_id: u64, reason: impl Into<String>) -> InteractionResponse {
        InteractionResponse {
            id: query_id,
            result: InteractionResponseKind::WebFetchRejected {
                reason: reason.into(),
            },
        }
    }

    pub fn mcp_auth_approved(query_id: u64) -> InteractionResponse {
        InteractionResponse {
            id: query_id,
            result: InteractionResponseKind::McpAuthApproved,
        }
    }

    pub fn mcp_auth_rejected(query_id: u64, reason: impl Into<String>) -> InteractionResponse {
        InteractionResponse {
            id: query_id,
            result: InteractionResponseKind::McpAuthRejected {
                reason: reason.into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContextInjectionState {
    Queued,
    Delivered {
        step: Value,
        delivery_batch_id: Value,
        delivered_at_ms: u64,
    },
    QueuedForNextTurn,
    Cancelled,
    Rejected { reason: Value },
}

#[derive(Debug, Clone, PartialEq)]
pub enum InteractionUpdate {
    TextDelta { text: String },
    ToolCallStarted {
        call_id: String,
        tool_call: Value,
        model_call_id: String,
    },
    ToolCallCompleted {
        call_id: String,
        tool_call: Value,
        model_call_id: String,
    },
    ToolCallDelta {
        call_id: String,
        tool_call_delta: Value,
        model_call_id: String,
    },
    ThinkingDelta { text: String, thinking_style: Value },
    ThinkingCompleted { thinking_duration_ms: u64 },
    UserMessageAppended { user_message: Value },
    PartialToolCall {
        call_id: String,
        tool_call: Value,
        model_call_id: String,
        args_text_delta: String,
    },
    TokenDelta { tokens: u64 },
    Summary { summary: String },
    SummaryStarted,
    Heartbeat,
    ContextInjectionState {
        injection_id: String,
        state: ContextInjectionState,
    },
    SummaryCompleted { hook_message: Value },
    ShellOutputDelta { event: Value },
    TurnEnded { usage: Option<TurnUsage> },
    StepStarted { step_id: String },
    StepCompleted {
        step_id: String,
        step_duration_ms: u64,
    },
    PromptSuggestion { suggestion: String },
    ActiveBranchChange {
        path: String,
        branch_name: Option<String>,
    },
    FeedbackRequest {
        request_id: String,
        canonical_model_name: String,
        categories: Vec<Value>,
        category_groups: Vec<Value>,
        copy: Value,
    },
}

impl InteractionUpdate {
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::TextDelta { .. } => "textDelta",
            Self::ToolCallStarted { .. } => "toolCallStarted",
            Self::ToolCallCompleted { .. } => "toolCallCompleted",
            Self::ToolCallDelta { .. } => "toolCallDelta",
            Self::ThinkingDelta { .. } => "thinkingDelta",
            Self::ThinkingCompleted { .. } => "thinkingCompleted",
            Self::UserMessageAppended { .. } => "userMessageAppended",
            Self::PartialToolCall { .. } => "partialToolCall",
            Self::TokenDelta { .. } => "tokenDelta",
            Self::Summary { .. } => "summary",
            Self::SummaryStarted => "summaryStarted",
            Self::Heartbeat => "heartbeat",
            Self::ContextInjectionState { .. } => "contextInjectionState",
            Self::SummaryCompleted { .. } => "summaryCompleted",
            Self::ShellOutputDelta { .. } => "shellOutputDelta",
            Self::TurnEnded { .. } => "turnEnded",
            Self::StepStarted { .. } => "stepStarted",
            Self::StepCompleted { .. } => "stepCompleted",
            Self::PromptSuggestion { .. } => "promptSuggestion",
            Self::ActiveBranchChange { .. } => "activeBranchChange",
            Self::FeedbackRequest { .. } => "feedbackRequest",
        }
    }
}

pub struct Updates;

impl Updates {
    pub fn text_delta(text: impl Into<String>) -> InteractionUpdate {
        InteractionUpdate::TextDelta { text: text.into() }
    }

    pub fn turn_ended(usage: Option<TurnUsage>) -> InteractionUpdate {
        InteractionUpdate::TurnEnded { usage }
    }

    pub fn prompt_suggestion(suggestion: impl Into<String>) -> InteractionUpdate {
        InteractionUpdate::PromptSuggestion {
            suggestion: suggestion.into(),
        }
    }

    pub fn feedback_request(
        request_id: impl Into<String>,
        canonical_model_name: impl Into<String>,
        categories: Vec<Value>,
        category_groups: Vec<Value>,
        copy: Value,
    ) -> InteractionUpdate {
        InteractionUpdate::FeedbackRequest {
            request_id: request_id.into(),
            canonical_model_name: canonical_model_name.into(),
            categories,
            category_groups,
            copy,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RedactedInteractionUpdate {
    pub privacy_mode: PrivacyMode,
    pub update: InteractionUpdate,
}

impl RedactedInteractionUpdate {
    pub fn audit_metadata(&self) -> Value {
        json!({
            "privacyMode": self.privacy_mode.code(),
            "interactionUpdate": self.update.kind_name(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RedactedInteractionQuery {
    pub privacy_mode: PrivacyMode,
    pub query: InteractionQuery,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RedactedInteractionResponse {
    pub privacy_mode: PrivacyMode,
    pub response: InteractionResponse,
}

pub struct RedactedUpdates;

impl RedactedUpdates {
    pub fn wrap(privacy_mode: PrivacyMode, update: InteractionUpdate) -> RedactedInteractionUpdate {
        RedactedInteractionUpdate {
            privacy_mode,
            update,
        }
    }

    pub fn text_delta(
        privacy_mode: PrivacyMode,
        text: impl Into<String>,
    ) -> RedactedInteractionUpdate {
        Self::wrap(privacy_mode, Updates::text_delta(text))
    }

    pub fn turn_ended(
        privacy_mode: PrivacyMode,
        usage: Option<TurnUsage>,
    ) -> RedactedInteractionUpdate {
        Self::wrap(privacy_mode, Updates::turn_ended(usage))
    }
}

pub fn redacted_update_for_kernel_event(
    event: &KernelEvent,
    privacy_mode: PrivacyMode,
) -> Option<RedactedInteractionUpdate> {
    let update = match event {
        KernelEvent::MessageDelta { delta, .. } => Updates::text_delta(delta.clone()),
        KernelEvent::ToolStarted {
            operation_id,
            tool_call_id,
            tool,
            arguments,
        } => InteractionUpdate::ToolCallStarted {
            call_id: tool_call_id.clone(),
            tool_call: json!({"name": tool, "arguments": arguments}),
            model_call_id: operation_id.as_str().to_string(),
        },
        KernelEvent::ToolCompleted {
            operation_id,
            tool_call_id,
            tool,
            output,
            success,
        } => InteractionUpdate::ToolCallCompleted {
            call_id: tool_call_id.clone(),
            tool_call: json!({"name": tool, "output": output, "success": success}),
            model_call_id: operation_id.as_str().to_string(),
        },
        KernelEvent::UsageUpdated { total_tokens, .. } => InteractionUpdate::TokenDelta {
            tokens: *total_tokens,
        },
        KernelEvent::Activity { metadata, .. } => {
            let step_id = metadata.get("stepId")?.as_str()?.to_string();
            match metadata.get("status").and_then(Value::as_str) {
                Some("completed") => InteractionUpdate::StepCompleted {
                    step_id,
                    step_duration_ms: metadata
                        .get("durationMs")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                },
                Some("failed") => InteractionUpdate::StepCompleted {
                    step_id,
                    step_duration_ms: metadata
                        .get("durationMs")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                },
                _ => InteractionUpdate::StepStarted { step_id },
            }
        }
        KernelEvent::OperationCompleted { .. } => Updates::turn_ended(None),
        _ => return None,
    };
    Some(RedactedUpdates::wrap(privacy_mode, update))
}

pub type PostTurnFuture =
    Pin<Box<dyn Future<Output = Result<(), InteractionError>> + Send + 'static>>;
pub type PostTurnWork = Box<dyn FnOnce() -> PostTurnFuture + Send + 'static>;

#[async_trait]
pub trait InteractionListener: Send + Sync {
    async fn send_update(
        &self,
        context: &InteractionListenerContext,
        update: InteractionUpdate,
    ) -> Result<(), InteractionError>;

    async fn query(
        &self,
        context: &InteractionListenerContext,
        query: InteractionQuery,
    ) -> Result<InteractionResponse, InteractionError>;

    fn enqueue_post_turn_ended_work(&self, work: PostTurnWork) -> Result<(), InteractionError>;

    async fn flush_post_turn_ended_work(
        &self,
        context: &InteractionListenerContext,
    ) -> Result<(), InteractionError>;
}

#[async_trait]
pub trait RedactedInteractionListener: Send + Sync {
    async fn send_update(
        &self,
        context: &InteractionListenerContext,
        update: RedactedInteractionUpdate,
    ) -> Result<(), InteractionError>;

    async fn query(
        &self,
        context: &InteractionListenerContext,
        query: RedactedInteractionQuery,
    ) -> Result<RedactedInteractionResponse, InteractionError>;

    fn enqueue_post_turn_ended_work(&self, work: PostTurnWork) -> Result<(), InteractionError>;

    async fn flush_post_turn_ended_work(
        &self,
        context: &InteractionListenerContext,
    ) -> Result<(), InteractionError>;
}

#[derive(Default)]
pub struct NoopInteractionListener {
    post_turn_ended_work_queue: Mutex<Vec<PostTurnWork>>,
}

#[async_trait]
impl InteractionListener for NoopInteractionListener {
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
    ) -> Result<InteractionResponse, InteractionError> {
        let response = match &query.kind {
            InteractionQueryKind::WebSearch { .. } => Responses::web_search_approved(query.id),
            InteractionQueryKind::WebFetch { .. } => Responses::web_fetch_approved(query.id),
            InteractionQueryKind::AskQuestion { .. } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::AskQuestionRejected {
                    reason: "Questions skipped by user (CLI fallback)".into(),
                },
            },
            InteractionQueryKind::SwitchMode { .. } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::SwitchModeRejected {
                    reason: "Mode switching not supported in CLI".into(),
                },
            },
            InteractionQueryKind::CreatePlan { .. } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::CreatePlanSuccess,
            },
            InteractionQueryKind::SetupVmEnvironment { .. } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::SetupVmEnvironmentSuccess,
            },
            InteractionQueryKind::ReplaceEnv { .. } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::ReplaceEnvFailure {
                    error_message: "Environment replacement is not supported in this environment"
                        .into(),
                    setup_logs: String::new(),
                },
            },
            InteractionQueryKind::PrManagement { .. } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::PrManagementError {
                    error: "PR management is only available in cloud agents".into(),
                },
            },
            InteractionQueryKind::McpAuth { .. } => Responses::mcp_auth_rejected(
                query.id,
                "MCP authentication is not supported in this environment",
            ),
            InteractionQueryKind::ConnectScm { .. } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::ConnectScmRejected {
                    reason: "Connecting GitHub is not supported in this environment".into(),
                },
            },
            InteractionQueryKind::GenerateImage { description } => InteractionResponse {
                id: query.id,
                result: InteractionResponseKind::GenerateImageApproved {
                    description: description.clone().unwrap_or_default(),
                },
            },
        };
        response.validate_for(&query)?;
        Ok(response)
    }

    fn enqueue_post_turn_ended_work(&self, work: PostTurnWork) -> Result<(), InteractionError> {
        self.post_turn_ended_work_queue
            .lock()
            .map_err(|_| InteractionError::QueuePoisoned)?
            .push(work);
        Ok(())
    }

    async fn flush_post_turn_ended_work(
        &self,
        _context: &InteractionListenerContext,
    ) -> Result<(), InteractionError> {
        let work = {
            let mut queue = self
                .post_turn_ended_work_queue
                .lock()
                .map_err(|_| InteractionError::QueuePoisoned)?;
            std::mem::take(&mut *queue)
        };

        let handles = work
            .into_iter()
            .map(|task| tokio::spawn(async move {
                let _ = task().await;
            }))
            .collect::<Vec<_>>();

        for handle in handles {
            let _ = handle.await;
        }
        Ok(())
    }
}

pub struct CancellationFencedListener<L> {
    delegate: L,
}

impl<L> CancellationFencedListener<L> {
    pub fn new(delegate: L) -> Self {
        Self { delegate }
    }
}

#[async_trait]
impl<L> InteractionListener for CancellationFencedListener<L>
where
    L: InteractionListener,
{
    async fn send_update(
        &self,
        context: &InteractionListenerContext,
        update: InteractionUpdate,
    ) -> Result<(), InteractionError> {
        if context.canceled {
            return Err(InteractionError::Cancelled);
        }
        self.delegate.send_update(context, update).await
    }

    async fn query(
        &self,
        context: &InteractionListenerContext,
        query: InteractionQuery,
    ) -> Result<InteractionResponse, InteractionError> {
        if context.canceled {
            return Err(InteractionError::Cancelled);
        }
        self.delegate.query(context, query).await
    }

    fn enqueue_post_turn_ended_work(&self, work: PostTurnWork) -> Result<(), InteractionError> {
        self.delegate.enqueue_post_turn_ended_work(work)
    }

    async fn flush_post_turn_ended_work(
        &self,
        context: &InteractionListenerContext,
    ) -> Result<(), InteractionError> {
        self.delegate.flush_post_turn_ended_work(context).await
    }
}

pub struct RedactedDelegateAdapter<R> {
    delegate: R,
    privacy_mode: PrivacyMode,
}

impl<R> RedactedDelegateAdapter<R> {
    pub fn new(delegate: R, privacy_mode: PrivacyMode) -> Self {
        Self {
            delegate,
            privacy_mode,
        }
    }
}

#[async_trait]
impl<R> InteractionListener for RedactedDelegateAdapter<R>
where
    R: RedactedInteractionListener,
{
    async fn send_update(
        &self,
        context: &InteractionListenerContext,
        update: InteractionUpdate,
    ) -> Result<(), InteractionError> {
        self.delegate
            .send_update(
                context,
                RedactedInteractionUpdate {
                    privacy_mode: self.privacy_mode,
                    update,
                },
            )
            .await
    }

    async fn query(
        &self,
        context: &InteractionListenerContext,
        query: InteractionQuery,
    ) -> Result<InteractionResponse, InteractionError> {
        let response = self
            .delegate
            .query(
                context,
                RedactedInteractionQuery {
                    privacy_mode: self.privacy_mode,
                    query: query.clone(),
                },
            )
            .await?;
        response.response.validate_for(&query)?;
        Ok(response.response)
    }

    fn enqueue_post_turn_ended_work(&self, work: PostTurnWork) -> Result<(), InteractionError> {
        self.delegate.enqueue_post_turn_ended_work(work)
    }

    async fn flush_post_turn_ended_work(
        &self,
        context: &InteractionListenerContext,
    ) -> Result<(), InteractionError> {
        self.delegate.flush_post_turn_ended_work(context).await
    }
}

pub struct UnredactedDelegateAdapter<L> {
    delegate: L,
    privacy_mode: PrivacyMode,
}

impl<L> UnredactedDelegateAdapter<L> {
    pub fn new(delegate: L, privacy_mode: PrivacyMode) -> Self {
        Self {
            delegate,
            privacy_mode,
        }
    }
}

#[async_trait]
impl<L> RedactedInteractionListener for UnredactedDelegateAdapter<L>
where
    L: InteractionListener,
{
    async fn send_update(
        &self,
        context: &InteractionListenerContext,
        update: RedactedInteractionUpdate,
    ) -> Result<(), InteractionError> {
        self.delegate.send_update(context, update.update).await
    }

    async fn query(
        &self,
        context: &InteractionListenerContext,
        query: RedactedInteractionQuery,
    ) -> Result<RedactedInteractionResponse, InteractionError> {
        let response = self.delegate.query(context, query.query).await?;
        Ok(RedactedInteractionResponse {
            privacy_mode: self.privacy_mode,
            response,
        })
    }

    fn enqueue_post_turn_ended_work(&self, work: PostTurnWork) -> Result<(), InteractionError> {
        self.delegate.enqueue_post_turn_ended_work(work)
    }

    async fn flush_post_turn_ended_work(
        &self,
        context: &InteractionListenerContext,
    ) -> Result<(), InteractionError> {
        self.delegate.flush_post_turn_ended_work(context).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionError {
    StreamClosed { message: String },
    UnexpectedResponse {
        query_id: u64,
        expected: &'static str,
        actual: &'static str,
    },
    InvalidPrivacyMode(u8),
    QueuePoisoned,
    Cancelled,
}

impl fmt::Display for InteractionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StreamClosed { message } => write!(formatter, "interaction stream closed: {message}"),
            Self::UnexpectedResponse {
                query_id,
                expected,
                actual,
            } => write!(
                formatter,
                "unexpected response for interaction query {query_id}: expected {expected}, received {actual}"
            ),
            Self::InvalidPrivacyMode(mode) => write!(formatter, "unknown privacy mode: {mode}"),
            Self::QueuePoisoned => formatter.write_str("post-turn interaction queue is poisoned"),
            Self::Cancelled => formatter.write_str("interaction was cancelled"),
        }
    }
}

impl Error for InteractionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn query(id: u64, kind: InteractionQueryKind) -> InteractionQuery {
        InteractionQuery { id, kind }
    }

    #[tokio::test]
    async fn noop_listener_preserves_ids_and_cli_fallback_matrix() {
        let listener = NoopInteractionListener::default();
        let context = InteractionListenerContext::default();

        let web = query(7, InteractionQueryKind::WebSearch { args: json!({"q":"x"}) });
        let web_response = listener.query(&context, web.clone()).await.expect("web response");
        assert_eq!(web_response.id, 7);
        assert_eq!(web_response.result, InteractionResponseKind::WebSearchApproved);
        web_response.validate_for(&web).expect("typed response");

        let auth = query(8, InteractionQueryKind::McpAuth { args: json!({"server":"docs"}) });
        assert!(matches!(
            listener.query(&context, auth).await.expect("auth response").result,
            InteractionResponseKind::McpAuthRejected { .. }
        ));

        let image = query(
            9,
            InteractionQueryKind::GenerateImage {
                description: Some("lotus".into()),
            },
        );
        assert_eq!(
            listener.query(&context, image).await.expect("image response").result,
            InteractionResponseKind::GenerateImageApproved {
                description: "lotus".into()
            }
        );
    }

    #[test]
    fn typed_response_validation_fails_closed_on_mismatch() {
        let query = query(42, InteractionQueryKind::WebFetch {
            args: json!({}),
            skip_approval: false,
            smart_mode_approval: None,
        });
        let response = Responses::web_search_approved(42);
        assert!(matches!(
            response.validate_for(&query),
            Err(InteractionError::UnexpectedResponse { .. })
        ));
    }

    #[tokio::test]
    async fn post_turn_flush_drains_once_and_best_effort_ignores_task_errors() {
        let listener = NoopInteractionListener::default();
        let counter = Arc::new(AtomicUsize::new(0));

        for should_fail in [false, true] {
            let counter = Arc::clone(&counter);
            listener
                .enqueue_post_turn_ended_work(Box::new(move || {
                    Box::pin(async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        if should_fail {
                            Err(InteractionError::StreamClosed {
                                message: "cleanup failed".into(),
                            })
                        } else {
                            Ok(())
                        }
                    })
                }))
                .expect("queue work");
        }

        listener
            .flush_post_turn_ended_work(&InteractionListenerContext::default())
            .await
            .expect("best-effort flush");
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        listener
            .flush_post_turn_ended_work(&InteractionListenerContext::default())
            .await
            .expect("second flush");
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancellation_fence_blocks_updates_and_queries_but_allows_cleanup_flush() {
        let listener = CancellationFencedListener::new(NoopInteractionListener::default());
        let context = InteractionListenerContext { canceled: true };

        assert!(matches!(
            listener
                .send_update(&context, Updates::text_delta("late"))
                .await,
            Err(InteractionError::Cancelled)
        ));
        assert!(matches!(
            listener
                .query(
                    &context,
                    query(1, InteractionQueryKind::WebSearch { args: json!({}) })
                )
                .await,
            Err(InteractionError::Cancelled)
        ));
        listener
            .flush_post_turn_ended_work(&context)
            .await
            .expect("cleanup is still allowed after cancellation");
    }

    #[test]
    fn privacy_modes_match_desktop_storage_and_training_rules() {
        assert!(!allowed_purpose(
            PrivacyMode::NoStorage,
            PrivacyCapability::StorageForUsage,
            DataClassification::Code
        ));
        assert!(!allowed_purpose(
            PrivacyMode::NoTraining,
            PrivacyCapability::StorageForTraining,
            DataClassification::Code
        ));
        assert!(allowed_purpose(
            PrivacyMode::NoTraining,
            PrivacyCapability::StorageForUsage,
            DataClassification::Code
        ));
        assert!(allowed_purpose(
            PrivacyMode::NoTraining,
            PrivacyCapability::StorageForLogging,
            DataClassification::Path
        ));
        assert!(!allowed_purpose(
            PrivacyMode::UsageCodebaseTrainingAllowed,
            PrivacyCapability::StorageForUsage,
            DataClassification::Credentials
        ));
        assert!(allowed_purpose(
            PrivacyMode::NoStorage,
            PrivacyCapability::UnsafeAlwaysAllowed,
            DataClassification::Credentials
        ));
    }

    #[test]
    fn redacted_values_fail_closed_for_credentials_without_unsafe_capability() {
        let value = RedactedValue::new(
            PrivacyMode::UsageCodebaseTrainingAllowed,
            DataClassification::Credentials,
            json!("secret"),
        );
        assert!(value.expose(PrivacyCapability::StorageForUsage).is_none());
        assert_eq!(value.expose_unsafe(), &json!("secret"));
    }

    #[test]
    fn kernel_tool_events_preserve_call_correlation_and_privacy_provenance() {
        let event = KernelEvent::ToolStarted {
            operation_id: mahayana_kernel::OperationId::from_string("model-call-1"),
            tool_call_id: "call-7".into(),
            tool: "web_search".into(),
            arguments: json!({"query":"release"}),
        };
        let update = redacted_update_for_kernel_event(&event, PrivacyMode::NoTraining)
            .expect("projected interaction update");
        assert_eq!(update.privacy_mode, PrivacyMode::NoTraining);
        assert_eq!(update.audit_metadata()["interactionUpdate"], "toolCallStarted");
        let InteractionUpdate::ToolCallStarted {
            call_id,
            model_call_id,
            tool_call,
        } = update.update
        else {
            panic!("tool start variant");
        };
        assert_eq!(call_id, "call-7");
        assert_eq!(model_call_id, "model-call-1");
        assert_eq!(tool_call["name"], "web_search");
    }
}
