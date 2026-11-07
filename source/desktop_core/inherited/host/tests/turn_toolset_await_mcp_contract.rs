use std::sync::Arc;

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedMcpMetaToolDefinition, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::box_tool_access::{
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxShellRequest, RunnerBoxWriteRequest,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::shell_terminal_watch::{
    ShellTerminalPollRead, TerminalReadResult,
};
use mahayana_host_runtime::runner::tools::sand_external_machine_tools::{
    ExternalMachineExecutor, ExternalMachineReadArgs, ExternalMachineShellArgs,
};
use mahayana_host_runtime::runner::tools::turn_toolset::{
    TurnToolsetDependencies, build_turn_toolset, project_turn_mcp_toolset,
};
use mahayana_host_runtime::sand_activity::{
    SAND_BOX_AWAIT_SHELL_TOOL_NAME, SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME,
};
use serde_json::{Value, json};

struct BaseBridge;

impl RoutedToolBridge for BaseBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![RoutedToolDefinition {
            name: "base_tool".into(),
            provider_identifier: "base".into(),
            tool_name: "base_tool".into(),
            description: None,
            input_schema: json!({"type":"object"}),
        }])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({"content":[{"type":"text","text":"base result"}]}))
    }
}

struct FakeAwaitBox;

impl RunnerBoxResourcePort for FakeAwaitBox {
    fn execute_shell(&self, _request: RunnerBoxShellRequest) -> Result<Value, ProviderSessionError> {
        Ok(json!({}))
    }

    fn execute_read(&self, _request: RunnerBoxReadRequest) -> Result<Value, ProviderSessionError> {
        Ok(json!({}))
    }

    fn execute_write(&self, _request: RunnerBoxWriteRequest) -> Result<(), ProviderSessionError> {
        Ok(())
    }

    fn poll_background_shell_terminal(&self, shell_id: &str) -> ShellTerminalPollRead {
        let content = if shell_id == "7" {
            "---\npid: 123\nrunning_for_ms: 900\n---\nready for traffic\n---\nexit_code: 0\nelapsed_ms: 900\n---"
        } else {
            "---\npid: 456\nrunning_for_ms: 250\n---\nbooting service\n"
        };
        ShellTerminalPollRead::Snapshot {
            output_path: format!("/box/terminals/{shell_id}.txt"),
            result: TerminalReadResult::SuccessText(content.into()),
        }
    }
}

struct FakeAwaitExternal;

impl ExternalMachineExecutor for FakeAwaitExternal {
    fn execute_shell(&self, _args: &ExternalMachineShellArgs) -> Result<Value, ProviderSessionError> {
        Ok(json!({}))
    }

    fn execute_read(&self, _args: &ExternalMachineReadArgs) -> Result<Value, ProviderSessionError> {
        Ok(json!({}))
    }

    fn poll_background_shell_terminal(
        &self,
        shell_id: &str,
        _tool_call_id: &str,
    ) -> ShellTerminalPollRead {
        ShellTerminalPollRead::Snapshot {
            output_path: format!("/external/terminals/{shell_id}.txt"),
            result: TerminalReadResult::SuccessText(
                "---\npid: 99\nrunning_for_ms: 500\n---\nexternal ready\n".into(),
            ),
        }
    }
}

#[test]
fn turn_toolset_owns_box_and_external_await_shell_surfaces() {
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            box_resources: Some(Arc::new(FakeAwaitBox)),
            external_machine_executor: Some(Arc::new(FakeAwaitExternal)),
            ..TurnToolsetDependencies::default()
        },
    );
    let tools = bridge.list_tools().expect("tools");
    let box_await = tools
        .iter()
        .find(|tool| tool.name == SAND_BOX_AWAIT_SHELL_TOOL_NAME)
        .expect("AwaitShell");
    let external_await = tools
        .iter()
        .find(|tool| tool.name == SAND_EXTERNAL_AWAIT_SHELL_TOOL_NAME)
        .expect("ExternalAwaitShell");

    let complete = bridge
        .call_tool(
            box_await,
            json!({"shell_id":7,"block_until_ms":0}),
            "await-box",
        )
        .expect("box await");
    assert_eq!(complete["status"], "complete");
    assert_eq!(complete["exitCode"], 0);
    assert_eq!(complete["runtimeMs"], 900);
    assert_eq!(complete["outputFilePath"], "/box/terminals/7.txt");

    let regex = bridge
        .call_tool(
            external_await,
            json!({"shell_id":"8","block_until_ms":1000,"pattern":"external\\s+ready"}),
            "await-external",
        )
        .expect("external await");
    assert_eq!(regex["status"], "stillRunning");
    assert_eq!(regex["runtimeMs"], 500);
    assert_eq!(regex["regexMatch"], "external ready");
    assert_eq!(regex["outputFilePath"], "/external/terminals/8.txt");

    let nonblocking = bridge
        .call_tool(
            box_await,
            json!({"task_id":"8","block_until_ms":"0"}),
            "await-running",
        )
        .expect("nonblocking await");
    assert_eq!(nonblocking["status"], "stillRunning");
}

#[test]
fn turn_toolset_owns_projected_dynamic_mcp_and_meta_discovery() {
    let projected = RoutedToolDefinition {
        name: "issues_lookup".into(),
        provider_identifier: "linear".into(),
        tool_name: "lookup".into(),
        description: Some("Lookup issue".into()),
        input_schema: json!({"type":"object"}),
    };
    let bridge = project_turn_mcp_toolset(
        Arc::new(BaseBridge),
        Some(Arc::new(vec![projected.clone()])),
        Some(Arc::new(vec![RoutedMcpMetaToolDefinition::from(projected)])),
        true,
    )
    .expect("projected MCP toolset");
    let tools = bridge.list_tools().expect("tools");
    assert!(tools.iter().any(|tool| tool.name == "GetMcpTools"));
    assert!(tools.iter().any(|tool| tool.name == "CallMcpTool"));
    let meta = bridge.list_mcp_meta_tools().expect("meta tools");
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].tool.provider_identifier, "linear");
    assert_eq!(meta[0].tool.tool_name, "lookup");
}

#[test]
fn shipping_composition_keeps_mcp_and_await_ownership_canonical() {
    const TURN_TOOLSET: &str = include_str!("../src/runner/tools/turn_toolset.rs");
    const TURN_COMPOSITION: &str = include_str!("../src/runner/turn_agent_composition.rs");
    const SHIPPING_BOX: &str = include_str!("../src/extensions/forever_box/runner_tools.rs");
    const SHIPPING_EXTERNAL: &str =
        include_str!("../src/runner/host_external_machine_dependencies.rs");

    assert!(TURN_TOOLSET.contains("project_turn_mcp_toolset"));
    assert!(TURN_TOOLSET.contains("McpManagementToolBridge::new"));
    assert!(TURN_TOOLSET.contains("SandAwaitShellToolBridge::new"));
    assert!(TURN_COMPOSITION.contains("project_turn_mcp_toolset("));
    assert!(!TURN_COMPOSITION.contains("McpManagementToolBridge::new"));
    assert!(SHIPPING_BOX.contains("fn poll_background_shell_terminal"));
    assert!(SHIPPING_BOX.contains("read_background_shell_terminal(shell_id)"));
    assert!(SHIPPING_EXTERNAL.contains("self.manager.terminals_folder()"));
    assert!(SHIPPING_EXTERNAL.contains("self.execute_read(&ExternalMachineReadArgs"));
}
