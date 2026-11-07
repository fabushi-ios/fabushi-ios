use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderPartialToolCall, ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::turn_toolset::project_turn_mcp_toolset;
use mahayana_host_runtime::ports::mcp_state_executor::{
    McpStateExecResult, SandMcpToolProvider, decode_canonical_mcp_state_result,
    encode_canonical_mcp_state_result, execute_mcp_state,
};
use serde_json::json;

struct Provider {
    tools: Vec<RoutedToolDefinition>,
    fail: bool,
}

impl SandMcpToolProvider for Provider {
    fn get_tools(&self) -> Result<Vec<RoutedToolDefinition>, String> {
        if self.fail {
            Err("unavailable".into())
        } else {
            Ok(self.tools.clone())
        }
    }
}

fn tool(provider: &str, name: &str, tool_name: &str) -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: name.into(),
        provider_identifier: provider.into(),
        tool_name: tool_name.into(),
        description: Some(format!("{name} description")),
        input_schema: json!({"type":"object","properties":{"q":{"type":"string"}}}),
    }
}

#[test]
fn groups_tools_by_provider_in_first_seen_order_and_marks_connected() {
    let provider = Provider {
        tools: vec![
            tool("github", "github_search", "search"),
            tool("linear", "linear_get", "get"),
            tool("github", "github_issue", "issue"),
        ],
        fail: false,
    };

    let McpStateExecResult::Success(result) = execute_mcp_state(&provider).unwrap();
    assert_eq!(result.servers.len(), 2);

    assert_eq!(result.servers[0].server_identifier, "github");
    assert_eq!(result.servers[0].server_name, "github");
    assert_eq!(result.servers[0].status, "connected");
    assert_eq!(result.servers[0].tools.len(), 2);
    assert_eq!(result.servers[0].tools[0].name, "github_search");
    assert_eq!(result.servers[0].tools[0].tool_name, "search");
    assert_eq!(
        result.servers[0].tools[0].input_schema["properties"]["q"]["type"],
        "string"
    );

    assert_eq!(result.servers[1].server_identifier, "linear");
    assert_eq!(result.servers[1].tools.len(), 1);
}

#[test]
fn empty_provider_is_success_and_provider_failure_is_not_hidden() {
    let empty = Provider { tools: Vec::new(), fail: false };
    let McpStateExecResult::Success(result) = execute_mcp_state(&empty).unwrap();
    assert!(result.servers.is_empty());

    let failed = Provider { tools: Vec::new(), fail: true };
    assert_eq!(execute_mcp_state(&failed).unwrap_err(), "unavailable");
}


#[test]
fn canonical_agent_v1_wire_round_trip_preserves_grouping_and_tool_schema() {
    let provider = Provider {
        tools: vec![
            tool("github", "github_search", "search"),
            tool("linear", "linear_get", "get"),
            tool("github", "github_issue", "issue"),
        ],
        fail: false,
    };
    let projected = execute_mcp_state(&provider).expect("project state");
    let bytes = encode_canonical_mcp_state_result(&projected).expect("encode canonical protobuf");
    let decoded = decode_canonical_mcp_state_result(&bytes).expect("decode canonical protobuf");
    assert_eq!(decoded, projected);
}

struct RecordingBridge {
    tools: Vec<RoutedToolDefinition>,
    observed: Arc<Mutex<Vec<String>>>,
    called: Arc<Mutex<Vec<String>>>,
}

impl RoutedToolBridge for RecordingBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(self.tools.clone())
    }

    fn observe_partial_tool_call(
        &self,
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.observed
            .lock()
            .expect("record partial")
            .push(partial.tool_call_id.clone());
        Ok(())
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        _args: serde_json::Value,
        tool_call_id: &str,
    ) -> Result<serde_json::Value, ProviderSessionError> {
        self.called
            .lock()
            .expect("record call")
            .push(format!("{}:{tool_call_id}", tool.name));
        Ok(json!({"ok": true}))
    }
}

#[test]
fn shipping_projection_bridge_routes_discovery_through_mcp_state_and_delegates_execution() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let called = Arc::new(Mutex::new(Vec::new()));
    let first = tool("github", "github_search", "search");
    let second = tool("linear", "linear_get", "get");
    let bridge: Arc<dyn RoutedToolBridge> = Arc::new(RecordingBridge {
        tools: vec![first.clone(), second.clone()],
        observed: Arc::clone(&observed),
        called: Arc::clone(&called),
    });
    let projected = project_turn_mcp_toolset(bridge, None, None, false)
        .expect("project turn MCP toolset");

    let tools = projected.list_tools().expect("projected tools");
    assert_eq!(tools, vec![first.clone(), second.clone()]);

    projected
        .observe_partial_tool_call(&ProviderPartialToolCall {
            tool: first.clone(),
            tool_call_id: "partial-1".into(),
            raw_arguments: "{".into(),
            model_call_id: Some("model-1".into()),
        })
        .expect("partial observation");
    assert_eq!(
        observed.lock().expect("observed lock").as_slice(),
        ["partial-1"]
    );

    let result = projected
        .call_tool(&second, json!({"q":"status"}), "call-2")
        .expect("delegated tool call");
    assert_eq!(result, json!({"ok": true}));
    assert_eq!(
        called.lock().expect("called lock").as_slice(),
        ["linear_get:call-2"]
    );
}

