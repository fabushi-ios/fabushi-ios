use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use mahayana_host_runtime::runner::subagent_runtime::SubagentLineage as HostSubagentLineage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentLifecycleStatus {
    Pending,
    Running,
    Done,
    Error,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentArtifactReceipt {
    pub receipt_id: String,
    pub title: String,
    pub path: Option<String>,
    pub summary: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentNode {
    pub agent_id: String,
    pub parent_id: Option<String>,
    pub root_parent_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub role: String,
    pub status: SubagentLifecycleStatus,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub tool_call_count: usize,
    pub artifacts: Vec<SubagentArtifactReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentTreeNode {
    pub node: SubagentNode,
    pub children: Vec<SubagentTreeNode>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SubagentLineage {
    pub nodes: HashMap<String, SubagentNode>,
}

impl SubagentLineage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_agent(&mut self, agent_id: String, parent_id: Option<String>, role: String) {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        let root_parent_id = parent_id.as_ref().map(|p| self.find_root_id(p));
        self.nodes.insert(
            agent_id.clone(),
            SubagentNode {
                agent_id,
                parent_id,
                root_parent_id,
                tool_call_id: None,
                role,
                status: SubagentLifecycleStatus::Running,
                started_at_ms: now_ms,
                completed_at_ms: None,
                tool_call_count: 0,
                artifacts: Vec::new(),
            },
        );
    }

    /// Add a subagent wrapping inherited HostSubagentLineage
    pub fn add_subagent_with_lineage(
        &mut self,
        agent_id: String,
        host_lineage: &HostSubagentLineage,
        role: String,
    ) {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        self.nodes.insert(
            agent_id.clone(),
            SubagentNode {
                agent_id,
                parent_id: host_lineage.parent_request_id.clone(),
                root_parent_id: host_lineage.root_parent_request_id.clone(),
                tool_call_id: host_lineage.parent_agent_tool_call_id.clone(),
                role,
                status: SubagentLifecycleStatus::Running,
                started_at_ms: now_ms,
                completed_at_ms: None,
                tool_call_count: 0,
                artifacts: Vec::new(),
            },
        );
    }

    /// Convert a registered subagent into inherited HostSubagentLineage
    pub fn to_host_lineage(&self, agent_id: &str) -> Option<HostSubagentLineage> {
        let node = self.nodes.get(agent_id)?;
        Some(HostSubagentLineage {
            parent_request_id: node.parent_id.clone(),
            root_parent_request_id: node.root_parent_id.clone(),
            parent_agent_tool_call_id: node.tool_call_id.clone(),
        })
    }

    pub fn find_root_id(&self, agent_id: &str) -> String {
        let mut current = agent_id;
        while let Some(node) = self.nodes.get(current) {
            if let Some(parent) = &node.parent_id {
                current = parent;
            } else {
                return current.to_string();
            }
        }
        current.to_string()
    }

    pub fn mark_completed(&mut self, agent_id: &str, status: SubagentLifecycleStatus) {
        if let Some(node) = self.nodes.get_mut(agent_id) {
            node.status = status;
            node.completed_at_ms = Some(chrono::Utc::now().timestamp_millis() as u64);
        }
    }

    pub fn add_artifact(&mut self, agent_id: &str, receipt: SubagentArtifactReceipt) {
        if let Some(node) = self.nodes.get_mut(agent_id) {
            node.artifacts.push(receipt);
        }
    }

    pub fn increment_tool_calls(&mut self, agent_id: &str) {
        if let Some(node) = self.nodes.get_mut(agent_id) {
            node.tool_call_count = node.tool_call_count.saturating_add(1);
        }
    }

    pub fn get_node(&self, agent_id: &str) -> Option<&SubagentNode> {
        self.nodes.get(agent_id)
    }

    /// Builds hierarchical tree nodes starting from root agents
    pub fn build_execution_tree(&self) -> Vec<SubagentTreeNode> {
        let mut roots = Vec::new();
        for node in self.nodes.values() {
            if node.parent_id.is_none() {
                roots.push(self.build_subtree(&node.agent_id));
            }
        }
        roots.sort_by(|a, b| a.node.agent_id.cmp(&b.node.agent_id));
        roots
    }

    pub fn build_subtree(&self, agent_id: &str) -> SubagentTreeNode {
        let node = self
            .nodes
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| SubagentNode {
                agent_id: agent_id.to_string(),
                parent_id: None,
                root_parent_id: None,
                tool_call_id: None,
                role: "unknown".into(),
                status: SubagentLifecycleStatus::Error,
                started_at_ms: 0,
                completed_at_ms: None,
                tool_call_count: 0,
                artifacts: Vec::new(),
            });

        let mut children = Vec::new();
        for candidate in self.nodes.values() {
            if candidate.parent_id.as_deref() == Some(agent_id) {
                children.push(self.build_subtree(&candidate.agent_id));
            }
        }
        children.sort_by(|a, b| a.node.agent_id.cmp(&b.node.agent_id));

        SubagentTreeNode { node, children }
    }
}
