use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_mcp_management_tools::{
    ADD_MCP_SERVER_TOOL_NAME, AUTHENTICATE_MCP_SERVER_TOOL_NAME,
    ConnectorCard, ConnectorCardVariant, GET_MCP_SERVER_STATUS_TOOL_NAME,
    McpAuthenticationResult, McpInstalledServer, McpManagementSink,
    McpManagementToolBridge, McpPluginDetail, McpPluginField, McpPluginSkill,
    McpPluginSummary, McpRemoveServerResult, McpUninstallPluginResult,
    SEARCH_PLUGINS_TOOL_NAME, build_server_config_json,
    encode_mcp_account_label_for_listing, rank_plugins_lexically, validate_remote_mcp_url,
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
        Ok(Value::String("delegated".into()))
    }
}

struct FakeManagement {
    plugins: Vec<McpPluginSummary>,
    installed: Mutex<Vec<McpInstalledServer>>,
    cards: Mutex<Vec<ConnectorCard>>,
    mutations: AtomicUsize,
    awaiting: bool,
    multi_account: bool,
    list_installed_error: bool,
    last_authenticated_server_id: Mutex<Option<String>>,
}

impl FakeManagement {
    fn new(awaiting: bool) -> Self {
        Self {
            plugins: vec![
                plugin("linear", "Linear", "Manage product issues", "productivity"),
                plugin("docs", "Documents", "Write Word documents", "writing"),
            ],
            installed: Mutex::new(Vec::new()),
            cards: Mutex::new(Vec::new()),
            mutations: AtomicUsize::new(0),
            awaiting,
            multi_account: false,
            list_installed_error: false,
            last_authenticated_server_id: Mutex::new(None),
        }
    }

    fn with_list_installed_error(mut self) -> Self {
        self.list_installed_error = true;
        self
    }
}

impl McpManagementSink for FakeManagement {
    fn list_plugins(&self) -> Result<Vec<McpPluginSummary>, ProviderSessionError> {
        Ok(self.plugins.clone())
    }

    fn get_plugin(&self, plugin_id: &str) -> Result<Option<McpPluginDetail>, ProviderSessionError> {
        Ok(self.plugins.iter().find(|plugin| plugin.plugin_id == plugin_id).cloned().map(|summary| {
            McpPluginDetail {
                summary,
                fields: vec![McpPluginField {
                    key: "TOKEN".into(),
                    label: "API token".into(),
                    is_required: false,
                    is_secret: true,
                }],
                servers: self.installed.lock().expect("installed").clone(),
            }
        }))
    }

    fn install_plugin(
        &self,
        _plugin_id: &str,
        _values: Option<&BTreeMap<String, String>>,
    ) -> Result<(), ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn add_server(
        &self,
        name: &str,
        _config_json: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        let mut installed = self.installed.lock().expect("installed");
        installed.push(McpInstalledServer {
            id: "17".into(),
            server_identifier: "custom".into(),
            name: name.into(),
            status: "needsAuth".into(),
            account_key: "default".into(),
            transport: "http".into(),
            tool_count: 0,
            disabled_tool_count: None,
            plugin_id: None,
            status_detail: None,
            custom_instructions: String::new(),
            is_team_server: Some(false),
        });
        Ok(installed.clone())
    }

    fn list_installed(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        if self.list_installed_error {
            return Err(ProviderSessionError::Tool("installed listing unavailable".into()));
        }
        Ok(self.installed.lock().expect("installed").clone())
    }

    fn remove_server(&self, server_id: &str) -> Result<McpRemoveServerResult, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        let mut installed = self.installed.lock().expect("installed");
        let before = installed.len();
        installed.retain(|server| server.id != server_id);
        Ok(McpRemoveServerResult {
            removed: installed.len() != before,
            reason: None,
            servers: installed.clone(),
        })
    }

    fn uninstall_plugin(&self, _plugin_id: &str) -> Result<McpUninstallPluginResult, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(McpUninstallPluginResult { removed: true, reason: None })
    }

    fn set_instructions(
        &self,
        server_id: &str,
        instructions: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        let mut installed = self.installed.lock().expect("installed");
        for server in &mut *installed {
            if server.id == server_id {
                server.custom_instructions = instructions.into();
            }
        }
        Ok(installed.clone())
    }

    fn restart(&self) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(self.installed.lock().expect("installed").clone())
    }

    fn authenticate(
        &self,
        server_id: &str,
        _account_key: &str,
        _requesting_agent_id: Option<&str>,
        _force_reauth: bool,
    ) -> Result<McpAuthenticationResult, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        *self
            .last_authenticated_server_id
            .lock()
            .expect("last authenticated server id") = Some(server_id.to_string());
        Ok(McpAuthenticationResult::Started {
            server_name: "Custom".into(),
        })
    }

    fn remove_account(
        &self,
        _server_id: &str,
        _account_key: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(self.installed.lock().expect("installed").clone())
    }

    fn rename_account(
        &self,
        _server_id: &str,
        _account_key: &str,
        _new_account_key: &str,
    ) -> Result<Vec<McpInstalledServer>, ProviderSessionError> {
        self.mutations.fetch_add(1, Ordering::SeqCst);
        Ok(self.installed.lock().expect("installed").clone())
    }

    fn requesting_agent_id(&self) -> Option<String> {
        Some("agent-a".into())
    }

    fn is_awaiting_user_selection(&self) -> bool {
        self.awaiting
    }

    fn is_multi_account_enabled(&self) -> bool {
        self.multi_account
    }

    fn emit_connector_card(&self, card: ConnectorCard) -> Result<(), ProviderSessionError> {
        self.cards.lock().expect("cards").push(card);
        Ok(())
    }
}

fn plugin(id: &str, display_name: &str, description: &str, category: &str) -> McpPluginSummary {
    McpPluginSummary {
        plugin_id: id.into(),
        name: id.into(),
        display_name: display_name.into(),
        description: description.into(),
        category: category.into(),
        is_installed: false,
        install_mode: None,
        connector_count: 1,
        skills: vec![McpPluginSkill {
            name: format!("{display_name} helper"),
            description: String::new(),
        }],
    }
}

#[test]
fn plugin_search_and_remote_url_validation_match_frozen_rules() {
    let management = FakeManagement::new(false);
    let ranked = rank_plugins_lexically(&management.plugins, "manage linear issues");
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].plugin_id, "linear");

    assert!(validate_remote_mcp_url("https://example.com/mcp").is_none());
    assert!(validate_remote_mcp_url("file:///tmp/server").is_some());
    assert!(validate_remote_mcp_url("https://user:secret@example.com/mcp").is_some());

    let mut headers = BTreeMap::new();
    headers.insert("Authorization".into(), "Bearer token".into());
    let config = build_server_config_json(
        Some("https://example.com/mcp"),
        Some(&headers),
    ).expect("config");
    let parsed: Value = serde_json::from_str(&config).expect("json");
    assert_eq!(parsed["type"], "http");
    assert_eq!(parsed["url"], "https://example.com/mcp");
    assert_eq!(parsed["headers"]["Authorization"], "Bearer token");
}

#[test]
fn management_bridge_routes_read_only_and_mutating_tools_and_emits_auth_cards() {
    let management = Arc::new(FakeManagement::new(false));
    let sink: Arc<dyn McpManagementSink> = management.clone();
    let bridge = McpManagementToolBridge::new(Arc::new(BaseBridge), sink);
    let tools = bridge.list_tools().expect("tools");

    let search = tools.iter().find(|tool| tool.name == SEARCH_PLUGINS_TOOL_NAME).expect("search");
    let search_result = bridge.call_tool(
        search,
        json!({"query":"linear"}),
        "tool-search",
    ).expect("search result");
    assert!(search_result.as_str().is_some_and(|text| text.contains("Linear")));

    let add = tools.iter().find(|tool| tool.name == ADD_MCP_SERVER_TOOL_NAME).expect("add");
    let add_result = bridge.call_tool(
        add,
        json!({"name":"Custom","url":"https://example.com/mcp"}),
        "tool-add",
    ).expect("add result");
    assert!(add_result.as_str().is_some_and(|text| text.contains("needs authentication")));
    assert_eq!(management.cards.lock().expect("cards").len(), 1);
    assert_eq!(
        management.cards.lock().expect("cards")[0].variant,
        ConnectorCardVariant::Connect
    );

    let status = tools.iter().find(|tool| tool.name == GET_MCP_SERVER_STATUS_TOOL_NAME).expect("status");
    let status_result = bridge.call_tool(
        status,
        json!({"server_id":"custom"}),
        "tool-status",
    ).expect("status result");
    assert!(status_result.as_str().is_some_and(|text| text.contains("[needsAuth]")));

    let auth = tools.iter().find(|tool| tool.name == AUTHENTICATE_MCP_SERVER_TOOL_NAME).expect("auth");
    let auth_result = bridge.call_tool(
        auth,
        json!({"server_id":"custom"}),
        "tool-auth",
    ).expect("auth result");
    assert!(auth_result.as_str().is_some_and(|text| text.contains("Authentication started")));
    assert_eq!(management.cards.lock().expect("cards").len(), 2);

    let base = tools.iter().find(|tool| tool.name == "base_tool").expect("base");
    assert_eq!(
        bridge.call_tool(base, json!({}), "tool-base").expect("delegate"),
        Value::String("delegated".into())
    );
}

#[test]
fn pending_user_selection_blocks_configuration_mutations() {
    let management = Arc::new(FakeManagement::new(true));
    let sink: Arc<dyn McpManagementSink> = management.clone();
    let bridge = McpManagementToolBridge::new(Arc::new(BaseBridge), sink);
    let tools = bridge.list_tools().expect("tools");
    let add = tools.iter().find(|tool| tool.name == ADD_MCP_SERVER_TOOL_NAME).expect("add");

    let result = bridge.call_tool(
        add,
        json!({"name":"Custom","url":"https://example.com/mcp"}),
        "tool-add",
    ).expect("guarded result");
    assert!(result.as_str().is_some_and(|text| text.contains("waiting on the user's selection")));
    assert_eq!(management.mutations.load(Ordering::SeqCst), 0);
    assert!(management.installed.lock().expect("installed").is_empty());
}


#[test]
fn hostile_account_labels_are_escaped_like_the_frozen_shared_mcp_helper() {
    assert_eq!(
        encode_mcp_account_label_for_listing("alpha\n\"'`\\[]{}()<>\u{2028}\u{2029}"),
        "\"alpha\\u000a\\u0022\\u0027\\u0060\\u005c\\u005b\\u005d\\u007b\\u007d\\u0028\\u0029\\u003c\\u003e\\u2028\\u2029\""
    );
}

#[test]
fn server_resolution_fails_soft_when_the_installed_listing_is_unreadable() {
    let management = Arc::new(FakeManagement::new(false).with_list_installed_error());
    let sink: Arc<dyn McpManagementSink> = management.clone();
    let bridge = McpManagementToolBridge::new(Arc::new(BaseBridge), sink);
    let tools = bridge.list_tools().expect("tools");
    let auth = tools
        .iter()
        .find(|tool| tool.name == AUTHENTICATE_MCP_SERVER_TOOL_NAME)
        .expect("auth");

    let result = bridge
        .call_tool(auth, json!({"server_id":"legacy-token"}), "tool-auth")
        .expect("fail-soft auth");
    assert!(result.as_str().is_some_and(|text| text.contains("Authentication started")));
    assert_eq!(
        management
            .last_authenticated_server_id
            .lock()
            .expect("last authenticated server id")
            .as_deref(),
        Some("legacy-token")
    );
}

#[test]
fn production_management_sink_requires_explicit_turn_interaction_callbacks() {
    let source = include_str!("../src/runner/tools/mcp_host_service_management_sink.rs");
    let main = include_str!("../app/src/main.rs");

    assert!(source.contains("with_interaction_callbacks"));
    assert!(source.contains("fn is_awaiting_user_selection(&self) -> bool"));
    assert!(source.contains("fn emit_connector_card(&self, card: ConnectorCard)"));
    assert!(main.contains("mcp_awaiting_sink.is_awaiting_user_selection()"));
    assert!(main.contains("mcp_connector_sink"));
    assert!(main.contains("\"type\": \"connector\""));
    assert!(main.contains("mcp_management_sink: Some(mcp_management_sink)"));
}
