use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{CloudWorkerLeaseScope, ReplayKey};

const MAX_REPLAY_KEY_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerDoctorAction {
    Inspect,
    RepairManagedArtifact,
    MigrateSecretRefs,
    ReconcileLease,
    ReconcileWarmImage,
}

impl CloudWorkerDoctorAction {
    fn requires_acknowledgement(self) -> bool {
        !matches!(self, Self::Inspect)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerDoctorRequest {
    pub scope: CloudWorkerLeaseScope,
    pub action: CloudWorkerDoctorAction,
    pub acknowledged_mutation: bool,
    pub replay_key: ReplayKey,
}

impl CloudWorkerDoctorRequest {
    pub fn validate(&self) -> Result<(), CloudWorkerDoctorError> {
        CloudWorkerLeaseScope::new(
            self.scope.account_scope.clone(),
            self.scope.device_scope.clone(),
        )
        .map_err(|_| CloudWorkerDoctorError::InvalidScope)?;
        if self.replay_key.0.trim().is_empty() {
            return Err(CloudWorkerDoctorError::MissingReplayKey);
        }
        if self.replay_key.0.chars().count() > MAX_REPLAY_KEY_CHARS
            || self.replay_key.0.chars().any(|character| character.is_control())
        {
            return Err(CloudWorkerDoctorError::InvalidReplayKey);
        }
        if self.action.requires_acknowledgement() && !self.acknowledged_mutation {
            return Err(CloudWorkerDoctorError::MutationNotAcknowledged);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerDoctorStatus {
    InspectionOnly,
    Pending,
    Completed,
    Failed,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerDoctorResult {
    pub status: CloudWorkerDoctorStatus,
    pub mutation_started: bool,
    pub diagnostic_code: Option<String>,
    #[serde(default)]
    pub state_persisted: bool,
    #[serde(default)]
    pub lease_count: usize,
    #[serde(default)]
    pub bootstrap_count: usize,
    #[serde(default)]
    pub artifact_count: usize,
    #[serde(default)]
    pub warm_image_count: usize,
}

impl CloudWorkerDoctorResult {
    pub fn inspection() -> Self {
        Self {
            status: CloudWorkerDoctorStatus::InspectionOnly,
            mutation_started: false,
            diagnostic_code: None,
            state_persisted: false,
            lease_count: 0,
            bootstrap_count: 0,
            artifact_count: 0,
            warm_image_count: 0,
        }
    }

    pub fn inspection_with_snapshot(
        state_persisted: bool,
        lease_count: usize,
        bootstrap_count: usize,
        artifact_count: usize,
        warm_image_count: usize,
    ) -> Self {
        Self {
            status: CloudWorkerDoctorStatus::InspectionOnly,
            mutation_started: false,
            diagnostic_code: None,
            state_persisted,
            lease_count,
            bootstrap_count,
            artifact_count,
            warm_image_count,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerDoctorError {
    #[error("doctor request scope is invalid")]
    InvalidScope,
    #[error("doctor request requires a replay key")]
    MissingReplayKey,
    #[error("doctor request replay key is invalid")]
    InvalidReplayKey,
    #[error("mutating doctor action requires explicit acknowledgement")]
    MutationNotAcknowledged,
    #[error("mutating doctor action is not wired to a recovery owner")]
    MutationNotWired,
    #[error("cloud-worker doctor owner snapshot is unavailable")]
    OwnerUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(action: CloudWorkerDoctorAction) -> CloudWorkerDoctorRequest {
        CloudWorkerDoctorRequest {
            scope: CloudWorkerLeaseScope::new("account", "device").unwrap(),
            action,
            acknowledged_mutation: false,
            replay_key: ReplayKey("doctor-1".into()),
        }
    }

    #[test]
    fn inspection_does_not_need_mutation_acknowledgement() {
        assert_eq!(request(CloudWorkerDoctorAction::Inspect).validate(), Ok(()));
        assert_eq!(
            CloudWorkerDoctorResult::inspection().mutation_started,
            false
        );
    }

    #[test]
    fn repair_requires_explicit_acknowledgement() {
        assert_eq!(
            request(CloudWorkerDoctorAction::RepairManagedArtifact).validate(),
            Err(CloudWorkerDoctorError::MutationNotAcknowledged)
        );
    }

    #[test]
    fn rejects_controlled_or_oversized_replay_identity() {
        let mut controlled = request(CloudWorkerDoctorAction::Inspect);
        controlled.replay_key = ReplayKey("doctor\nrequest".into());
        assert_eq!(
            controlled.validate(),
            Err(CloudWorkerDoctorError::InvalidReplayKey)
        );

        let mut oversized = request(CloudWorkerDoctorAction::Inspect);
        oversized.replay_key = ReplayKey("x".repeat(257));
        assert_eq!(
            oversized.validate(),
            Err(CloudWorkerDoctorError::InvalidReplayKey)
        );
    }
}
