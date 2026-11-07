use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::registry::{McpToolDefinition, ToolRiskLevel};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPermissionRequest {
    pub request_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub risk_level: ToolRiskLevel,
    pub caller_agent_id: String,
    pub requested_at: DateTime<Utc>,
}

impl ToolPermissionRequest {
    pub fn new(
        request_id: impl Into<String>,
        tool_name: impl Into<String>,
        arguments: serde_json::Value,
        risk_level: ToolRiskLevel,
        caller_agent_id: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            tool_name: tool_name.into(),
            arguments,
            risk_level,
            caller_agent_id: caller_agent_id.into(),
            requested_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPermissionDock {
    pub pending_requests: Vec<String>,
    pub requests_by_id: HashMap<String, ToolPermissionRequest>,
    pub resolved_history: Vec<(String, bool, DateTime<Utc>)>,
}

impl Default for ToolPermissionDock {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolPermissionDock {
    pub fn new() -> Self {
        Self {
            pending_requests: Vec::new(),
            requests_by_id: HashMap::new(),
            resolved_history: Vec::new(),
        }
    }

    pub fn push_request(&mut self, req_id: String) {
        if !self.pending_requests.contains(&req_id) {
            self.pending_requests.push(req_id);
        }
    }

    pub fn submit_request(&mut self, req: ToolPermissionRequest) {
        let req_id = req.request_id.clone();
        if !self.pending_requests.contains(&req_id) {
            self.pending_requests.push(req_id.clone());
        }
        self.requests_by_id.insert(req_id, req);
    }

    pub fn evaluate_risk(&self, tool: &McpToolDefinition, args: &serde_json::Value) -> ToolRiskLevel {
        if tool.risk_level == ToolRiskLevel::CriticalDestructive {
            return ToolRiskLevel::CriticalDestructive;
        }

        const DESTRUCTIVE_KEYWORDS: &[&str] = &["rm", "delete", "drop", "kill", "exec", "write"];

        fn check_value(val: &serde_json::Value) -> bool {
            match val {
                serde_json::Value::String(s) => {
                    let lower = s.to_lowercase();
                    let words: Vec<&str> = lower
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .filter(|w| !w.is_empty())
                        .collect();
                    for kw in DESTRUCTIVE_KEYWORDS {
                        if *kw == "rm" {
                            if words.contains(&"rm") {
                                return true;
                            }
                        } else if lower.contains(kw) {
                            return true;
                        }
                    }
                    false
                }
                serde_json::Value::Array(arr) => arr.iter().any(check_value),
                serde_json::Value::Object(map) => {
                    map.iter().any(|(k, v)| {
                        let k_lower = k.to_lowercase();
                        DESTRUCTIVE_KEYWORDS.iter().any(|kw| {
                            if *kw == "rm" {
                                k_lower.split('_').any(|part| part == "rm")
                            } else {
                                k_lower.contains(kw)
                            }
                        }) || check_value(v)
                    })
                }
                _ => false,
            }
        }

        if check_value(args) {
            ToolRiskLevel::CriticalDestructive
        } else {
            tool.risk_level
        }
    }

    pub fn get_request(&self, req_id: &str) -> Option<&ToolPermissionRequest> {
        self.requests_by_id.get(req_id)
    }

    pub fn list_pending(&self) -> Vec<&ToolPermissionRequest> {
        self.pending_requests
            .iter()
            .filter_map(|id| self.requests_by_id.get(id))
            .collect()
    }

    pub fn pending_count(&self) -> usize {
        self.pending_requests.len()
    }

    pub fn resolve(&mut self, req_id: &str, allow: bool) -> Option<ToolPermissionRequest> {
        self.pending_requests.retain(|id| id != req_id);
        let req = self.requests_by_id.remove(req_id);
        if req.is_some() {
            self.resolved_history.push((req_id.to_string(), allow, Utc::now()));
        }
        req
    }
}
