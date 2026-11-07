//! Live node-host session facts owned by the Host transport boundary.
//!
//! Pairing approval and RPC projection deliberately do not mutate this
//! registry. A transport/session owner admits a connection, receives a
//! generation-fenced lease, and must use that lease to publish activity or
//! retire the session. This keeps predecessor callbacks from changing a
//! replacement session and prevents paired-but-disconnected devices from
//! being advertised as live nodes.

use std::collections::BTreeMap;
use std::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostSessionMetadata {
    pub node_id: String,
    pub display_name: String,
    pub platform: String,
    pub device_family: String,
    pub commands: Vec<String>,
    pub capabilities: Vec<String>,
    pub paired: bool,
    pub gateway_local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostSessionSnapshot {
    pub node_id: String,
    pub display_name: String,
    pub platform: String,
    pub device_family: String,
    pub commands: Vec<String>,
    pub capabilities: Vec<String>,
    pub connected: bool,
    pub paired: bool,
    pub active: bool,
    pub gateway_local: bool,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostSessionLease {
    pub node_id: String,
    pub generation: u64,
}

#[derive(Debug, Default)]
struct RegistryState {
    next_generation: u64,
    sessions: BTreeMap<String, NodeHostSessionSnapshot>,
}

#[derive(Debug, Default)]
pub struct NodeHostSessionRegistry {
    state: RwLock<RegistryState>,
}

impl NodeHostSessionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit one live transport session and retire any predecessor for the
    /// same node id before publishing the replacement.
    pub fn connect(&self, metadata: NodeHostSessionMetadata) -> NodeHostSessionLease {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.next_generation = state.next_generation.saturating_add(1).max(1);
        let generation = state.next_generation;
        let node_id = metadata.node_id.clone();
        state.sessions.insert(
            node_id.clone(),
            NodeHostSessionSnapshot {
                node_id: node_id.clone(),
                display_name: metadata.display_name,
                platform: metadata.platform,
                device_family: metadata.device_family,
                commands: metadata.commands,
                capabilities: metadata.capabilities,
                connected: true,
                paired: metadata.paired,
                active: false,
                gateway_local: metadata.gateway_local,
                generation,
            },
        );
        NodeHostSessionLease {
            node_id,
            generation,
        }
    }

    /// Mark activity only when the caller still owns the current generation.
    pub fn set_active(&self, lease: &NodeHostSessionLease, active: bool) -> bool {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = state.sessions.get_mut(&lease.node_id) else {
            return false;
        };
        if session.generation != lease.generation || !session.connected {
            return false;
        }
        session.active = active;
        true
    }

    pub fn update_manifest(
        &self,
        lease: &NodeHostSessionLease,
        commands: Vec<String>,
        capabilities: Vec<String>,
    ) -> bool {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = state.sessions.get_mut(&lease.node_id) else {
            return false;
        };
        if session.generation != lease.generation || !session.connected {
            return false;
        }
        session.commands = commands;
        session.capabilities = capabilities;
        true
    }

    /// Check that a lease still owns the connected session generation.
    pub fn is_current(&self, lease: &NodeHostSessionLease) -> bool {
        let state = self
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .sessions
            .get(&lease.node_id)
            .is_some_and(|session| session.connected && session.generation == lease.generation)
    }

    /// Retire only the session represented by this lease. A late predecessor
    /// callback cannot disconnect a successor with a newer generation.
    pub fn disconnect(&self, lease: &NodeHostSessionLease) -> bool {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(session) = state.sessions.get_mut(&lease.node_id) else {
            return false;
        };
        if session.generation != lease.generation {
            return false;
        }
        session.connected = false;
        session.active = false;
        session.commands.clear();
        session.capabilities.clear();
        true
    }

    pub fn get(&self, node_id: &str) -> Option<NodeHostSessionSnapshot> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .sessions
            .get(node_id)
            .cloned()
    }

    pub fn list(&self) -> Vec<NodeHostSessionSnapshot> {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .sessions
            .values()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeHostSessionMetadata, NodeHostSessionRegistry};

    fn metadata(node_id: &str) -> NodeHostSessionMetadata {
        NodeHostSessionMetadata {
            node_id: node_id.into(),
            display_name: node_id.into(),
            platform: "test".into(),
            device_family: "test".into(),
            commands: vec!["system.run".into()],
            capabilities: vec!["test.capability".into()],
            paired: true,
            gateway_local: false,
        }
    }

    #[test]
    fn replacement_generation_fences_predecessor_disconnect() {
        let registry = NodeHostSessionRegistry::new();
        let predecessor = registry.connect(metadata("node-1"));
        let successor = registry.connect(metadata("node-1"));

        assert!(!registry.disconnect(&predecessor));
        assert!(registry.set_active(&successor, true));
        assert_eq!(registry.get("node-1").map(|node| node.generation), Some(successor.generation));
        assert_eq!(registry.get("node-1").map(|node| node.active), Some(true));
    }

    #[test]
    fn disconnect_clears_advertised_surface() {
        let registry = NodeHostSessionRegistry::new();
        let lease = registry.connect(metadata("node-2"));

        assert!(registry.set_active(&lease, true));
        assert!(registry.disconnect(&lease));
        let node = registry.get("node-2").expect("retired node remains observable");
        assert!(!node.connected);
        assert!(!node.active);
        assert!(node.commands.is_empty());
        assert!(node.capabilities.is_empty());
    }
}

