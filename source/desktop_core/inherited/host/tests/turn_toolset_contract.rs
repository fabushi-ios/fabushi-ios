use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::extensions::memory::agent_state::SandAgentState;
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_browser_tools::{
    BrowserDriverOutput, BrowserToolExecutor, BrowserToolSpec,
};
use mahayana_host_runtime::runner::tools::sand_computer_tool::{
    ComputerActionArgs, ComputerCoordinate, ComputerToolExecutor, ComputerUseResult,
    ComputerUseSuccess,
};
use mahayana_host_runtime::runner::tools::sand_state_tool::{
    SAND_UPDATE_STATE_TOOL_NAME, SandStateWriter,
};
use mahayana_host_runtime::runner::tools::turn_toolset::{
    TurnToolsetDependencies, TurnToolsetRole, build_turn_toolset, fence_turn_toolset,
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

struct DynamicBaseBridge;

impl RoutedToolBridge for DynamicBaseBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![
            RoutedToolDefinition {
                name: "base_tool".into(),
                provider_identifier: "base".into(),
                tool_name: "base_tool".into(),
                description: None,
                input_schema: json!({"type":"object"}),
            },
            RoutedToolDefinition {
                name: "CloudAgent".into(),
                provider_identifier: "fabushi-runner".into(),
                tool_name: "CloudAgent".into(),
                description: Some("Launch a cloud agent".into()),
                input_schema: json!({
                    "type":"object",
                    "properties":{"prompt":{"type":"string"}}
                }),
            },
        ])
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({"tool":tool.name,"args":args}))
    }
}

struct FakeComputerExecutor;

impl ComputerToolExecutor for FakeComputerExecutor {
    fn execute(
        &self,
        args: &ComputerActionArgs,
        tool_call_id: &str,
    ) -> Result<ComputerUseResult, ProviderSessionError> {
        Ok(ComputerUseResult::Success(ComputerUseSuccess {
            screenshot: None,
            screenshot_path: Some(format!("file:///computer-{tool_call_id}.webp")),
            cursor_position: Some(ComputerCoordinate {
                x: if args.action.as_str() == "click" { 10 } else { 0 },
                y: 20,
            }),
        }))
    }
}

struct FakeBrowserExecutor;

impl BrowserToolExecutor for FakeBrowserExecutor {
    fn execute(
        &self,
        spec: &BrowserToolSpec,
        _args: &serde_json::Map<String, Value>,
        tool_call_id: &str,
    ) -> Result<BrowserDriverOutput, ProviderSessionError> {
        Ok(BrowserDriverOutput {
            text: format!("{}:{tool_call_id}", spec.op),
            image_b64: None,
            is_error: false,
            view_id: None,
            generation: None,
            frame_name: None,
            frame_url: None,
        })
    }
}

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-turn-toolset-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn turn_toolset_composes_host_state_capability_without_hiding_base_tools() {
    let root = temp_root("state");
    let state = Arc::new(SandAgentState::new(&root, "agent-a").expect("state"));
    let state_writer: Arc<dyn SandStateWriter> = state;
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            state_writer: Some(state_writer),
            ..TurnToolsetDependencies::default()
        },
    );

    let tools = bridge.list_tools().expect("tools");
    assert!(tools.iter().any(|tool| tool.name == SAND_UPDATE_STATE_TOOL_NAME));
    assert!(tools.iter().any(|tool| tool.name == "base_tool"));

    let update_state = tools
        .iter()
        .find(|tool| tool.name == SAND_UPDATE_STATE_TOOL_NAME)
        .expect("update_state");
    let result = bridge.call_tool(
        update_state,
        json!({
            "target":"memory",
            "action":"write",
            "fact":"Turn toolset owns the per-turn state route",
            "tier":"log"
        }),
        "tool-state",
    ).expect("state result");
    assert!(result.as_str().is_some_and(|value| value.contains("Remembered")));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn turn_toolset_spotlight_fence_preserves_tool_inventory() {
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies::default(),
    );
    let fenced = fence_turn_toolset(bridge, true);
    let tools = fenced.list_tools().expect("tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "base_tool");

    let result = fenced
        .call_tool(&tools[0], json!({}), "tool-base")
        .expect("result");
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.get(1))
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .expect("fenced text");
    assert_eq!(text, "base result");
}


#[test]
fn ordinary_subagent_without_nested_configs_gets_no_tools() {
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            role: TurnToolsetRole {
                is_subagent_runner: true,
                is_box_scoped_subagent: true,
                subagent_configs_present: false,
                ..TurnToolsetRole::default()
            },
            ..TurnToolsetDependencies::default()
        },
    );

    assert!(bridge.list_tools().expect("tools").is_empty());
}

#[test]
fn turn_toolset_composes_browser_capability_without_hiding_base_tools() {
    let browser: Arc<dyn BrowserToolExecutor> = Arc::new(FakeBrowserExecutor);
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            role: TurnToolsetRole {
                is_subagent_runner: true,
                is_box_scoped_subagent: true,
                is_browser_use_subagent: true,
                ..TurnToolsetRole::default()
            },
            browser_executor: Some(browser),
            ..TurnToolsetDependencies::default()
        },
    );

    let tools = bridge.list_tools().expect("tools");
    let navigate = tools
        .iter()
        .find(|tool| tool.name == "browser_navigate")
        .expect("browser_navigate");
    assert!(tools.iter().any(|tool| tool.name == "base_tool"));

    let result = bridge
        .call_tool(
            navigate,
            json!({"url":"https://example.com"}),
            "tool-browser",
        )
        .expect("browser result");
    assert_eq!(result["text"], "navigate:tool-browser");
    assert_eq!(result["isError"], false);
}



#[test]
fn turn_toolset_hides_browser_from_non_browser_runner_roles() {
    let browser: Arc<dyn BrowserToolExecutor> = Arc::new(FakeBrowserExecutor);
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            browser_executor: Some(browser),
            ..TurnToolsetDependencies::default()
        },
    );

    let tools = bridge.list_tools().expect("tools");
    assert!(tools.iter().all(|tool| !tool.name.starts_with("browser_")));
    assert!(tools.iter().any(|tool| tool.name == "base_tool"));
}

#[test]
fn dynamic_turn_toolset_moves_native_tools_behind_cursor_meta_dispatch() {
    let bridge = build_turn_toolset(
        Arc::new(DynamicBaseBridge),
        TurnToolsetDependencies {
            role: TurnToolsetRole {
                dynamic_tools_enabled: true,
                ..TurnToolsetRole::default()
            },
            ..TurnToolsetDependencies::default()
        },
    );

    let tools = bridge.list_tools().expect("tools");
    assert!(tools.iter().any(|tool| tool.name == "base_tool"));
    assert!(tools.iter().all(|tool| tool.name != "CloudAgent"));
    let discovery = tools
        .iter()
        .find(|tool| tool.name == "GetMcpTools")
        .expect("GetMcpTools");
    let discovered = bridge
        .call_tool(discovery, json!({"server":"cursor"}), "discover")
        .expect("dynamic discovery");
    assert_eq!(discovered["mcpDescriptors"][0]["serverIdentifier"], "cursor");
    assert_eq!(discovered["mcpDescriptors"][0]["tools"][0]["toolName"], "CloudAgent");
    assert!(
        discovered["mcpDescriptors"][0]["serverUseInstructions"]
            .as_str()
            .expect("server instructions")
            .contains("Launch and manage Cursor cloud coding agents for repository work.")
    );

    let invocation = tools
        .iter()
        .find(|tool| tool.name == "CallMcpTool")
        .expect("CallMcpTool");
    let result = bridge
        .call_tool(
            invocation,
            json!({
                "server":"cursor",
                "toolName":"CloudAgent",
                "arguments":{"prompt":"fix it"}
            }),
            "invoke",
        )
        .expect("dynamic invocation");
    assert_eq!(result["tool"], "CloudAgent");
    assert_eq!(result["args"]["prompt"], "fix it");
}

#[test]
fn turn_toolset_composes_computer_and_screenshot_capabilities_without_hiding_base_tools() {
    let computer: Arc<dyn ComputerToolExecutor> = Arc::new(FakeComputerExecutor);
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            computer_executor: Some(computer),
            ..TurnToolsetDependencies::default()
        },
    );

    let tools = bridge.list_tools().expect("tools");
    let computer = tools
        .iter()
        .find(|tool| tool.name == "Computer")
        .expect("Computer");
    let screenshot = tools
        .iter()
        .find(|tool| tool.name == "Screenshot")
        .expect("Screenshot");
    assert!(tools.iter().any(|tool| tool.name == "base_tool"));

    let result = bridge
        .call_tool(
            computer,
            json!({"action":"click","x":10,"y":20}),
            "tool-computer",
        )
        .expect("computer result");
    assert_eq!(result["screenshotPath"], "file:///computer-tool-computer.webp");
    assert!(result["content"].as_str().is_some_and(|text| text.contains("Computer action ran")));

    let screenshot_result = bridge
        .call_tool(screenshot, json!({}), "tool-screenshot")
        .expect("screenshot result");
    assert_eq!(
        screenshot_result["screenshotPath"],
        "file:///computer-tool-screenshot.webp"
    );
}
