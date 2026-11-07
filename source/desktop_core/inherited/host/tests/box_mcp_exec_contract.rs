use std::sync::Mutex;

use mahayana_host_runtime::extensions::inference::provider_session::RoutedToolDefinition;
use mahayana_host_runtime::extensions::mcp::box_mcp_exec::{
    BoxMcpExecBackend, BoxMcpToolCall, BoxMcpToolResult, SandBoxMcpExec,
};
use mahayana_host_runtime::ports::mcp_state_executor::SandMcpToolProvider;
use serde_json::json;

struct Provider;
impl SandMcpToolProvider for Provider {
    fn get_tools(&self) -> Result<Vec<RoutedToolDefinition>, String> {
        Ok(vec![
            RoutedToolDefinition {
                name: "calendar.create".into(),
                provider_identifier: "calendar".into(),
                tool_name: "create".into(),
                description: Some("create event".into()),
                input_schema: json!({"type":"object"}),
            },
            RoutedToolDefinition {
                name: "calendar.list".into(),
                provider_identifier: "calendar".into(),
                tool_name: "list".into(),
                description: None,
                input_schema: json!({}),
            },
            RoutedToolDefinition {
                name: "mail.send".into(),
                provider_identifier: "mail".into(),
                tool_name: "send".into(),
                description: None,
                input_schema: json!({}),
            },
        ])
    }
}

struct Backend {
    loads: Mutex<Vec<String>>,
    fail_exec: bool,
}
impl BoxMcpExecBackend for Backend {
    fn load_servers(&self, config_json: &str) -> Result<(), String> {
        self.loads.lock().unwrap().push(config_json.into());
        Ok(())
    }
    fn state_provider(&self) -> Result<&dyn SandMcpToolProvider, String> {
        Ok(&Provider)
    }
    fn execute_tool(&self, args: &BoxMcpToolCall) -> Result<serde_json::Value, String> {
        if self.fail_exec {
            Err("transport down".into())
        } else {
            Ok(json!({"toolCallId":args.tool_call_id}))
        }
    }
}

#[test]
fn load_and_filtered_discovery_preserve_server_and_tool_identity() {
    let exec = SandBoxMcpExec::new(Backend {
        loads: Mutex::new(Vec::new()),
        fail_exec: false,
    });
    exec.load_servers("{\"mcpServers\":{}}").unwrap();
    let servers = exec.list_tools(&["calendar".into()], true).unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].server_identifier, "calendar");
    assert_eq!(servers[0].status, "connected");
    assert_eq!(servers[0].tool_count, 2);
    assert_eq!(servers[0].tools[0].client_key, "calendar");
    assert_eq!(servers[0].tools[0].tool_name, "create");
}

#[test]
fn empty_filter_returns_all_discovered_servers() {
    let exec = SandBoxMcpExec::new(Backend {
        loads: Mutex::new(Vec::new()),
        fail_exec: false,
    });
    let servers = exec.list_tools(&[], false).unwrap();
    assert_eq!(servers.len(), 2);
}

#[test]
fn execute_errors_are_terminal_results_not_panics() {
    let exec = SandBoxMcpExec::new(Backend {
        loads: Mutex::new(Vec::new()),
        fail_exec: true,
    });
    let result = exec.execute_tool(&BoxMcpToolCall {
        tool_call_id: "call-1".into(),
        name: "calendar.create".into(),
        arguments: json!({"title":"x"}),
    });
    assert_eq!(
        result,
        BoxMcpToolResult::Error {
            error: "Box MCP execution failed for \"calendar.create\": transport down".into()
        }
    );
}
