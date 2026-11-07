//! Generation-fenced node invocation admission and terminal settlement.
//!
//! This registry owns only correlation and lifecycle state. Command side
//! effects remain with the existing Host owners, and the Gateway transport
//! remains responsible for framing and delivery.

use super::session_registry::NodeHostSessionLease;
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const MAX_ACTIVE_INVOCATIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeInvocationState {
    Running,
    Cancelled,
    Settled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InvocationRecord {
    node_id: String,
    generation: u64,
    state: NodeInvocationState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInvocationAdmission {
    pub invoke_id: String,
    pub node_id: String,
    pub generation: u64,
}

#[derive(Debug, Default)]
pub struct NodeHostInvocationRegistry {
    records: Mutex<BTreeMap<(String, String), InvocationRecord>>,
}

impl NodeHostInvocationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn admit(
        &self,
        lease: &NodeHostSessionLease,
        invoke_id: impl Into<String>,
    ) -> Result<NodeInvocationAdmission, String> {
        let invoke_id = invoke_id.into();
        if invoke_id.trim().is_empty() {
            return Err("node invocation id must not be empty".into());
        }
        let key = (lease.node_id.clone(), invoke_id.clone());
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = records.get(&key) {
            let can_replace_drained_predecessor =
                existing.state == NodeInvocationState::Cancelled
                    && existing.generation != lease.generation;
            if !can_replace_drained_predecessor {
                return Err("duplicate node invocation id".into());
            }
            records.remove(&key);
        }
        if records
            .values()
            .filter(|record| record.state == NodeInvocationState::Running)
            .count()
            >= MAX_ACTIVE_INVOCATIONS
        {
            return Err("node invocation concurrency limit reached".into());
        }
        records.insert(
            key,
            InvocationRecord {
                node_id: lease.node_id.clone(),
                generation: lease.generation,
                state: NodeInvocationState::Running,
            },
        );
        Ok(NodeInvocationAdmission {
            invoke_id,
            node_id: lease.node_id.clone(),
            generation: lease.generation,
        })
    }

    pub fn cancel(&self, admission: &NodeInvocationAdmission) -> bool {
        self.transition(admission, NodeInvocationState::Cancelled)
    }

    pub fn settle(&self, admission: &NodeInvocationAdmission) -> bool {
        self.transition(admission, NodeInvocationState::Settled)
    }

    pub fn is_running(&self, admission: &NodeInvocationAdmission) -> bool {
        let key = (admission.node_id.clone(), admission.invoke_id.clone());
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .is_some_and(|record| {
                record.generation == admission.generation
                    && record.state == NodeInvocationState::Running
            })
    }

    pub fn drain_generation(&self, node_id: &str, generation: u64) -> Vec<String> {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let keys = records
            .iter()
            .filter(|(_, record)| {
                record.node_id == node_id
                    && record.generation == generation
                    && record.state == NodeInvocationState::Running
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in &keys {
            if let Some(record) = records.get_mut(key) {
                record.state = NodeInvocationState::Cancelled;
            }
        }
        keys.into_iter().map(|(_, invoke_id)| invoke_id).collect()
    }

    fn transition(&self, admission: &NodeInvocationAdmission, next: NodeInvocationState) -> bool {
        let key = (admission.node_id.clone(), admission.invoke_id.clone());
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(record) = records.get_mut(&key) else {
            return false;
        };
        if record.generation != admission.generation || record.state != NodeInvocationState::Running {
            return false;
        }
        record.state = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeHostInvocationRegistry, MAX_ACTIVE_INVOCATIONS};
    use crate::extensions::node_host::session_registry::{
        NodeHostSessionMetadata, NodeHostSessionRegistry,
    };

    fn lease() -> (NodeHostSessionRegistry, crate::extensions::node_host::session_registry::NodeHostSessionLease) {
        let sessions = NodeHostSessionRegistry::new();
        let lease = sessions.connect(NodeHostSessionMetadata {
            node_id: "node-1".into(),
            display_name: "Node 1".into(),
            platform: "test".into(),
            device_family: "test".into(),
            commands: vec![],
            capabilities: vec![],
            paired: true,
            gateway_local: false,
        });
        (sessions, lease)
    }

    #[test]
    fn duplicate_and_late_settlement_are_rejected() {
        let (_sessions, lease) = lease();
        let registry = NodeHostInvocationRegistry::new();
        let admission = registry.admit(&lease, "invoke-1").expect("admit");
        assert!(registry.admit(&lease, "invoke-1").is_err());
        assert!(registry.settle(&admission));
        assert!(!registry.settle(&admission));
    }

    #[test]
    fn stale_generation_cannot_settle_successor_invocation() {
        let (sessions, old_lease) = lease();
        let registry = NodeHostInvocationRegistry::new();
        let old = registry.admit(&old_lease, "old").expect("admit old");
        assert_eq!(registry.drain_generation(&old_lease.node_id, old_lease.generation), vec!["old"]);
        let new_lease = sessions.connect(NodeHostSessionMetadata {
            node_id: "node-1".into(),
            display_name: "Node 1".into(),
            platform: "test".into(),
            device_family: "test".into(),
            commands: vec![],
            capabilities: vec![],
            paired: true,
            gateway_local: false,
        });
        assert!(!registry.settle(&old));
        let successor = registry.admit(&new_lease, "new").expect("admit successor");
        assert!(registry.cancel(&successor));
    }

    #[test]
    fn drained_predecessor_allows_same_invocation_id_on_new_generation() {
        let (sessions, old_lease) = lease();
        let registry = NodeHostInvocationRegistry::new();
        let old = registry.admit(&old_lease, "same").expect("admit old");
        assert_eq!(registry.drain_generation(&old.node_id, old.generation), vec!["same"]);

        let new_lease = sessions.connect(NodeHostSessionMetadata {
            node_id: "node-1".into(),
            display_name: "Node 1".into(),
            platform: "test".into(),
            device_family: "test".into(),
            commands: vec![],
            capabilities: vec![],
            paired: true,
            gateway_local: false,
        });
        assert!(registry.admit(&new_lease, "same").is_ok());
    }

    #[test]
    fn cancelled_invocation_cannot_be_reused_in_same_generation() {
        let (_sessions, lease) = lease();
        let registry = NodeHostInvocationRegistry::new();
        let admission = registry.admit(&lease, "same").expect("admit");
        assert!(registry.cancel(&admission));
        assert!(registry.admit(&lease, "same").is_err());
    }

    #[test]
    fn disconnect_drain_cancels_only_matching_generation() {
        let (_sessions, lease) = lease();
        let registry = NodeHostInvocationRegistry::new();
        let first = registry.admit(&lease, "first").expect("admit first");
        let second = registry.admit(&lease, "second").expect("admit second");
        let drained = registry.drain_generation(&lease.node_id, lease.generation);
        assert_eq!(drained.len(), 2);
        assert!(!registry.settle(&first));
        assert!(!registry.cancel(&second));
    }

    #[test]
    fn active_invocation_limit_is_bounded_and_reopens_after_settlement() {
        let (_sessions, lease) = lease();
        let registry = NodeHostInvocationRegistry::new();
        let admissions = (0..MAX_ACTIVE_INVOCATIONS)
            .map(|index| registry.admit(&lease, format!("invoke-{index}")))
            .collect::<Result<Vec<_>, _>>()
            .expect("limit should admit its bounded capacity");
        assert!(registry.admit(&lease, "over-capacity").is_err());
        assert!(registry.settle(&admissions[0]));
        assert!(registry.admit(&lease, "after-settlement").is_ok());
    }
}
