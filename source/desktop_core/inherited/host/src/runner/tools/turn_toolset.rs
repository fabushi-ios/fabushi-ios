use std::sync::{Arc, Mutex};

use crate::cloud_agents::cloud_agent_tool::{
    CLOUD_AGENT_TOOL_NAME, CloudAgentToolBridge, CloudAgentToolDependencies,
};
use crate::runner::box_tool_access::{
    BoxShellAutoReviewCallback, RUNNER_BOX_READ_TOOL_NAME, RUNNER_BOX_SHELL_TOOL_NAME,
    RunnerBoxResourcePort, RunnerBoxToolBridge,
};
use crate::sand_activity::{
    SAND_BOX_AWAIT_SHELL_TOOL_NAME, SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME,
    SAND_EXTERNAL_READ_TOOL_NAME, SAND_EXTERNAL_SHELL_TOOL_NAME,
};
use crate::extensions::local_tool_permission::local_tool_permission_controller::{
    SandLocalToolPermissionController, SandLocalToolScope,
};
use crate::extensions::inference::provider_session::{
    ProviderPartialToolCall, ProviderSessionError, RoutedMcpMetaToolDefinition,
    RoutedToolDefinition,
};
use crate::ports::mcp_state_executor::{
    McpStateExecResult, SandMcpToolProvider, execute_mcp_state as execute_canonical_mcp_state,
};
use crate::runner::routed_provider_runtime::{
    RoutedProviderCancellation, RoutedToolBridge,
};
use crate::runner::subagent_runtime::SubagentRuntime;

use super::box_help_tool::{BoxHelpToolBridge, SAND_REQUEST_BOX_HELP_TOOL_NAME};
use super::mcp_meta_tools::{
    CALL_MCP_TOOL_NAME, GET_MCP_TOOLS_TOOL_NAME, McpMetaToolBridge,
    mcp_meta_tool_definitions,
};
use super::sand_await_shell_tool::SandAwaitShellToolBridge;
use super::sand_agent_management_tools::{
    AgentManagementSink, AgentManagementToolBridge,
};
use super::sand_browser_tools::{BrowserToolExecutor, SandBrowserToolBridge};
use super::sand_web_tools::{SandWebToolBridge, WebToolExecutor};
use super::sand_generate_image_tool::{
    GENERATE_IMAGE_TOOL_NAME, GenerateImageToolExecutor, SandGenerateImageToolBridge,
};
use super::sand_computer_tool::{ComputerToolExecutor, ComputerToolExposure, SandComputerToolBridge};
use super::sand_file_transfer_tools::{
    COPY_FROM_BOX_TOOL_NAME, COPY_TO_BOX_TOOL_NAME, FileTransferExecutor,
    SandFileTransferToolBridge,
};
use super::sand_external_machine_tools::{
    ExternalMachineExecutor, ExternalMachineToolBridge, ExternalShellAutoReviewCallback,
};
use super::sand_reaction_tool::{ReactionSink, ReactionToolBridge};
use super::sand_spotlight_tools::SpotlightedRoutedToolBridge;
use super::sand_state_tool::{
    RoutineAutoReviewCallback, RoutinePostWriteCallback, SandStateToolBridge, SandStateWriter,
    StateApprovalBarrier,
};
use super::sand_multitask_todo_tool::{
    MultitaskTodoState, SandMultitaskTodoToolBridge,
};
use super::sand_mcp_management_tools::{
    AUTHENTICATE_MCP_SERVER_TOOL_NAME, McpManagementSink, McpManagementToolBridge,
    SEARCH_PLUGINS_TOOL_NAME,
};
use super::send_message_tool::{
    SAND_SEND_MESSAGE_TOOL_NAME, SendMessageInteractionSink, SendMessageSink,
    SendMessageToolBridge,
};
use super::sand_task_subagent_tool::{
    SubagentTaskReviewCallback, SubagentTaskSink, SubagentTaskToolBridge,
};
use super::sand_subagent_management_tools::{
    CHECK_SUBAGENT_TOOL_NAME, MESSAGE_SUBAGENT_TOOL_NAME, STOP_SUBAGENT_TOOL_NAME,
    SubagentManagementToolBridge, SubagentSteerReviewCallback,
};

struct TurnToolsetMcpProvider<'a> {
    bridge: &'a dyn RoutedToolBridge,
}

impl SandMcpToolProvider for TurnToolsetMcpProvider<'_> {
    fn get_tools(&self) -> Result<Vec<RoutedToolDefinition>, String> {
        self.bridge.list_tools().map_err(|error| error.to_string())
    }
}

pub struct TurnToolsetMcpProjectionBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    projected_tools: Option<Arc<Vec<RoutedToolDefinition>>>,
}

impl TurnToolsetMcpProjectionBridge {
    fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        projected_tools: Option<Arc<Vec<RoutedToolDefinition>>>,
    ) -> Self {
        Self { delegate, projected_tools }
    }
}

impl RoutedToolBridge for TurnToolsetMcpProjectionBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        if let Some(projected_tools) = self.projected_tools.as_ref() {
            return Ok(projected_tools.as_ref().clone());
        }
        let McpStateExecResult::Success(state) = execute_canonical_mcp_state(
            &TurnToolsetMcpProvider { bridge: self.delegate.as_ref() },
        )
        .map_err(|error| ProviderSessionError::Tool(format!("MCP state projection failed: {error}")))?;
        Ok(state.servers.into_iter().flat_map(|server| server.tools).map(|tool| RoutedToolDefinition {
            name: tool.name,
            provider_identifier: tool.provider_identifier,
            tool_name: tool.tool_name,
            description: tool.description,
            input_schema: tool.input_schema,
        }).collect())
    }

    fn observe_partial_tool_call(&self, partial: &ProviderPartialToolCall) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(&self, tool: &RoutedToolDefinition, args: serde_json::Value, tool_call_id: &str) -> Result<serde_json::Value, ProviderSessionError> {
        self.delegate.call_tool(tool, args, tool_call_id)
    }
}

pub fn project_turn_mcp_toolset(
    base: Arc<dyn RoutedToolBridge>,
    projected_tools: Option<Arc<Vec<RoutedToolDefinition>>>,
    projected_meta_tools: Option<Arc<Vec<RoutedMcpMetaToolDefinition>>>,
    mcp_meta_enabled: bool,
) -> Result<Arc<dyn RoutedToolBridge>, ProviderSessionError> {
    let source_meta_tools = if mcp_meta_enabled {
        Some(match projected_meta_tools {
            Some(tools) => tools.as_ref().clone(),
            None => base.list_mcp_meta_tools()?,
        })
    } else {
        None
    };
    let bridge: Arc<dyn RoutedToolBridge> = Arc::new(TurnToolsetMcpProjectionBridge::new(base, projected_tools));
    Ok(match source_meta_tools {
        Some(tools) => Arc::new(McpMetaToolBridge::new(bridge, tools)),
        None => bridge,
    })
}

/// Per-turn Runner tool dependency projection.
///
/// The Host provides concrete capabilities; this module owns which Runner
/// tool bridges are present and their composition order for one prepared turn.
/// Cross-cutting audit/observation wrappers stay outside this owner.
#[derive(Clone, Default)]
pub struct TurnToolsetDependencies {
    pub role: TurnToolsetRole,
    pub local_tool_permission: Option<TurnLocalToolPermissionBinding>,
    pub cancellation: RoutedProviderCancellation,
    pub box_resources: Option<Arc<dyn RunnerBoxResourcePort>>,
    pub box_shell_review: Option<BoxShellAutoReviewCallback>,
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
    pub send_message_interaction_sink: Option<Arc<dyn SendMessageInteractionSink>>,
    pub reaction_sink: Option<Arc<dyn ReactionSink>>,
    pub agent_management_sink: Option<Arc<dyn AgentManagementSink>>,
    pub subagent_task_sink: Option<Arc<dyn SubagentTaskSink>>,
    pub subagent_task_allowed_types: Option<Arc<Vec<String>>>,
    pub subagent_task_review: Option<SubagentTaskReviewCallback>,
    pub subagent_runtime: Option<Arc<Mutex<SubagentRuntime>>>,
    pub subagent_steer_review: Option<SubagentSteerReviewCallback>,
    pub state_writer: Option<Arc<dyn SandStateWriter>>,
    pub routine_auto_review: Option<RoutineAutoReviewCallback>,
    pub state_approval_barrier: Option<StateApprovalBarrier>,
    pub routine_post_write: Option<RoutinePostWriteCallback>,
    pub multitask_enabled: bool,
    pub multitask_todo_state: Option<Arc<dyn MultitaskTodoState>>,
    pub cloud_agent_tool: Option<CloudAgentToolDependencies>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnToolsetRole {
    pub is_subagent_runner: bool,
    pub is_shared_room_runner: bool,
    pub is_box_scoped_subagent: bool,
    pub is_computer_use_subagent: bool,
    pub is_browser_use_subagent: bool,
    pub subagent_configs_present: bool,
    pub shared_room_box_tools_enabled: bool,
    pub remote_box_available: bool,
    pub remote_box_has_desktop: bool,
    pub dynamic_tools_enabled: bool,
}

impl Default for TurnToolsetRole {
    fn default() -> Self {
        Self {
            is_subagent_runner: false,
            is_shared_room_runner: false,
            is_box_scoped_subagent: false,
            is_computer_use_subagent: false,
            is_browser_use_subagent: false,
            subagent_configs_present: true,
            shared_room_box_tools_enabled: true,
            remote_box_available: true,
            remote_box_has_desktop: true,
            dynamic_tools_enabled: false,
        }
    }
}

#[derive(Clone)]
pub struct TurnLocalToolPermissionBinding {
    pub controller: Arc<SandLocalToolPermissionController>,
    pub agent_id: String,
    pub session_key: String,
    pub direction_epoch: u64,
}

struct LocalToolScopeBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    binding: TurnLocalToolPermissionBinding,
}

impl LocalToolScopeBridge {
    fn scoped_action(tool: &RoutedToolDefinition) -> Option<Option<&'static str>> {
        let names = [tool.name.as_str(), tool.tool_name.as_str()];
        if names.contains(&SAND_EXTERNAL_SHELL_TOOL_NAME) {
            return Some(Some("run-command"));
        }
        if names.contains(&SAND_EXTERNAL_READ_TOOL_NAME)
            || names.contains(&SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME)
        {
            return Some(Some("read-file"));
        }
        if names.contains(&RUNNER_BOX_SHELL_TOOL_NAME)
            || names.contains(&RUNNER_BOX_READ_TOOL_NAME)
            || names.contains(&SAND_BOX_AWAIT_SHELL_TOOL_NAME)
            || names.contains(&COPY_TO_BOX_TOOL_NAME)
            || names.contains(&COPY_FROM_BOX_TOOL_NAME)
        {
            return Some(None);
        }
        None
    }
}

impl RoutedToolBridge for LocalToolScopeBridge {
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
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: serde_json::Value,
        tool_call_id: &str,
    ) -> Result<serde_json::Value, ProviderSessionError> {
        let Some(action) = Self::scoped_action(tool) else {
            return self.delegate.call_tool(tool, args, tool_call_id);
        };
        let scope = SandLocalToolScope {
            agent_id: self.binding.agent_id.clone(),
            session_key: Some(self.binding.session_key.clone()),
            tool_call_id: Some(tool_call_id.to_string()),
            action: action.map(str::to_string),
            direction_epoch: Some(self.binding.direction_epoch),
        };
        let result = self.delegate.call_tool(tool, args, tool_call_id);
        self.binding.controller.complete_scope(Some(&scope));
        result
    }
}

const CURSOR_DYNAMIC_TOOLS_NAMESPACE: &str = "cursor";

fn dynamic_tool_hint(tool: &RoutedToolDefinition) -> Option<&'static str> {
    let names = [tool.name.as_str(), tool.tool_name.as_str()];
    if names.contains(&CLOUD_AGENT_TOOL_NAME) {
        return Some("Launch and manage Cursor cloud coding agents for repository work.");
    }
    if names.contains(&SEARCH_PLUGINS_TOOL_NAME) {
        return Some("Search installable plugins/connectors when a task needs a service.");
    }
    if names.contains(&AUTHENTICATE_MCP_SERVER_TOOL_NAME) {
        return Some("Start authentication for a connector that needs auth.");
    }
    if names.contains(&COPY_TO_BOX_TOOL_NAME) {
        return Some("Copy a file from the user's computer onto your box.");
    }
    if names.contains(&COPY_FROM_BOX_TOOL_NAME) {
        return Some("Copy a file from your box onto the user's computer.");
    }
    if names.contains(&SAND_REQUEST_BOX_HELP_TOOL_NAME) {
        return Some("Hand your box's desktop to the user for a sign-in or manual step.");
    }
    if names.contains(&CHECK_SUBAGENT_TOOL_NAME) {
        return Some("Inspect a running background subagent's status and recent actions.");
    }
    if names.contains(&MESSAGE_SUBAGENT_TOOL_NAME) {
        return Some("Send a new instruction into a running background subagent.");
    }
    if names.contains(&STOP_SUBAGENT_TOOL_NAME) {
        return Some("Abort a running background subagent.");
    }
    None
}

fn build_dynamic_server_use_instructions(tools: &[RoutedToolDefinition]) -> String {
    let hints = tools
        .iter()
        .filter_map(|tool| dynamic_tool_hint(tool).map(|hint| format!("- {}: {hint}", tool.name)))
        .collect::<Vec<_>>();
    if hints.is_empty() {
        "Native Cursor tools for this session. Read their schemas before calling them.".into()
    } else {
        format!(
            "Native Cursor tools for this session. Read their schemas before calling them.\n{}",
            hints.join("\n")
        )
    }
}

fn is_dynamic_first_party_tool(tool: &RoutedToolDefinition) -> bool {
    [
        CLOUD_AGENT_TOOL_NAME,
        SEARCH_PLUGINS_TOOL_NAME,
        AUTHENTICATE_MCP_SERVER_TOOL_NAME,
        COPY_TO_BOX_TOOL_NAME,
        COPY_FROM_BOX_TOOL_NAME,
        SAND_REQUEST_BOX_HELP_TOOL_NAME,
        CHECK_SUBAGENT_TOOL_NAME,
        MESSAGE_SUBAGENT_TOOL_NAME,
        STOP_SUBAGENT_TOOL_NAME,
        GENERATE_IMAGE_TOOL_NAME,
    ]
    .iter()
    .any(|candidate| tool.name == *candidate || tool.tool_name == *candidate)
}

struct DynamicToolPlacementBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    dynamic_tools: Vec<RoutedToolDefinition>,
    delegate_has_meta_tools: bool,
}

impl DynamicToolPlacementBridge {
    fn new(delegate: Arc<dyn RoutedToolBridge>) -> Result<Self, ProviderSessionError> {
        let offered = delegate.list_tools()?;
        let dynamic_tools = offered
            .iter()
            .filter(|tool| is_dynamic_first_party_tool(tool))
            .cloned()
            .collect::<Vec<_>>();
        let delegate_has_meta_tools = offered.iter().any(|tool| {
            tool.name == GET_MCP_TOOLS_TOOL_NAME || tool.tool_name == GET_MCP_TOOLS_TOOL_NAME
        });
        Ok(Self {
            delegate,
            dynamic_tools,
            delegate_has_meta_tools,
        })
    }

    fn cursor_discovery(&self, args: &serde_json::Value) -> serde_json::Value {
        let requested_tool = args
            .get("toolName")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let pattern = args
            .get("pattern")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let tools = self
            .dynamic_tools
            .iter()
            .filter(|tool| {
                if let Some(requested_tool) = requested_tool
                    && requested_tool != tool.name
                    && requested_tool != tool.tool_name
                {
                    return false;
                }
                match pattern.as_ref() {
                    None => true,
                    Some(pattern) => {
                        tool.name.to_ascii_lowercase().contains(pattern)
                            || tool.tool_name.to_ascii_lowercase().contains(pattern)
                            || tool
                                .description
                                .as_deref()
                                .unwrap_or_default()
                                .to_ascii_lowercase()
                                .contains(pattern)
                    }
                }
            })
            .map(|tool| {
                serde_json::json!({
                    "toolName": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema,
                })
            })
            .collect::<Vec<_>>();
        let instructions = build_dynamic_server_use_instructions(&self.dynamic_tools);
        serde_json::json!({
            "enabled": true,
            "mcpDescriptors": [{
                "serverIdentifier": CURSOR_DYNAMIC_TOOLS_NAMESPACE,
                "serverName": CURSOR_DYNAMIC_TOOLS_NAMESPACE,
                "serverUseInstructions": instructions,
                "tools": tools,
            }]
        })
    }

    fn merge_cursor_discovery(
        &self,
        delegate_value: serde_json::Value,
        args: &serde_json::Value,
    ) -> serde_json::Value {
        let cursor = self.cursor_discovery(args);
        let mut descriptors = delegate_value
            .get("mcpDescriptors")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(cursor_descriptor) = cursor
            .get("mcpDescriptors")
            .and_then(serde_json::Value::as_array)
            .and_then(|values| values.first())
            .cloned()
        {
            descriptors.push(cursor_descriptor);
        }
        serde_json::json!({
            "enabled": true,
            "mcpDescriptors": descriptors,
        })
    }
}

impl RoutedToolBridge for DynamicToolPlacementBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self
            .delegate
            .list_tools()?
            .into_iter()
            .filter(|tool| !is_dynamic_first_party_tool(tool))
            .collect::<Vec<_>>();
        for meta in mcp_meta_tool_definitions() {
            if !tools.iter().any(|tool| tool.name == meta.name) {
                tools.push(meta);
            }
        }
        Ok(tools)
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        self.delegate.list_mcp_meta_tools()
    }

    fn observe_partial_tool_call(
        &self,
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: serde_json::Value,
        tool_call_id: &str,
    ) -> Result<serde_json::Value, ProviderSessionError> {
        let effective = if tool.tool_name.trim().is_empty() {
            tool.name.as_str()
        } else {
            tool.tool_name.as_str()
        };
        if effective == GET_MCP_TOOLS_TOOL_NAME {
            let server = args
                .get("server")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if server == Some(CURSOR_DYNAMIC_TOOLS_NAMESPACE) || !self.delegate_has_meta_tools {
                return Ok(self.cursor_discovery(&args));
            }
            if server.is_none() {
                let delegated = self.delegate.call_tool(tool, args.clone(), tool_call_id)?;
                return Ok(self.merge_cursor_discovery(delegated, &args));
            }
        }
        if effective == CALL_MCP_TOOL_NAME
            && args
                .get("server")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|server| server == CURSOR_DYNAMIC_TOOLS_NAMESPACE)
        {
            let tool_name = args
                .get("toolName")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ProviderSessionError::Tool("toolName is required".into()))?;
            let target = self
                .dynamic_tools
                .iter()
                .find(|candidate| candidate.name == tool_name || candidate.tool_name == tool_name)
                .cloned()
                .ok_or_else(|| {
                    ProviderSessionError::Tool(format!(
                        "dynamic tool {tool_name} was not found in the cursor namespace"
                    ))
                })?;
            return self.delegate.call_tool(
                &target,
                args.get("arguments").cloned().unwrap_or_else(|| serde_json::json!({})),
                tool_call_id,
            );
        }
        self.delegate.call_tool(tool, args, tool_call_id)
    }
}

struct EmptyTurnToolBridge;

impl RoutedToolBridge for EmptyTurnToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        _args: serde_json::Value,
        _tool_call_id: &str,
    ) -> Result<serde_json::Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool(format!(
            "{} is not available to this subagent Runner",
            tool.name
        )))
    }
}

struct SharedRoomToolFilterBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    box_tools_enabled: bool,
}

impl SharedRoomToolFilterBridge {
    fn allows(&self, tool: &RoutedToolDefinition) -> bool {
        if tool.name == SAND_SEND_MESSAGE_TOOL_NAME || tool.tool_name == SAND_SEND_MESSAGE_TOOL_NAME {
            return true;
        }
        self.box_tools_enabled
            && [
                RUNNER_BOX_SHELL_TOOL_NAME,
                RUNNER_BOX_READ_TOOL_NAME,
                SAND_BOX_AWAIT_SHELL_TOOL_NAME,
                "Screenshot",
            ]
            .iter()
            .any(|allowed| tool.name == *allowed || tool.tool_name == *allowed)
    }
}

impl RoutedToolBridge for SharedRoomToolFilterBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(self
            .delegate
            .list_tools()?
            .into_iter()
            .filter(|tool| self.allows(tool))
            .collect())
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        Ok(self
            .delegate
            .list_mcp_meta_tools()?
            .into_iter()
            .filter(|meta| self.allows(&meta.tool))
            .collect())
    }

    fn observe_partial_tool_call(
        &self,
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: serde_json::Value,
        tool_call_id: &str,
    ) -> Result<serde_json::Value, ProviderSessionError> {
        if !self.allows(tool) {
            return Err(ProviderSessionError::Tool(format!(
                "{} is not available in a shared-room Runner",
                tool.name
            )));
        }
        self.delegate.call_tool(tool, args, tool_call_id)
    }
}

pub fn build_turn_toolset(
    base: Arc<dyn RoutedToolBridge>,
    dependencies: TurnToolsetDependencies,
) -> Arc<dyn RoutedToolBridge> {
    let role = dependencies.role;
    if role.is_subagent_runner
        && !role.is_computer_use_subagent
        && !role.is_browser_use_subagent
        && !role.subagent_configs_present
    {
        return Arc::new(EmptyTurnToolBridge);
    }
    let await_box_resources = role
        .remote_box_available
        .then(|| dependencies.box_resources.clone())
        .flatten();
    let generate_image_box_resources = dependencies.box_resources.clone();
    let await_external_machine = dependencies.external_machine_executor.clone();
    let await_cancellation = dependencies.cancellation.clone();
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_subagent_runner, dependencies.mcp_management_sink) {
            (false, Some(management)) => Arc::new(McpManagementToolBridge::new(base, management)),
            _ => base,
        };
    let bridge: Arc<dyn RoutedToolBridge> = match (role.remote_box_available, dependencies.box_resources) {
        (true, Some(box_resources)) => {
            let mut box_bridge = RunnerBoxToolBridge::new(bridge, box_resources);
            if let Some(review) = dependencies.box_shell_review {
                box_bridge = box_bridge.with_shell_review(review);
            }
            Arc::new(box_bridge)
        }
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> = match (
        role.is_browser_use_subagent,
        role.remote_box_available,
        role.remote_box_has_desktop,
        dependencies.browser_executor,
    ) {
        (true, true, true, Some(executor)) => Arc::new(SandBrowserToolBridge::new(bridge, executor)),
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_box_scoped_subagent, dependencies.web_executor) {
            (false, Some(executor)) => Arc::new(SandWebToolBridge::new(bridge, executor)),
            _ => bridge,
        };
    let bridge: Arc<dyn RoutedToolBridge> = match (
        role.is_subagent_runner,
        dependencies.generate_image_executor,
        generate_image_box_resources,
    ) {
        (false, Some(executor), Some(box_resources)) => Arc::new(
            SandGenerateImageToolBridge::new(bridge, executor, box_resources),
        ),
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> = match (
        role.remote_box_available,
        role.remote_box_has_desktop,
        dependencies.computer_executor,
    ) {
        (true, true, Some(executor)) => Arc::new(
            SandComputerToolBridge::new(bridge, executor)
                .with_exposure(dependencies.computer_exposure),
        ),
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_box_scoped_subagent, dependencies.external_machine_executor) {
        (false, Some(executor)) => {
            let mut external = ExternalMachineToolBridge::new(bridge, executor);
            if let Some(review) = dependencies.external_shell_review {
                external = external.with_shell_review(review);
            }
            Arc::new(external)
        }
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (
            role.is_box_scoped_subagent,
            role.remote_box_available,
            dependencies.file_transfer_executor,
        ) {
            (false, true, Some(executor)) => Arc::new(SandFileTransferToolBridge::new(bridge, executor)),
            _ => bridge,
        };
    let bridge: Arc<dyn RoutedToolBridge> = if !role.is_box_scoped_subagent
        && (await_box_resources.is_some() || await_external_machine.is_some())
    {
        Arc::new(SandAwaitShellToolBridge::new(
            bridge,
            await_box_resources,
            await_external_machine,
            await_cancellation,
        ))
    } else {
        bridge
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_subagent_runner, dependencies.reaction_sink) {
            (false, Some(sink)) => Arc::new(ReactionToolBridge::new(bridge, sink)),
            _ => bridge,
        };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_subagent_runner, dependencies.agent_management_sink) {
            (false, Some(sink)) => Arc::new(AgentManagementToolBridge::new(bridge, sink)),
            _ => bridge,
        };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_subagent_runner, dependencies.subagent_task_sink) {
        (false, Some(sink)) => {
            let mut task_bridge = SubagentTaskToolBridge::new(bridge, sink);
            if let Some(allowed) = dependencies.subagent_task_allowed_types {
                task_bridge = task_bridge.with_allowed_subagent_types(allowed);
            }
            if let Some(review) = dependencies.subagent_task_review {
                task_bridge = task_bridge.with_review(review);
            }
            Arc::new(task_bridge)
        }
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_subagent_runner, dependencies.subagent_runtime) {
        (false, Some(runtime)) => Arc::new(SubagentManagementToolBridge::new(
            bridge,
            runtime,
            dependencies.subagent_steer_review,
        )),
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_subagent_runner, dependencies.state_writer) {
        (false, Some(state)) => {
            let mut state_bridge = SandStateToolBridge::new(bridge, state);
            if let Some(review) = dependencies.routine_auto_review {
                state_bridge = state_bridge.with_routine_auto_review(review);
            }
            if let Some(barrier) = dependencies.state_approval_barrier {
                state_bridge = state_bridge.with_approval_barrier(barrier);
            }
            if let Some(callback) = dependencies.routine_post_write {
                state_bridge = state_bridge.with_routine_post_write(callback);
            }
            Arc::new(state_bridge)
        }
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        if !role.is_subagent_runner && dependencies.multitask_enabled {
        match dependencies.multitask_todo_state {
            Some(state) => Arc::new(SandMultitaskTodoToolBridge::new(bridge, state)),
            None => bridge,
        }
    } else {
        bridge
    };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (role.is_box_scoped_subagent, dependencies.cloud_agent_tool) {
            (false, Some(deps)) => Arc::new(CloudAgentToolBridge::new(bridge, deps)),
            _ => bridge,
        };
    let bridge: Arc<dyn RoutedToolBridge> =
        match (
            role.is_subagent_runner,
            role.remote_box_available,
            role.remote_box_has_desktop,
            &dependencies.send_message_sink,
        ) {
        (false, true, true, Some(sink)) => Arc::new(BoxHelpToolBridge::new(
            bridge,
            Arc::clone(sink),
            dependencies.cancellation.clone(),
        )),
        _ => bridge,
    };
    let bridge = match (role.is_subagent_runner, dependencies.send_message_sink) {
        (false, Some(sink)) => {
            let mut send_bridge = SendMessageToolBridge::new(bridge, sink);
            if let Some(interaction_sink) = dependencies.send_message_interaction_sink {
                send_bridge = send_bridge.with_interaction_sink(interaction_sink);
            }
            Arc::new(send_bridge)
        }
        _ => bridge,
    };
    let bridge: Arc<dyn RoutedToolBridge> = if role.dynamic_tools_enabled
        && !role.is_subagent_runner
        && !role.is_shared_room_runner
        && !role.is_box_scoped_subagent
    {
        match DynamicToolPlacementBridge::new(bridge.clone()) {
            Ok(dynamic) => Arc::new(dynamic),
            Err(_) => bridge,
        }
    } else {
        bridge
    };
    let bridge: Arc<dyn RoutedToolBridge> = match dependencies.local_tool_permission {
        Some(binding) => Arc::new(LocalToolScopeBridge {
            delegate: bridge,
            binding,
        }),
        None => bridge,
    };
    if role.is_shared_room_runner {
        Arc::new(SharedRoomToolFilterBridge {
            delegate: bridge,
            box_tools_enabled: role.shared_room_box_tools_enabled,
        })
    } else {
        bridge
    }
}

pub fn fence_turn_toolset(
    bridge: Arc<dyn RoutedToolBridge>,
    spotlight_enabled: bool,
) -> Arc<dyn RoutedToolBridge> {
    if spotlight_enabled {
        Arc::new(SpotlightedRoutedToolBridge::new(bridge))
    } else {
        bridge
    }
}
