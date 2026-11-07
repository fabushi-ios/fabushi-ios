use std::path::Component;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    CloudWorkerLeaseScope, CloudWorkerLeasedCapability, CloudWorkerRuntimeIdentity,
    LeaseGeneration, LeaseId, OwnerEpoch, ReplayKey,
};

const MAX_ID_CHARS: usize = 256;
const MAX_PATH_CHARS: usize = 4096;
const MAX_ARGV: usize = 128;
const MAX_ARG_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerSandboxCapability {
    pub lease_id: LeaseId,
    pub scope: CloudWorkerLeaseScope,
    pub owner_epoch: OwnerEpoch,
    pub generation: LeaseGeneration,
    pub runtime: CloudWorkerRuntimeIdentity,
    pub workspace_id: String,
    pub remote_shell: bool,
    pub filesystem: bool,
}

impl CloudWorkerSandboxCapability {
    pub fn new(
        leased_capability: CloudWorkerLeasedCapability,
        workspace_id: impl Into<String>,
    ) -> Result<Self, CloudWorkerSandboxError> {
        let workspace_id = workspace_id.into();
        validate_id(&leased_capability.lease_id().0)?;
        let scope = leased_capability.scope().clone();
        CloudWorkerLeaseScope::new(scope.account_scope.clone(), scope.device_scope.clone())
            .map_err(|_| CloudWorkerSandboxError::InvalidScope)?;
        if !valid_text_id(&workspace_id) {
            return Err(CloudWorkerSandboxError::InvalidWorkspace);
        }
        Ok(Self {
            lease_id: leased_capability.lease_id().clone(),
            scope,
            owner_epoch: leased_capability.owner_epoch(),
            generation: leased_capability.generation(),
            runtime: leased_capability.runtime().clone(),
            workspace_id,
            remote_shell: true,
            filesystem: true,
        })
    }

    pub fn validate(&self) -> Result<(), CloudWorkerSandboxError> {
        validate_id(&self.lease_id.0)?;
        CloudWorkerLeaseScope::new(
            self.scope.account_scope.clone(),
            self.scope.device_scope.clone(),
        )
        .map_err(|_| CloudWorkerSandboxError::InvalidScope)?;
        CloudWorkerRuntimeIdentity::new(
            self.runtime.provider.clone(),
            self.runtime.profile.clone(),
            self.runtime.runtime_id.clone(),
        )
        .map_err(|_| CloudWorkerSandboxError::InvalidRuntimeIdentity)?;
        if !valid_text_id(&self.workspace_id) {
            return Err(CloudWorkerSandboxError::InvalidWorkspace);
        }
        Ok(())
    }

    pub fn validate_against(
        &self,
        leased_capability: &CloudWorkerLeasedCapability,
    ) -> Result<(), CloudWorkerSandboxError> {
        self.validate()?;
        if self.lease_id != *leased_capability.lease_id()
            || self.scope != *leased_capability.scope()
            || self.owner_epoch != leased_capability.owner_epoch()
            || self.generation != leased_capability.generation()
            || self.runtime != *leased_capability.runtime()
        {
            return Err(CloudWorkerSandboxError::StaleCapability);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerSandboxOperation {
    ReadFile { path: String },
    WriteFile { path: String },
    ExecuteArgv { argv: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerSandboxRequest {
    pub lease_id: LeaseId,
    pub workspace_id: String,
    pub operation: CloudWorkerSandboxOperation,
    pub replay_key: ReplayKey,
}

impl CloudWorkerSandboxRequest {
    pub fn validate(&self) -> Result<(), CloudWorkerSandboxError> {
        validate_id(&self.lease_id.0)?;
        if !valid_text_id(&self.workspace_id) || !valid_text_id(&self.replay_key.0) {
            return Err(CloudWorkerSandboxError::InvalidRequest);
        }
        match &self.operation {
            CloudWorkerSandboxOperation::ReadFile { path }
            | CloudWorkerSandboxOperation::WriteFile { path } => validate_workspace_path(path),
            CloudWorkerSandboxOperation::ExecuteArgv { argv } => {
                if argv.is_empty()
                    || argv.len() > MAX_ARGV
                    || argv.iter().any(|arg| {
                        arg.is_empty()
                            || arg.chars().count() > MAX_ARG_CHARS
                            || arg.chars().any(|character| character.is_control())
                    })
                {
                    return Err(CloudWorkerSandboxError::InvalidArgv);
                }
                Ok(())
            }
        }
    }

    pub fn validate_against(
        &self,
        capability: &CloudWorkerSandboxCapability,
    ) -> Result<(), CloudWorkerSandboxError> {
        self.validate()?;
        capability.validate()?;
        if self.lease_id != capability.lease_id {
            return Err(CloudWorkerSandboxError::StaleCapability);
        }
        if self.workspace_id != capability.workspace_id {
            return Err(CloudWorkerSandboxError::WorkspaceMismatch);
        }
        match &self.operation {
            CloudWorkerSandboxOperation::ExecuteArgv { .. } if !capability.remote_shell => {
                Err(CloudWorkerSandboxError::RemoteShellUnavailable)
            }
            CloudWorkerSandboxOperation::ReadFile { .. }
            | CloudWorkerSandboxOperation::WriteFile { .. }
                if !capability.filesystem =>
            {
                Err(CloudWorkerSandboxError::FilesystemUnavailable)
            }
            _ => Ok(()),
        }
    }
}

fn validate_workspace_path(path: &str) -> Result<(), CloudWorkerSandboxError> {
    let path = std::path::Path::new(path);
    if path.as_os_str().is_empty()
        || path.to_string_lossy().chars().count() > MAX_PATH_CHARS
        || path.to_string_lossy().chars().any(|character| character.is_control())
        || path.to_string_lossy().contains('\\')
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(component, Component::ParentDir | Component::Prefix(_))
        })
    {
        return Err(CloudWorkerSandboxError::UnsafePath);
    }
    Ok(())
}

fn valid_text_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= MAX_ID_CHARS
        && !value.chars().any(|character| character.is_control())
}

fn validate_id(value: &str) -> Result<(), CloudWorkerSandboxError> {
    if valid_text_id(value) {
        Ok(())
    } else {
        Err(CloudWorkerSandboxError::InvalidLease)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerSandboxError {
    #[error("sandbox workspace must be non-empty")]
    InvalidWorkspace,
    #[error("sandbox lease id is invalid")]
    InvalidLease,
    #[error("sandbox lease scope is invalid")]
    InvalidScope,
    #[error("sandbox capability is stale")]
    StaleCapability,
    #[error("sandbox runtime identity is invalid")]
    InvalidRuntimeIdentity,
    #[error("sandbox request workspace does not match the capability")]
    WorkspaceMismatch,
    #[error("sandbox remote-shell capability is unavailable")]
    RemoteShellUnavailable,
    #[error("sandbox filesystem capability is unavailable")]
    FilesystemUnavailable,
    #[error("sandbox request must include workspace and replay identities")]
    InvalidRequest,
    #[error("sandbox path escapes the workspace")]
    UnsafePath,
    #[error("sandbox argv must be non-empty and contain no NUL bytes")]
    InvalidArgv,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leased_capability() -> CloudWorkerLeasedCapability {
        super::super::CloudWorkerLease {
            scope: CloudWorkerLeaseScope::new("account", "device").unwrap(),
            lease_id: LeaseId("lease-1".into()),
            owner_epoch: OwnerEpoch(3),
            generation: LeaseGeneration(7),
            runtime: CloudWorkerRuntimeIdentity::new("provider", "profile", "runtime")
                .unwrap(),
            lifecycle: super::super::CloudWorkerLifecycle::Enrolled,
            last_outcome: super::super::CloudWorkerMutationOutcome::Completed,
            replay_history: Vec::new(),
        }
        .capability()
        .unwrap()
    }

    fn request(operation: CloudWorkerSandboxOperation) -> CloudWorkerSandboxRequest {
        CloudWorkerSandboxRequest {
            lease_id: LeaseId("lease-1".into()),
            workspace_id: "workspace-1".into(),
            operation,
            replay_key: ReplayKey("request-1".into()),
        }
    }

    #[test]
    fn accepts_argv_without_shell_string_interpolation() {
        assert_eq!(
            request(CloudWorkerSandboxOperation::ExecuteArgv {
                argv: vec!["git".into(), "status".into()],
            })
            .validate(),
            Ok(())
        );
    }

    #[test]
    fn rejects_workspace_escape_paths() {
        assert_eq!(
            request(CloudWorkerSandboxOperation::ReadFile {
                path: "../secret".into(),
            })
            .validate(),
            Err(CloudWorkerSandboxError::UnsafePath)
        );
    }

    #[test]
    fn rejects_cross_platform_path_separators_and_control_text() {
        assert_eq!(
            request(CloudWorkerSandboxOperation::ReadFile {
                path: "dir\\secret".into(),
            })
            .validate(),
            Err(CloudWorkerSandboxError::UnsafePath)
        );
        assert_eq!(
            request(CloudWorkerSandboxOperation::ExecuteArgv {
                argv: vec!["git\nstatus".into()],
            })
            .validate(),
            Err(CloudWorkerSandboxError::InvalidArgv)
        );
    }

    #[test]
    fn revalidates_deserialized_capability_identity() {
        let capability = CloudWorkerSandboxCapability {
            lease_id: LeaseId("lease-1".into()),
            scope: CloudWorkerLeaseScope {
                account_scope: "account\nunsafe".into(),
                device_scope: "device".into(),
            },
            owner_epoch: OwnerEpoch(3),
            generation: LeaseGeneration(7),
            runtime: CloudWorkerRuntimeIdentity::new("provider", "profile", "runtime")
                .unwrap(),
            workspace_id: "workspace-1".into(),
            remote_shell: true,
            filesystem: true,
        };
        assert_eq!(
            capability.validate(),
            Err(CloudWorkerSandboxError::InvalidScope)
        );
    }

    #[test]
    fn binds_and_rejects_stale_lease_fencing_material() {
        let capability = CloudWorkerSandboxCapability::new(leased_capability(), "workspace-1")
            .unwrap();
        assert_eq!(capability.validate_against(&leased_capability()), Ok(()));

        let current = leased_capability();
        let stale = super::super::CloudWorkerLease {
            scope: current.scope().clone(),
            lease_id: current.lease_id().clone(),
            owner_epoch: current.owner_epoch(),
            generation: LeaseGeneration(8),
            runtime: current.runtime().clone(),
            lifecycle: super::super::CloudWorkerLifecycle::Enrolled,
            last_outcome: super::super::CloudWorkerMutationOutcome::Completed,
            replay_history: Vec::new(),
        }
        .capability()
        .unwrap();
        assert_eq!(
            capability.validate_against(&stale),
            Err(CloudWorkerSandboxError::StaleCapability)
        );
    }

    #[test]
    fn binds_request_to_capability_scope_and_operation_grants() {
        let capability = CloudWorkerSandboxCapability::new(leased_capability(), "workspace-1")
            .unwrap();
        assert_eq!(
            request(CloudWorkerSandboxOperation::ExecuteArgv {
                argv: vec!["git".into(), "status".into()],
            })
            .validate_against(&capability),
            Ok(())
        );

        let mut wrong_workspace = request(CloudWorkerSandboxOperation::ReadFile {
            path: "README.md".into(),
        });
        wrong_workspace.workspace_id = "workspace-2".into();
        assert_eq!(
            wrong_workspace.validate_against(&capability),
            Err(CloudWorkerSandboxError::WorkspaceMismatch)
        );

        let mut no_shell = capability.clone();
        no_shell.remote_shell = false;
        assert_eq!(
            request(CloudWorkerSandboxOperation::ExecuteArgv {
                argv: vec!["git".into()],
            })
            .validate_against(&no_shell),
            Err(CloudWorkerSandboxError::RemoteShellUnavailable)
        );
    }
}
