//! Lease-bound node invocation lifecycle composition.
//!
//! This owner joins strict request/input/cancel/result protocol values to the
//! canonical session and invocation registries. It deliberately does not own
//! sockets, credentials, command side effects, or result delivery.

use super::invoke_input::NodeInvokeInputBuffer;
use super::invoke_protocol::{
    NodeInvokeCancel, NodeInvokeInput, NodeInvokeRequest, NodeInvokeResult,
};
use super::runtime_state::NodeHostRuntimeState;
use super::session_registry::NodeHostSessionLease;
use std::sync::Arc;

pub struct NodeHostInvocationRuntime {
    runtime: Arc<NodeHostRuntimeState>,
    lease: NodeHostSessionLease,
    invoke_id: String,
    input: NodeInvokeInputBuffer,
    terminal_result: Option<NodeInvokeResult>,
}

impl NodeHostInvocationRuntime {
    pub fn start(
        runtime: Arc<NodeHostRuntimeState>,
        lease: NodeHostSessionLease,
        request: &NodeInvokeRequest,
    ) -> Result<Self, String> {
        let admission = runtime.admit_request(&lease, request)?;
        Ok(Self {
            runtime,
            lease,
            invoke_id: admission.invoke_id,
            input: NodeInvokeInputBuffer::new(),
            terminal_result: None,
        })
    }

    /// Construct from an admission already created by the Gateway session
    /// owner. This prevents a routing table from admitting the same wire
    /// request twice while preserving the same generation fence.
    pub fn start_admitted(
        runtime: Arc<NodeHostRuntimeState>,
        lease: NodeHostSessionLease,
        request: &NodeInvokeRequest,
        admission: super::invocation_registry::NodeInvocationAdmission,
    ) -> Result<Self, String> {
        if admission.invoke_id != request.id
            || admission.node_id != request.node_id
            || admission.node_id != lease.node_id
            || admission.generation != lease.generation
            || !runtime.is_current(&lease)
            || !runtime.is_invocation_current(&admission)
        {
            return Err("node invocation admission does not match the current request session".into());
        }
        Ok(Self {
            runtime,
            lease,
            invoke_id: admission.invoke_id,
            input: NodeInvokeInputBuffer::new(),
            terminal_result: None,
        })
    }

    pub fn invoke_id(&self) -> &str {
        &self.invoke_id
    }

    pub fn node_id(&self) -> &str {
        &self.lease.node_id
    }

    pub fn generation(&self) -> u64 {
        self.lease.generation
    }

    pub fn dispatch_input(&mut self, input: &NodeInvokeInput) -> Result<bool, String> {
        if input.invoke_id != self.invoke_id || input.node_id != self.lease.node_id {
            return Err("node invocation input does not belong to the invocation".into());
        }
        let accepted = self.input.dispatch(input.seq, input.payload_json.clone());
        if self.input.input_failed() {
            let admission = self.admission();
            self.runtime.cancel_invocation(&admission);
            return Err("node invocation input buffer failed; invocation cancelled".into());
        }
        Ok(accepted)
    }

    pub fn register_input_handler<F>(&mut self, handler: F)
    where
        F: FnMut(String) + Send + 'static,
    {
        self.input.register_handler(handler);
    }

    pub fn input_failed(&self) -> bool {
        self.input.input_failed()
    }

    pub fn cancel(&self, cancel: &NodeInvokeCancel) -> Result<bool, String> {
        self.runtime.cancel_invocation_frame(&self.lease, cancel)
    }

    pub fn complete(&mut self, result: NodeInvokeResult) -> Result<NodeInvokeResult, String> {
        if result.id != self.invoke_id || result.node_id != self.lease.node_id {
            return Err("node invocation result does not belong to the invocation".into());
        }
        if result.payload.is_some() && result.payload_json.is_some() {
            return Err("node invocation result cannot contain payload and payloadJSON".into());
        }
        let admission = self.admission();
        if !self.runtime.settle_invocation(&admission) {
            return Err("node invocation is no longer active".into());
        }
        self.terminal_result = Some(result.clone());
        Ok(result)
    }

    pub fn terminal_result(&self) -> Option<&NodeInvokeResult> {
        self.terminal_result.as_ref()
    }

    fn admission(&self) -> super::invocation_registry::NodeInvocationAdmission {
        super::invocation_registry::NodeInvocationAdmission {
            invoke_id: self.invoke_id.clone(),
            node_id: self.lease.node_id.clone(),
            generation: self.lease.generation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NodeHostInvocationRuntime;
    use crate::extensions::node_host::invoke_input::MAX_PENDING_INPUT_BYTES;
    use crate::extensions::node_host::invoke_protocol::{
        NodeInvokeInput, NodeInvokeRequest, NodeInvokeResult,
    };
    use crate::extensions::node_host::runtime_state::NodeHostRuntimeState;
    use crate::extensions::node_host::session_registry::NodeHostSessionMetadata;
    use serde_json::Value;
    use std::sync::Arc;

    fn request(node_id: &str) -> NodeInvokeRequest {
        NodeInvokeRequest {
            id: "invoke-1".into(),
            node_id: node_id.into(),
            command: "example.duplex".into(),
            params: Value::Null,
            received_params_bytes: 0,
            received_params_json: None,
            timeout_ms: None,
            idempotency_key: None,
            session_key: None,
        }
    }

    fn runtime() -> (Arc<NodeHostRuntimeState>, crate::extensions::node_host::session_registry::NodeHostSessionLease) {
        let runtime = Arc::new(NodeHostRuntimeState::new());
        let lease = runtime.connect(NodeHostSessionMetadata {
            node_id: "node-1".into(),
            display_name: "Node 1".into(),
            platform: "test".into(),
            device_family: "test".into(),
            commands: vec!["example.duplex".into()],
            capabilities: vec![],
            paired: true,
            gateway_local: false,
        });
        (runtime, lease)
    }

    #[test]
    fn lifecycle_binds_input_and_result_to_one_invocation() {
        let (runtime, lease) = runtime();
        let mut invocation =
            NodeHostInvocationRuntime::start(runtime, lease, &request("node-1")).expect("start");
        assert!(invocation.dispatch_input(&NodeInvokeInput {
            invoke_id: "invoke-1".into(),
            node_id: "node-1".into(),
            seq: 0,
            payload_json: "{\"input\":true}".into(),
        }).expect("input belongs"));
        let result = invocation
            .complete(NodeInvokeResult::success_json(
                "invoke-1",
                "node-1",
                "{\"done\":true}",
            ))
            .expect("complete");
        assert_eq!(result.id, "invoke-1");
        assert!(invocation.terminal_result().is_some());
        assert!(invocation.complete(result).is_err());
    }

    #[test]
    fn lifecycle_rejects_cross_node_input_and_result() {
        let (runtime, lease) = runtime();
        let mut invocation =
            NodeHostInvocationRuntime::start(runtime, lease, &request("node-1")).expect("start");
        assert!(invocation.dispatch_input(&NodeInvokeInput {
            invoke_id: "invoke-1".into(),
            node_id: "node-2".into(),
            seq: 0,
            payload_json: "{}".into(),
        }).is_err());
        assert!(invocation
            .complete(NodeInvokeResult::success_payload(
                "invoke-1",
                "node-2",
                serde_json::json!({}),
            ))
            .is_err());
    }

    #[test]
    fn input_buffer_overflow_cancels_invocation_before_terminal_result() {
        let (runtime, lease) = runtime();
        let mut invocation =
            NodeHostInvocationRuntime::start(Arc::clone(&runtime), lease, &request("node-1"))
                .expect("start");
        assert!(invocation
            .dispatch_input(&NodeInvokeInput {
                invoke_id: "invoke-1".into(),
                node_id: "node-1".into(),
                seq: 0,
                payload_json: "x".repeat(MAX_PENDING_INPUT_BYTES),
            })
            .expect("capacity-sized input is accepted"));
        assert!(invocation
            .dispatch_input(&NodeInvokeInput {
                invoke_id: "invoke-1".into(),
                node_id: "node-1".into(),
                seq: 1,
                payload_json: "{}".into(),
            })
            .is_err());
        assert!(invocation.input_failed());
        assert!(invocation
            .complete(NodeInvokeResult::success_json(
                "invoke-1",
                "node-1",
                "{}",
            ))
            .is_err());
    }
}
