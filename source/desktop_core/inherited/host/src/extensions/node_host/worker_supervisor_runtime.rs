//! Generation-fenced in-memory node-worker launch lifecycle.
//!
//! This is the state-machine boundary between validated launch admission and
//! the eventual durable supervisor/process owners. It never claims that a
//! child process, container, cleanup action, or durable receipt exists.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::worker_supervisor_protocol::{
    NodeWorkerLaunchBinding, NodeWorkerSupervisorIdentity, NodeWorkerSupervisorReceipt,
    NodeWorkerSupervisorReceiptState, WORKER_ERROR_TEXT_MAX_BYTES, WORKER_RESULT_JSON_MAX_BYTES,
};
use super::worker_workspace::{NodeWorkerWorkspaceLease, NodeWorkerWorkspaceOwner};

#[derive(Debug, Clone, Default)]
pub struct NodeWorkerSupervisorRuntime {
    state: Arc<Mutex<SupervisorState>>,
    workspace: NodeWorkerWorkspaceOwner,
}

#[derive(Debug, Default)]
struct SupervisorState {
    launches: BTreeMap<String, LaunchRecord>,
}

#[derive(Debug, Clone)]
struct LaunchRecord {
    identity: NodeWorkerSupervisorIdentity,
    workspace: NodeWorkerWorkspaceLease,
    state: NodeWorkerSupervisorReceiptState,
}

impl NodeWorkerSupervisorRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn workspace_owner(&self) -> &NodeWorkerWorkspaceOwner {
        &self.workspace
    }

    /// Admit one validated binding and bind it to an owner-supplied workspace
    /// lease. A duplicate launch id is never replaced.
    pub fn admit(
        &self,
        binding: &NodeWorkerLaunchBinding,
        plan_hash: impl Into<String>,
        run_id: impl Into<String>,
        root: impl AsRef<std::path::Path>,
    ) -> Result<NodeWorkerSupervisorIdentity, String> {
        let plan_hash = plan_hash.into();
        let run_id = run_id.into();
        validate_hash(&plan_hash, "planHash")?;
        if plan_hash != binding.expected_bundle_hash {
            return Err("INVALID_REQUEST: planHash does not match expectedBundleHash".into());
        }
        validate_identifier(&run_id, "runId")?;
        if binding.launch_id.is_empty() {
            return Err("INVALID_REQUEST: launchId must not be empty".into());
        }
        let workspace_identity = binding.workspace.as_ref().ok_or_else(|| {
            "WORKSPACE_IDENTITY_REQUIRED: worker owner must supply workspace identity".to_string()
        })?;
        let identity = NodeWorkerSupervisorIdentity {
            launch_id: binding.launch_id.clone(),
            plan_hash,
            environment_id: workspace_identity.environment_id.clone(),
            session_id: workspace_identity.session_id.clone(),
            owner_epoch: workspace_identity.owner_epoch,
            placement_generation: binding.placement_generation,
            run_id,
        };

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.launches.contains_key(&identity.launch_id) {
            return Err("DUPLICATE_REQUEST: launchId is already admitted".into());
        }

        let workspace = self.workspace.admit_binding(binding, root)?;

        // A workspace replacement makes any older non-terminal physical
        // outcome uncertain after the successor lease has been admitted.
        for record in state.launches.values_mut().filter(|record| {
            record.identity.environment_id == identity.environment_id
                && !is_terminal(&record.state)
        }) {
            record.state = NodeWorkerSupervisorReceiptState::OutcomeUnknown {
                error_text: "workspace owner replaced before terminal observation".into(),
            };
        }

        state.launches.insert(
            identity.launch_id.clone(),
            LaunchRecord {
                identity: identity.clone(),
                workspace,
                state: NodeWorkerSupervisorReceiptState::Pending,
            },
        );
        Ok(identity)
    }

    pub fn start(&self, identity: &NodeWorkerSupervisorIdentity) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let record = state
            .launches
            .get_mut(&identity.launch_id)
            .ok_or_else(|| "UNKNOWN_REQUEST: worker launch is not admitted".to_string())?;
        ensure_identity(record, identity)?;
        if !record.workspace.is_current() {
            mark_unknown(record, "workspace lease is no longer current");
            return Err("WORKSPACE_STALE: worker launch workspace is no longer current".into());
        }
        if !matches!(record.state, NodeWorkerSupervisorReceiptState::Pending) {
            return Err("INVALID_STATE: worker launch is not pending".into());
        }
        record.state = NodeWorkerSupervisorReceiptState::Running;
        Ok(())
    }

    /// Settle only a running launch. A missing or stale physical observation
    /// must be represented explicitly as `OutcomeUnknown`, not retried here.
    pub fn settle(
        &self,
        identity: &NodeWorkerSupervisorIdentity,
        state: NodeWorkerSupervisorReceiptState,
    ) -> Result<(), String> {
        validate_terminal_state(&state)?;
        let mut supervisor = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let record = supervisor
            .launches
            .get_mut(&identity.launch_id)
            .ok_or_else(|| "UNKNOWN_REQUEST: worker launch is not admitted".to_string())?;
        ensure_identity(record, identity)?;
        if !record.workspace.is_current() {
            mark_unknown(record, "workspace lease is no longer current");
            return Err("WORKSPACE_STALE: worker launch workspace is no longer current".into());
        }
        if !matches!(record.state, NodeWorkerSupervisorReceiptState::Running) {
            return Err("INVALID_STATE: worker launch is not running".into());
        }
        record.state = state;
        Ok(())
    }

    pub fn receipt(&self, identity: &NodeWorkerSupervisorIdentity) -> Result<NodeWorkerSupervisorReceipt, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let record = state
            .launches
            .get_mut(&identity.launch_id)
            .ok_or_else(|| "UNKNOWN_REQUEST: worker launch is not admitted".to_string())?;
        ensure_identity(record, identity)?;
        if !record.workspace.is_current() && !is_terminal(&record.state) {
            mark_unknown(record, "workspace lease is no longer current");
        }
        Ok(NodeWorkerSupervisorReceipt {
            identity: record.identity.clone(),
            state: record.state.clone(),
        })
    }

    /// Remove only a terminal launch after its durable successor has taken
    /// custody. This owner never assumes that physical cleanup is complete.
    pub fn release(&self, identity: &NodeWorkerSupervisorIdentity) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let record = state
            .launches
            .get(identity.launch_id.as_str())
            .ok_or_else(|| "UNKNOWN_REQUEST: worker launch is not admitted".to_string())?;
        ensure_identity(record, identity)?;
        if !is_terminal(&record.state) {
            return Err("INVALID_STATE: worker launch is not terminal".into());
        }
        if !self.workspace.is_current(&record.workspace) {
            // The workspace owner already moved to a successor; retaining the
            // launch record would create a false cleanup claim.
            return Err("WORKSPACE_STALE: worker launch workspace was replaced".into());
        }
        let record = state.launches.remove(&identity.launch_id).expect("checked above");
        let _ = self.workspace.release(&record.workspace);
        Ok(())
    }
}

fn ensure_identity(record: &LaunchRecord, expected: &NodeWorkerSupervisorIdentity) -> Result<(), String> {
    if &record.identity == expected {
        Ok(())
    } else {
        Err("STALE_REQUEST: worker launch identity does not match".into())
    }
}

fn mark_unknown(record: &mut LaunchRecord, reason: &str) {
    record.state = NodeWorkerSupervisorReceiptState::OutcomeUnknown {
        error_text: bounded_error(reason),
    };
}

fn is_terminal(state: &NodeWorkerSupervisorReceiptState) -> bool {
    matches!(
        state,
        NodeWorkerSupervisorReceiptState::Completed { .. }
            | NodeWorkerSupervisorReceiptState::Failed { .. }
            | NodeWorkerSupervisorReceiptState::Interrupted { .. }
            | NodeWorkerSupervisorReceiptState::Cancelled { .. }
            | NodeWorkerSupervisorReceiptState::OutcomeUnknown { .. }
    )
}

fn validate_terminal_state(state: &NodeWorkerSupervisorReceiptState) -> Result<(), String> {
    match state {
        NodeWorkerSupervisorReceiptState::Completed { result_json } => {
            if result_json.is_empty() || result_json.len() > WORKER_RESULT_JSON_MAX_BYTES {
                return Err("INVALID_REQUEST: resultJson exceeds worker limit".into());
            }
            let value: serde_json::Value = serde_json::from_str(result_json)
                .map_err(|_| "INVALID_REQUEST: resultJson must be valid JSON".to_string())?;
            if !value.is_object() {
                return Err("INVALID_REQUEST: resultJson must be a JSON object".into());
            }
        }
        NodeWorkerSupervisorReceiptState::Failed { error_text }
        | NodeWorkerSupervisorReceiptState::Interrupted { error_text }
        | NodeWorkerSupervisorReceiptState::Cancelled { error_text }
        | NodeWorkerSupervisorReceiptState::OutcomeUnknown { error_text } => {
            validate_error(error_text)?;
        }
        NodeWorkerSupervisorReceiptState::Pending | NodeWorkerSupervisorReceiptState::Running => {
            return Err("INVALID_STATE: settlement must be terminal".into());
        }
    }
    Ok(())
}

fn validate_error(error_text: &str) -> Result<(), String> {
    if error_text.is_empty()
        || error_text.len() > WORKER_ERROR_TEXT_MAX_BYTES
        || error_text.chars().any(|character| matches!(character, '\r' | '\n'))
    {
        return Err("INVALID_REQUEST: worker error text is not bounded".into());
    }
    Ok(())
}

fn bounded_error(error_text: &str) -> String {
    error_text
        .chars()
        .filter(|character| !matches!(character, '\r' | '\n'))
        .take(WORKER_ERROR_TEXT_MAX_BYTES)
        .collect()
}

fn validate_identifier(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 256 || value.trim() != value || value.contains('\0') {
        return Err(format!("INVALID_REQUEST: {field} must be a bounded identifier"));
    }
    Ok(())
}

fn validate_hash(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        return Err(format!("INVALID_REQUEST: {field} must be 64 lowercase hexadecimal characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::NodeWorkerSupervisorRuntime;
    use crate::extensions::node_host::worker_supervisor_protocol::{
        NodeWorkerLaunchBinding, NodeWorkerSupervisorReceiptState, NodeWorkerWorkspaceIdentity,
    };
    use std::fs;

    const PLAN_HASH: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn binding(launch_id: &str, session_id: &str, owner_epoch: u64) -> NodeWorkerLaunchBinding {
        NodeWorkerLaunchBinding {
            launch_id: launch_id.into(),
            gateway_namespace: "gateway".into(),
            expected_bundle_hash: PLAN_HASH.into(),
            placement_generation: 1,
            workspace: Some(NodeWorkerWorkspaceIdentity {
                environment_id: "environment-1".into(),
                session_id: session_id.into(),
                owner_epoch,
            }),
        }
    }

    #[test]
    fn launch_state_is_identity_fenced_and_preserves_terminal_receipts() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-node-worker-supervisor-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture root");
        let runtime = NodeWorkerSupervisorRuntime::new();
        let identity = runtime
            .admit(&binding("launch-1", "session-1", 1), PLAN_HASH, "run-1", &root)
            .expect("admit");
        runtime.start(&identity).expect("start");
        runtime
            .settle(
                &identity,
                NodeWorkerSupervisorReceiptState::Completed {
                    result_json: "{}".into(),
                },
            )
            .expect("settle");
        let receipt = runtime.receipt(&identity).expect("receipt");
        assert!(matches!(receipt.state, NodeWorkerSupervisorReceiptState::Completed { .. }));
        assert!(runtime.release(&identity).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_replacement_marks_running_launch_outcome_unknown() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-node-worker-supervisor-replacement-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture root");
        let runtime = NodeWorkerSupervisorRuntime::new();
        let first = runtime
            .admit(&binding("launch-1", "session-1", 1), PLAN_HASH, "run-1", &root)
            .expect("first admit");
        runtime.start(&first).expect("first start");
        let _successor = runtime
            .admit(&binding("launch-2", "session-2", 2), PLAN_HASH, "run-2", &root)
            .expect("successor admit");
        let receipt = runtime.receipt(&first).expect("unknown receipt");
        assert!(matches!(
            receipt.state,
            NodeWorkerSupervisorReceiptState::OutcomeUnknown { .. }
        ));
        let _ = fs::remove_dir_all(root);
    }
}

