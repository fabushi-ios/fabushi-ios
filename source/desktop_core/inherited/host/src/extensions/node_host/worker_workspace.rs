//! Node-worker workspace lease and path-containment owner.
//!
//! This owner binds a canonical prepared root to the existing worker
//! environment/session/owner-epoch identity. It owns no process, persistence,
//! transfer, retention, or directory creation lifecycle; those operations
//! must use this lease as their currentness and containment boundary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::worker_supervisor_protocol::{NodeWorkerLaunchBinding, NodeWorkerWorkspaceIdentity};

const MAX_WORKSPACE_IDENTIFIER_BYTES: usize = 256;

#[derive(Debug, Clone)]
pub struct NodeWorkerWorkspaceLease {
    owner: NodeWorkerWorkspaceOwner,
    identity: NodeWorkerWorkspaceIdentity,
    root: PathBuf,
    generation: u64,
}

impl NodeWorkerWorkspaceLease {
    pub fn identity(&self) -> &NodeWorkerWorkspaceIdentity {
        &self.identity
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_current(&self) -> bool {
        self.owner.is_current(self)
    }

    /// Canonicalize and authorize one path under the current workspace root.
    /// The check is intentionally repeated for every operation so a replaced
    /// lease or symlink escape cannot reuse an earlier admission.
    pub fn resolve_contained_path(&self, candidate: impl AsRef<Path>) -> Result<PathBuf, String> {
        self.owner.resolve_contained_path(self, candidate.as_ref())
    }
}

#[derive(Debug, Clone, Default)]
pub struct NodeWorkerWorkspaceOwner {
    state: Arc<NodeWorkerWorkspaceState>,
}

#[derive(Debug, Default)]
struct NodeWorkerWorkspaceState {
    state: Mutex<WorkspaceState>,
}

#[derive(Debug, Default)]
struct WorkspaceState {
    next_generation: u64,
    current: BTreeMap<String, CurrentWorkspace>,
}

#[derive(Debug, Clone)]
struct CurrentWorkspace {
    identity: NodeWorkerWorkspaceIdentity,
    root: PathBuf,
    generation: u64,
}

impl NodeWorkerWorkspaceOwner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit an already-prepared directory. Preparation and ownership of the
    /// directory itself remain with the worker workspace owner that calls this
    /// boundary; this method only reads and validates its canonical identity.
    pub fn admit(
        &self,
        identity: NodeWorkerWorkspaceIdentity,
        root: impl AsRef<Path>,
    ) -> Result<NodeWorkerWorkspaceLease, String> {
        validate_identity(&identity)?;
        let root = std::fs::canonicalize(root.as_ref())
            .map_err(|error| format!("WORKSPACE_UNAVAILABLE: cannot canonicalize root: {error}"))?;
        if !root.is_dir() {
            return Err("WORKSPACE_INVALID: workspace root is not a directory".into());
        }

        let mut state = self
            .state
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(current) = state.current.get(&identity.environment_id) {
            if current.identity == identity && current.root == root {
                return Ok(NodeWorkerWorkspaceLease {
                    owner: self.clone(),
                    identity: current.identity.clone(),
                    root: current.root.clone(),
                    generation: current.generation,
                });
            }
        }

        state.next_generation = state.next_generation.saturating_add(1).max(1);
        let generation = state.next_generation;
        state.current.insert(
            identity.environment_id.clone(),
            CurrentWorkspace {
                identity: identity.clone(),
                root: root.clone(),
                generation,
            },
        );
        Ok(NodeWorkerWorkspaceLease {
            owner: self.clone(),
            identity,
            root,
            generation,
        })
    }

    /// Bind a validated launch only when the owner has supplied complete
    /// workspace identity. The launch descriptor cannot manufacture this
    /// authority because `validate_worker_launch_bindings` rejects nested
    /// identity fields before this method is reached.
    pub fn admit_binding(
        &self,
        binding: &NodeWorkerLaunchBinding,
        root: impl AsRef<Path>,
    ) -> Result<NodeWorkerWorkspaceLease, String> {
        let identity = binding.workspace.clone().ok_or_else(|| {
            "WORKSPACE_IDENTITY_REQUIRED: worker owner must supply workspace identity".to_string()
        })?;
        self.admit(identity, root)
    }

    pub fn is_current(&self, lease: &NodeWorkerWorkspaceLease) -> bool {
        let state = self
            .state
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.current.get(&lease.identity.environment_id).is_some_and(|current| {
            current.generation == lease.generation
                && current.identity == lease.identity
                && current.root == lease.root
        })
    }

    pub fn release(&self, lease: &NodeWorkerWorkspaceLease) -> bool {
        let mut state = self
            .state
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = state.current.get(&lease.identity.environment_id);
        if !current.is_some_and(|current| {
            current.generation == lease.generation
                && current.identity == lease.identity
                && current.root == lease.root
        }) {
            return false;
        }
        state.current.remove(&lease.identity.environment_id).is_some()
    }

    fn resolve_contained_path(
        &self,
        lease: &NodeWorkerWorkspaceLease,
        candidate: &Path,
    ) -> Result<PathBuf, String> {
        if !self.is_current(lease) {
            return Err("WORKSPACE_STALE: workspace lease is no longer current".into());
        }
        let canonical = std::fs::canonicalize(candidate)
            .map_err(|error| format!("WORKSPACE_UNAVAILABLE: cannot canonicalize path: {error}"))?;
        if !canonical.starts_with(&lease.root) {
            return Err("WORKSPACE_CONTAINMENT: path escapes the admitted root".into());
        }
        if !self.is_current(lease) {
            return Err("WORKSPACE_STALE: workspace lease changed during path admission".into());
        }
        Ok(canonical)
    }
}

fn validate_identity(identity: &NodeWorkerWorkspaceIdentity) -> Result<(), String> {
    if identity.owner_epoch == 0 {
        return Err("WORKSPACE_INVALID: ownerEpoch must be greater than zero".into());
    }
    for (label, value) in [
        ("environmentId", identity.environment_id.as_str()),
        ("sessionId", identity.session_id.as_str()),
    ] {
        if value.is_empty()
            || value.len() > MAX_WORKSPACE_IDENTIFIER_BYTES
            || value.trim() != value
            || value.chars().any(|character| character.is_control())
        {
            return Err(format!("WORKSPACE_INVALID: {label} is not a bounded identifier"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::NodeWorkerWorkspaceOwner;
    use crate::extensions::node_host::worker_supervisor_protocol::NodeWorkerWorkspaceIdentity;
    use std::fs;
    use std::path::PathBuf;

    fn identity(session_id: &str, owner_epoch: u64) -> NodeWorkerWorkspaceIdentity {
        NodeWorkerWorkspaceIdentity {
            environment_id: "environment-1".into(),
            session_id: session_id.into(),
            owner_epoch,
        }
    }

    fn fixture_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-node-worker-workspace-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture root");
        root
    }

    #[test]
    fn replacement_stales_predecessor_and_release_is_generation_fenced() {
        let root = fixture_root();
        let owner = NodeWorkerWorkspaceOwner::new();
        let first = owner.admit(identity("session-1", 1), &root).expect("first lease");
        let second = owner.admit(identity("session-2", 2), &root).expect("successor lease");

        assert!(!first.is_current());
        assert!(second.is_current());
        assert!(!owner.release(&first));
        assert!(owner.release(&second));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn path_resolution_rejects_escape_and_stale_leases() {
        let root = fixture_root();
        let inside = root.join("inside.txt");
        fs::write(&inside, b"fixture").expect("inside file");
        let owner = NodeWorkerWorkspaceOwner::new();
        let lease = owner.admit(identity("session-1", 1), &root).expect("lease");
        assert_eq!(lease.resolve_contained_path(&inside).expect("inside"), fs::canonicalize(&inside).unwrap());

        let outside = root.parent().expect("parent").join("outside.txt");
        fs::write(&outside, b"fixture").expect("outside file");
        assert!(lease.resolve_contained_path(&outside).is_err());
        let _ = fs::remove_file(outside);
        let _ = owner.release(&lease);
        assert!(lease.resolve_contained_path(&inside).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
