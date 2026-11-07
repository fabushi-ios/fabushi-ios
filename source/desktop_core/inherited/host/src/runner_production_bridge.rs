use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::extensions::inference::cursor_inference_transport::CursorInferenceAuth;
use crate::extensions::inference::provider_session::{ProviderTokenUsage, RoutedProvider};
use crate::cloud_agents::cloud_agent_tool::CloudAgentToolDependencies;
use crate::runner::box_tool_access::{BoxShellAutoReviewCallback, RunnerBoxResourcePort};
use crate::runner::production_agent_checkpoint::AgentStateCheckpointSink;
use crate::runner::production_turn_agent_owner::{
    ProductionTurnAgentBuildBindings, ProductionTurnAgentLifecycleBindings,
    ProductionTurnAgentOwner,
};
use crate::runner::background_work::{RunnerBackgroundShellWatches, RunnerCloudAgentWatches};
use crate::runner::computer_use::ComputerUseCoordination;
use crate::runner::sand_agent_runner::SandAgentRunner;
use crate::runner::subagent_runtime::SubagentRuntime;
use crate::runner::production_turn_run_shell_adapter::{
    ProviderRetryEvent, ProviderRetryReport, RoutedProviderCheckpointStore,
};
use crate::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge, RunnerRequestContextSnapshot,
};
use crate::runner::sand_action_audit::{ActionAuditSink, RoutedMcpAuditConfig};
use crate::runner::tools::sand_reaction_tool::ReactionSink;
use crate::runner::tools::sand_agent_management_tools::AgentManagementSink;
use crate::runner::tools::sand_multitask_todo_tool::MultitaskTodoState;
use crate::runner::tools::sand_state_tool::{
    RoutineAutoReviewCallback, RoutinePostWriteCallback, SandStateWriter,
    StateApprovalBarrier,
};
use crate::runner::tools::sand_subagent_management_tools::SubagentSteerReviewCallback;
use crate::runner::tools::sand_task_subagent_tool::{SubagentTaskReviewCallback, SubagentTaskSink};
use crate::runner::tools::sand_browser_tools::BrowserToolExecutor;
use crate::runner::tools::sand_web_tools::WebToolExecutor;
use crate::runner::tools::sand_generate_image_tool::GenerateImageToolExecutor;
use crate::runner::tools::sand_computer_tool::{ComputerToolExecutor, ComputerToolExposure};
use crate::runner::tools::sand_file_transfer_tools::FileTransferExecutor;
use crate::runner::tools::sand_external_machine_tools::{
    ExternalMachineExecutor, ExternalShellAutoReviewCallback,
};
use crate::runner::tools::sand_mcp_management_tools::McpManagementSink;
use crate::runner::tools::send_message_tool::SendMessageSink;
use crate::runner::tools::turn_toolset::{TurnLocalToolPermissionBinding, TurnToolsetRole};
use crate::runner::turn_agent_composition::TurnAgentComposition;
use crate::runner::turn_observation::TurnObservationHandle;

/// Typed production half of the frozen Host -> Runner turn bridge.
///
/// The Host owns concrete platform/session services. The Runner bridge owns
/// one immutable per-turn dependency projection and creates the Runner
/// composition. This keeps provider/cancellation/checkpoint/tool identities
/// out of ad-hoc Host assembly while the full generated Agent turn engine is
/// still being ported behind the same boundary.
pub struct ProductionActionAuditInput {
    pub agent_id: String,
    pub turn_id: Option<String>,
    pub sink: Arc<dyn ActionAuditSink>,
}

/// Host-resolved, per-turn dependencies that the frozen production bridge owns
/// as one immutable projection into the Runner. Concrete services retain their
/// existing owners; this struct only prevents a second decoration path after
/// the bridge has created the turn composition.
pub struct ProductionRunnerCompositionHooks {
    pub agent_management_sink: Arc<dyn AgentManagementSink>,
    pub state_writer: Option<Arc<dyn SandStateWriter>>,
    pub routine_auto_review: RoutineAutoReviewCallback,
    pub state_approval_barrier: StateApprovalBarrier,
    pub box_shell_review: BoxShellAutoReviewCallback,
    pub subagent_task_sink: Option<Arc<dyn SubagentTaskSink>>,
    pub subagent_task_allowed_types: Option<Arc<Vec<String>>>,
    pub subagent_task_review: Option<SubagentTaskReviewCallback>,
    pub subagent_management_runtime: Option<Arc<Mutex<SubagentRuntime>>>,
    pub subagent_steer_review: Option<SubagentSteerReviewCallback>,
    pub routine_post_write: Option<RoutinePostWriteCallback>,
    pub multitask_todo_state: Option<Arc<dyn MultitaskTodoState>>,
}

pub struct ProductionRunnerCompositionInput {
    pub provider: RoutedProvider,
    pub bridge: Arc<dyn RoutedToolBridge>,
    pub cursor_auth: Option<Arc<dyn CursorInferenceAuth>>,
    pub request_context: RunnerRequestContextSnapshot,
    pub cancellation: RoutedProviderCancellation,
    pub checkpoint_store: Arc<dyn RoutedProviderCheckpointStore>,
    pub retry_sink: Option<Arc<dyn Fn(&ProviderRetryEvent) + Send + Sync>>,
    pub retry_report_sink: Option<Arc<dyn Fn(&ProviderRetryReport) + Send + Sync>>,
    pub usage_sink: Option<Arc<dyn Fn(ProviderTokenUsage) + Send + Sync>>,
    pub box_resources: Option<Arc<dyn RunnerBoxResourcePort>>,
    pub browser_executor: Option<Arc<dyn BrowserToolExecutor>>,
    pub web_executor: Option<Arc<dyn WebToolExecutor>>,
    pub generate_image_executor: Option<Arc<dyn GenerateImageToolExecutor>>,
    pub computer_executor: Option<Arc<dyn ComputerToolExecutor>>,
    pub computer_exposure: ComputerToolExposure,
    pub file_transfer_executor: Option<Arc<dyn FileTransferExecutor>>,
    pub external_machine_executor: Option<Arc<dyn ExternalMachineExecutor>>,
    pub external_shell_review: Option<ExternalShellAutoReviewCallback>,
    pub mcp_management_sink: Option<Arc<dyn McpManagementSink>>,
    pub send_message_sink: Option<Arc<dyn SendMessageSink>>,
    pub reaction_sink: Option<Arc<dyn ReactionSink>>,
    pub cloud_agent_tool: Option<CloudAgentToolDependencies>,
    pub multitask_enabled: bool,
    pub spotlight_enabled: bool,
    pub toolset_role: TurnToolsetRole,
    pub local_tool_permission: Option<TurnLocalToolPermissionBinding>,
    pub action_audit: Option<ProductionActionAuditInput>,
    pub observation: Option<TurnObservationHandle>,
}

pub fn create_production_runner_composition(
    input: ProductionRunnerCompositionInput,
) -> TurnAgentComposition {
    let mut composition = TurnAgentComposition::new(
        input.provider,
        input.bridge,
        input.request_context,
        input.cancellation,
        input.checkpoint_store,
    );
    if let Some(cursor_auth) = input.cursor_auth {
        composition = composition.with_cursor_auth(cursor_auth);
    }
    if let Some(retry_sink) = input.retry_sink {
        composition = composition.with_retry_sink(retry_sink);
    }
    if let Some(retry_report_sink) = input.retry_report_sink {
        composition = composition.with_retry_report_sink(retry_report_sink);
    }
    if let Some(usage_sink) = input.usage_sink {
        composition = composition.with_usage_sink(usage_sink);
    }
    composition = composition
        .with_multitask_enabled(input.multitask_enabled)
        .with_spotlight_enabled(input.spotlight_enabled)
        .with_toolset_role(input.toolset_role);
    if let Some(local_tool_permission) = input.local_tool_permission {
        composition = composition.with_local_tool_permission(local_tool_permission);
    }
    if let Some(action_audit) = input.action_audit {
        composition = composition.with_action_audit(RoutedMcpAuditConfig::new(
            action_audit.agent_id,
            action_audit.turn_id,
            action_audit.sink,
        ));
    }
    if let Some(box_resources) = input.box_resources {
        composition = composition.with_box_resources(box_resources);
    }
    if let Some(browser_executor) = input.browser_executor {
        composition = composition.with_browser_executor(browser_executor);
    }
    if let Some(web_executor) = input.web_executor {
        composition = composition.with_web_executor(web_executor);
    }
    if let Some(generate_image_executor) = input.generate_image_executor {
        composition = composition.with_generate_image_executor(generate_image_executor);
    }
    composition = composition.with_computer_exposure(input.computer_exposure);
    if let Some(computer_executor) = input.computer_executor {
        composition = composition.with_computer_executor(computer_executor);
    }
    if let Some(file_transfer_executor) = input.file_transfer_executor {
        composition = composition.with_file_transfer_executor(file_transfer_executor);
    }
    if let Some(external_machine_executor) = input.external_machine_executor {
        composition = composition.with_external_machine_executor(external_machine_executor);
    }
    if let Some(external_shell_review) = input.external_shell_review {
        composition = composition.with_external_shell_review(external_shell_review);
    }
    if let Some(mcp_management_sink) = input.mcp_management_sink {
        composition = composition.with_mcp_management_sink(mcp_management_sink);
    }
    if let Some(reaction_sink) = input.reaction_sink {
        composition = composition.with_reaction_sink(reaction_sink);
    }
    if let Some(cloud_agent_tool) = input.cloud_agent_tool {
        composition = composition.with_cloud_agent_tool(cloud_agent_tool);
    }
    if let Some(send_message_sink) = input.send_message_sink {
        composition = composition.with_send_message_sink(send_message_sink);
    }
    if let Some(observation) = input.observation {
        composition = composition.with_observation(observation);
    }
    composition
}

/// Complete the frozen immutable per-turn projection in the bridge itself.
/// HostRunnerComposition may resolve concrete services, but it must not decorate
/// TurnAgentComposition after this boundary returns.
pub fn create_production_runner_composition_with_hooks(
    input: ProductionRunnerCompositionInput,
    hooks: ProductionRunnerCompositionHooks,
) -> TurnAgentComposition {
    let mut composition = create_production_runner_composition(input)
        .with_agent_management_sink(hooks.agent_management_sink)
        .with_routine_auto_review(hooks.routine_auto_review)
        .with_state_approval_barrier(hooks.state_approval_barrier)
        .with_box_shell_review(hooks.box_shell_review);

    if let Some(state_writer) = hooks.state_writer {
        composition = composition.with_state_writer(state_writer);
    }
    if let Some(subagent_task_sink) = hooks.subagent_task_sink {
        composition = composition.with_subagent_task_sink(subagent_task_sink);
        if let Some(allowed) = hooks.subagent_task_allowed_types {
            composition = composition.with_subagent_task_allowed_types(allowed);
        }
        if let Some(subagent_task_review) = hooks.subagent_task_review {
            composition = composition.with_subagent_task_review(subagent_task_review);
        }
    }
    if let Some(subagent_runtime) = hooks.subagent_management_runtime {
        composition = composition.with_subagent_management(
            subagent_runtime,
            hooks.subagent_steer_review,
        );
    }
    if let Some(routine_post_write) = hooks.routine_post_write {
        composition = composition.with_routine_post_write(routine_post_write);
    }
    if let Some(multitask_todo_state) = hooks.multitask_todo_state {
        composition = composition.with_multitask_todo_state(multitask_todo_state);
    }

    composition
}

/// Mandatory production Runner binding for the generated turn engine.
/// The bridge owns the one-time binding; ProductionTurnAgentOwner continues to
/// own turn lifecycle/settlement and SandAgentRunner remains the Runner facade.
pub fn create_production_runner(
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
    let mut owner = ProductionTurnAgentOwner::new(composition)
        .with_build_bindings(build_bindings)
        .with_lifecycle_bindings(lifecycle_bindings)
        .with_upgrade_quiesce_signal(upgrade_quiesce_signal);
    if let Some(checkpoint_sink) = checkpoint_sink {
        owner = owner.with_agent_state_checkpoint_sink(checkpoint_sink);
    }
    SandAgentRunner::new(owner)
        .with_generated_agent_runtime(generated_agent_runtime)
        .with_runtime_services(
            observation,
            cloud_agent_watches,
            background_shell_watches,
            computer_use,
        )
}
