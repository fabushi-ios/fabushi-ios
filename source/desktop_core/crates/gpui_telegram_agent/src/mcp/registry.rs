use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

pub use mahayana_host_runtime::extensions::mcp::coordinator_relay::CoordinatorMcpLifecycleRelay;
pub use mahayana_host_runtime::extensions::mcp::mcp_service::{
    CatalogPlugin, CatalogSkill, McpServerSummary,
};
pub use mahayana_node_agent_coordinator::routed_mcp_bridge::{
    RoutedMcpBackend, RoutedMcpHttpOutcome, RoutedMcpProtocolBridge, RoutedTool, RoutedToolCall,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRiskLevel {
    ReadOnly,
    Low,
    Medium,
    High,
    CriticalDestructive,
}

impl ToolRiskLevel {
    pub const CRITICAL: Self = Self::CriticalDestructive;

    pub fn is_sensitive(&self) -> bool {
        matches!(self, Self::Medium | Self::High | Self::CriticalDestructive)
    }

    pub fn requires_explicit_confirmation(&self) -> bool {
        matches!(self, Self::High | Self::CriticalDestructive)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema_json: String,
    pub requires_approval: bool,
    pub risk_level: ToolRiskLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
}

impl McpToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema_json: impl Into<String>,
        risk_level: ToolRiskLevel,
    ) -> Self {
        let requires_approval = risk_level.requires_explicit_confirmation();
        Self {
            name: name.into(),
            description: description.into(),
            input_schema_json: input_schema_json.into(),
            requires_approval,
            risk_level,
            server_id: None,
        }
    }

    pub fn with_server_id(mut self, server_id: impl Into<String>) -> Self {
        self.server_id = Some(server_id.into());
        self
    }
}

pub struct McpRegistry {
    pub tools: HashMap<String, McpToolDefinition>,
    bridge: RoutedMcpProtocolBridge,
    relay: Option<Arc<CoordinatorMcpLifecycleRelay>>,
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRegistry")
            .field("tools", &self.tools)
            .field("bridge", &self.bridge)
            .field("relay", &self.relay.is_some())
            .finish()
    }
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            bridge: RoutedMcpProtocolBridge::new(),
            relay: None,
        }
    }

    pub fn with_relay(relay: Arc<CoordinatorMcpLifecycleRelay>) -> Self {
        Self {
            tools: HashMap::new(),
            bridge: RoutedMcpProtocolBridge::new(),
            relay: Some(relay),
        }
    }

    pub fn register(&mut self, tool: McpToolDefinition) {
        self.tools.insert(tool.name.clone(), tool);
    }

    pub fn register_routed_tool(
        &mut self,
        routed_tool: RoutedTool,
        requires_approval: bool,
        risk_level: ToolRiskLevel,
        server_id: Option<String>,
    ) {
        let name = routed_tool.name.clone();
        let description = routed_tool.description.clone().unwrap_or_default();
        let input_schema_json = routed_tool
            .input_schema
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "{}".to_string());

        let def = McpToolDefinition {
            name: name.clone(),
            description,
            input_schema_json,
            requires_approval,
            risk_level,
            server_id,
        };
        self.tools.insert(name, def);
    }

    pub fn get(&self, name: &str) -> Option<&McpToolDefinition> {
        self.tools.get(name)
    }

    pub fn list_tools(&self) -> Vec<McpToolDefinition> {
        let mut list: Vec<McpToolDefinition> = self.tools.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    pub fn remove(&mut self, name: &str) -> Option<McpToolDefinition> {
        self.tools.remove(name)
    }

    pub fn bridge(&mut self) -> &mut RoutedMcpProtocolBridge {
        &mut self.bridge
    }

    pub fn bridge_ref(&self) -> &RoutedMcpProtocolBridge {
        &self.bridge
    }

    pub fn relay(&self) -> Option<&Arc<CoordinatorMcpLifecycleRelay>> {
        self.relay.as_ref()
    }

    pub fn set_relay(&mut self, relay: Arc<CoordinatorMcpLifecycleRelay>) {
        self.relay = Some(relay);
    }
}
