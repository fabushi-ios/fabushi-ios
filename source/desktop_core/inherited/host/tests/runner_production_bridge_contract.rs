use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::cursor_backend::SandPrivacyMode;
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedProvider, RoutedProviderCheckpoint,
    RoutedToolDefinition,
};
use mahayana_host_runtime::host_request_context::HostRequestContext;
use mahayana_host_runtime::runner::production_turn_agent_owner::{
    ProductionTurnAgentBuildBindings, ProductionTurnAgentLifecycleBindings,
    ProductionTurnAgentStaticConfig, ProductionTurnPrivacyModeResolver,
    ProductionTurnSummarizationPrompt,
};
use mahayana_host_runtime::runner::production_turn_run_shell_adapter::RoutedProviderCheckpointStore;
use mahayana_host_runtime::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge, RunnerRequestContextSnapshot,
};
use mahayana_host_runtime::runner::subagent_runtime::SubagentRuntime;
use mahayana_host_runtime::runner::turn_observation::TurnObservation;
use mahayana_host_runtime::agents::agent_messaging::AgentMessageImage;
use mahayana_host_runtime::runner::tools::sand_agent_management_tools::{
    AgentManagementRecord, AgentManagementSink,
};
use mahayana_host_runtime::runner::tools::sand_computer_tool::ComputerToolExposure;
use mahayana_host_runtime::runner::tools::sand_mcp_management_tools::{
    McpAuthenticationResult, McpInstalledServer, McpManagementSink, McpPluginDetail,
    McpPluginSummary, McpRemoveServerResult, McpUninstallPluginResult,
};
use mahayana_host_runtime::runner_production_bridge::{
    ProductionRunnerCompositionHooks, ProductionRunnerCompositionInput,
    create_production_runner, create_production_runner_composition,
    create_production_runner_composition_with_hooks,
};

const PRODUCTION_BRIDGE: &str = include_str!("../src/runner_production_bridge.rs");
const HOST_COMPOSITION: &str = include_str!("../src/host_runner_composition.rs");
const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
use mahayana_host_runtime::ports::mcp_state_executor::McpStateExecResult;
use serde_json::{Value, json};

struct EmptyBridge;

impl RoutedToolBridge for EmptyBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("no tools".into()))
    }
}

struct McpStateBridge;

impl RoutedToolBridge for McpStateBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![
            RoutedToolDefinition {
                name: "github_search".into(),
                provider_identifier: "github".into(),
                tool_name: "search".into(),
                description: Some("Search GitHub".into()),
                input_schema: json!({"type":"object","properties":{"q":{"type":"string"}}}),
            },
            RoutedToolDefinition {
                name: "linear_get".into(),
                provider_identifier: "linear".into(),
                tool_name: "get".into(),
                description: Some("Get Linear issue".into()),
                input_schema: json!({"type":"object","properties":{"id":{"type":"string"}}}),
            },
            RoutedToolDefinition {
                name: "github_issue".into(),
                provider_identifier: "github".into(),
                tool_name: "issue".into(),
                description: None,
                input_schema: json!({"type":"object"}),
            },
        ])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("not used by MCP state projection".into()))
    }
}

struct EmptyAgentManagementSink;

impl AgentManagementSink for EmptyAgentManagementSink {
    fn self_agent_id(&self) -> &str {
        "agent-contract"
    }

    fn send_to_agent(
        &self,
        _target_id: &str,
        _message: &str,
        _images: &[AgentMessageImage],
        _priority: bool,
    ) -> Result<String, ProviderSessionError> {
        Err(ProviderSessionError::Tool("not exercised".into()))
    }

    fn create_agent(
        &self,
        _name: &str,
        _description: &str,
    ) -> Result<AgentManagementRecord, ProviderSessionError> {
        Err(ProviderSessionError::Tool("not exercised".into()))
    }

    fn update_agent(
        &self,
        _agent_id: &str,
        _name: Option<&str>,
        _description: Option<&str>,
    ) -> Result<Option<AgentManagementRecord>, ProviderSessionError> {
        Err(ProviderSessionError::Tool("not exercised".into()))
    }
}

struct EmptyMcpManagementSink;

impl McpManagementSink for EmptyMcpManagementSink {
    fn list_plugins(&self) -> Result<Vec<McpPluginSummary>, ProviderSessionError> { Ok(Vec::new()) }
    fn get_plugin(&self, _plugin_id: &str) -> Result<Option<McpPluginDetail>, ProviderSessionError> { Ok(None) }
    fn install_plugin(&self, _plugin_id: &str, _values: Option<&BTreeMap<String, String>>) -> Result<(), ProviderSessionError> { Ok(()) }
    fn add_server(&self, _name: &str, _config_json: &str) -> Result<Vec<McpInstalledServer>, ProviderSessionError> { Ok(Vec::new()) }
    fn list_installed(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError> { Ok(Vec::new()) }
    fn remove_server(&self, _server_id: &str) -> Result<McpRemoveServerResult, ProviderSessionError> {
        Ok(McpRemoveServerResult { removed: false, reason: None, servers: Vec::new() })
    }
    fn uninstall_plugin(&self, _plugin_id: &str) -> Result<McpUninstallPluginResult, ProviderSessionError> {
        Ok(McpUninstallPluginResult { removed: false, reason: None })
    }
    fn set_instructions(&self, _server_id: &str, _instructions: &str) -> Result<Vec<McpInstalledServer>, ProviderSessionError> { Ok(Vec::new()) }
    fn restart(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError> { Ok(Vec::new()) }
    fn authenticate(
        &self,
        _server_id: &str,
        _account_key: &str,
        _requesting_agent_id: Option<&str>,
        _force_reauth: bool,
    ) -> Result<McpAuthenticationResult, ProviderSessionError> {
        Ok(McpAuthenticationResult::NotConfigured { server_name: "test".into() })
    }
    fn remove_account(&self, _server_id: &str, _account_key: &str) -> Result<Vec<McpInstalledServer>, ProviderSessionError> { Ok(Vec::new()) }
    fn rename_account(&self, _server_id: &str, _account_key: &str, _new_account_key: &str) -> Result<Vec<McpInstalledServer>, ProviderSessionError> { Ok(Vec::new()) }
}

struct MemoryCheckpointStore;

impl RoutedProviderCheckpointStore for MemoryCheckpointStore {
    fn persist(
        &self,
        _checkpoint: &RoutedProviderCheckpoint,
    ) -> Result<String, ProviderSessionError> {
        Ok("checkpoint".into())
    }
}

fn request_context() -> RunnerRequestContextSnapshot {
    RunnerRequestContextSnapshot {
        context: HostRequestContext {
            os_version: "test".into(),
            shell: None,
            time_zone: Some("UTC".into()),
            transcripts_folder: "/tmp/transcripts".into(),
            user_full_name: None,
        },
        rules: None,
    }
}

#[test]
fn production_bridge_preserves_provider_and_cancellation_identity() {
    let cancellation = RoutedProviderCancellation::default();
    let composition = create_production_runner_composition(
        ProductionRunnerCompositionInput {
            provider: RoutedProvider::OpenRouter,
            bridge: Arc::new(EmptyBridge),
            cursor_auth: None,
            request_context: request_context(),
            cancellation: cancellation.clone(),
            checkpoint_store: Arc::new(MemoryCheckpointStore),
            retry_sink: None,
            retry_report_sink: None,
            usage_sink: None,
            spotlight_enabled: true,
            toolset_role: Default::default(),
            local_tool_permission: None,
            box_resources: None,
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
            cloud_agent_tool: None,
            multitask_enabled: true,
            action_audit: None,
            observation: None,
        },
    );

    assert_eq!(composition.provider(), RoutedProvider::OpenRouter);
    assert!(!cancellation.is_cancelled());
    assert!(composition.cancellation().cancel("contract"));
    assert!(cancellation.is_cancelled());
    assert_eq!(cancellation.reason().as_deref(), Some("contract"));
    assert!(!composition.has_box_resources());
    assert!(!composition.has_browser_executor());
    assert!(!composition.has_computer_executor());
    assert!(!composition.has_external_machine_executor());
    assert!(!composition.has_external_shell_review());
    assert!(!composition.has_mcp_management_sink());
    assert!(!composition.has_send_message_sink());
    assert!(!composition.has_reaction_sink());
    assert!(!composition.has_cloud_agent_tool());
    assert!(composition.has_multitask_enabled());
    assert!(!composition.has_action_audit());
    assert!(composition.has_spotlight_enabled());
}


#[test]
fn production_bridge_projects_mcp_management_into_canonical_turn_composition() {
    let composition = create_production_runner_composition(
        ProductionRunnerCompositionInput {
            provider: RoutedProvider::OpenRouter,
            bridge: Arc::new(EmptyBridge),
            cursor_auth: None,
            request_context: request_context(),
            cancellation: RoutedProviderCancellation::default(),
            checkpoint_store: Arc::new(MemoryCheckpointStore),
            retry_sink: None,
            retry_report_sink: None,
            usage_sink: None,
            spotlight_enabled: false,
            toolset_role: Default::default(),
            local_tool_permission: None,
            box_resources: None,
            browser_executor: None,
            web_executor: None,
            generate_image_executor: None,
            computer_executor: None,
            computer_exposure: ComputerToolExposure::Full,
            file_transfer_executor: None,
            external_machine_executor: None,
            external_shell_review: None,
            mcp_management_sink: Some(Arc::new(EmptyMcpManagementSink)),
            send_message_sink: None,
            reaction_sink: None,
            cloud_agent_tool: None,
            multitask_enabled: false,
            action_audit: None,
            observation: None,
        },
    );

    assert!(composition.has_mcp_management_sink());
}


#[test]
fn production_bridge_registers_canonical_mcp_state_executor_on_turn_composition() {
    let composition = create_production_runner_composition(
        ProductionRunnerCompositionInput {
            provider: RoutedProvider::OpenRouter,
            bridge: Arc::new(McpStateBridge),
            cursor_auth: None,
            request_context: request_context(),
            cancellation: RoutedProviderCancellation::default(),
            checkpoint_store: Arc::new(MemoryCheckpointStore),
            retry_sink: None,
            retry_report_sink: None,
            usage_sink: None,
            spotlight_enabled: false,
            toolset_role: Default::default(),
            local_tool_permission: None,
            box_resources: None,
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
            cloud_agent_tool: None,
            multitask_enabled: false,
            action_audit: None,
            observation: None,
        },
    );

    let McpStateExecResult::Success(state) = composition
        .execute_mcp_state()
        .expect("production turn MCP state projection");
    assert_eq!(state.servers.len(), 2);
    assert_eq!(state.servers[0].server_identifier, "github");
    assert_eq!(state.servers[0].tools.len(), 2);
    assert_eq!(state.servers[0].tools[0].name, "github_search");
    assert_eq!(
        state.servers[0].tools[0].input_schema["properties"]["q"]["type"],
        "string"
    );
    assert_eq!(state.servers[1].server_identifier, "linear");
    assert_eq!(state.servers[1].tools.len(), 1);
}

#[test]
fn production_bridge_owns_immutable_turn_projection_and_generated_runner_binding() {
    for needle in [
        "pub struct ProductionRunnerCompositionHooks",
        "pub fn create_production_runner_composition_with_hooks(",
        ".with_agent_management_sink(hooks.agent_management_sink)",
        ".with_routine_auto_review(hooks.routine_auto_review)",
        ".with_state_approval_barrier(hooks.state_approval_barrier)",
        ".with_box_shell_review(hooks.box_shell_review)",
        "composition = composition.with_state_writer(state_writer)",
        "composition = composition.with_subagent_task_sink(subagent_task_sink)",
        "composition.with_subagent_management(",
        "composition = composition.with_routine_post_write(routine_post_write)",
        "composition = composition.with_multitask_todo_state(multitask_todo_state)",
        "pub fn create_production_runner(",
        "ProductionTurnAgentOwner::new(composition)",
        ".with_build_bindings(build_bindings)",
        ".with_lifecycle_bindings(lifecycle_bindings)",
        "owner = owner.with_agent_state_checkpoint_sink(checkpoint_sink)",
        "SandAgentRunner::new(owner)",
        ".with_generated_agent_runtime(generated_agent_runtime)",
        ".with_runtime_services(",
    ] {
        assert!(
            PRODUCTION_BRIDGE.contains(needle),
            "production bridge missing frozen ownership: {needle}"
        );
    }

    assert!(
        HOST_COMPOSITION.contains("create_production_runner_composition_with_hooks(input, hooks)")
            && HOST_COMPOSITION.contains("create_production_runner("),
        "HostRunnerComposition must delegate immutable turn and Runner binding to the bridge",
    );
    for forbidden in [
        ".with_agent_management_sink(hooks.agent_management_sink)",
        ".with_routine_auto_review(hooks.routine_auto_review)",
        "ProductionTurnAgentOwner::new(composition)",
        "SandAgentRunner::new(owner)",
    ] {
        assert!(
            !HOST_COMPOSITION.contains(forbidden),
            "HostRunnerComposition still duplicates bridge ownership: {forbidden}",
        );
    }
    assert!(
        SHIPPING_HOST.contains("worker_host_runner_composition.compose_production_turn(")
            && SHIPPING_HOST.contains("worker_host_runner_composition.compose_production_runner(")
            && SHIPPING_HOST.contains("ProductionTurnAgentBuildBindings::new(")
            && SHIPPING_HOST.contains("ProductionTurnAgentStaticConfig {")
            && SHIPPING_HOST.contains("turn_input.ack_token = ack_token.clone()")
            && SHIPPING_HOST.contains("run_routed_provider_with_projected_turn_input(")
            && SHIPPING_HOST.contains("resolve_sand_privacy_mode(")
            && SHIPPING_HOST.contains("ProductionTurnAgentLifecycleBindings::new(")
            && SHIPPING_HOST.contains(".disk_pressure_reminder_episodes()")
            && SHIPPING_HOST.contains("Some(Arc::clone(&observation))")
            && SHIPPING_HOST.contains("Some(Arc::clone(&worker_cloud_agent_watches))")
            && SHIPPING_HOST.contains("Some(Arc::clone(&background_shell_watches))")
            && SHIPPING_HOST.contains("Some(worker_host_runner_composition.computer_use_coordination())")
            && !SHIPPING_HOST.contains("create_production_runner_composition_with_hooks(")
            && !SHIPPING_HOST.contains("create_production_runner("),
        "shipping Host must retain one HostRunnerComposition entrypoint into the bridge",
    );
}

#[test]
fn production_bridge_projects_host_resolved_turn_hooks_behaviorally() {
    let composition = create_production_runner_composition_with_hooks(
        ProductionRunnerCompositionInput {
            provider: RoutedProvider::OpenRouter,
            bridge: Arc::new(EmptyBridge),
            cursor_auth: None,
            request_context: request_context(),
            cancellation: RoutedProviderCancellation::default(),
            checkpoint_store: Arc::new(MemoryCheckpointStore),
            retry_sink: None,
            retry_report_sink: None,
            usage_sink: None,
            spotlight_enabled: false,
            toolset_role: Default::default(),
            local_tool_permission: None,
            box_resources: None,
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
            cloud_agent_tool: None,
            multitask_enabled: false,
            action_audit: None,
            observation: None,
        },
        ProductionRunnerCompositionHooks {
            agent_management_sink: Arc::new(EmptyAgentManagementSink),
            state_writer: None,
            routine_auto_review: Arc::new(|_, _| Ok(None)),
            state_approval_barrier: Arc::new(|| Ok(())),
            box_shell_review: Arc::new(|_| Ok(None)),
            subagent_task_sink: None,
            subagent_task_allowed_types: None,
            subagent_task_review: None,
            subagent_management_runtime: None,
            subagent_steer_review: None,
            routine_post_write: None,
            multitask_todo_state: None,
        },
    );

    assert!(composition.has_agent_management_sink());
    assert!(composition.has_routine_auto_review());
    assert!(composition.has_box_shell_review());
}

#[test]
fn production_bridge_binds_the_generated_runner_facade() {
    let composition = create_production_runner_composition(
        ProductionRunnerCompositionInput {
            provider: RoutedProvider::OpenRouter,
            bridge: Arc::new(EmptyBridge),
            cursor_auth: None,
            request_context: request_context(),
            cancellation: RoutedProviderCancellation::default(),
            checkpoint_store: Arc::new(MemoryCheckpointStore),
            retry_sink: None,
            retry_report_sink: None,
            usage_sink: None,
            spotlight_enabled: false,
            toolset_role: Default::default(),
            local_tool_permission: None,
            box_resources: None,
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
            cloud_agent_tool: None,
            multitask_enabled: false,
            action_audit: None,
            observation: None,
        },
    );
    let runtime = Arc::new(Mutex::new(SubagentRuntime::default()));
    let summarization: ProductionTurnSummarizationPrompt =
        Arc::new(|_, _, _| Ok("summary".into()));
    let privacy_mode_resolver: ProductionTurnPrivacyModeResolver =
        Arc::new(|| Some(SandPrivacyMode::NoTraining));
    let build_bindings = ProductionTurnAgentBuildBindings::new(
        ProductionTurnAgentStaticConfig {
            model_id: "grok-contract".into(),
            agent_token_limit: 200_000,
            conversation_id: "bridge-agent".into(),
            is_box_scoped_subagent: false,
            is_subagent_runner: false,
            is_shared_room_runner: false,
            sand_send_message_delivery_owed: true,
            transcripts_folder_available: true,
        },
        privacy_mode_resolver,
        summarization,
    );
    let lifecycle_bindings = ProductionTurnAgentLifecycleBindings::new(
        "bridge-agent",
        "bridge-request",
    );
    let observation = TurnObservation::shared("bridge-agent", None);
    let runner = create_production_runner(
        composition,
        build_bindings,
        lifecycle_bindings,
        None,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::clone(&runtime),
        Some(Arc::clone(&observation)),
        None,
        None,
        None,
    );

    assert!(runner.generated_agent_runtime().is_some());
    let projected_runtime = runner
        .generated_agent_runtime()
        .expect("generated runtime");
    assert!(Arc::ptr_eq(&projected_runtime, &runtime));
    assert!(runner.list_async_tasks().expect("async tasks").is_empty());
    assert!(runner.get_activity_snapshot().expect("activity").is_empty());
    assert_eq!(
        runner
            .get_observed_tool_call_count()
            .expect("observed tool calls"),
        0
    );
    assert!(!runner.is_quiescing_for_upgrade());
    runner.request_quiesce_for_upgrade();
    assert!(runner.is_quiescing_for_upgrade());
    runner.cancel_quiesce_for_upgrade();
    assert!(!runner.is_quiescing_for_upgrade());
    assert!(!runner.is_awaiting_user_selection());
}
