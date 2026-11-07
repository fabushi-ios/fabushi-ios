use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::extensions::inference::cursor_inference_transport::CursorInferenceAuth;
use crate::extensions::session::typed_replay::TypedReplayEntry;
use crate::extensions::inference::provider_session::{
    ProviderMessage, ProviderSessionError, ProviderTokenUsage, ProviderToolStepReminderHook,
    RoutedMcpMetaToolDefinition, RoutedProvider, RoutedProviderCheckpoint, RoutedToolDefinition,
};
use crate::cloud_agents::cloud_agent_tool::CloudAgentToolDependencies;
use crate::ports::mcp_state_executor::{
    McpStateExecResult, SandMcpToolProvider, execute_mcp_state as execute_canonical_mcp_state,
};

use super::box_tool_access::{BoxShellAutoReviewCallback, RunnerBoxResourcePort};
use super::production_turn_run_shell_adapter::{
    ProviderRetryEvent, ProviderRetryReport, RoutedProviderCheckpointStore,
};
use super::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedProviderRun, RoutedToolBridge,
    RunnerRequestContextSnapshot, run_routed_provider_in_runner,
};
use super::sand_action_audit::{AuditedRoutedToolBridge, RoutedMcpAuditConfig};
use super::sand_auto_review::{SandAutoReviewController, SandAutoReviewMode};
use super::sand_auto_review_classifier_run::{
    AutoReviewClassifierDecision, AutoReviewClassifierError,
};
use super::sand_subagent_auto_review::{
    SubagentReviewOutcome,
    build_sand_subagent_launch_review_target, review_sand_subagent_action,
};
use super::subagent_runtime::SubagentRuntime;
use super::StreamAttemptRuntime;
use super::{ToolLoopDecision, ToolLoopGate, TurnOwnerToken};
use super::turn_shape::checkpoint_ended_on_silent_tool_calls;
use super::turn_tool_session_reminders::TurnToolSessionReminderState;
use super::turn_observation::{
    McpObservedRoutedToolBridge, ObservedRoutedToolBridge, TurnObservationHandle,
};
use super::tools::communicate_tool::{
    CommunicateInteractionSink, CommunicateRoutedToolBridge,
    TurnObservationCommunicateSink,
};
use super::tools::send_message_tool::{
    SendMessageInteractionSink, SendMessageSink, TurnObservationSendMessageSink,
};
use super::tools::sand_reaction_tool::ReactionSink;
use super::tools::sand_agent_management_tools::AgentManagementSink;
use super::tools::sand_task_subagent_tool::{SubagentTaskReviewCallback, SubagentTaskSink};
use super::tools::sand_subagent_management_tools::SubagentSteerReviewCallback;
use super::tools::sand_browser_tools::BrowserToolExecutor;
use super::tools::sand_web_tools::WebToolExecutor;
use super::tools::sand_generate_image_tool::GenerateImageToolExecutor;
use super::tools::sand_computer_tool::{ComputerToolExecutor, ComputerToolExposure};
use super::tools::sand_file_transfer_tools::FileTransferExecutor;
use super::tools::sand_external_machine_tools::{
    ExternalMachineExecutor, ExternalShellAutoReviewCallback,
};
use super::tools::sand_mcp_management_tools::McpManagementSink;
use super::tools::sand_state_tool::{
    RoutineAutoReviewCallback, RoutinePostWriteCallback, SandStateWriter,
    StateApprovalBarrier,
};
use super::tools::sand_multitask_todo_tool::MultitaskTodoState;
use super::tools::turn_toolset::{
    TurnLocalToolPermissionBinding, TurnToolsetDependencies, TurnToolsetRole,
    build_turn_toolset, fence_turn_toolset, project_turn_mcp_toolset,
};


#[derive(Debug, Clone, PartialEq)]
pub struct CompletedRoutedToolCall {
    pub tool: RoutedToolDefinition,
    pub args: Value,
    pub tool_call_id: String,
    pub result: Result<Value, String>,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
    pub sequence: u64,
}

struct CheckpointRecordingRoutedToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    completed: Arc<Mutex<Vec<CompletedRoutedToolCall>>>,
    next_sequence: Arc<AtomicU64>,
    tool_loop_gate: Option<(ToolLoopGate, TurnOwnerToken)>,
    cancellation: RoutedProviderCancellation,
}

impl CheckpointRecordingRoutedToolBridge {
    fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        completed: Arc<Mutex<Vec<CompletedRoutedToolCall>>>,
        next_sequence: Arc<AtomicU64>,
        tool_loop_gate: Option<(ToolLoopGate, TurnOwnerToken)>,
        cancellation: RoutedProviderCancellation,
    ) -> Self {
        Self {
            delegate,
            completed,
            next_sequence,
            tool_loop_gate,
            cancellation,
        }
    }
}

impl RoutedToolBridge for CheckpointRecordingRoutedToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        self.delegate.list_tools()
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        self.delegate.list_mcp_meta_tools()
    }

    fn observe_partial_tool_call(
        &self,
        partial: &crate::extensions::inference::provider_session::ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if let Some((gate, owner)) = &self.tool_loop_gate {
            let effective_tool_name = if tool.tool_name.trim().is_empty() {
                tool.name.as_str()
            } else {
                tool.tool_name.as_str()
            };
            match gate
                .observe_tool_call(owner, effective_tool_name, &args)
                .map_err(|error| ProviderSessionError::Tool(error.to_string()))?
            {
                ToolLoopDecision::Continue { .. } => {}
                ToolLoopDecision::Cancel { reason } => {
                    self.cancellation.cancel(reason);
                    return Err(self.cancellation.check().expect_err("cancellation was just requested"));
                }
            }
        }
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let started_at_ms = unix_epoch_ms();
        let result = self.delegate.call_tool(tool, args.clone(), tool_call_id);
        let completed_at_ms = unix_epoch_ms();
        self.completed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(CompletedRoutedToolCall {
                tool: tool.clone(),
                args,
                tool_call_id: tool_call_id.to_string(),
                result: result
                    .as_ref()
                    .map(Clone::clone)
                    .map_err(ToString::to_string),
                started_at_ms,
                completed_at_ms,
                sequence,
            });
        result
    }
}

fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

struct RoutedBridgeMcpToolProvider<'a> {
    bridge: &'a dyn RoutedToolBridge,
}

impl SandMcpToolProvider for RoutedBridgeMcpToolProvider<'_> {
    fn get_tools(&self) -> Result<Vec<RoutedToolDefinition>, String> {
        self.bridge.list_tools().map_err(|error| error.to_string())
    }
}

struct FrozenRoutedMcpToolProvider<'a> {
    tools: &'a [RoutedMcpMetaToolDefinition],
}

impl SandMcpToolProvider for FrozenRoutedMcpToolProvider<'_> {
    fn get_tools(&self) -> Result<Vec<RoutedToolDefinition>, String> {
        Ok(self.tools.iter().map(|source| source.tool.clone()).collect())
    }
}

struct ObservedRoutedProviderCheckpointStore {
    delegate: Arc<dyn RoutedProviderCheckpointStore>,
    latest: Arc<Mutex<Option<RoutedProviderCheckpoint>>>,
}

impl RoutedProviderCheckpointStore for ObservedRoutedProviderCheckpointStore {
    fn persist(
        &self,
        checkpoint: &RoutedProviderCheckpoint,
    ) -> Result<String, ProviderSessionError> {
        let cursor = self.delegate.persist(checkpoint)?;
        *self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(checkpoint.clone());
        Ok(cursor)
    }
}

pub const SAND_AGENT_MAX_STEPS: usize = 5_000;
pub const SAND_AGENT_TOKEN_LIMIT: usize = 200_000;
pub const GENERAL_PURPOSE_SUBAGENT_TYPE: &str = "general-purpose";
pub const EXECUTOR_SUBAGENT_TYPE: &str = "executor";
pub const COMPUTER_USE_SUBAGENT_TYPE: &str = "computeruse";
pub const BROWSER_USE_SUBAGENT_TYPE: &str = "browseruse";

pub type TurnSubagentLaunchClassifier = Arc<
    dyn Fn(&Value, &str, &str) -> Result<AutoReviewClassifierDecision, AutoReviewClassifierError>
        + Send
        + Sync,
>;

pub struct TurnSubagentLaunchReviewBindings {
    pub mode: Arc<dyn Fn() -> SandAutoReviewMode + Send + Sync>,
    pub assert_no_pending_approval:
        Arc<dyn Fn() -> Result<(), ProviderSessionError> + Send + Sync>,
    pub controller: Arc<SandAutoReviewController>,
    pub request_source: String,
    pub should_cancel: Arc<dyn Fn() -> bool + Send + Sync>,
    pub classify: TurnSubagentLaunchClassifier,
}

/// Builds the per-turn subagent launch reviewer from raw Host services.
///
/// The Host supplies authentication-backed classifier I/O and the shared
/// approval controller, while the Runner composition owns target construction,
/// policy mode lookup, cancellation, approval semantics and result mapping.
/// This mirrors the frozen Grok owner boundary without creating a second
/// Auto-review policy implementation in the Host entrypoint.
pub fn create_turn_subagent_task_review(
    bindings: TurnSubagentLaunchReviewBindings,
) -> SubagentTaskReviewCallback {
    Arc::new(move |prompt: &str, subagent_type: &str, tool_call_id: &str| {
        let Some(target) = build_sand_subagent_launch_review_target(
            prompt,
            Some(subagent_type),
        ) else {
            return Ok(Some("Task requires a non-empty prompt.".into()));
        };
        (bindings.assert_no_pending_approval)()?;
        let mode = (bindings.mode)();
        let outcome = review_sand_subagent_action(
            mode,
            &target,
            Some(bindings.controller.as_ref()),
            &bindings.request_source,
            || (bindings.should_cancel)(),
            |risk_target, classifier_mode| {
                (bindings.classify)(risk_target, tool_call_id, classifier_mode)
            },
        )
        .map_err(|error| ProviderSessionError::Tool(format!("{error:?}")))?;
        Ok(match outcome {
            SubagentReviewOutcome::Allowed => None,
            SubagentReviewOutcome::Blocked(reason) => Some(reason),
            SubagentReviewOutcome::Cancelled => {
                Some("The subagent launch was cancelled.".into())
            }
        })
    })
}

pub fn build_turn_subagent_types(
    is_subagent_runner: bool,
    multitask_enabled: bool,
    remote_box_available: bool,
    remote_box_has_desktop: bool,
    browser_use_enabled: bool,
) -> Option<Vec<String>> {
    if is_subagent_runner {
        return None;
    }
    let mut types = vec![if multitask_enabled {
        EXECUTOR_SUBAGENT_TYPE.to_string()
    } else {
        GENERAL_PURPOSE_SUBAGENT_TYPE.to_string()
    }];
    if remote_box_available && remote_box_has_desktop {
        types.push(COMPUTER_USE_SUBAGENT_TYPE.to_string());
        if browser_use_enabled {
            types.push(BROWSER_USE_SUBAGENT_TYPE.to_string());
        }
    }
    Some(types)
}

/// Shipping Runner composition for one provider-backed turn.
///
/// This module deliberately owns the dependency assembly that used to be
/// hand-built by the Host entrypoint: provider selection, Host tool bridge,
/// request-context projection, cancellation, and the durable checkpoint store.
/// Provider execution itself remains in the Runner-owned routed-provider
/// runtime so the Host process does not absorb Runner responsibilities.
#[derive(Clone)]
pub struct TurnAgentComposition {
    provider: RoutedProvider,
    bridge: Arc<dyn RoutedToolBridge>,
    cursor_auth: Option<Arc<dyn CursorInferenceAuth>>,
    request_context: RunnerRequestContextSnapshot,
    cancellation: RoutedProviderCancellation,
    checkpoint_store: Arc<dyn RoutedProviderCheckpointStore>,
    latest_provider_checkpoint: Arc<Mutex<Option<RoutedProviderCheckpoint>>>,
    completed_tool_calls: Arc<Mutex<Vec<CompletedRoutedToolCall>>>,
    next_tool_sequence: Arc<AtomicU64>,
    retry_sink: Option<Arc<dyn Fn(&ProviderRetryEvent) + Send + Sync>>,
    retry_report_sink: Option<Arc<dyn Fn(&ProviderRetryReport) + Send + Sync>>,
    stream_attempt_runtime: Option<Arc<StreamAttemptRuntime>>,
    usage_sink: Option<Arc<dyn Fn(ProviderTokenUsage) + Send + Sync>>,
    box_resources: Option<Arc<dyn RunnerBoxResourcePort>>,
    box_shell_review: Option<BoxShellAutoReviewCallback>,
    browser_executor: Option<Arc<dyn BrowserToolExecutor>>,
    web_executor: Option<Arc<dyn WebToolExecutor>>,
    generate_image_executor: Option<Arc<dyn GenerateImageToolExecutor>>,
    computer_executor: Option<Arc<dyn ComputerToolExecutor>>,
    computer_exposure: ComputerToolExposure,
    file_transfer_executor: Option<Arc<dyn FileTransferExecutor>>,
    external_machine_executor: Option<Arc<dyn ExternalMachineExecutor>>,
    external_shell_review: Option<ExternalShellAutoReviewCallback>,
    mcp_management_sink: Option<Arc<dyn McpManagementSink>>,
    send_message_sink: Option<Arc<dyn SendMessageSink>>,
    reaction_sink: Option<Arc<dyn ReactionSink>>,
    agent_management_sink: Option<Arc<dyn AgentManagementSink>>,
    subagent_task_sink: Option<Arc<dyn SubagentTaskSink>>,
    subagent_task_allowed_types: Option<Arc<Vec<String>>>,
    subagent_task_review: Option<SubagentTaskReviewCallback>,
    subagent_runtime: Option<Arc<Mutex<SubagentRuntime>>>,
    subagent_steer_review: Option<SubagentSteerReviewCallback>,
    state_writer: Option<Arc<dyn SandStateWriter>>,
    routine_auto_review: Option<RoutineAutoReviewCallback>,
    state_approval_barrier: Option<StateApprovalBarrier>,
    routine_post_write: Option<RoutinePostWriteCallback>,
    multitask_todo_state: Option<Arc<dyn MultitaskTodoState>>,
    cloud_agent_tool: Option<CloudAgentToolDependencies>,
    multitask_enabled: bool,
    spotlight_enabled: bool,
    action_audit: Option<RoutedMcpAuditConfig>,
    observation: Option<TurnObservationHandle>,
    projected_mcp_tools: Option<Arc<Vec<RoutedToolDefinition>>>,
    projected_mcp_meta_tools: Option<Arc<Vec<RoutedMcpMetaToolDefinition>>>,
    mcp_meta_enabled: bool,
    is_computer_use_subagent: bool,
    toolset_role: TurnToolsetRole,
    local_tool_permission: Option<TurnLocalToolPermissionBinding>,
    is_silence_allowed: bool,
    max_steps: usize,
    tool_loop_gate: Option<(ToolLoopGate, TurnOwnerToken)>,
}

impl TurnAgentComposition {
    pub fn new(
        provider: RoutedProvider,
        bridge: Arc<dyn RoutedToolBridge>,
        request_context: RunnerRequestContextSnapshot,
        cancellation: RoutedProviderCancellation,
        checkpoint_store: Arc<dyn RoutedProviderCheckpointStore>,
    ) -> Self {
        let latest_provider_checkpoint = Arc::new(Mutex::new(None));
        let checkpoint_store: Arc<dyn RoutedProviderCheckpointStore> =
            Arc::new(ObservedRoutedProviderCheckpointStore {
                delegate: checkpoint_store,
                latest: Arc::clone(&latest_provider_checkpoint),
            });
        Self {
            provider,
            bridge,
            cursor_auth: None,
            request_context,
            cancellation,
            checkpoint_store,
            latest_provider_checkpoint,
            completed_tool_calls: Arc::new(Mutex::new(Vec::new())),
            next_tool_sequence: Arc::new(AtomicU64::new(0)),
            retry_sink: None,
            retry_report_sink: None,
            stream_attempt_runtime: None,
            usage_sink: None,
            box_resources: None,
            box_shell_review: None,
            browser_executor: None,
            web_executor: None,
            generate_image_executor: None,
            computer_executor: None,
            computer_exposure: ComputerToolExposure::Full,
            file_transfer_executor: None,
            external_machine_executor: None,
            external_shell_review: None,
            mcp_management_sink: None,
            send_message_sink: None,
            reaction_sink: None,
            agent_management_sink: None,
            subagent_task_sink: None,
            subagent_task_allowed_types: None,
            subagent_task_review: None,
            subagent_runtime: None,
            subagent_steer_review: None,
            state_writer: None,
            routine_auto_review: None,
            state_approval_barrier: None,
            routine_post_write: None,
            multitask_todo_state: None,
            cloud_agent_tool: None,
            multitask_enabled: false,
            spotlight_enabled: false,
            action_audit: None,
            observation: None,
            projected_mcp_tools: None,
            projected_mcp_meta_tools: None,
            mcp_meta_enabled: false,
            is_computer_use_subagent: false,
            toolset_role: TurnToolsetRole::default(),
            local_tool_permission: None,
            is_silence_allowed: false,
            max_steps: SAND_AGENT_MAX_STEPS,
            tool_loop_gate: None,
        }
    }

    pub fn with_cursor_auth(
        mut self,
        cursor_auth: Arc<dyn CursorInferenceAuth>,
    ) -> Self {
        self.cursor_auth = Some(cursor_auth);
        self
    }

    pub fn with_silence_allowed(mut self, allowed: bool) -> Self {
        self.is_silence_allowed = allowed;
        self
    }

    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps.max(1);
        self
    }

    pub fn with_tool_loop_gate(
        mut self,
        gate: ToolLoopGate,
        owner: TurnOwnerToken,
    ) -> Self {
        self.tool_loop_gate = Some((gate, owner));
        self
    }

    pub fn max_steps(&self) -> usize {
        self.max_steps
    }

    pub fn with_retry_sink(
        mut self,
        sink: Arc<dyn Fn(&ProviderRetryEvent) + Send + Sync>,
    ) -> Self {
        self.retry_sink = Some(sink);
        self
    }

    pub fn with_retry_report_sink(
        mut self,
        sink: Arc<dyn Fn(&ProviderRetryReport) + Send + Sync>,
    ) -> Self {
        self.retry_report_sink = Some(sink);
        self
    }

    pub fn with_stream_attempt_runtime(
        mut self,
        runtime: Arc<StreamAttemptRuntime>,
    ) -> Self {
        self.stream_attempt_runtime = Some(runtime);
        self
    }

    pub fn with_usage_sink(
        mut self,
        sink: Arc<dyn Fn(ProviderTokenUsage) + Send + Sync>,
    ) -> Self {
        self.usage_sink = Some(sink);
        self
    }

    pub fn with_box_resources(
        mut self,
        box_resources: Arc<dyn RunnerBoxResourcePort>,
    ) -> Self {
        self.box_resources = Some(box_resources);
        self
    }

    pub fn has_box_resources(&self) -> bool {
        self.box_resources.is_some()
    }

    pub fn with_box_shell_review(mut self, review: BoxShellAutoReviewCallback) -> Self {
        self.box_shell_review = Some(review);
        self
    }

    pub fn has_box_shell_review(&self) -> bool {
        self.box_shell_review.is_some()
    }

    pub fn with_browser_executor(
        mut self,
        executor: Arc<dyn BrowserToolExecutor>,
    ) -> Self {
        self.browser_executor = Some(executor);
        self
    }

    pub fn has_browser_executor(&self) -> bool {
        self.browser_executor.is_some()
    }

    pub fn with_web_executor(mut self, executor: Arc<dyn WebToolExecutor>) -> Self {
        self.web_executor = Some(executor);
        self
    }

    pub fn has_web_executor(&self) -> bool {
        self.web_executor.is_some()
    }

    pub fn with_generate_image_executor(
        mut self,
        executor: Arc<dyn GenerateImageToolExecutor>,
    ) -> Self {
        self.generate_image_executor = Some(executor);
        self
    }

    pub fn with_computer_executor(
        mut self,
        executor: Arc<dyn ComputerToolExecutor>,
    ) -> Self {
        self.computer_executor = Some(executor);
        self
    }

    pub fn has_computer_executor(&self) -> bool {
        self.computer_executor.is_some()
    }

    pub fn with_computer_exposure(mut self, exposure: ComputerToolExposure) -> Self {
        self.computer_exposure = exposure;
        self.is_computer_use_subagent = exposure == ComputerToolExposure::Full;
        self.mcp_meta_enabled = exposure == ComputerToolExposure::ScreenshotOnly;
        self
    }

    pub fn with_toolset_role(mut self, role: TurnToolsetRole) -> Self {
        self.toolset_role = role;
        self
    }

    pub fn toolset_role(&self) -> TurnToolsetRole {
        self.toolset_role
    }

    pub fn with_local_tool_permission(
        mut self,
        binding: TurnLocalToolPermissionBinding,
    ) -> Self {
        self.local_tool_permission = Some(binding);
        self
    }


    pub fn with_file_transfer_executor(
        mut self,
        executor: Arc<dyn FileTransferExecutor>,
    ) -> Self {
        self.file_transfer_executor = Some(executor);
        self
    }

    pub fn has_file_transfer_executor(&self) -> bool {
        self.file_transfer_executor.is_some()
    }

    pub fn with_external_machine_executor(
        mut self,
        executor: Arc<dyn ExternalMachineExecutor>,
    ) -> Self {
        self.external_machine_executor = Some(executor);
        self
    }

    pub fn has_external_machine_executor(&self) -> bool {
        self.external_machine_executor.is_some()
    }

    pub fn with_external_shell_review(
        mut self,
        review: ExternalShellAutoReviewCallback,
    ) -> Self {
        self.external_shell_review = Some(review);
        self
    }

    pub fn has_external_shell_review(&self) -> bool {
        self.external_shell_review.is_some()
    }

    pub fn with_mcp_management_sink(
        mut self,
        sink: Arc<dyn McpManagementSink>,
    ) -> Self {
        self.mcp_management_sink = Some(sink);
        self
    }

    pub fn has_mcp_management_sink(&self) -> bool {
        self.mcp_management_sink.is_some()
    }

    pub fn with_send_message_sink(
        mut self,
        sink: Arc<dyn SendMessageSink>,
    ) -> Self {
        self.send_message_sink = Some(sink);
        self
    }

    pub fn has_send_message_sink(&self) -> bool {
        self.send_message_sink.is_some()
    }

    pub fn with_reaction_sink(
        mut self,
        sink: Arc<dyn ReactionSink>,
    ) -> Self {
        self.reaction_sink = Some(sink);
        self
    }

    pub fn has_reaction_sink(&self) -> bool {
        self.reaction_sink.is_some()
    }

    pub fn with_agent_management_sink(
        mut self,
        sink: Arc<dyn AgentManagementSink>,
    ) -> Self {
        self.agent_management_sink = Some(sink);
        self
    }

    pub fn has_agent_management_sink(&self) -> bool {
        self.agent_management_sink.is_some()
    }

    pub fn with_subagent_task_sink(mut self, sink: Arc<dyn SubagentTaskSink>) -> Self {
        self.subagent_task_sink = Some(sink);
        self
    }

    pub fn has_subagent_task_sink(&self) -> bool {
        self.subagent_task_sink.is_some()
    }

    pub fn with_subagent_task_allowed_types(mut self, allowed: Arc<Vec<String>>) -> Self {
        self.subagent_task_allowed_types = Some(allowed);
        self
    }

    pub fn with_subagent_task_review(mut self, review: SubagentTaskReviewCallback) -> Self {
        self.subagent_task_review = Some(review);
        self
    }

    pub fn with_subagent_management(
        mut self,
        runtime: Arc<Mutex<SubagentRuntime>>,
        review: Option<SubagentSteerReviewCallback>,
    ) -> Self {
        self.subagent_runtime = Some(runtime);
        self.subagent_steer_review = review;
        self
    }

    pub fn has_subagent_management(&self) -> bool {
        self.subagent_runtime.is_some()
    }

    pub fn with_state_writer(
        mut self,
        state: Arc<dyn SandStateWriter>,
    ) -> Self {
        self.state_writer = Some(state);
        self
    }

    pub fn has_state_writer(&self) -> bool {
        self.state_writer.is_some()
    }

    pub fn with_routine_auto_review(
        mut self,
        review: RoutineAutoReviewCallback,
    ) -> Self {
        self.routine_auto_review = Some(review);
        self
    }

    pub fn has_routine_auto_review(&self) -> bool {
        self.routine_auto_review.is_some()
    }

    pub fn with_state_approval_barrier(
        mut self,
        barrier: StateApprovalBarrier,
    ) -> Self {
        self.state_approval_barrier = Some(barrier);
        self
    }

    pub fn with_routine_post_write(
        mut self,
        callback: RoutinePostWriteCallback,
    ) -> Self {
        self.routine_post_write = Some(callback);
        self
    }

    pub fn has_routine_post_write(&self) -> bool {
        self.routine_post_write.is_some()
    }

    pub fn with_multitask_todo_state(
        mut self,
        state: Arc<dyn MultitaskTodoState>,
    ) -> Self {
        self.multitask_todo_state = Some(state);
        self
    }

    pub fn has_multitask_todo_state(&self) -> bool {
        self.multitask_todo_state.is_some()
    }

    pub fn with_cloud_agent_tool(
        mut self,
        deps: CloudAgentToolDependencies,
    ) -> Self {
        self.cloud_agent_tool = Some(deps);
        self
    }

    pub fn has_cloud_agent_tool(&self) -> bool {
        self.cloud_agent_tool.is_some()
    }

    pub fn with_multitask_enabled(mut self, enabled: bool) -> Self {
        self.multitask_enabled = enabled;
        self
    }

    pub fn has_multitask_enabled(&self) -> bool {
        self.multitask_enabled
    }

    pub fn with_spotlight_enabled(mut self, enabled: bool) -> Self {
        self.spotlight_enabled = enabled;
        self
    }

    pub fn has_spotlight_enabled(&self) -> bool {
        self.spotlight_enabled
    }

    pub fn with_action_audit(mut self, config: RoutedMcpAuditConfig) -> Self {
        self.action_audit = Some(config);
        self
    }

    pub fn has_action_audit(&self) -> bool {
        self.action_audit.is_some()
    }

    pub fn with_observation(
        mut self,
        observation: TurnObservationHandle,
    ) -> Self {
        self.observation = Some(observation);
        self
    }

    pub fn has_observation(&self) -> bool {
        self.observation.is_some()
    }

    /// Execute the canonical per-turn MCP state projection from the exact
    /// shipping routed-tool bridge already owned by this Runner composition.
    ///
    /// This is the Rust equivalent of Grok's mcpStateExecutorResource and
    /// deliberately avoids creating a second MCP discovery owner.
    pub fn execute_mcp_state(&self) -> Result<McpStateExecResult, String> {
        execute_canonical_mcp_state(&RoutedBridgeMcpToolProvider {
            bridge: self.bridge.as_ref(),
        })
    }

    pub fn snapshot_mcp_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        if let Some(projected_tools) = self.projected_mcp_tools.as_ref() {
            return Ok(projected_tools.as_ref().clone());
        }
        let McpStateExecResult::Success(state) = self.execute_mcp_state().map_err(|error| {
            ProviderSessionError::Tool(format!("MCP state projection failed: {error}"))
        })?;
        Ok(state
            .servers
            .into_iter()
            .flat_map(|server| server.tools)
            .map(|tool| RoutedToolDefinition {
                name: tool.name,
                provider_identifier: tool.provider_identifier,
                tool_name: tool.tool_name,
                description: tool.description,
                input_schema: tool.input_schema,
            })
            .collect())
    }

    pub fn snapshot_mcp_projection(
        &self,
    ) -> Result<
        (
            Vec<RoutedToolDefinition>,
            Vec<RoutedMcpMetaToolDefinition>,
        ),
        ProviderSessionError,
    > {
        if let (Some(projected_tools), Some(projected_meta_tools)) = (
            self.projected_mcp_tools.as_ref(),
            self.projected_mcp_meta_tools.as_ref(),
        ) {
            return Ok((
                projected_tools.as_ref().clone(),
                projected_meta_tools.as_ref().clone(),
            ));
        }

        let meta_tools = self.bridge.list_mcp_meta_tools()?;
        let McpStateExecResult::Success(state) =
            execute_canonical_mcp_state(&FrozenRoutedMcpToolProvider {
                tools: &meta_tools,
            })
            .map_err(|error| {
                ProviderSessionError::Tool(format!("MCP state projection failed: {error}"))
            })?;
        let tools = state
            .servers
            .into_iter()
            .flat_map(|server| server.tools)
            .map(|tool| RoutedToolDefinition {
                name: tool.name,
                provider_identifier: tool.provider_identifier,
                tool_name: tool.tool_name,
                description: tool.description,
                input_schema: tool.input_schema,
            })
            .collect();
        Ok((tools, meta_tools))
    }

    pub fn with_projected_mcp_tools(
        mut self,
        tools: Vec<RoutedToolDefinition>,
    ) -> Self {
        self.projected_mcp_tools = Some(Arc::new(tools));
        self
    }

    pub fn snapshot_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        if let Some(projected_tools) = self.projected_mcp_meta_tools.as_ref() {
            return Ok(projected_tools.as_ref().clone());
        }
        self.bridge.list_mcp_meta_tools()
    }

    pub fn with_projected_mcp_meta_tools(
        mut self,
        tools: Vec<RoutedMcpMetaToolDefinition>,
    ) -> Self {
        self.projected_mcp_meta_tools = Some(Arc::new(tools));
        self
    }

    pub fn provider(&self) -> RoutedProvider {
        self.provider
    }

    pub fn cancellation(&self) -> RoutedProviderCancellation {
        self.cancellation.clone()
    }

    pub fn last_run_ended_on_silent_tool_calls(&self, final_text: &str) -> bool {
        self.latest_provider_checkpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|checkpoint| {
                checkpoint_ended_on_silent_tool_calls(checkpoint, final_text)
            })
    }

    fn reset_latest_provider_checkpoint(&self) {
        *self
            .latest_provider_checkpoint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    fn reset_completed_tool_calls(&self) {
        self.completed_tool_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.next_tool_sequence.store(0, Ordering::Relaxed);
    }

    pub fn take_completed_tool_calls(&self) -> Vec<CompletedRoutedToolCall> {
        let mut calls = std::mem::take(
            &mut *self
                .completed_tool_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        calls.sort_by_key(|call| call.sequence);
        calls
    }

    pub fn run(
        &self,
        data_dir: &Path,
        messages: &[ProviderMessage],
        typed_replay: Option<&[TypedReplayEntry]>,
        on_text_delta: &mut dyn FnMut(&str, &str),
    ) -> Result<String, ProviderSessionError> {
        self.reset_latest_provider_checkpoint();
        self.reset_completed_tool_calls();
        let bridge = project_turn_mcp_toolset(
            Arc::clone(&self.bridge),
            self.projected_mcp_tools.clone(),
            self.projected_mcp_meta_tools.clone(),
            self.mcp_meta_enabled,
        )?;
        let bridge: Arc<dyn RoutedToolBridge> = match &self.observation {
            Some(observation) => Arc::new(McpObservedRoutedToolBridge::new(
                bridge,
                Arc::clone(observation),
            )),
            None => bridge,
        };
        let bridge: Arc<dyn RoutedToolBridge> = match &self.action_audit {
            Some(config) => Arc::new(AuditedRoutedToolBridge::new(
                bridge,
                config.clone(),
            )),
            None => bridge,
        };
        let send_message_interaction_sink: Option<Arc<dyn SendMessageInteractionSink>> =
            self.observation.as_ref().map(|observation| {
                Arc::new(TurnObservationSendMessageSink::new(Arc::clone(observation)))
                    as Arc<dyn SendMessageInteractionSink>
            });
        let bridge = build_turn_toolset(
            bridge,
            TurnToolsetDependencies {
                role: self.toolset_role,
                local_tool_permission: self.local_tool_permission.clone(),
                cancellation: self.cancellation.clone(),
                box_resources: self.box_resources.clone(),
                box_shell_review: self.box_shell_review.clone(),
                browser_executor: self.browser_executor.clone(),
                web_executor: self.web_executor.clone(),
                generate_image_executor: self.generate_image_executor.clone(),
                computer_executor: self.computer_executor.clone(),
                computer_exposure: self.computer_exposure,
                file_transfer_executor: self.file_transfer_executor.clone(),
                external_machine_executor: self.external_machine_executor.clone(),
                external_shell_review: self.external_shell_review.clone(),
                mcp_management_sink: self.mcp_management_sink.clone(),
                send_message_sink: self.send_message_sink.clone(),
                send_message_interaction_sink,
                reaction_sink: self.reaction_sink.clone(),
                agent_management_sink: self.agent_management_sink.clone(),
                subagent_task_sink: self.subagent_task_sink.clone(),
                subagent_task_allowed_types: self.subagent_task_allowed_types.clone(),
                subagent_task_review: self.subagent_task_review.clone(),
                subagent_runtime: self.subagent_runtime.clone(),
                subagent_steer_review: self.subagent_steer_review.clone(),
                state_writer: self.state_writer.clone(),
                routine_auto_review: self.routine_auto_review.clone(),
                state_approval_barrier: self.state_approval_barrier.clone(),
                routine_post_write: self.routine_post_write.clone(),
                multitask_enabled: self.multitask_enabled,
                multitask_todo_state: self.multitask_todo_state.clone(),
                cloud_agent_tool: self.cloud_agent_tool.clone(),
            },
        );
        let communicate_sink: Option<Arc<dyn CommunicateInteractionSink>> =
            self.observation.as_ref().map(|observation| {
                Arc::new(TurnObservationCommunicateSink::new(Arc::clone(observation)))
                    as Arc<dyn CommunicateInteractionSink>
            });
        let bridge: Arc<dyn RoutedToolBridge> =
            Arc::new(CommunicateRoutedToolBridge::new(bridge, communicate_sink));
        let bridge: Arc<dyn RoutedToolBridge> = match &self.observation {
            Some(observation) => Arc::new(ObservedRoutedToolBridge::new(
                bridge,
                Arc::clone(observation),
            )),
            None => bridge,
        };
        let bridge = fence_turn_toolset(bridge, self.spotlight_enabled);
        let bridge: Arc<dyn RoutedToolBridge> = Arc::new(
            CheckpointRecordingRoutedToolBridge::new(
                bridge,
                Arc::clone(&self.completed_tool_calls),
                Arc::clone(&self.next_tool_sequence),
                self.tool_loop_gate.clone(),
                self.cancellation.clone(),
            ),
        );
        let tool_step_reminder = if self.toolset_role.is_subagent_runner || self.is_silence_allowed {
            None
        } else {
            let state = Arc::new(Mutex::new(TurnToolSessionReminderState::default()));
            let hook_state = Arc::clone(&state);
            let hook: Arc<ProviderToolStepReminderHook> = Arc::new(move |calls| {
                hook_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .observe_tool_step(calls)
            });
            Some(hook)
        };
        run_routed_provider_in_runner(
            RoutedProviderRun {
                provider: self.provider,
                data_dir,
                messages,
                typed_replay,
                bridge,
                cursor_auth: self.cursor_auth.clone(),
                request_context: self.request_context.clone(),
                cancellation: self.cancellation.clone(),
                checkpoint_store: Arc::clone(&self.checkpoint_store),
                retry_sink: self.retry_sink.clone(),
                retry_report_sink: self.retry_report_sink.clone(),
                stream_attempt_runtime: self.stream_attempt_runtime.clone(),
                usage_sink: self.usage_sink.clone(),
                cloud_agents_enabled: self.cloud_agent_tool.is_some(),
                multitask_enabled: self.multitask_enabled,
                is_computer_use_subagent: self.is_computer_use_subagent,
                max_steps: self.max_steps,
                tool_step_reminder,
            },
            on_text_delta,
        )
    }
}
