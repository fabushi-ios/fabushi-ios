//! Typed boundary for native cloud-worker provisioning.
//!
//! This module deliberately contains no provider, process, device, or UI side
//! effects. It is the state/identity contract that later slices may consume.
//! Fabushi account/device identity, Computer authorization, Sand state and
//! Runner composition remain owned by their existing modules.

pub mod extension;

mod auxiliary;
mod lease;
mod artifact_admission;
mod artifact_store;
mod doctor;
mod node_bootstrap;
mod owner;
mod platform_bridge;
mod process_identity;
mod provider_catalog;
mod runtime_control;
mod sandbox;
mod store;
mod warm_image;

pub use artifact_admission::{
    CloudWorkerArtifactAdmission, CloudWorkerArtifactAdmissionError,
    CloudWorkerArtifactAdmissionState, CloudWorkerArtifactDigest,
    CloudWorkerArtifactKind, CloudWorkerArtifactPolicy, CloudWorkerArtifactSource,
    CloudWorkerVerifiedArtifact, verify_artifact_bytes,
};
pub use artifact_store::{
    CloudWorkerArtifactPublisher, CloudWorkerArtifactStoreError,
    CloudWorkerPublishedArtifact,
};
pub use auxiliary::{
    validate_worker_wallpaper_png, CloudWorkerAuxiliaryError, CloudWorkerEnvironmentProfile,
    CloudWorkerInstallMode, CloudWorkerProfileSuggestion, CloudWorkerWarmImagePolicy,
};
pub use doctor::{
    CloudWorkerDoctorAction, CloudWorkerDoctorError, CloudWorkerDoctorRequest,
    CloudWorkerDoctorResult, CloudWorkerDoctorStatus,
};
pub use node_bootstrap::{
    CloudWorkerBootstrapPhase, CloudWorkerBootstrapRecord, CloudWorkerBootstrapRequest,
    CloudWorkerBootstrapRetryClass, CloudWorkerDiagnostic, CloudWorkerNodeBootstrapError,
    CloudWorkerSecretRef,
};
pub use owner::{
    CloudWorkerMigrationOutcome, CloudWorkerMigrationReceipt, CloudWorkerOwnerError,
    CloudWorkerOwnerStatus, CloudWorkerPersistentSnapshot,
    CloudWorkerProvisioningConfig, CloudWorkerProvisioningOwner,
};
pub use platform_bridge::{
    CloudWorkerDesktopSession, CloudWorkerPlatform, CloudWorkerPlatformBridge,
    CloudWorkerPlatformBridgeError,
};
pub use process_identity::{
    CloudWorkerProcessIdentityError, CloudWorkerProcessReceipt,
};
pub use lease::{
    CloudWorkerLease, CloudWorkerLeaseError, CloudWorkerLeaseMutation,
    CloudWorkerLeaseScope, CloudWorkerLeasedCapability, CloudWorkerLifecycle,
    CloudWorkerMutationOutcome, CloudWorkerRuntimeIdentity, LeaseGeneration,
    LeaseId, OwnerEpoch, ReplayKey,
};
pub use warm_image::{
    CloudWorkerWarmImage, CloudWorkerWarmImageError, CloudWorkerWarmImageGeneration,
    CloudWorkerWarmImageOperation, CloudWorkerWarmImageState,
};
pub use sandbox::{
    CloudWorkerSandboxCapability, CloudWorkerSandboxError, CloudWorkerSandboxOperation,
    CloudWorkerSandboxRequest,
};
pub use store::{
    CloudWorkerProvisioningStore, CloudWorkerStoreError, CLOUD_WORKER_MAX_STATE_BYTES,
};
pub use provider_catalog::{
    CloudWorkerBinaryIdentity, CloudWorkerCatalog, CloudWorkerCatalogCache,
    CloudWorkerCatalogError, CloudWorkerCatalogParser, CloudWorkerMachine,
    CloudWorkerMachineRequirement, CloudWorkerOs, CloudWorkerProvider,
    CloudWorkerProfile, CloudWorkerResourceLimits,
};
pub use runtime_control::{
    CloudWorkerRuntimeControlAction, CloudWorkerRuntimeControlBudget,
    CloudWorkerRuntimeControlError, CloudWorkerRuntimeControlRequest,
    CloudWorkerRuntimeControlResult,
};
