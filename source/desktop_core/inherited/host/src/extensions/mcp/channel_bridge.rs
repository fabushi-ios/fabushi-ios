//! MCP Channel Bridge, Queue Management & Claude Desktop Permission Reply Parsing
//!
//! Mirrors upstream OpenClaw `src/mcp/channel-bridge.ts` & `channel-shared.ts`:
//! Bridges MCP server interactions to the Gateway client, queues events up to 1000 items,
//! parses Claude desktop shortcode permission replies, and manages pending approval TTLs.

use std::collections::VecDeque;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub const MCP_QUEUE_LIMIT: usize = 1_000;
pub const PENDING_CLAUDE_PERMISSION_TTL_MS: u64 = 60 * 60 * 1_000; // 1 hour
pub const PENDING_APPROVAL_DEFAULT_TTL_MS: u64 = 30 * 60 * 1_000; // 30 mins
pub const PENDING_SWEEP_INTERVAL_MS: u64 = 5 * 60 * 1_000; // 5 mins

static RE_CLAUDE_PERMISSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(yes|no)\s+([a-km-z]{5})$").unwrap()
});

/// Mode for Claude channel behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClaudeChannelMode {
    Off,
    On,
    Auto,
}

impl Default for ClaudeChannelMode {
    fn default() -> Self {
        Self::Auto
    }
}

/// Parses a Claude Desktop inline permission reply:
/// Matches `^(yes|no)\s+([a-km-z]{5})$` (case-insensitive, 5-letter alphabet excluding 'l').
/// Returns `Some((is_approved, code))`.
pub fn parse_claude_permission_reply(text: &str) -> Option<(bool, String)> {
    let trimmed = text.trim();
    if let Some(caps) = RE_CLAUDE_PERMISSION.captures(trimmed) {
        let decision_str = caps.get(1)?.as_str().to_lowercase();
        let code = caps.get(2)?.as_str().to_lowercase();
        let is_approved = decision_str == "yes";
        return Some((is_approved, code));
    }
    None
}

/// Queued channel event for polling clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpQueueEvent {
    pub id: String,
    pub event_type: String,
    pub timestamp_ms: u64,
    pub payload: serde_json::Value,
}

/// Pending approval tracked in bridge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPendingApproval {
    pub approval_id: String,
    pub channel: String,
    pub session_key: String,
    pub code: Option<String>,
    pub tracked_at_ms: u64,
    pub expires_at_ms: u64,
}

impl McpPendingApproval {
    pub fn is_expired(&self, current_time_ms: u64) -> bool {
        current_time_ms >= self.expires_at_ms
    }
}

/// In-memory bounded queue and approval tracker.
#[derive(Debug, Clone, Default)]
pub struct McpChannelBridgeState {
    pub events: VecDeque<McpQueueEvent>,
    pub approvals: Vec<McpPendingApproval>,
}

impl McpChannelBridgeState {
    pub fn new() -> Self {
        Self {
            events: VecDeque::with_capacity(MCP_QUEUE_LIMIT),
            approvals: Vec::new(),
        }
    }

    /// Enqueues an event, evicting the oldest if limit is reached.
    pub fn push_event(&mut self, event: McpQueueEvent) {
        if self.events.len() >= MCP_QUEUE_LIMIT {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// Prunes expired approvals.
    pub fn sweep_expired_approvals(&mut self, current_time_ms: u64) -> usize {
        let before = self.approvals.len();
        self.approvals.retain(|a| !a.is_expired(current_time_ms));
        before - self.approvals.len()
    }
}
