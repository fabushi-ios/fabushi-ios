//! Stdio MCP Tools Server, Lifecycle & Tool Execution Filtering
//!
//! Mirrors upstream OpenClaw `src/mcp/tools-stdio-server.ts` & `plugin-tools-serve.ts`:
//! Manages stdio MCP tool server lifecycles, drains in-flight tool tasks on close,
//! and filters registered plugin tools through tool profile allow/deny policies.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use serde::{Deserialize, Serialize};

/// Tool declaration exposed over MCP tools/list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Tool execution policy filter for MCP sessions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpToolPolicy {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl McpToolPolicy {
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        if self.deny.iter().any(|d| d == tool_name || d == "*") {
            return false;
        }
        if self.allow.is_empty() {
            return true;
        }
        self.allow.iter().any(|a| a == tool_name || a == "*")
    }
}

/// Server lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerState {
    Ready,
    Serving,
    Closing,
    Closed,
}

/// Stdio MCP server lifecycle controller with active work draining.
#[derive(Debug, Clone)]
pub struct McpToolsServerLifecycle {
    closing: Arc<AtomicBool>,
    active_requests: Arc<std::sync::atomic::AtomicUsize>,
}

impl Default for McpToolsServerLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl McpToolsServerLifecycle {
    pub fn new() -> Self {
        Self {
            closing: Arc::new(AtomicBool::new(false)),
            active_requests: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    pub fn active_count(&self) -> usize {
        self.active_requests.load(Ordering::SeqCst)
    }

    /// Acquires a slot for request execution. Fails if server is closing.
    pub fn enter_request(&self) -> Result<(), &'static str> {
        if self.is_closing() {
            return Err("Connection closed");
        }
        self.active_requests.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Releases a slot after request completion.
    pub fn exit_request(&self) {
        self.active_requests.fetch_sub(1, Ordering::SeqCst);
    }

    /// Initiates graceful close.
    pub fn close(&self) {
        self.closing.store(true, Ordering::SeqCst);
    }
}

/// Filters registered tools according to policy.
pub fn filter_tools_for_mcp(
    tools: &[McpToolDescriptor],
    policy: &McpToolPolicy,
) -> Vec<McpToolDescriptor> {
    let mut seen = BTreeSet::new();
    let mut filtered = Vec::new();

    for t in tools {
        if policy.is_allowed(&t.name) && seen.insert(t.name.clone()) {
            filtered.push(t.clone());
        }
    }

    filtered
}
