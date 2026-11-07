//! Canonical binding of Gateway invoke frames to invocation runtimes.
//!
//! The router owns only live invocation objects and their frame-to-runtime
//! lookup. It owns no socket, credential, command side effect, timer, or
//! result delivery. A caller supplies the already admitted request lease and
//! obtains the same runtime object for an injected side-effect owner.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::invocation_registry::NodeInvocationAdmission;
use super::invocation_runtime::NodeHostInvocationRuntime;
use super::invoke_protocol::{NodeInvokeCancel, NodeInvokeInput, NodeInvokeRequest};
use super::runtime_state::NodeHostRuntimeState;
use super::session_registry::NodeHostSessionLease;

type InvocationKey = (String, String);

#[derive(Default)]
pub struct NodeHostInvocationRouter {
    active: BTreeMap<InvocationKey, NodeHostInvocationRuntime>,
}

impl NodeHostInvocationRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.active.len()
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Register a request using the admission already made by the session
    /// owner. A duplicate key cannot replace a live invocation.
    pub fn admit_request(
        &mut self,
        runtime: Arc<NodeHostRuntimeState>,
        lease: NodeHostSessionLease,
        request: &NodeInvokeRequest,
        admission: NodeInvocationAdmission,
    ) -> Result<(), String> {
        let key = (request.node_id.clone(), request.id.clone());
        if self.active.contains_key(&key) {
            return Err("node invocation router already owns this invocation".into());
        }
        let invocation = NodeHostInvocationRuntime::start_admitted(
            runtime, lease, request, admission,
        )?;
        self.active.insert(key, invocation);
        Ok(())
    }

    pub fn invocation_mut(
        &mut self,
        node_id: &str,
        invoke_id: &str,
    ) -> Option<&mut NodeHostInvocationRuntime> {
        self.active
            .get_mut(&(node_id.to_string(), invoke_id.to_string()))
    }

    /// Drop only invocation runtimes owned by the retired session generation.
    /// The canonical `NodeHostRuntimeState` performs the state transition and
    /// external side-effect owners remain responsible for stopping work.
    pub fn drain_generation(&mut self, node_id: &str, generation: u64) -> Vec<String> {
        let keys = self
            .active
            .iter()
            .filter(|((key_node_id, _), invocation)| {
                key_node_id == node_id && invocation.generation() == generation
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        let mut drained = Vec::with_capacity(keys.len());
        for key in keys {
            if self.active.remove(&key).is_some() {
                drained.push(key.1);
            }
        }
        drained
    }

    pub fn dispatch_input(&mut self, input: &NodeInvokeInput) -> Result<bool, String> {
        let invocation = self
            .invocation_mut(&input.node_id, &input.invoke_id)
            .ok_or_else(|| "node invocation input has no admitted runtime".to_string())?;
        invocation.dispatch_input(input)
    }

    pub fn cancel(&mut self, cancel: &NodeInvokeCancel) -> Result<bool, String> {
        let invocation = self
            .invocation_mut(&cancel.node_id, &cancel.invoke_id)
            .ok_or_else(|| "node invocation cancel has no admitted runtime".to_string())?;
        invocation.cancel(cancel)
    }

    /// Remove only an invocation that has already settled successfully. A
    /// caller should do this after the session's one-shot result send has
    /// returned; failed delivery remains outcome-unknown and is not replayed.
    pub fn remove_settled(
        &mut self,
        node_id: &str,
        invoke_id: &str,
    ) -> Result<NodeHostInvocationRuntime, String> {
        let key = (node_id.to_string(), invoke_id.to_string());
        let settled = self
            .active
            .get(&key)
            .and_then(|invocation| invocation.terminal_result())
            .is_some();
        if !settled {
            return Err("node invocation cannot be removed before terminal settlement".into());
        }
        self.active
            .remove(&key)
            .ok_or_else(|| "node invocation runtime is missing".into())
    }
}
