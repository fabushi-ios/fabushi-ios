use std::sync::Arc;

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_web_tools::{
    SandWebToolBridge, WEB_FETCH_TOOL_NAME, WEB_SEARCH_TOOL_NAME, WebToolExecutor,
};
use mahayana_host_runtime::runner::tools::turn_toolset::{
    TurnToolsetDependencies, TurnToolsetRole, build_turn_toolset,
};
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
        Err(ProviderSessionError::Tool("unexpected delegate".into()))
    }
}

struct FakeWeb;

impl WebToolExecutor for FakeWeb {
    fn search(
        &self,
        search_term: &str,
        explanation: Option<&str>,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({"query":search_term,"explanation":explanation}))
    }

    fn fetch(&self, url: &str) -> Result<Value, ProviderSessionError> {
        Ok(json!({"url":url}))
    }
}

#[test]
fn web_tools_execute_and_reject_local_network_fetches() {
    let bridge: Arc<dyn RoutedToolBridge> =
        Arc::new(SandWebToolBridge::new(Arc::new(EmptyBridge), Arc::new(FakeWeb)));
    let tools = bridge.list_tools().expect("tools");
    let search = tools
        .iter()
        .find(|tool| tool.name == WEB_SEARCH_TOOL_NAME)
        .expect("WebSearch");
    let result = bridge
        .call_tool(
            search,
            json!({"query":"rust 2026","explanation":"fresh"}),
            "search",
        )
        .expect("search");
    assert_eq!(result["query"], "rust 2026");

    let fetch = tools
        .iter()
        .find(|tool| tool.name == WEB_FETCH_TOOL_NAME)
        .expect("WebFetch");
    assert!(
        bridge
            .call_tool(fetch, json!({"url":"http://127.0.0.1/secret"}), "fetch")
            .is_err()
    );
    let result = bridge
        .call_tool(
            fetch,
            json!({"url":"https://user:pass@example.com/page"}),
            "fetch",
        )
        .expect("fetch");
    assert_eq!(result["url"], "https://example.com/page");
}

#[test]
fn box_scoped_subagent_does_not_receive_web_tools() {
    let bridge = build_turn_toolset(
        Arc::new(EmptyBridge),
        TurnToolsetDependencies {
            role: TurnToolsetRole {
                is_subagent_runner: true,
                is_box_scoped_subagent: true,
                ..TurnToolsetRole::default()
            },
            web_executor: Some(Arc::new(FakeWeb)),
            ..TurnToolsetDependencies::default()
        },
    );
    assert!(bridge.list_tools().expect("tools").iter().all(|tool| {
        tool.name != WEB_SEARCH_TOOL_NAME && tool.name != WEB_FETCH_TOOL_NAME
    }));
}
