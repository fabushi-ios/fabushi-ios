const SHIPPING_HOST_MAIN: &str = include_str!("../app/src/main.rs");
const MCP_SERVICE: &str = include_str!("../src/extensions/mcp/mcp_service.rs");

use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::sand_action_audit::{
    ActionAuditRecord, ActionAuditSink, AuditedRoutedToolBridge, RoutedMcpAuditConfig,
    computer_use_audit_kind, mcp_audit_status, navigation_probe_command,
    normalize_navigation_url, parse_navigation_probe_output,
};
use serde_json::{Value, json};

struct DirectBridge { fail: bool }

impl RoutedToolBridge for DirectBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![tool()])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if self.fail {
            Err(ProviderSessionError::Tool("boom".into()))
        } else {
            Ok(json!({"content":[{"type":"text","text":"ok"}]}))
        }
    }
}

#[derive(Default)]
struct CaptureSink { records: Mutex<Vec<ActionAuditRecord>> }

impl ActionAuditSink for CaptureSink {
    fn record(&self, record: ActionAuditRecord) {
        self.records.lock().expect("records").push(record);
    }
}

fn tool() -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: "github_search".into(),
        provider_identifier: "github".into(),
        tool_name: "search".into(),
        description: None,
        input_schema: json!({"type":"object"}),
    }
}

#[test]
fn mcp_status_preserves_frozen_envelope_and_shipping_direct_result() {
    assert_eq!(mcp_audit_status(&json!({"result":{"case":"success","value":{"isError":false}}})), "ok");
    assert_eq!(mcp_audit_status(&json!({"result":{"case":"success","value":{"isError":true}}})), "error");
    assert_eq!(mcp_audit_status(&json!({"result":{"case":"approved"}})), "ok");
    assert_eq!(mcp_audit_status(&json!({"result":{"case":"denied"}})), "error");
    assert_eq!(mcp_audit_status(&json!({"content":[]})), "ok");
    assert_eq!(mcp_audit_status(&json!({"isError":true})), "error");
}

#[test]
fn navigation_helpers_match_frozen_probe_contract() {
    assert_eq!(
        navigation_probe_command(1),
        r#"curl -sf --max-time 2 "http://127.0.0.1:9223/json/list""#
    );
    assert_eq!(
        normalize_navigation_url(" https://www.example.com/a?q=1#fragment "),
        Some("https://www.example.com/a".into())
    );
    assert_eq!(normalize_navigation_url("chrome://settings"), None);
    assert_eq!(normalize_navigation_url("not a url"), None);
    let targets = parse_navigation_probe_output(
        "noise\n[{\"type\":\"page\",\"id\":\"1\",\"url\":\"https://example.com/a\"},{\"type\":\"other\"}]\ntrailer",
    );
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0]["id"], "1");
}

#[test]
fn computer_use_kind_preserves_frozen_names() {
    assert_eq!(computer_use_audit_kind("mouseMove"), Some("mouse_move"));
    assert_eq!(computer_use_audit_kind("screenshot"), Some("screenshot"));
    assert_eq!(computer_use_audit_kind("cursorPosition"), None);
}

#[test]
fn shipping_action_audit_resolves_transport_from_live_mcp_registry() {
    for binding in [
        "with_transport_resolving_sink(",
        "transport_for_server_identifier(server_identifier)",
        ".unwrap_or_else(|_| \"unknown\".to_string())",
    ] {
        assert!(
            SHIPPING_HOST_MAIN.contains(binding),
            "shipping Host does not wire live MCP audit transport: {binding}"
        );
    }
    assert!(
        MCP_SERVICE.contains("pub fn transport_for_server_identifier("),
        "MCP service must own the live connector transport lookup"
    );
    assert!(
        MCP_SERVICE.contains(".find(|server| server.server_identifier == server_identifier)"),
        "transport lookup must bind provider/server identity to the live registry"
    );
}

#[test]
fn audited_shipping_bridge_records_success_and_error_without_changing_result() {
    for (fail, expected_status) in [(false, "ok"), (true, "error")] {
        let sink = Arc::new(CaptureSink::default());
        let sink_trait: Arc<dyn ActionAuditSink> = sink.clone();
        let config = RoutedMcpAuditConfig::new("agent-a", Some("turn-1".into()), sink_trait)
            .with_transport_resolver(Arc::new(|server| format!("relay:{server}")));
        let bridge = AuditedRoutedToolBridge::new(Arc::new(DirectBridge { fail }), config);
        let result = bridge.call_tool(&tool(), json!({"q":"rust"}), "tool-1");
        assert_eq!(result.is_err(), fail);
        let records = sink.records.lock().expect("records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent_id, "agent-a");
        assert_eq!(records[0].turn_id.as_deref(), Some("turn-1"));
        assert_eq!(records[0].action["kind"], "mcpToolCall");
        assert_eq!(records[0].action["toolCallId"], "tool-1");
        assert_eq!(records[0].action["serverIdentifier"], "github");
        assert_eq!(records[0].action["toolName"], "search");
        assert_eq!(records[0].action["transport"], "relay:github");
        assert_eq!(records[0].action["status"], expected_status);
    }
}
