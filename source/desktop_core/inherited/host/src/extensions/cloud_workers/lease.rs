use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_LEASE_ID_CHARS: usize = 256;
const MAX_SCOPE_CHARS: usize = 256;
const MAX_RUNTIME_FIELD_CHARS: usize = 256;
const MAX_REPLAY_KEY_CHARS: usize = 256;
const MAX_REPLAY_HISTORY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerLeaseScope {
    pub account_scope: String,
    pub device_scope: String,
}

impl CloudWorkerLeaseScope {
    pub fn new(
        account_scope: impl Into<String>,
        device_scope: impl Into<String>,
    ) -> Result<Self, CloudWorkerLeaseError> {
        let account_scope = account_scope.into();
        let device_scope = device_scope.into();
        if !valid_text(&account_scope, MAX_SCOPE_CHARS)
            || !valid_text(&device_scope, MAX_SCOPE_CHARS)
        {
            return Err(CloudWorkerLeaseError::InvalidScope);
        }
        Ok(Self {
            account_scope,
            device_scope,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerRuntimeIdentity {
    pub provider: String,
    pub profile: String,
    pub runtime_id: String,
}

impl CloudWorkerRuntimeIdentity {
    pub fn new(
        provider: impl Into<String>,
        profile: impl Into<String>,
        runtime_id: impl Into<String>,
    ) -> Result<Self, CloudWorkerLeaseError> {
        let identity = Self {
            provider: provider.into(),
            profile: profile.into(),
            runtime_id: runtime_id.into(),
        };
        if [&identity.provider, &identity.profile, &identity.runtime_id]
            .iter()
            .any(|value| !valid_text(value, MAX_RUNTIME_FIELD_CHARS))
        {
            return Err(CloudWorkerLeaseError::InvalidRuntimeIdentity);
        }
        Ok(identity)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LeaseId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerEpoch(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReplayKey(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerLifecycle {
    Admitted,
    Preparing,
    Warming,
    Bootstrapping,
    Enrolled,
    Inspecting,
    Heartbeating,
    Stopping,
    Disposed,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerMutationOutcome {
    Submitted,
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerLease {
    pub scope: CloudWorkerLeaseScope,
    pub lease_id: LeaseId,
    pub owner_epoch: OwnerEpoch,
    pub generation: LeaseGeneration,
    pub runtime: CloudWorkerRuntimeIdentity,
    pub lifecycle: CloudWorkerLifecycle,
    pub last_outcome: CloudWorkerMutationOutcome,
    #[serde(default)]
    pub replay_history: Vec<ReplayKey>,
}

/// Ephemeral capability handed to an existing Runner, Computer or sandbox
/// consumer after the provisioning owner has established an enrolled lease.
/// It carries identity/fencing material only; it is not a second lifecycle
/// store and cannot launch, authorize, or mutate a worker by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudWorkerLeasedCapability {
    scope: CloudWorkerLeaseScope,
    lease_id: LeaseId,
    owner_epoch: OwnerEpoch,
    generation: LeaseGeneration,
    runtime: CloudWorkerRuntimeIdentity,
}

impl CloudWorkerLeasedCapability {
    pub fn scope(&self) -> &CloudWorkerLeaseScope {
        &self.scope
    }

    pub fn lease_id(&self) -> &LeaseId {
        &self.lease_id
    }

    pub fn owner_epoch(&self) -> OwnerEpoch {
        self.owner_epoch
    }

    pub fn generation(&self) -> LeaseGeneration {
        self.generation
    }

    pub fn runtime(&self) -> &CloudWorkerRuntimeIdentity {
        &self.runtime
    }
}

impl CloudWorkerLease {
    pub fn validate_persisted(&self) -> Result<(), CloudWorkerLeaseError> {
        if !valid_text(&self.lease_id.0, MAX_LEASE_ID_CHARS) {
            return Err(CloudWorkerLeaseError::InvalidPersistedRecord);
        }
        if !valid_text(&self.scope.account_scope, MAX_SCOPE_CHARS)
            || !valid_text(&self.scope.device_scope, MAX_SCOPE_CHARS)
        {
            return Err(CloudWorkerLeaseError::InvalidPersistedRecord);
        }
        if !valid_text(&self.runtime.provider, MAX_RUNTIME_FIELD_CHARS)
            || !valid_text(&self.runtime.profile, MAX_RUNTIME_FIELD_CHARS)
            || !valid_text(&self.runtime.runtime_id, MAX_RUNTIME_FIELD_CHARS)
        {
            return Err(CloudWorkerLeaseError::InvalidPersistedRecord);
        }
        let mut replay_keys = HashSet::with_capacity(self.replay_history.len());
        if self.replay_history.len() > MAX_REPLAY_HISTORY
            || self
                .replay_history
                .iter()
                .any(|key| !valid_text(&key.0, MAX_REPLAY_KEY_CHARS) || !replay_keys.insert(key))
        {
            return Err(CloudWorkerLeaseError::InvalidPersistedRecord);
        }
        if (self.lifecycle == CloudWorkerLifecycle::OutcomeUnknown)
            != (self.last_outcome == CloudWorkerMutationOutcome::OutcomeUnknown)
            || (self.lifecycle == CloudWorkerLifecycle::Disposed
                && self.last_outcome != CloudWorkerMutationOutcome::Completed)
        {
            return Err(CloudWorkerLeaseError::InvalidPersistedRecord);
        }
        Ok(())
    }

    pub fn capability(&self) -> Result<CloudWorkerLeasedCapability, CloudWorkerLeaseError> {
        if !matches!(
            self.lifecycle,
            CloudWorkerLifecycle::Enrolled
                | CloudWorkerLifecycle::Inspecting
                | CloudWorkerLifecycle::Heartbeating
        ) || self.last_outcome != CloudWorkerMutationOutcome::Completed
        {
            return Err(CloudWorkerLeaseError::CapabilityUnavailable {
                lifecycle: self.lifecycle,
                outcome: self.last_outcome,
            });
        }
        Ok(CloudWorkerLeasedCapability {
            scope: self.scope.clone(),
            lease_id: self.lease_id.clone(),
            owner_epoch: self.owner_epoch,
            generation: self.generation,
            runtime: self.runtime.clone(),
        })
    }

    pub fn mutation(
        &self,
        owner_epoch: OwnerEpoch,
        generation: LeaseGeneration,
        runtime: CloudWorkerRuntimeIdentity,
        replay_key: ReplayKey,
    ) -> CloudWorkerLeaseMutation {
        CloudWorkerLeaseMutation {
            lease_id: self.lease_id.clone(),
            owner_epoch,
            generation,
            runtime,
            replay_key,
        }
    }

    pub fn validate(
        &self,
        mutation: &CloudWorkerLeaseMutation,
    ) -> Result<(), CloudWorkerLeaseError> {
        if mutation.lease_id != self.lease_id {
            return Err(CloudWorkerLeaseError::StaleLease);
        }
        if mutation.owner_epoch != self.owner_epoch {
            return Err(CloudWorkerLeaseError::StaleOwnerEpoch);
        }
        if mutation.generation != self.generation {
            return Err(CloudWorkerLeaseError::StaleGeneration);
        }
        if mutation.runtime != self.runtime {
            return Err(CloudWorkerLeaseError::StaleRuntimeIdentity);
        }
        if mutation.replay_key.0.trim().is_empty() {
            return Err(CloudWorkerLeaseError::MissingReplayKey);
        }
        if !valid_text(&mutation.replay_key.0, MAX_REPLAY_KEY_CHARS) {
            return Err(CloudWorkerLeaseError::InvalidReplayKey);
        }
        if self.replay_history.contains(&mutation.replay_key) {
            return Err(CloudWorkerLeaseError::ReplayAlreadySettled);
        }
        Ok(())
    }

    pub fn apply_outcome(
        &mut self,
        next: CloudWorkerLifecycle,
        outcome: CloudWorkerMutationOutcome,
        replay_key: ReplayKey,
    ) -> Result<(), CloudWorkerLeaseError> {
        if !valid_text(&replay_key.0, MAX_REPLAY_KEY_CHARS) {
            return Err(CloudWorkerLeaseError::InvalidReplayKey);
        }
        if self.replay_history.contains(&replay_key) {
            return Err(CloudWorkerLeaseError::ReplayAlreadySettled);
        }
        if self.replay_history.len() >= MAX_REPLAY_HISTORY {
            return Err(CloudWorkerLeaseError::ReplayHistoryLimitReached);
        }
        let advances_generation = matches!(
            outcome,
            CloudWorkerMutationOutcome::Completed | CloudWorkerMutationOutcome::OutcomeUnknown
        );
        let next_generation = if advances_generation {
            Some(LeaseGeneration(
                self.generation
                    .0
                    .checked_add(1)
                    .ok_or(CloudWorkerLeaseError::GenerationExhausted)?,
            ))
        } else {
            None
        };
        match outcome {
            CloudWorkerMutationOutcome::Submitted => {
                if next != self.lifecycle {
                    return Err(CloudWorkerLeaseError::SubmittedStateChanged);
                }
            }
            CloudWorkerMutationOutcome::Completed => {
                if !is_allowed_transition(self.lifecycle, next) {
                    return Err(CloudWorkerLeaseError::InvalidLifecycleTransition {
                        from: self.lifecycle,
                        to: next,
                    });
                }
                self.lifecycle = next;
            }
            CloudWorkerMutationOutcome::Failed | CloudWorkerMutationOutcome::Cancelled => {
                if next != self.lifecycle {
                    return Err(CloudWorkerLeaseError::UnsettledStateChanged);
                }
            }
            CloudWorkerMutationOutcome::OutcomeUnknown => {
                self.lifecycle = CloudWorkerLifecycle::OutcomeUnknown;
            }
        }
        if let Some(generation) = next_generation {
            self.generation = generation;
        }
        self.last_outcome = outcome;
        self.replay_history.push(replay_key);
        Ok(())
    }
}

fn is_allowed_transition(from: CloudWorkerLifecycle, to: CloudWorkerLifecycle) -> bool {
    matches!(
        (from, to),
        (
            CloudWorkerLifecycle::Admitted,
            CloudWorkerLifecycle::Preparing
        ) | (
            CloudWorkerLifecycle::Admitted,
            CloudWorkerLifecycle::Enrolled
        ) | (
            CloudWorkerLifecycle::Preparing,
            CloudWorkerLifecycle::Warming
        ) | (
            CloudWorkerLifecycle::Warming,
            CloudWorkerLifecycle::Bootstrapping
        ) | (
            CloudWorkerLifecycle::Bootstrapping,
            CloudWorkerLifecycle::Enrolled
        ) | (
            CloudWorkerLifecycle::Enrolled,
            CloudWorkerLifecycle::Enrolled
        ) | (
            CloudWorkerLifecycle::Enrolled,
            CloudWorkerLifecycle::Inspecting
        ) | (
            CloudWorkerLifecycle::Enrolled,
            CloudWorkerLifecycle::Heartbeating
        ) | (
            CloudWorkerLifecycle::Enrolled,
            CloudWorkerLifecycle::Stopping
        ) | (
            CloudWorkerLifecycle::Inspecting,
            CloudWorkerLifecycle::Enrolled
        ) | (
            CloudWorkerLifecycle::Heartbeating,
            CloudWorkerLifecycle::Enrolled
        ) | (
            CloudWorkerLifecycle::Stopping,
            CloudWorkerLifecycle::Disposed
        ) | (
            CloudWorkerLifecycle::OutcomeUnknown,
            CloudWorkerLifecycle::Admitted
        ) | (
            CloudWorkerLifecycle::OutcomeUnknown,
            CloudWorkerLifecycle::Stopping
        ) | (
            CloudWorkerLifecycle::OutcomeUnknown,
            CloudWorkerLifecycle::Disposed
        )
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerLeaseMutation {
    pub lease_id: LeaseId,
    pub owner_epoch: OwnerEpoch,
    pub generation: LeaseGeneration,
    pub runtime: CloudWorkerRuntimeIdentity,
    pub replay_key: ReplayKey,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerLeaseError {
    #[error("account and device scope must be non-empty")]
    InvalidScope,
    #[error("provider, profile and runtime identity must be non-empty")]
    InvalidRuntimeIdentity,
    #[error("mutation refers to another lease")]
    StaleLease,
    #[error("mutation refers to a stale owner epoch")]
    StaleOwnerEpoch,
    #[error("mutation refers to a stale lease generation")]
    StaleGeneration,
    #[error("mutation refers to another runtime identity")]
    StaleRuntimeIdentity,
    #[error("mutation requires a replay key")]
    MissingReplayKey,
    #[error("mutation replay key is invalid or oversized")]
    InvalidReplayKey,
    #[error("mutation replay key was already settled for this lease")]
    ReplayAlreadySettled,
    #[error("lease replay history is full; refusing to settle another mutation")]
    ReplayHistoryLimitReached,
    #[error("lease capability is unavailable in lifecycle {lifecycle:?} with outcome {outcome:?}")]
    CapabilityUnavailable {
        lifecycle: CloudWorkerLifecycle,
        outcome: CloudWorkerMutationOutcome,
    },
    #[error("a submitted mutation cannot change lifecycle state")]
    SubmittedStateChanged,
    #[error("an unsettled mutation cannot change lifecycle state")]
    UnsettledStateChanged,
    #[error("invalid worker lifecycle transition from {from:?} to {to:?}")]
    InvalidLifecycleTransition {
        from: CloudWorkerLifecycle,
        to: CloudWorkerLifecycle,
    },
    #[error("lease generation exhausted")]
    GenerationExhausted,
    #[error("persisted lease record is invalid")]
    InvalidPersistedRecord,
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(|character| character.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease() -> CloudWorkerLease {
        CloudWorkerLease {
            scope: CloudWorkerLeaseScope::new("account", "device").unwrap(),
            lease_id: LeaseId("lease-1".into()),
            owner_epoch: OwnerEpoch(3),
            generation: LeaseGeneration(7),
            runtime: CloudWorkerRuntimeIdentity::new("provider", "profile", "runtime").unwrap(),
            lifecycle: CloudWorkerLifecycle::Admitted,
            last_outcome: CloudWorkerMutationOutcome::Completed,
            replay_history: Vec::new(),
        }
    }

    #[test]
    fn accepts_a_matching_mutation_identity() {
        let lease = lease();
        let mutation = lease.mutation(
            OwnerEpoch(3),
            LeaseGeneration(7),
            lease.runtime.clone(),
            ReplayKey("request-1".into()),
        );
        assert_eq!(lease.validate(&mutation), Ok(()));
    }

    #[test]
    fn rejects_stale_generation_before_side_effects() {
        let lease = lease();
        let mutation = lease.mutation(
            OwnerEpoch(3),
            LeaseGeneration(8),
            lease.runtime.clone(),
            ReplayKey("request-1".into()),
        );
        assert_eq!(
            lease.validate(&mutation),
            Err(CloudWorkerLeaseError::StaleGeneration)
        );
    }

    #[test]
    fn rejects_empty_replay_keys() {
        let lease = lease();
        let mutation = lease.mutation(
            OwnerEpoch(3),
            LeaseGeneration(7),
            lease.runtime.clone(),
            ReplayKey("  ".into()),
        );
        assert_eq!(
            lease.validate(&mutation),
            Err(CloudWorkerLeaseError::MissingReplayKey)
        );
    }

    #[test]
    fn rejects_control_and_oversized_replay_keys() {
        let lease = lease();
        for replay_key in ["request\n1".into(), "r".repeat(MAX_REPLAY_KEY_CHARS + 1)] {
            let mutation = lease.mutation(
                OwnerEpoch(3),
                LeaseGeneration(7),
                lease.runtime.clone(),
                ReplayKey(replay_key),
            );
            assert_eq!(
                lease.validate(&mutation),
                Err(CloudWorkerLeaseError::InvalidReplayKey)
            );
        }
    }

    #[test]
    fn constructors_reject_control_and_oversized_identity_fields() {
        assert_eq!(
            CloudWorkerLeaseScope::new("account\n", "device"),
            Err(CloudWorkerLeaseError::InvalidScope)
        );
        assert_eq!(
            CloudWorkerLeaseScope::new("a".repeat(MAX_SCOPE_CHARS + 1), "device"),
            Err(CloudWorkerLeaseError::InvalidScope)
        );
        assert_eq!(
            CloudWorkerRuntimeIdentity::new("provider", "profile\t", "runtime"),
            Err(CloudWorkerLeaseError::InvalidRuntimeIdentity)
        );
        assert_eq!(
            CloudWorkerRuntimeIdentity::new(
                "provider",
                "profile",
                "r".repeat(MAX_RUNTIME_FIELD_CHARS + 1)
            ),
            Err(CloudWorkerLeaseError::InvalidRuntimeIdentity)
        );
    }

    #[test]
    fn rejects_replaying_a_settled_mutation_key() {
        let mut lease = lease();
        let replay_key = ReplayKey("failed-1".into());
        let mutation = lease.mutation(
            OwnerEpoch(3),
            LeaseGeneration(7),
            lease.runtime.clone(),
            replay_key.clone(),
        );
        lease
            .apply_outcome(
                CloudWorkerLifecycle::Admitted,
                CloudWorkerMutationOutcome::Failed,
                replay_key,
            )
            .unwrap();
        assert_eq!(
            lease.validate(&mutation),
            Err(CloudWorkerLeaseError::ReplayAlreadySettled)
        );
    }

    #[test]
    fn rejects_duplicate_replay_history_on_restore() {
        let mut lease = lease();
        lease.replay_history = vec![ReplayKey("settled-1".into()), ReplayKey("settled-1".into())];
        assert_eq!(
            lease.validate_persisted(),
            Err(CloudWorkerLeaseError::InvalidPersistedRecord)
        );
    }

    #[test]
    fn disposed_lease_requires_a_confirmed_completed_outcome() {
        let mut lease = lease();
        lease.lifecycle = CloudWorkerLifecycle::Disposed;
        lease.last_outcome = CloudWorkerMutationOutcome::Failed;
        assert_eq!(
            lease.validate_persisted(),
            Err(CloudWorkerLeaseError::InvalidPersistedRecord)
        );
        lease.last_outcome = CloudWorkerMutationOutcome::Completed;
        assert!(lease.validate_persisted().is_ok());
    }

    #[test]
    fn only_completed_results_advance_the_lifecycle() {
        let mut lease = lease();
        assert_eq!(
            lease.apply_outcome(
                CloudWorkerLifecycle::Preparing,
                CloudWorkerMutationOutcome::Submitted,
                ReplayKey("submitted-1".into()),
            ),
            Err(CloudWorkerLeaseError::SubmittedStateChanged)
        );
        assert_eq!(
            lease.apply_outcome(
                CloudWorkerLifecycle::Preparing,
                CloudWorkerMutationOutcome::Completed,
                ReplayKey("completed-1".into()),
            ),
            Ok(())
        );
        assert_eq!(lease.lifecycle, CloudWorkerLifecycle::Preparing);
        assert_eq!(lease.generation, LeaseGeneration(8));
    }

    #[test]
    fn ambiguous_results_enter_recovery_state() {
        let mut lease = lease();
        assert_eq!(
            lease.apply_outcome(
                CloudWorkerLifecycle::Admitted,
                CloudWorkerMutationOutcome::OutcomeUnknown,
                ReplayKey("unknown-1".into()),
            ),
            Ok(())
        );
        assert_eq!(lease.lifecycle, CloudWorkerLifecycle::OutcomeUnknown);
        assert_eq!(lease.generation, LeaseGeneration(8));
        assert_eq!(
            lease.apply_outcome(
                CloudWorkerLifecycle::Preparing,
                CloudWorkerMutationOutcome::Completed,
                ReplayKey("recovery-1".into()),
            ),
            Err(CloudWorkerLeaseError::InvalidLifecycleTransition {
                from: CloudWorkerLifecycle::OutcomeUnknown,
                to: CloudWorkerLifecycle::Preparing,
            })
        );
    }

    #[test]
    fn capability_requires_an_enrolled_completed_lease() {
        let mut lease = lease();
        assert!(matches!(
            lease.capability(),
            Err(CloudWorkerLeaseError::CapabilityUnavailable { .. })
        ));
        lease.lifecycle = CloudWorkerLifecycle::Enrolled;
        assert!(lease.capability().is_ok());
        lease.last_outcome = CloudWorkerMutationOutcome::OutcomeUnknown;
        assert!(matches!(
            lease.capability(),
            Err(CloudWorkerLeaseError::CapabilityUnavailable { .. })
        ));
    }

    #[test]
    fn generation_overflow_fails_before_lifecycle_mutation() {
        let mut lease = lease();
        lease.generation = LeaseGeneration(u64::MAX);
        assert_eq!(
            lease.apply_outcome(
                CloudWorkerLifecycle::Preparing,
                CloudWorkerMutationOutcome::Completed,
                ReplayKey("overflow-1".into()),
            ),
            Err(CloudWorkerLeaseError::GenerationExhausted)
        );
        assert_eq!(lease.lifecycle, CloudWorkerLifecycle::Admitted);
        assert_eq!(lease.generation, LeaseGeneration(u64::MAX));
    }
}
