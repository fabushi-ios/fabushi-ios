//! Canonical Host composition for node sessions and invocation generations.
//!
//! This is a narrow owner boundary: it couples session retirement to
//! invocation draining without owning sockets, credentials, or side effects.

use super::invocation_registry::{NodeHostInvocationRegistry, NodeInvocationAdmission};
use super::invoke_protocol::{NodeInvokeCancel, NodeInvokeRequest};
use super::session_registry::{
    NodeHostSessionLease, NodeHostSessionMetadata, NodeHostSessionRegistry,
    NodeHostSessionSnapshot,
};
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct NodeHostRuntimeState {
    sessions: Arc<NodeHostSessionRegistry>,
    invocations: Arc<NodeHostInvocationRegistry>,
}

impl NodeHostRuntimeState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn connect(&self, metadata: NodeHostSessionMetadata) -> NodeHostSessionLease {
        if let Some(previous) = self.sessions.get(&metadata.node_id) {
            let _ = self
                .invocations
                .drain_generation(&previous.node_id, previous.generation);
        }
        self.sessions.connect(metadata)
    }

    pub fn disconnect(&self, lease: &NodeHostSessionLease) -> Vec<String> {
        let drained = self
            .invocations
            .drain_generation(&lease.node_id, lease.generation);
        let _ = self.sessions.disconnect(lease);
        drained
    }

    pub fn set_active(&self, lease: &NodeHostSessionLease, active: bool) -> bool {
        self.sessions.set_active(lease, active)
    }

    pub fn update_manifest(
        &self,
        lease: &NodeHostSessionLease,
        commands: Vec<String>,
        capabilities: Vec<String>,
    ) -> bool {
        self.sessions.update_manifest(lease, commands, capabilities)
    }

    pub fn is_current(&self, lease: &NodeHostSessionLease) -> bool {
        self.sessions.is_current(lease)
    }

    pub fn get(&self, node_id: &str) -> Option<NodeHostSessionSnapshot> {
        self.sessions.get(node_id)
    }

    pub fn list(&self) -> Vec<NodeHostSessionSnapshot> {
        self.sessions.list()
    }

    pub fn admit_invocation(
        &self,
        lease: &NodeHostSessionLease,
        invoke_id: impl Into<String>,
    ) -> Result<NodeInvocationAdmission, String> {
        if !self.sessions.is_current(lease) {
            return Err("node invocation session is no longer current".into());
        }
        let admission = self.invocations.admit(lease, invoke_id)?;
        if !self.sessions.is_current(lease) {
            let _ = self.invocations.cancel(&admission);
            return Err("node invocation session retired during admission".into());
        }
        Ok(admission)
    }

    /// Admit a parsed wire request only after binding its node identity to the
    /// live session lease. Transport callers should prefer this over the
    /// lower-level id-only method so a request cannot cross node boundaries.
    pub fn admit_request(
        &self,
        lease: &NodeHostSessionLease,
        request: &NodeInvokeRequest,
    ) -> Result<NodeInvocationAdmission, String> {
        if request.node_id != lease.node_id {
            return Err("node invocation request does not belong to the session".into());
        }
        let session = self
            .sessions
            .get(&lease.node_id)
            .filter(|session| session.connected && session.generation == lease.generation)
            .ok_or_else(|| "node invocation session is no longer current".to_string())?;
        if !session
            .commands
            .iter()
            .any(|command| command == &request.command)
        {
            return Err("node invocation command is not advertised by the current session".into());
        }
        self.admit_invocation(lease, &request.id)
    }

    /// Cancel an invocation only when the caller still owns the admitted
    /// session generation. Transport/session owners use this before sending
    /// a cancel frame; a predecessor or duplicate callback cannot cancel a
    /// successor invocation with the same id.
    pub fn cancel_invocation(&self, admission: &NodeInvocationAdmission) -> bool {
        self.invocations.cancel(admission)
    }

    /// Bind a parsed cancel frame to the currently owned session before
    /// changing invocation state. A frame for another node cannot cross the
    /// transport/session boundary, even when its invocation id is valid.
    pub fn cancel_invocation_frame(
        &self,
        lease: &NodeHostSessionLease,
        cancel: &NodeInvokeCancel,
    ) -> Result<bool, String> {
        if cancel.node_id != lease.node_id {
            return Err("node invocation cancel does not belong to the session".into());
        }
        let admission = NodeInvocationAdmission {
            invoke_id: cancel.invoke_id.clone(),
            node_id: lease.node_id.clone(),
            generation: lease.generation,
        };
        Ok(self.cancel_invocation(&admission))
    }

    pub fn settle_invocation(&self, admission: &NodeInvocationAdmission) -> bool {
        self.invocations.settle(admission)
    }

    pub fn is_invocation_current(&self, admission: &NodeInvocationAdmission) -> bool {
        self.invocations.is_running(admission)
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeHostRuntimeState, NodeHostSessionMetadata};
    use super::super::invoke_protocol::{NodeInvokeCancel, NodeInvokeRequest};

    fn metadata(node_id: &str) -> NodeHostSessionMetadata {
        NodeHostSessionMetadata {
            node_id: node_id.into(),
            display_name: node_id.into(),
            platform: "test".into(),
            device_family: "test".into(),
            commands: vec!["system.run".into()],
            capabilities: vec![],
            paired: true,
            gateway_local: false,
        }
    }

    #[test]
    fn replacement_drains_predecessor_invocations_before_successor() {
        let runtime = NodeHostRuntimeState::new();
        let previous = runtime.connect(metadata("node-1"));
        let admission = runtime.admit_invocation(&previous, "invoke-1").expect("admit");
        let successor = runtime.connect(metadata("node-1"));
        assert!(!runtime.settle_invocation(&admission));
        assert!(runtime.admit_invocation(&successor, "invoke-1").is_ok());
    }

    #[test]
    fn cancellation_is_generation_fenced_and_exactly_once() {
        let runtime = NodeHostRuntimeState::new();
        let session = runtime.connect(metadata("node-1"));
        let admission = runtime
            .admit_invocation(&session, "invoke-1")
            .expect("admit");

        assert!(runtime.cancel_invocation(&admission));
        assert!(!runtime.cancel_invocation(&admission));
        assert!(!runtime.settle_invocation(&admission));
    }

    #[test]
    fn cancel_frame_cannot_cross_node_session_boundary() {
        let runtime = NodeHostRuntimeState::new();
        let session = runtime.connect(metadata("node-1"));
        let admission = runtime
            .admit_invocation(&session, "invoke-1")
            .expect("admit");
        let cancel = NodeInvokeCancel {
            invoke_id: admission.invoke_id.clone(),
            node_id: "node-2".into(),
        };

        assert!(runtime.cancel_invocation_frame(&session, &cancel).is_err());
        assert!(runtime.settle_invocation(&admission));
    }

    #[test]
    fn request_admission_cannot_cross_node_session_boundary() {
        let runtime = NodeHostRuntimeState::new();
        let session = runtime.connect(metadata("node-1"));
        let request = NodeInvokeRequest {
            id: "invoke-1".into(),
            node_id: "node-2".into(),
            command: "system.run".into(),
            params: serde_json::Value::Null,
            received_params_bytes: 0,
            received_params_json: None,
            timeout_ms: None,
            idempotency_key: None,
            session_key: None,
        };

        assert!(runtime.admit_request(&session, &request).is_err());
    }

    #[test]
    fn request_admission_rejects_command_missing_from_live_manifest() {
        let runtime = NodeHostRuntimeState::new();
        let session = runtime.connect(metadata("node-1"));
        let request = NodeInvokeRequest {
            id: "invoke-1".into(),
            node_id: "node-1".into(),
            command: "not-advertised".into(),
            params: serde_json::Value::Null,
            received_params_bytes: 0,
            received_params_json: None,
            timeout_ms: None,
            idempotency_key: None,
            session_key: None,
        };

        let error = runtime.admit_request(&session, &request).unwrap_err();
        assert!(error.contains("not advertised"));
    }
}
