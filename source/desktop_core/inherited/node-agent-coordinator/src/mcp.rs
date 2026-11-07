use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::Failure;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpRoute {
    pub server_name: String,
    pub transport: String,
    pub generation: u64,
}

#[derive(Debug, Default)]
pub struct RoutedMcpBridge {
    routes: HashMap<String, McpRoute>,
}

impl RoutedMcpBridge {
    pub fn upsert(&mut self, route: McpRoute) {
        self.routes.insert(route.server_name.clone(), route);
    }

    pub fn remove(&mut self, server_name: &str) {
        self.routes.remove(server_name);
    }

    pub fn route_tool_call(
        &self,
        server_name: &str,
        tool_name: &str,
        args: Value,
    ) -> Result<(McpRoute, String, Value), Failure> {
        if tool_name.trim().is_empty() {
            return Err(Failure::new("MCP_INVALID_TOOL", "tool name is empty"));
        }
        let route = self.routes.get(server_name).cloned().ok_or_else(|| {
            Failure::new("MCP_SERVER_UNAVAILABLE", format!("MCP server {server_name} is not routed"))
        })?;
        Ok((route, tool_name.to_string(), args))
    }
}
