use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::extensions::extension_ids_generated::HostExtensionId;

use super::{
    CloudWorkerArtifactAdmission, CloudWorkerArtifactAdmissionError,
    CloudWorkerArtifactAdmissionState, CloudWorkerArtifactDigest, CloudWorkerArtifactKind,
    CloudWorkerArtifactPolicy, CloudWorkerArtifactPublisher, CloudWorkerArtifactSource,
    CloudWorkerArtifactStoreError, CloudWorkerBinaryIdentity, CloudWorkerBootstrapPhase,
    CloudWorkerBootstrapRecord, CloudWorkerBootstrapRequest, CloudWorkerBootstrapRetryClass,
    CloudWorkerCatalog, CloudWorkerCatalogError, CloudWorkerDiagnostic, CloudWorkerDoctorAction,
    CloudWorkerDoctorError, CloudWorkerDoctorRequest, CloudWorkerDoctorResult, CloudWorkerLease,
    CloudWorkerLeaseMutation, CloudWorkerLeaseScope, CloudWorkerLeasedCapability,
    CloudWorkerLifecycle, CloudWorkerMutationOutcome, CloudWorkerOwnerError,
    CloudWorkerOwnerStatus, CloudWorkerProcessReceipt, CloudWorkerProfileSuggestion,
    CloudWorkerProvisioningConfig, CloudWorkerProvisioningOwner, CloudWorkerProvisioningStore,
    CloudWorkerPublishedArtifact, CloudWorkerRuntimeControlAction,
    CloudWorkerRuntimeControlRequest, CloudWorkerRuntimeControlResult, CloudWorkerRuntimeIdentity,
    CloudWorkerSandboxCapability, CloudWorkerSandboxError, CloudWorkerSandboxRequest,
    CloudWorkerStoreError, CloudWorkerWarmImage, CloudWorkerWarmImageGeneration,
    CloudWorkerWarmImageOperation, LeaseId, verify_artifact_bytes,
};

pub const CLOUD_WORKERS_EXTENSION_ID: HostExtensionId = HostExtensionId::CloudWorkers;
pub const CLOUD_WORKERS_DEPENDENCIES: &[HostExtensionId] =
    &[HostExtensionId::Auth, HostExtensionId::Secrets];

#[derive(Clone)]
pub struct CloudWorkerProvisioningExtension {
    owner: Arc<Mutex<CloudWorkerProvisioningOwner>>,
    state_path: PathBuf,
    store: CloudWorkerProvisioningStore,
    artifact_publisher: CloudWorkerArtifactPublisher,
}

/// Read-only projection produced by the Cloud Worker owner for Gateway and
/// Coordinator consumers. It intentionally exposes readiness and mutation as
/// false until a real provider producer and side-effect owner are connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloudWorkerStatusSnapshot {
    pub owner: &'static str,
    pub status: &'static str,
    #[serde(rename = "providerReady")]
    pub provider_ready: bool,
    #[serde(rename = "productionMutation")]
    pub production_mutation: bool,
    #[serde(rename = "statePersisted")]
    pub state_persisted: bool,
    #[serde(rename = "leaseCount")]
    pub lease_count: usize,
    #[serde(rename = "warmImageCount")]
    pub warm_image_count: usize,
    #[serde(rename = "bootstrapCount")]
    pub bootstrap_count: usize,
    #[serde(rename = "artifactCount")]
    pub artifact_count: usize,
    #[serde(rename = "migrationReceiptCount")]
    pub migration_receipt_count: usize,
}

impl CloudWorkerProvisioningExtension {
    pub fn status(&self) -> CloudWorkerOwnerStatus {
        self.owner
            .lock()
            .map(|owner| owner.status())
            .unwrap_or(CloudWorkerOwnerStatus::Unavailable)
    }

    /// Routes provider capability discovery through the canonical owner and
    /// its exact-binary-identity cache. The loader remains the explicit
    /// provider/process boundary and may not authorize or launch a worker.
    pub fn resolve_provider_catalog<F>(
        &self,
        identity: CloudWorkerBinaryIdentity,
        load: F,
    ) -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>
    where
        F: FnOnce() -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>,
    {
        let mut owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerCatalogError::OwnerUnavailable)?;
        owner.resolve_catalog(identity, load)
    }

    /// Selects a machine only from the exact-binary-identity catalog already
    /// admitted by the owner. A cache miss or incompatible profile is an
    /// explicit discovery failure; this method grants no permission and does
    /// not launch or mutate a provider.
    pub fn select_provider_machine(
        &self,
        identity: &CloudWorkerBinaryIdentity,
        provider_id: &str,
        profile_id: &str,
        requirement: &super::CloudWorkerMachineRequirement,
    ) -> Result<super::CloudWorkerMachine, CloudWorkerCatalogError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerCatalogError::OwnerUnavailable)?;
        owner.select_cached_machine(identity, provider_id, profile_id, requirement)
    }

    /// Returns the owner-bound setup suggestion projection. It performs no
    /// provider discovery, authorization, readiness check or launch.
    pub fn suggest_profile(
        &self,
        provider_display_id: impl Into<String>,
        machine_class: impl Into<String>,
    ) -> Result<CloudWorkerProfileSuggestion, CloudWorkerMutationError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerMutationError::PoisonedOwner)?;
        owner
            .suggest_profile(provider_display_id, machine_class)
            .map_err(CloudWorkerMutationError::Owner)
    }

    /// Registers one policy-validated artifact admission in the canonical
    /// owner snapshot. Acquisition and publication remain external producers.
    pub fn register_artifact(
        &self,
        artifact: CloudWorkerArtifactAdmission,
        policy: &CloudWorkerArtifactPolicy,
    ) -> Result<(), CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.register_artifact(artifact, policy))
    }

    /// Verifies caller-supplied bytes and atomically records the resulting
    /// verified admission. Download, credential egress and publication remain
    /// owned by the future artifact producer.
    pub fn verify_and_register_artifact(
        &self,
        kind: CloudWorkerArtifactKind,
        policy: &CloudWorkerArtifactPolicy,
        source: &CloudWorkerArtifactSource,
        bytes: &[u8],
    ) -> Result<super::CloudWorkerVerifiedArtifact, CloudWorkerMutationError> {
        let verified = verify_artifact_bytes(policy, source, bytes)
            .map_err(CloudWorkerMutationError::Artifact)?;
        let admission = CloudWorkerArtifactAdmission {
            kind,
            source: source.clone(),
            size_bytes: verified.size_bytes,
            state: CloudWorkerArtifactAdmissionState::Verified,
        };
        self.mutate_and_persist(|owner| {
            owner.register_artifact(admission, policy)?;
            Ok(verified.clone())
        })
    }

    /// Settles one persisted artifact admission without retrying or hiding an
    /// outcome-unknown state.
    pub fn settle_artifact(
        &self,
        digest: &CloudWorkerArtifactDigest,
        next: CloudWorkerArtifactAdmissionState,
    ) -> Result<CloudWorkerArtifactAdmission, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.settle_artifact(digest, next))
    }

    /// Publishes one explicitly admitted artifact through the canonical
    /// owner boundary. The admission is durable before publication begins;
    /// known publication failures settle to `Damaged`, while a directory
    /// barrier failure settles to `OutcomeUnknown`. A successful file rename
    /// is not reported as success until the owner snapshot also settles to
    /// `Verified`.
    pub fn publish_verified_artifact(
        &self,
        kind: CloudWorkerArtifactKind,
        policy: &CloudWorkerArtifactPolicy,
        source: &CloudWorkerArtifactSource,
        bytes: &[u8],
    ) -> Result<CloudWorkerPublishedArtifact, CloudWorkerArtifactPublicationError> {
        let pending = CloudWorkerArtifactAdmission {
            kind: kind.clone(),
            source: source.clone(),
            size_bytes: bytes.len() as u64,
            state: CloudWorkerArtifactAdmissionState::Pending,
        };
        self.register_artifact(pending, policy)
            .map_err(CloudWorkerArtifactPublicationError::Admission)?;

        let published = match self
            .artifact_publisher
            .publish_verified(kind, policy, source, bytes)
        {
            Ok(published) => published,
            Err(error) => {
                let state = if matches!(
                    &error,
                    CloudWorkerArtifactStoreError::DirectoryBarrierOutcomeUnknown(_)
                ) {
                    CloudWorkerArtifactAdmissionState::OutcomeUnknown
                } else {
                    CloudWorkerArtifactAdmissionState::Damaged
                };
                let settled = self.settle_artifact(&source_digest(source), state);
                return match (state, settled) {
                    (CloudWorkerArtifactAdmissionState::Damaged, Ok(_)) => {
                        Err(CloudWorkerArtifactPublicationError::Publication(error))
                    }
                    (CloudWorkerArtifactAdmissionState::OutcomeUnknown, Ok(_)) => {
                        Err(CloudWorkerArtifactPublicationError::PublicationOutcomeUnknown(error))
                    }
                    (_, Err(settlement)) => Err(
                        CloudWorkerArtifactPublicationError::SettlementAfterFailure {
                            publication: error,
                            settlement,
                        },
                    ),
                    (CloudWorkerArtifactAdmissionState::Pending, Ok(_))
                    | (CloudWorkerArtifactAdmissionState::Verified, Ok(_)) => Err(
                        CloudWorkerArtifactPublicationError::SettlementAfterFailure {
                            publication: error,
                            settlement: CloudWorkerMutationError::Owner(
                                super::CloudWorkerOwnerError::ArtifactNotVerified,
                            ),
                        },
                    ),
                };
            }
        };

        self.settle_artifact(
            &published.proof.digest,
            CloudWorkerArtifactAdmissionState::Verified,
        )
        .map_err(|settlement| {
            CloudWorkerArtifactPublicationError::SettlementAfterPublication {
                publication: published.clone(),
                settlement,
            }
        })?;
        Ok(published)
    }

    /// Revalidates owner admission and the managed bytes before a future
    /// bootstrap/provider consumer uses a `Verified` artifact. This is a
    /// read-only check: a missing, replaced or mismatched file fails closed
    /// and is never repaired implicitly.
    pub fn resolve_verified_artifact(
        &self,
        digest: &CloudWorkerArtifactDigest,
        policy: &CloudWorkerArtifactPolicy,
    ) -> Result<CloudWorkerPublishedArtifact, CloudWorkerMutationError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerMutationError::PoisonedOwner)?;
        let admission = owner
            .verified_artifact_admission(digest)
            .map_err(CloudWorkerMutationError::Owner)?;
        let expected = super::CloudWorkerVerifiedArtifact {
            digest: digest.clone(),
            size_bytes: admission.size_bytes,
        };
        self.artifact_publisher
            .resolve_verified(admission.kind, policy, &admission.source, &expected)
            .map_err(CloudWorkerMutationError::ArtifactStore)
    }

    /// Registers warm-image state through the canonical owner and its atomic
    /// persistence adapter. No provider snapshot or image process is started.
    pub fn register_warm_image(
        &self,
        image: CloudWorkerWarmImage,
    ) -> Result<(), CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.register_warm_image(image))
    }

    /// Returns one owner-bound warm-image record after rechecking persisted
    /// state invariants. This read projection does not query a provider,
    /// capture/delete an image, clear cleanup debt or create a second store.
    pub fn inspect_warm_image(
        &self,
        image_id: &str,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerOwnerError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerOwnerError::Unavailable)?;
        owner.warm_image(image_id)
    }

    /// Returns the deterministic, read-only set of warm-image rows requiring
    /// explicit recovery handling. It does not select or execute a provider
    /// mutation.
    pub fn inspect_warm_image_recovery_candidates(
        &self,
    ) -> Result<Vec<CloudWorkerWarmImage>, CloudWorkerOwnerError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerOwnerError::Unavailable)?;
        Ok(owner.warm_image_recovery_candidates())
    }

    /// Selects one exact warm-image recovery row by ID. Healthy images are
    /// rejected so a read/recovery surface cannot accidentally broaden its
    /// target into a normal warm-image inventory.
    pub fn inspect_warm_image_recovery_candidate(
        &self,
        image_id: &str,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerOwnerError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerOwnerError::Unavailable)?;
        owner.warm_image_recovery_candidate(image_id)
    }

    /// Applies one generation-fenced warm-image transition and persists the
    /// resulting owner snapshot. Provider side effects remain a separate
    /// adapter and are intentionally not implied by this method.
    pub fn apply_warm_image(
        &self,
        image_id: &str,
        expected_generation: CloudWorkerWarmImageGeneration,
        operation: CloudWorkerWarmImageOperation,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| {
            owner.apply_warm_image(image_id, expected_generation, operation)
        })
    }

    /// Persists an ambiguous capture outcome as explicit recovery debt. The
    /// provider snapshot and cleanup consumer remain outside this façade.
    pub fn mark_warm_image_capture_uncertain(
        &self,
        image_id: &str,
        expected_generation: CloudWorkerWarmImageGeneration,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| {
            owner.mark_warm_image_capture_uncertain(image_id, expected_generation)
        })
    }

    /// Admits one lease through the canonical owner and persists the exact
    /// owner snapshot. Provider discovery and worker launch remain separate.
    pub fn admit_lease(
        &self,
        scope: CloudWorkerLeaseScope,
        lease_id: LeaseId,
        runtime: CloudWorkerRuntimeIdentity,
    ) -> Result<CloudWorkerLease, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.admit(scope, lease_id, runtime))
    }

    /// Builds a lease-fenced runtime-control mutation through the canonical
    /// owner. It does not call a provider or change persisted state.
    pub fn admit_runtime_control(
        &self,
        lease_id: &LeaseId,
        action: CloudWorkerRuntimeControlAction,
        replay_key: super::ReplayKey,
    ) -> Result<super::CloudWorkerLeaseMutation, CloudWorkerMutationError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerMutationError::PoisonedOwner)?;
        owner
            .admit_runtime_control(lease_id, action, replay_key)
            .map_err(CloudWorkerMutationError::Owner)
    }

    /// Admits a bounded runtime-control request against the current lease.
    /// The budget is carried to a future producer, while this façade still
    /// performs no provider RPC, process launch or timer side effect.
    pub fn admit_bounded_runtime_control(
        &self,
        lease_id: &LeaseId,
        request: CloudWorkerRuntimeControlRequest,
    ) -> Result<super::CloudWorkerLeaseMutation, CloudWorkerMutationError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerMutationError::PoisonedOwner)?;
        let lease = owner
            .lease(lease_id)
            .map_err(CloudWorkerMutationError::Owner)?;
        request.admit(lease).map_err(|error| {
            CloudWorkerMutationError::Owner(super::CloudWorkerOwnerError::RuntimeControl(error))
        })
    }

    /// Settles one fenced lease mutation and persists its lifecycle outcome,
    /// including outcome-unknown states without retrying or hiding them.
    pub fn settle_lease(
        &self,
        mutation: &CloudWorkerLeaseMutation,
        next: CloudWorkerLifecycle,
        outcome: CloudWorkerMutationOutcome,
    ) -> Result<CloudWorkerLease, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.settle_lease(mutation, next, outcome))
    }

    /// Settles a runtime-control mutation through its action-specific
    /// lifecycle contract and the canonical persistence boundary.
    pub fn settle_runtime_control(
        &self,
        mutation: &CloudWorkerLeaseMutation,
        action: CloudWorkerRuntimeControlAction,
        outcome: CloudWorkerMutationOutcome,
    ) -> Result<CloudWorkerLease, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.settle_runtime_control(mutation, action, outcome))
    }

    /// Validates a bounded producer result and then settles the same
    /// action-specific lifecycle through the canonical owner. Diagnostics are
    /// accepted only in the existing redacted form; this method still does
    /// not persist raw output, launch a process or retry an ambiguous result.
    pub fn settle_bounded_runtime_control(
        &self,
        request: &CloudWorkerRuntimeControlRequest,
        mutation: &CloudWorkerLeaseMutation,
        result: &CloudWorkerRuntimeControlResult,
    ) -> Result<CloudWorkerLease, CloudWorkerMutationError> {
        result
            .validate_against(request, mutation)
            .map_err(|error| {
                CloudWorkerMutationError::Owner(super::CloudWorkerOwnerError::RuntimeControl(error))
            })?;
        self.settle_runtime_control(mutation, request.action, result.outcome)
    }

    /// Registers a validated bootstrap record in the canonical owner snapshot.
    /// Secret values remain opaque references; no enrollment or process action
    /// is performed by this persistence façade.
    pub fn register_bootstrap(
        &self,
        record: CloudWorkerBootstrapRecord,
    ) -> Result<(), CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.register_bootstrap(record))
    }

    /// Settles a lease-fenced bootstrap phase and persists its bounded,
    /// redacted diagnostic state.
    pub fn settle_bootstrap(
        &self,
        mutation: &CloudWorkerLeaseMutation,
        phase: CloudWorkerBootstrapPhase,
        retry_class: CloudWorkerBootstrapRetryClass,
        diagnostic: Option<CloudWorkerDiagnostic>,
    ) -> Result<CloudWorkerBootstrapRecord, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| {
            owner.settle_bootstrap(mutation, phase, retry_class, diagnostic)
        })
    }

    /// Attaches a validated process launch receipt during the fenced
    /// Enrolling phase. This persists identity only; no process inspection or
    /// cleanup is performed here.
    pub fn attach_bootstrap_process_receipt(
        &self,
        mutation: &CloudWorkerLeaseMutation,
        receipt: CloudWorkerProcessReceipt,
    ) -> Result<CloudWorkerBootstrapRecord, CloudWorkerMutationError> {
        self.mutate_and_persist(|owner| owner.attach_bootstrap_process_receipt(mutation, receipt))
    }

    /// Provides the read-only doctor projection from the canonical owner.
    /// Mutating recovery actions remain explicitly unavailable until their
    /// provider, persistence and recovery owners are wired.
    pub fn inspect_doctor(
        &self,
        request: CloudWorkerDoctorRequest,
    ) -> Result<CloudWorkerDoctorResult, CloudWorkerDoctorError> {
        request.validate()?;
        if !matches!(request.action, CloudWorkerDoctorAction::Inspect) {
            return Err(CloudWorkerDoctorError::MutationNotWired);
        }
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerDoctorError::OwnerUnavailable)?;
        let snapshot = owner.snapshot();
        Ok(CloudWorkerDoctorResult::inspection_with_snapshot(
            self.state_path.exists(),
            snapshot.leases.len(),
            snapshot.bootstrap_records.len(),
            snapshot.artifacts.len(),
            snapshot.warm_images.len(),
        ))
    }

    /// Returns a read-only snapshot for the existing Host/Coordinator status
    /// consumer. The extension remains the only owner of the live state.
    pub fn snapshot(&self) -> Option<super::CloudWorkerPersistentSnapshot> {
        self.owner.lock().ok().map(|owner| owner.snapshot())
    }

    /// Re-reads the canonical owner record before an existing executor acts.
    /// The returned capability is still ephemeral; this method does not grant
    /// a device, start a provider, or create another lease store.
    pub fn validate_current_capability(
        &self,
        capability: &CloudWorkerLeasedCapability,
    ) -> Result<CloudWorkerLeasedCapability, CloudWorkerOwnerError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerOwnerError::Unavailable)?;
        if owner.status() != super::CloudWorkerOwnerStatus::Enabled {
            return Err(CloudWorkerOwnerError::Disabled);
        }
        let current = owner.leased_capability(capability.lease_id())?;
        if current != *capability {
            return Err(CloudWorkerOwnerError::StaleCapability);
        }
        Ok(current)
    }

    /// Re-reads the canonical lease before a future sandbox consumer uses a
    /// serialized capability. The sandbox descriptor owns no lifecycle state;
    /// stale, disabled or outcome-unknown leases fail closed here.
    pub fn validate_current_sandbox_capability(
        &self,
        capability: &CloudWorkerSandboxCapability,
    ) -> Result<CloudWorkerSandboxCapability, CloudWorkerOwnerError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerOwnerError::Unavailable)?;
        if owner.status() != super::CloudWorkerOwnerStatus::Enabled {
            return Err(CloudWorkerOwnerError::Disabled);
        }
        let current = owner.leased_capability(&capability.lease_id)?;
        capability
            .validate_against(&current)
            .map_err(|error| match error {
                CloudWorkerSandboxError::StaleCapability => CloudWorkerOwnerError::StaleCapability,
                _ => CloudWorkerOwnerError::StaleCapability,
            })?;
        Ok(capability.clone())
    }

    /// Revalidates both the canonical lease and the requested sandbox
    /// operation before delegation. Existing sandbox owners retain all actual
    /// filesystem/process side effects.
    pub fn validate_current_sandbox_request(
        &self,
        capability: &CloudWorkerSandboxCapability,
        request: &CloudWorkerSandboxRequest,
    ) -> Result<(), CloudWorkerOwnerError> {
        self.validate_current_sandbox_capability(capability)?;
        request
            .validate_against(capability)
            .map_err(|_| CloudWorkerOwnerError::StaleCapability)
    }

    /// Re-reads the canonical lease before a bootstrap producer may act. The
    /// request carries only fencing material and typed admission proofs; this
    /// method performs no enrollment, process launch or provider mutation.
    pub fn validate_current_bootstrap_request(
        &self,
        request: &CloudWorkerBootstrapRequest,
    ) -> Result<(), CloudWorkerOwnerError> {
        request
            .validate()
            .map_err(|_| CloudWorkerOwnerError::StaleCapability)?;
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerOwnerError::Unavailable)?;
        if owner.status() != super::CloudWorkerOwnerStatus::Enabled {
            return Err(CloudWorkerOwnerError::Disabled);
        }
        let current = owner.leased_capability(&request.lease_id)?;
        if current.scope() != &request.scope
            || current.owner_epoch() != request.owner_epoch
            || current.generation() != request.generation
            || current.runtime() != &request.runtime
        {
            return Err(CloudWorkerOwnerError::StaleCapability);
        }
        owner.verified_artifact(&request.artifact)?;
        Ok(())
    }

    /// Fences a bootstrap request and then resolves the actual managed
    /// artifact bytes for a future producer. The returned handle is usable
    /// only when owner admission and on-disk digest verification agree;
    /// enrollment, secret resolution, process launch and provider RPC remain
    /// outside this façade.
    pub fn resolve_current_bootstrap_artifact(
        &self,
        request: &CloudWorkerBootstrapRequest,
        policy: &CloudWorkerArtifactPolicy,
    ) -> Result<CloudWorkerPublishedArtifact, CloudWorkerMutationError> {
        self.validate_current_bootstrap_request(request)
            .map_err(CloudWorkerMutationError::Owner)?;
        self.resolve_verified_artifact(&request.artifact.digest, policy)
    }

    pub fn status_snapshot(&self) -> CloudWorkerStatusSnapshot {
        let (status, snapshot) = match self.owner.lock() {
            Ok(owner) => {
                let status = match owner.status() {
                    CloudWorkerOwnerStatus::Disabled => "disabled",
                    CloudWorkerOwnerStatus::Enabled => "enabled",
                    CloudWorkerOwnerStatus::Unavailable => "unavailable",
                };
                (status, Some(owner.snapshot()))
            }
            Err(_) => ("unavailable", None),
        };
        let state_persisted = fs::symlink_metadata(&self.state_path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false);
        CloudWorkerStatusSnapshot {
            owner: "cloud-workers",
            status,
            provider_ready: false,
            production_mutation: false,
            state_persisted,
            lease_count: snapshot.as_ref().map_or(0, |value| value.leases.len()),
            warm_image_count: snapshot.as_ref().map_or(0, |value| value.warm_images.len()),
            bootstrap_count: snapshot
                .as_ref()
                .map_or(0, |value| value.bootstrap_records.len()),
            artifact_count: snapshot.as_ref().map_or(0, |value| value.artifacts.len()),
            migration_receipt_count: snapshot
                .as_ref()
                .map_or(0, |value| value.migration_receipts.len()),
        }
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn persist(&self) -> Result<(), CloudWorkerStoreError> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerStoreError::PoisonedOwner)?;
        self.store.save_atomic(&self.state_path, &owner)
    }

    pub fn mutate_and_persist<T, F>(&self, mutate: F) -> Result<T, CloudWorkerMutationError>
    where
        F: FnOnce(&mut CloudWorkerProvisioningOwner) -> Result<T, CloudWorkerOwnerError>,
    {
        let mut owner = self
            .owner
            .lock()
            .map_err(|_| CloudWorkerMutationError::PoisonedOwner)?;
        let before = owner.snapshot();
        let value = mutate(&mut owner).map_err(CloudWorkerMutationError::Owner)?;
        match self.store.save_atomic(&self.state_path, &owner) {
            Ok(()) => Ok(value),
            Err(CloudWorkerStoreError::DirectoryBarrierOutcomeUnknown(error)) => {
                Err(CloudWorkerMutationError::OutcomeUnknown(
                    CloudWorkerStoreError::DirectoryBarrierOutcomeUnknown(error),
                ))
            }
            Err(error) => {
                owner
                    .restore_snapshot(before)
                    .map_err(CloudWorkerMutationError::Rollback)?;
                Err(CloudWorkerMutationError::Persistence(error))
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloudWorkerMutationError {
    #[error(transparent)]
    Owner(CloudWorkerOwnerError),
    #[error(transparent)]
    Artifact(CloudWorkerArtifactAdmissionError),
    #[error(transparent)]
    ArtifactStore(CloudWorkerArtifactStoreError),
    #[error(transparent)]
    Persistence(CloudWorkerStoreError),
    #[error("cloud-worker mutation persistence outcome is unknown: {0}")]
    OutcomeUnknown(#[source] CloudWorkerStoreError),
    #[error("cloud-worker mutation rollback failed: {0}")]
    Rollback(#[source] CloudWorkerOwnerError),
    #[error("cloud-worker owner lock is poisoned")]
    PoisonedOwner,
}

fn source_digest(source: &CloudWorkerArtifactSource) -> CloudWorkerArtifactDigest {
    match source {
        CloudWorkerArtifactSource::Https { digest, .. }
        | CloudWorkerArtifactSource::ManagedLocal { digest } => digest.clone(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloudWorkerArtifactPublicationError {
    #[error("artifact admission failed: {0}")]
    Admission(#[source] CloudWorkerMutationError),
    #[error("artifact publication failed after settling admission as damaged: {0}")]
    Publication(#[source] CloudWorkerArtifactStoreError),
    #[error("artifact publication outcome is unknown after settling admission: {0}")]
    PublicationOutcomeUnknown(#[source] CloudWorkerArtifactStoreError),
    #[error(
        "artifact publication failed and its recovery settlement failed: {publication}; settlement: {settlement}"
    )]
    SettlementAfterFailure {
        publication: CloudWorkerArtifactStoreError,
        settlement: CloudWorkerMutationError,
    },
    #[error("artifact was published but owner verification settlement failed: {settlement}")]
    SettlementAfterPublication {
        publication: CloudWorkerPublishedArtifact,
        settlement: CloudWorkerMutationError,
    },
}

pub fn start_cloud_worker_provisioning_extension(
    state_path: impl AsRef<Path>,
) -> Result<CloudWorkerProvisioningExtension, CloudWorkerExtensionError> {
    let state_path = state_path.as_ref().to_path_buf();
    let artifact_root = state_path
        .parent()
        .ok_or(CloudWorkerExtensionError::InvalidStatePath)?
        .join("artifacts");
    let artifact_publisher = CloudWorkerArtifactPublisher::new(artifact_root)?;
    let config = CloudWorkerProvisioningConfig::default();
    let store = CloudWorkerProvisioningStore;
    let owner = if state_path.exists() {
        let metadata = fs::symlink_metadata(&state_path).map_err(CloudWorkerStoreError::Read)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(CloudWorkerExtensionError::InvalidStatePath);
        }
        let snapshot = store.load(&state_path)?;
        CloudWorkerProvisioningOwner::from_snapshot(config, snapshot)?
    } else {
        CloudWorkerProvisioningOwner::new(config)?
    };
    Ok(CloudWorkerProvisioningExtension {
        owner: Arc::new(Mutex::new(owner)),
        state_path,
        store,
        artifact_publisher,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum CloudWorkerExtensionError {
    #[error("cloud-worker state path must be a regular non-symlink file")]
    InvalidStatePath,
    #[error(transparent)]
    Store(#[from] CloudWorkerStoreError),
    #[error(transparent)]
    Owner(#[from] CloudWorkerOwnerError),
    #[error(transparent)]
    ArtifactStore(#[from] CloudWorkerArtifactStoreError),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use super::{
        CloudWorkerArtifactPublicationError, CloudWorkerMutationError, CloudWorkerOwnerStatus,
        CloudWorkerProvisioningExtension, CloudWorkerProvisioningStore,
        start_cloud_worker_provisioning_extension,
    };
    use crate::extensions::cloud_workers::{
        CloudWorkerArtifactAdmissionError, CloudWorkerArtifactDigest, CloudWorkerArtifactKind,
        CloudWorkerArtifactPolicy, CloudWorkerArtifactSource, CloudWorkerDoctorAction,
        CloudWorkerDoctorError, CloudWorkerDoctorRequest, CloudWorkerDoctorStatus,
        CloudWorkerLeaseScope, CloudWorkerMutationOutcome, CloudWorkerProvisioningConfig,
        CloudWorkerProvisioningOwner, CloudWorkerRuntimeControlAction,
        CloudWorkerRuntimeControlBudget, CloudWorkerRuntimeControlRequest,
        CloudWorkerRuntimeControlResult, CloudWorkerRuntimeIdentity, CloudWorkerWarmImage, LeaseId,
        ReplayKey,
    };

    fn enabled_extension(state_path: PathBuf) -> CloudWorkerProvisioningExtension {
        let owner = CloudWorkerProvisioningOwner::new(CloudWorkerProvisioningConfig {
            enabled: true,
            max_leases: 1,
            max_warm_images: 1,
            max_artifacts: 1,
        })
        .unwrap();
        let artifact_root = state_path.parent().unwrap().join(format!(
            "cloud-worker-test-artifacts-{}",
            std::process::id()
        ));
        CloudWorkerProvisioningExtension {
            owner: Arc::new(Mutex::new(owner)),
            state_path,
            store: CloudWorkerProvisioningStore,
            artifact_publisher: super::CloudWorkerArtifactPublisher::new(artifact_root).unwrap(),
        }
    }

    fn artifact_policy() -> CloudWorkerArtifactPolicy {
        CloudWorkerArtifactPolicy {
            allowed_https_origins: Vec::new(),
            max_bytes: 1024,
        }
    }

    fn test_state_path(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "cloud-worker-{label}-{}-{nonce}.json",
            std::process::id()
        ))
    }

    #[test]
    fn poisoned_owner_is_projected_as_unavailable_without_public_owner_access() {
        let state_path = std::env::temp_dir().join("cloud-worker-poisoned-extension-state.json");
        let extension = start_cloud_worker_provisioning_extension(state_path).unwrap();
        let owner = std::sync::Arc::clone(&extension.owner);
        let join = std::thread::spawn(move || {
            let _guard = owner
                .lock()
                .expect("owner lock should be healthy before poisoning");
            panic!("poison cloud-worker owner for the contract");
        })
        .join();
        assert!(join.is_err());
        assert_eq!(extension.status(), CloudWorkerOwnerStatus::Unavailable);
    }

    #[test]
    fn doctor_inspection_is_owner_bound_but_mutation_stays_closed() {
        let state_path = std::env::temp_dir().join("cloud-worker-doctor-inspection-state.json");
        let extension = start_cloud_worker_provisioning_extension(state_path).unwrap();
        let request = CloudWorkerDoctorRequest {
            scope: CloudWorkerLeaseScope::new("account", "device").unwrap(),
            action: CloudWorkerDoctorAction::Inspect,
            acknowledged_mutation: false,
            replay_key: ReplayKey("doctor-1".into()),
        };
        assert_eq!(
            extension.inspect_doctor(request).unwrap().status,
            CloudWorkerDoctorStatus::InspectionOnly
        );

        let request = CloudWorkerDoctorRequest {
            scope: CloudWorkerLeaseScope::new("account", "device").unwrap(),
            action: CloudWorkerDoctorAction::ReconcileLease,
            acknowledged_mutation: true,
            replay_key: ReplayKey("doctor-2".into()),
        };
        assert_eq!(
            extension.inspect_doctor(request),
            Err(CloudWorkerDoctorError::MutationNotWired)
        );
    }

    #[test]
    fn verified_artifact_bytes_are_persisted_as_one_owner_mutation() {
        let state_path = test_state_path("verified-artifact");
        let extension = enabled_extension(state_path.clone());
        let source = CloudWorkerArtifactSource::ManagedLocal {
            digest: CloudWorkerArtifactDigest::new(
                "87eba76e7f3164534045ba922e7770fb58bbd14ad732bbf5ba6f11cc56989e6e",
            )
            .unwrap(),
        };

        let verified = extension
            .verify_and_register_artifact(
                CloudWorkerArtifactKind::WorkerBinary,
                &artifact_policy(),
                &source,
                b"worker",
            )
            .unwrap();

        assert_eq!(verified.size_bytes, 6);
        assert_eq!(extension.snapshot().unwrap().artifacts.len(), 1);
        assert!(state_path.is_file());
    }

    #[test]
    fn digest_mismatch_does_not_persist_an_artifact_admission() {
        let state_path = test_state_path("rejected-artifact");
        let extension = enabled_extension(state_path.clone());
        let source = CloudWorkerArtifactSource::ManagedLocal {
            digest: CloudWorkerArtifactDigest::new("a".repeat(64)).unwrap(),
        };

        let result = extension.verify_and_register_artifact(
            CloudWorkerArtifactKind::WorkerBinary,
            &artifact_policy(),
            &source,
            b"worker",
        );

        assert!(matches!(
            result,
            Err(CloudWorkerMutationError::Artifact(
                CloudWorkerArtifactAdmissionError::DigestMismatch { .. }
            ))
        ));
        assert!(extension.snapshot().unwrap().artifacts.is_empty());
        assert!(!state_path.exists());
    }

    #[test]
    fn bounded_runtime_control_result_settles_through_the_extension_owner() {
        let state_path = test_state_path("bounded-runtime-control");
        let extension = enabled_extension(state_path.clone());
        let lease_id = LeaseId("bounded-lease".into());
        let runtime = CloudWorkerRuntimeIdentity::new("provider", "profile", "runtime").unwrap();
        extension
            .admit_lease(
                CloudWorkerLeaseScope::new("account", "device").unwrap(),
                lease_id.clone(),
                runtime,
            )
            .unwrap();
        let request = CloudWorkerRuntimeControlRequest {
            action: CloudWorkerRuntimeControlAction::Inspect,
            replay_key: ReplayKey("inspect-bounded".into()),
            budget: CloudWorkerRuntimeControlBudget {
                timeout_ms: 30_000,
                max_output_bytes: 64 * 1024,
            },
        };
        let mutation = extension
            .admit_bounded_runtime_control(&lease_id, request.clone())
            .unwrap();
        let result = CloudWorkerRuntimeControlResult {
            action: request.action,
            mutation: mutation.clone(),
            outcome: CloudWorkerMutationOutcome::Completed,
            elapsed_ms: 100,
            diagnostic: Some(super::super::CloudWorkerDiagnostic::redacted(
                "inspect",
                "worker inspected",
            )),
        };
        let settled = extension
            .settle_bounded_runtime_control(&request, &mutation, &result)
            .unwrap();
        assert_eq!(
            settled.lifecycle,
            super::super::CloudWorkerLifecycle::Enrolled
        );
        assert!(state_path.is_file());
    }

    #[test]
    fn warm_image_inspection_rechecks_the_canonical_owner_record() {
        let state_path = test_state_path("warm-image-inspection");
        let extension = enabled_extension(state_path);
        extension
            .register_warm_image(CloudWorkerWarmImage::new("image-inspect").unwrap())
            .unwrap();
        let image = extension.inspect_warm_image("image-inspect").unwrap();
        assert_eq!(image.image_id, "image-inspect");
        assert_eq!(
            image.generation,
            super::super::CloudWorkerWarmImageGeneration(0)
        );
        assert_eq!(
            extension.inspect_warm_image("missing-image"),
            Err(super::super::CloudWorkerOwnerError::UnknownWarmImage)
        );
    }

    #[test]
    fn publication_settles_the_owner_only_after_the_file_is_published() {
        let state_path = test_state_path("published-artifact");
        let extension = enabled_extension(state_path.clone());
        let source = CloudWorkerArtifactSource::ManagedLocal {
            digest: CloudWorkerArtifactDigest::new(
                "87eba76e7f3164534045ba922e7770fb58bbd14ad732bbf5ba6f11cc56989e6e",
            )
            .unwrap(),
        };

        let published = extension
            .publish_verified_artifact(
                CloudWorkerArtifactKind::WorkerBinary,
                &artifact_policy(),
                &source,
                b"worker",
            )
            .unwrap();

        assert!(published.path.is_file());
        assert_eq!(
            extension
                .snapshot()
                .unwrap()
                .artifacts
                .iter()
                .find(|artifact| super::source_digest(&artifact.source) == published.proof.digest)
                .unwrap()
                .state,
            super::super::CloudWorkerArtifactAdmissionState::Verified
        );
        assert_eq!(
            extension
                .resolve_verified_artifact(&published.proof.digest, &artifact_policy())
                .unwrap(),
            published
        );
        let _ = std::fs::remove_file(&published.path);
        assert!(matches!(
            extension.resolve_verified_artifact(&published.proof.digest, &artifact_policy()),
            Err(CloudWorkerMutationError::ArtifactStore(
                super::super::CloudWorkerArtifactStoreError::MissingArtifact
            ))
        ));
        let _ = std::fs::remove_file(state_path);
    }

    #[test]
    fn known_publication_failure_settles_the_pending_admission_as_damaged() {
        let state_path = test_state_path("damaged-artifact");
        let extension = enabled_extension(state_path.clone());
        let source = CloudWorkerArtifactSource::ManagedLocal {
            digest: CloudWorkerArtifactDigest::new("a".repeat(64)).unwrap(),
        };

        let result = extension.publish_verified_artifact(
            CloudWorkerArtifactKind::WorkerBinary,
            &artifact_policy(),
            &source,
            b"worker",
        );

        assert!(matches!(
            result,
            Err(CloudWorkerArtifactPublicationError::Publication(_))
        ));
        assert_eq!(
            extension
                .snapshot()
                .unwrap()
                .artifacts
                .iter()
                .find(|artifact| super::source_digest(&artifact.source).0 == "a".repeat(64))
                .unwrap()
                .state,
            super::super::CloudWorkerArtifactAdmissionState::Damaged
        );
        let _ = std::fs::remove_file(state_path);
    }
}
