use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    CloudWorkerDiagnostic, CloudWorkerLease, CloudWorkerLeaseError, CloudWorkerLeaseMutation,
    CloudWorkerLifecycle, CloudWorkerMutationOutcome, ReplayKey,
};

const MAX_CONTROL_TIMEOUT_MS: u64 = 15 * 60 * 1000;
const MAX_CONTROL_OUTPUT_BYTES: u64 = 1024 * 1024;

/// Bounded execution material that a future provider/process producer must
/// honor. It is admission data only; no timer, process or transport is owned
/// by this contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerRuntimeControlBudget {
    pub timeout_ms: u64,
    pub max_output_bytes: u64,
}

impl CloudWorkerRuntimeControlBudget {
    pub fn validate(self) -> Result<(), CloudWorkerRuntimeControlError> {
        if self.timeout_ms == 0 || self.timeout_ms > MAX_CONTROL_TIMEOUT_MS {
            return Err(CloudWorkerRuntimeControlError::InvalidTimeoutBudget);
        }
        if self.max_output_bytes == 0 || self.max_output_bytes > MAX_CONTROL_OUTPUT_BYTES {
            return Err(CloudWorkerRuntimeControlError::InvalidOutputBudget);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerRuntimeControlRequest {
    pub action: CloudWorkerRuntimeControlAction,
    pub replay_key: ReplayKey,
    pub budget: CloudWorkerRuntimeControlBudget,
}

impl CloudWorkerRuntimeControlRequest {
    pub fn admit(
        &self,
        lease: &CloudWorkerLease,
    ) -> Result<CloudWorkerLeaseMutation, CloudWorkerRuntimeControlError> {
        self.budget.validate()?;
        self.action.admit(lease, self.replay_key.clone())
    }
}

/// A bounded, already-classified observation returned by a future producer.
/// Validation is pure and must complete before the owner applies the
/// lifecycle outcome; the result itself does not perform or retry work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerRuntimeControlResult {
    pub action: CloudWorkerRuntimeControlAction,
    pub mutation: CloudWorkerLeaseMutation,
    pub outcome: CloudWorkerMutationOutcome,
    pub elapsed_ms: u64,
    pub diagnostic: Option<CloudWorkerDiagnostic>,
}

impl CloudWorkerRuntimeControlResult {
    pub fn validate_against(
        &self,
        request: &CloudWorkerRuntimeControlRequest,
        expected_mutation: &CloudWorkerLeaseMutation,
    ) -> Result<(), CloudWorkerRuntimeControlError> {
        request.budget.validate()?;
        if self.action != request.action {
            return Err(CloudWorkerRuntimeControlError::ActionMismatch);
        }
        if self.mutation != *expected_mutation {
            return Err(CloudWorkerRuntimeControlError::MutationMismatch);
        }
        if self.elapsed_ms > request.budget.timeout_ms
            && self.outcome != CloudWorkerMutationOutcome::OutcomeUnknown
        {
            return Err(CloudWorkerRuntimeControlError::ElapsedExceedsTimeout);
        }
        self.diagnostic
            .as_ref()
            .ok_or(CloudWorkerRuntimeControlError::InvalidDiagnostic)?
            .validate()
            .map_err(|_| CloudWorkerRuntimeControlError::InvalidDiagnostic)?;
        Ok(())
    }
}

/// The non-replayable control operations that a future provider producer may
/// perform against an already admitted worker. This type only admits a
/// lease-fenced mutation; it never performs the operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerRuntimeControlAction {
    Prepare,
    Warmup,
    Bootstrap,
    Inspect,
    Heartbeat,
    Stop,
    Dispose,
}

impl CloudWorkerRuntimeControlAction {
    /// Returns the lifecycle that a completed operation is allowed to enter.
    /// Submitted, failed, cancelled and outcome-unknown results are settled
    /// by the lease owner separately.
    pub fn completed_lifecycle(
        self,
        current: CloudWorkerLifecycle,
    ) -> Result<CloudWorkerLifecycle, CloudWorkerRuntimeControlError> {
        let next = match (self, current) {
            // Inspecting and Heartbeating are transient lifecycle labels used
            // by the wider lease state machine. A completed read/control
            // operation returns the lease to Enrolled so its capability is not
            // stranded after one successful observation.
            (Self::Prepare, CloudWorkerLifecycle::Admitted) => CloudWorkerLifecycle::Preparing,
            (Self::Warmup, CloudWorkerLifecycle::Preparing) => CloudWorkerLifecycle::Warming,
            (Self::Bootstrap, CloudWorkerLifecycle::Warming) => CloudWorkerLifecycle::Bootstrapping,
            (Self::Inspect, CloudWorkerLifecycle::Admitted | CloudWorkerLifecycle::Enrolled)
            | (Self::Heartbeat, CloudWorkerLifecycle::Admitted | CloudWorkerLifecycle::Enrolled) => {
                CloudWorkerLifecycle::Enrolled
            }
            (Self::Stop, CloudWorkerLifecycle::Enrolled)
            | (Self::Stop, CloudWorkerLifecycle::OutcomeUnknown) => CloudWorkerLifecycle::Stopping,
            (Self::Dispose, CloudWorkerLifecycle::Stopping)
            | (Self::Dispose, CloudWorkerLifecycle::OutcomeUnknown) => {
                CloudWorkerLifecycle::Disposed
            }
            (_, current) => {
                return Err(CloudWorkerRuntimeControlError::InvalidLifecycle {
                    action: self,
                    current,
                });
            }
        };
        Ok(next)
    }

    /// Builds the exact mutation identity a producer must use for settlement.
    /// The owner remains the only component allowed to apply the result.
    pub fn admit(
        self,
        lease: &CloudWorkerLease,
        replay_key: ReplayKey,
    ) -> Result<CloudWorkerLeaseMutation, CloudWorkerRuntimeControlError> {
        self.completed_lifecycle(lease.lifecycle)?;
        let mutation = lease.mutation(
            lease.owner_epoch,
            lease.generation,
            lease.runtime.clone(),
            replay_key,
        );
        lease.validate(&mutation)?;
        Ok(mutation)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerRuntimeControlError {
    #[error("runtime control {action:?} is invalid while lease is {current:?}")]
    InvalidLifecycle {
        action: CloudWorkerRuntimeControlAction,
        current: CloudWorkerLifecycle,
    },
    #[error("runtime control timeout budget is outside the allowed bounds")]
    InvalidTimeoutBudget,
    #[error("runtime control output budget is outside the allowed bounds")]
    InvalidOutputBudget,
    #[error("runtime control result action does not match its request")]
    ActionMismatch,
    #[error("runtime control result mutation does not match the admitted mutation")]
    MutationMismatch,
    #[error("runtime control result exceeded its admitted timeout budget")]
    ElapsedExceedsTimeout,
    #[error("runtime control result diagnostic is missing redaction or contains invalid text")]
    InvalidDiagnostic,
    #[error(transparent)]
    Lease(#[from] CloudWorkerLeaseError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::cloud_workers::{
        CloudWorkerLeaseScope, CloudWorkerMutationOutcome, CloudWorkerRuntimeIdentity,
        LeaseGeneration, LeaseId, OwnerEpoch,
    };

    fn lease(lifecycle: CloudWorkerLifecycle) -> CloudWorkerLease {
        CloudWorkerLease {
            scope: CloudWorkerLeaseScope::new("account", "device").unwrap(),
            lease_id: LeaseId("lease-1".into()),
            owner_epoch: OwnerEpoch(4),
            generation: LeaseGeneration(9),
            runtime: CloudWorkerRuntimeIdentity::new("provider", "profile", "runtime").unwrap(),
            lifecycle,
            last_outcome: if lifecycle == CloudWorkerLifecycle::OutcomeUnknown {
                CloudWorkerMutationOutcome::OutcomeUnknown
            } else {
                CloudWorkerMutationOutcome::Completed
            },
            replay_history: Vec::new(),
        }
    }

    #[test]
    fn inspect_and_heartbeat_require_enrolled_lease() {
        assert_eq!(
            CloudWorkerRuntimeControlAction::Inspect
                .completed_lifecycle(CloudWorkerLifecycle::Enrolled),
            Ok(CloudWorkerLifecycle::Enrolled)
        );
        assert_eq!(
            CloudWorkerRuntimeControlAction::Heartbeat
                .completed_lifecycle(CloudWorkerLifecycle::Enrolled),
            Ok(CloudWorkerLifecycle::Enrolled)
        );
        assert!(matches!(
            CloudWorkerRuntimeControlAction::Inspect
                .completed_lifecycle(CloudWorkerLifecycle::Preparing),
            Err(CloudWorkerRuntimeControlError::InvalidLifecycle { .. })
        ));
    }

    #[test]
    fn preparation_actions_follow_the_declared_lifecycle_order() {
        assert_eq!(
            CloudWorkerRuntimeControlAction::Prepare
                .completed_lifecycle(CloudWorkerLifecycle::Admitted),
            Ok(CloudWorkerLifecycle::Preparing)
        );
        assert_eq!(
            CloudWorkerRuntimeControlAction::Warmup
                .completed_lifecycle(CloudWorkerLifecycle::Preparing),
            Ok(CloudWorkerLifecycle::Warming)
        );
        assert_eq!(
            CloudWorkerRuntimeControlAction::Bootstrap
                .completed_lifecycle(CloudWorkerLifecycle::Warming),
            Ok(CloudWorkerLifecycle::Bootstrapping)
        );
        assert!(matches!(
            CloudWorkerRuntimeControlAction::Bootstrap
                .completed_lifecycle(CloudWorkerLifecycle::Admitted),
            Err(CloudWorkerRuntimeControlError::InvalidLifecycle { .. })
        ));
    }

    #[test]
    fn stop_and_dispose_preserve_outcome_unknown_recovery() {
        assert_eq!(
            CloudWorkerRuntimeControlAction::Stop
                .completed_lifecycle(CloudWorkerLifecycle::OutcomeUnknown),
            Ok(CloudWorkerLifecycle::Stopping)
        );
        assert_eq!(
            CloudWorkerRuntimeControlAction::Dispose
                .completed_lifecycle(CloudWorkerLifecycle::OutcomeUnknown),
            Ok(CloudWorkerLifecycle::Disposed)
        );
        assert!(matches!(
            CloudWorkerRuntimeControlAction::Dispose
                .completed_lifecycle(CloudWorkerLifecycle::Enrolled),
            Err(CloudWorkerRuntimeControlError::InvalidLifecycle { .. })
        ));
    }

    #[test]
    fn admission_preserves_fencing_identity() {
        let lease = lease(CloudWorkerLifecycle::Enrolled);
        let mutation = CloudWorkerRuntimeControlAction::Heartbeat
            .admit(&lease, ReplayKey("heartbeat-1".into()))
            .unwrap();
        assert_eq!(mutation.lease_id, lease.lease_id);
        assert_eq!(mutation.owner_epoch, lease.owner_epoch);
        assert_eq!(mutation.generation, lease.generation);
        assert_eq!(mutation.runtime, lease.runtime);
    }

    #[test]
    fn admission_rejects_invalid_replay_key_before_side_effects() {
        let lease = lease(CloudWorkerLifecycle::Enrolled);
        assert_eq!(
            CloudWorkerRuntimeControlAction::Inspect.admit(&lease, ReplayKey("  ".into())),
            Err(CloudWorkerRuntimeControlError::Lease(
                CloudWorkerLeaseError::MissingReplayKey
            ))
        );
    }

    #[test]
    fn bounded_request_rejects_unbounded_execution_material_before_admission() {
        let lease = lease(CloudWorkerLifecycle::Enrolled);
        let request = CloudWorkerRuntimeControlRequest {
            action: CloudWorkerRuntimeControlAction::Heartbeat,
            replay_key: ReplayKey("heartbeat-1".into()),
            budget: CloudWorkerRuntimeControlBudget {
                timeout_ms: 0,
                max_output_bytes: 1024,
            },
        };
        assert_eq!(
            request.admit(&lease),
            Err(CloudWorkerRuntimeControlError::InvalidTimeoutBudget)
        );

        let request = CloudWorkerRuntimeControlRequest {
            budget: CloudWorkerRuntimeControlBudget {
                timeout_ms: 1000,
                max_output_bytes: 1024 * 1024 + 1,
            },
            ..request
        };
        assert_eq!(
            request.admit(&lease),
            Err(CloudWorkerRuntimeControlError::InvalidOutputBudget)
        );
    }

    #[test]
    fn bounded_request_preserves_fenced_mutation_identity() {
        let lease = lease(CloudWorkerLifecycle::Enrolled);
        let request = CloudWorkerRuntimeControlRequest {
            action: CloudWorkerRuntimeControlAction::Heartbeat,
            replay_key: ReplayKey("heartbeat-bounded".into()),
            budget: CloudWorkerRuntimeControlBudget {
                timeout_ms: 30_000,
                max_output_bytes: 64 * 1024,
            },
        };
        let mutation = request.admit(&lease).unwrap();
        assert_eq!(mutation.lease_id, lease.lease_id);
        assert_eq!(mutation.owner_epoch, lease.owner_epoch);
        assert_eq!(mutation.generation, lease.generation);
        assert_eq!(mutation.runtime, lease.runtime);
        assert_eq!(mutation.replay_key, request.replay_key);
    }

    #[test]
    fn result_validation_binds_action_mutation_timeout_and_redaction() {
        let lease = lease(CloudWorkerLifecycle::Enrolled);
        let request = CloudWorkerRuntimeControlRequest {
            action: CloudWorkerRuntimeControlAction::Heartbeat,
            replay_key: ReplayKey("heartbeat-result".into()),
            budget: CloudWorkerRuntimeControlBudget {
                timeout_ms: 30_000,
                max_output_bytes: 64 * 1024,
            },
        };
        let mutation = request.admit(&lease).unwrap();
        let result = CloudWorkerRuntimeControlResult {
            action: request.action,
            mutation: mutation.clone(),
            outcome: CloudWorkerMutationOutcome::Completed,
            elapsed_ms: 250,
            diagnostic: Some(CloudWorkerDiagnostic::redacted(
                "heartbeat",
                "worker responded",
            )),
        };
        assert_eq!(result.validate_against(&request, &mutation), Ok(()));

        let mut missing_diagnostic = result.clone();
        missing_diagnostic.diagnostic = None;
        assert_eq!(
            missing_diagnostic.validate_against(&request, &mutation),
            Err(CloudWorkerRuntimeControlError::InvalidDiagnostic)
        );

        let mut invalid = result.clone();
        invalid.elapsed_ms = request.budget.timeout_ms + 1;
        assert_eq!(
            invalid.validate_against(&request, &mutation),
            Err(CloudWorkerRuntimeControlError::ElapsedExceedsTimeout)
        );

        invalid = result;
        invalid.diagnostic = Some(CloudWorkerDiagnostic {
            code: "raw".into(),
            message: "unredacted".into(),
            redacted: false,
        });
        assert_eq!(
            invalid.validate_against(&request, &mutation),
            Err(CloudWorkerRuntimeControlError::InvalidDiagnostic)
        );

        let timed_out = CloudWorkerRuntimeControlResult {
            action: request.action,
            mutation,
            outcome: CloudWorkerMutationOutcome::OutcomeUnknown,
            elapsed_ms: request.budget.timeout_ms + 1,
            diagnostic: Some(CloudWorkerDiagnostic::redacted(
                "timeout",
                "control response was not observed",
            )),
        };
        assert_eq!(
            timed_out.validate_against(&request, &timed_out.mutation),
            Ok(())
        );
    }
}
