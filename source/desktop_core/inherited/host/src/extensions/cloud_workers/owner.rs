use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{
    CloudWorkerArtifactAdmission, CloudWorkerArtifactAdmissionError,
    CloudWorkerArtifactAdmissionState, CloudWorkerArtifactPolicy, CloudWorkerArtifactDigest,
    CloudWorkerVerifiedArtifact,
    CloudWorkerAuxiliaryError, CloudWorkerProfileSuggestion,
    CloudWorkerBinaryIdentity, CloudWorkerCatalog, CloudWorkerCatalogCache,
    CloudWorkerCatalogError, CloudWorkerMachine, CloudWorkerMachineRequirement,
    CloudWorkerBootstrapPhase, CloudWorkerBootstrapRecord, CloudWorkerBootstrapRetryClass,
    CloudWorkerDiagnostic, CloudWorkerNodeBootstrapError,
    CloudWorkerProcessReceipt,
    CloudWorkerLease, CloudWorkerLeaseError, CloudWorkerLeaseMutation, CloudWorkerLeaseScope,
    CloudWorkerLeasedCapability, CloudWorkerLifecycle, CloudWorkerMutationOutcome,
    CloudWorkerRuntimeIdentity,
    CloudWorkerWarmImage, CloudWorkerWarmImageError, CloudWorkerWarmImageGeneration,
    CloudWorkerWarmImageOperation, LeaseId,
    CloudWorkerRuntimeControlAction, CloudWorkerRuntimeControlError,
};

pub const CLOUD_WORKER_STATE_SCHEMA_VERSION: u32 = 1;
const MAX_MIGRATION_RECEIPTS: usize = 256;
const MAX_MIGRATION_OPERATION_CHARS: usize = 256;
const MAX_MIGRATION_BACKUP_PATH_CHARS: usize = 4096;
const MAX_MIGRATION_DETAIL_CHARS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloudWorkerProvisioningConfig {
    pub enabled: bool,
    pub max_leases: usize,
    pub max_warm_images: usize,
    pub max_artifacts: usize,
}

impl Default for CloudWorkerProvisioningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_leases: 64,
            max_warm_images: 128,
            max_artifacts: 128,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudWorkerOwnerStatus {
    Disabled,
    Enabled,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerMigrationOutcome {
    Previewed,
    Converted,
    RolledBack,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerMigrationReceipt {
    pub operation_id: String,
    pub from_schema_version: u32,
    pub to_schema_version: u32,
    pub backup_path: String,
    pub outcome: CloudWorkerMigrationOutcome,
    pub detail: String,
}

impl CloudWorkerMigrationReceipt {
    fn validate(&self) -> Result<(), CloudWorkerOwnerError> {
        if !valid_text(&self.operation_id, MAX_MIGRATION_OPERATION_CHARS)
            || !valid_text(&self.backup_path, MAX_MIGRATION_BACKUP_PATH_CHARS)
            || !valid_text(&self.detail, MAX_MIGRATION_DETAIL_CHARS)
        {
            return Err(CloudWorkerOwnerError::InvalidMigrationReceipt);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct CloudWorkerProvisioningOwner {
    config: CloudWorkerProvisioningConfig,
    leases: HashMap<LeaseId, CloudWorkerLease>,
    bootstrap_records: HashMap<LeaseId, CloudWorkerBootstrapRecord>,
    artifacts: HashMap<String, CloudWorkerArtifactAdmission>,
    warm_images: HashMap<String, CloudWorkerWarmImage>,
    migration_receipts: Vec<CloudWorkerMigrationReceipt>,
    catalog_cache: CloudWorkerCatalogCache,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerPersistentSnapshot {
    pub schema_version: u32,
    pub leases: Vec<CloudWorkerLease>,
    #[serde(default)]
    pub bootstrap_records: Vec<CloudWorkerBootstrapRecord>,
    #[serde(default)]
    pub artifacts: Vec<CloudWorkerArtifactAdmission>,
    pub warm_images: Vec<CloudWorkerWarmImage>,
    #[serde(default)]
    pub migration_receipts: Vec<CloudWorkerMigrationReceipt>,
}

impl CloudWorkerProvisioningOwner {
    pub fn new(config: CloudWorkerProvisioningConfig) -> Result<Self, CloudWorkerOwnerError> {
        if config.max_leases == 0 {
            return Err(CloudWorkerOwnerError::InvalidLeaseLimit);
        }
        if config.max_warm_images == 0 {
            return Err(CloudWorkerOwnerError::InvalidWarmImageLimit);
        }
        if config.max_artifacts == 0 {
            return Err(CloudWorkerOwnerError::InvalidArtifactLimit);
        }
        Ok(Self {
            config,
            leases: HashMap::new(),
            bootstrap_records: HashMap::new(),
            artifacts: HashMap::new(),
            warm_images: HashMap::new(),
            migration_receipts: Vec::new(),
            catalog_cache: CloudWorkerCatalogCache::default(),
        })
    }

    pub fn status(&self) -> CloudWorkerOwnerStatus {
        if self.config.enabled {
            CloudWorkerOwnerStatus::Enabled
        } else {
            CloudWorkerOwnerStatus::Disabled
        }
    }

    /// Resolves provider capability data through the canonical owner while
    /// keeping the cache ephemeral and keyed by the exact binary identity.
    /// The loader remains an explicit provider/process boundary; this method
    /// does not discover, authorize, launch or mutate a worker.
    pub fn resolve_catalog<F>(
        &mut self,
        identity: CloudWorkerBinaryIdentity,
        load: F,
    ) -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>
    where
        F: FnOnce() -> Result<CloudWorkerCatalog, CloudWorkerCatalogError>,
    {
        self.catalog_cache.resolve(identity, load)
    }

    /// Produces the typed setup suggestion only while this owner is enabled.
    /// The suggestion is not provider discovery, authorization, readiness or
    /// a launch request.
    pub fn suggest_profile(
        &self,
        provider_display_id: impl Into<String>,
        machine_class: impl Into<String>,
    ) -> Result<CloudWorkerProfileSuggestion, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        CloudWorkerProfileSuggestion::new(provider_display_id, machine_class)
            .map_err(CloudWorkerOwnerError::from)
    }

    pub fn select_cached_machine(
        &self,
        identity: &CloudWorkerBinaryIdentity,
        provider_id: &str,
        profile_id: &str,
        requirement: &CloudWorkerMachineRequirement,
    ) -> Result<CloudWorkerMachine, CloudWorkerCatalogError> {
        let catalog = self
            .catalog_cache
            .get(identity)
            .ok_or(CloudWorkerCatalogError::UnknownBinaryIdentity)?;
        catalog.select_machine(provider_id, profile_id, requirement)
    }

    pub fn snapshot(&self) -> CloudWorkerPersistentSnapshot {
        let mut leases = self.leases.values().cloned().collect::<Vec<_>>();
        leases.sort_by(|left, right| left.lease_id.0.cmp(&right.lease_id.0));
        let mut warm_images = self.warm_images.values().cloned().collect::<Vec<_>>();
        warm_images.sort_by(|left, right| left.image_id.cmp(&right.image_id));
        let mut bootstrap_records = self.bootstrap_records.values().cloned().collect::<Vec<_>>();
        bootstrap_records.sort_by(|left, right| {
            left.request.lease_id.0.cmp(&right.request.lease_id.0)
        });
        let mut artifacts = self.artifacts.values().cloned().collect::<Vec<_>>();
        artifacts.sort_by(|left, right| artifact_key(left).cmp(&artifact_key(right)));
        CloudWorkerPersistentSnapshot {
            schema_version: CLOUD_WORKER_STATE_SCHEMA_VERSION,
            leases,
            bootstrap_records,
            artifacts,
            warm_images,
            migration_receipts: self.migration_receipts.clone(),
        }
    }

    pub fn from_snapshot(
        config: CloudWorkerProvisioningConfig,
        snapshot: CloudWorkerPersistentSnapshot,
    ) -> Result<Self, CloudWorkerOwnerError> {
        if snapshot.schema_version != CLOUD_WORKER_STATE_SCHEMA_VERSION {
            return Err(CloudWorkerOwnerError::UnsupportedSnapshotVersion(
                snapshot.schema_version,
            ));
        }
        let mut owner = Self::new(config)?;
        if snapshot.leases.len() > owner.config.max_leases {
            return Err(CloudWorkerOwnerError::LeaseLimitReached);
        }
        if snapshot.warm_images.len() > owner.config.max_warm_images {
            return Err(CloudWorkerOwnerError::WarmImageLimitReached);
        }
        for lease in snapshot.leases {
            lease.validate_persisted()?;
            if owner.leases.insert(lease.lease_id.clone(), lease).is_some() {
                return Err(CloudWorkerOwnerError::InvalidSnapshot);
            }
        }
        if snapshot.bootstrap_records.len() > owner.config.max_leases {
            return Err(CloudWorkerOwnerError::BootstrapRecordLimitReached);
        }
        for record in snapshot.bootstrap_records {
            record.validate_persisted()?;
            let lease = owner
                .leases
                .get(&record.request.lease_id)
                .ok_or(CloudWorkerOwnerError::UnknownLease)?;
            if !bootstrap_matches_lease(&record, lease) {
                return Err(CloudWorkerOwnerError::BootstrapLeaseMismatch);
            }
            if owner
                .bootstrap_records
                .insert(record.request.lease_id.clone(), record)
                .is_some()
            {
                return Err(CloudWorkerOwnerError::InvalidSnapshot);
            }
        }
        if snapshot.artifacts.len() > owner.config.max_artifacts {
            return Err(CloudWorkerOwnerError::ArtifactLimitReached);
        }
        for artifact in snapshot.artifacts {
            artifact.validate_persisted()?;
            let key = artifact_key(&artifact);
            if owner.artifacts.insert(key, artifact).is_some() {
                return Err(CloudWorkerOwnerError::InvalidSnapshot);
            }
        }
        for image in snapshot.warm_images {
            image.validate_persisted()?;
            if owner
                .warm_images
                .insert(image.image_id.clone(), image)
                .is_some()
            {
                return Err(CloudWorkerOwnerError::InvalidSnapshot);
            }
        }
        if snapshot.migration_receipts.len() > MAX_MIGRATION_RECEIPTS {
            return Err(CloudWorkerOwnerError::MigrationReceiptLimitReached);
        }
        let mut operation_ids = std::collections::HashSet::new();
        for receipt in snapshot.migration_receipts {
            receipt.validate()?;
            if !operation_ids.insert(receipt.operation_id.clone()) {
                return Err(CloudWorkerOwnerError::InvalidSnapshot);
            }
            owner.migration_receipts.push(receipt);
        }
        Ok(owner)
    }

    pub fn restore_snapshot(
        &mut self,
        snapshot: CloudWorkerPersistentSnapshot,
    ) -> Result<(), CloudWorkerOwnerError> {
        let restored = Self::from_snapshot(self.config, snapshot)?;
        *self = restored;
        Ok(())
    }

    pub fn record_migration_receipt(
        &mut self,
        receipt: CloudWorkerMigrationReceipt,
    ) -> Result<(), CloudWorkerOwnerError> {
        receipt.validate()?;
        if self.migration_receipts.len() >= MAX_MIGRATION_RECEIPTS {
            return Err(CloudWorkerOwnerError::MigrationReceiptLimitReached);
        }
        if self
            .migration_receipts
            .iter()
            .any(|known| known.operation_id == receipt.operation_id)
        {
            return Err(CloudWorkerOwnerError::DuplicateMigrationReceipt);
        }
        self.migration_receipts.push(receipt);
        Ok(())
    }

    pub fn admit(
        &mut self,
        scope: CloudWorkerLeaseScope,
        lease_id: LeaseId,
        runtime: CloudWorkerRuntimeIdentity,
    ) -> Result<CloudWorkerLease, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        if self.leases.len() >= self.config.max_leases {
            return Err(CloudWorkerOwnerError::LeaseLimitReached);
        }
        if lease_id.0.trim().is_empty() {
            return Err(CloudWorkerOwnerError::InvalidLeaseId);
        }
        if self.leases.contains_key(&lease_id) {
            return Err(CloudWorkerOwnerError::DuplicateLease);
        }
        let lease = CloudWorkerLease {
            scope,
            lease_id: lease_id.clone(),
            owner_epoch: super::OwnerEpoch(0),
            generation: super::LeaseGeneration(0),
            runtime,
            lifecycle: CloudWorkerLifecycle::Admitted,
            last_outcome: CloudWorkerMutationOutcome::Completed,
            replay_history: Vec::new(),
        };
        lease.validate_persisted()?;
        self.leases.insert(lease_id, lease.clone());
        Ok(lease)
    }

    pub fn lease(&self, lease_id: &LeaseId) -> Result<&CloudWorkerLease, CloudWorkerOwnerError> {
        self.leases
            .get(lease_id)
            .ok_or(CloudWorkerOwnerError::UnknownLease)
    }

    /// Returns an ephemeral consumer capability only while the lease is
    /// enrolled and its last mutation completed successfully.
    pub fn leased_capability(
        &self,
        lease_id: &LeaseId,
    ) -> Result<CloudWorkerLeasedCapability, CloudWorkerOwnerError> {
        self.lease(lease_id)?
            .capability()
            .map_err(CloudWorkerOwnerError::from)
    }

    /// Admits a lease-fenced runtime-control mutation without performing it.
    /// The future provider producer must settle the returned identity through
    /// `settle_lease`; this owner remains the only state writer.
    pub fn admit_runtime_control(
        &self,
        lease_id: &LeaseId,
        action: CloudWorkerRuntimeControlAction,
        replay_key: super::ReplayKey,
    ) -> Result<CloudWorkerLeaseMutation, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        action
            .admit(self.lease(lease_id)?, replay_key)
            .map_err(CloudWorkerOwnerError::from)
    }

    pub fn register_bootstrap(
        &mut self,
        record: CloudWorkerBootstrapRecord,
    ) -> Result<(), CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        if self.bootstrap_records.len() >= self.config.max_leases {
            return Err(CloudWorkerOwnerError::BootstrapRecordLimitReached);
        }
        record.validate_persisted()?;
        let lease = self.lease(&record.request.lease_id)?;
        if !bootstrap_matches_lease(&record, lease) {
            return Err(CloudWorkerOwnerError::BootstrapLeaseMismatch);
        }
        if self
            .bootstrap_records
            .insert(record.request.lease_id.clone(), record)
            .is_some()
        {
            return Err(CloudWorkerOwnerError::DuplicateBootstrapRecord);
        }
        Ok(())
    }

    pub fn register_artifact(
        &mut self,
        artifact: CloudWorkerArtifactAdmission,
        policy: &CloudWorkerArtifactPolicy,
    ) -> Result<(), CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        if self.artifacts.len() >= self.config.max_artifacts {
            return Err(CloudWorkerOwnerError::ArtifactLimitReached);
        }
        artifact.validate(policy)?;
        let key = artifact_key(&artifact);
        if self.artifacts.insert(key, artifact).is_some() {
            return Err(CloudWorkerOwnerError::DuplicateArtifact);
        }
        Ok(())
    }

    pub fn settle_artifact(
        &mut self,
        digest: &CloudWorkerArtifactDigest,
        next: CloudWorkerArtifactAdmissionState,
    ) -> Result<CloudWorkerArtifactAdmission, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        let artifact = self
            .artifacts
            .get_mut(&digest.0)
            .ok_or(CloudWorkerOwnerError::UnknownArtifact)?;
        artifact.settle(next)?;
        Ok(artifact.clone())
    }

    pub fn verified_artifact(
        &self,
        artifact: &CloudWorkerVerifiedArtifact,
    ) -> Result<CloudWorkerVerifiedArtifact, CloudWorkerOwnerError> {
        artifact.validate()?;
        let admission = self
            .artifacts
            .get(&artifact.digest.0)
            .ok_or(CloudWorkerOwnerError::UnknownArtifact)?;
        if admission.state != CloudWorkerArtifactAdmissionState::Verified
            || artifact.size_bytes != admission.size_bytes
        {
            return Err(CloudWorkerOwnerError::ArtifactNotVerified);
        }
        Ok(artifact.clone())
    }

    pub fn verified_artifact_admission(
        &self,
        digest: &CloudWorkerArtifactDigest,
    ) -> Result<CloudWorkerArtifactAdmission, CloudWorkerOwnerError> {
        let admission = self
            .artifacts
            .get(&digest.0)
            .ok_or(CloudWorkerOwnerError::UnknownArtifact)?;
        admission.validate_persisted()?;
        if admission.state != CloudWorkerArtifactAdmissionState::Verified {
            return Err(CloudWorkerOwnerError::ArtifactNotVerified);
        }
        Ok(admission.clone())
    }

    pub fn settle_bootstrap(
        &mut self,
        mutation: &CloudWorkerLeaseMutation,
        phase: CloudWorkerBootstrapPhase,
        retry_class: CloudWorkerBootstrapRetryClass,
        diagnostic: Option<CloudWorkerDiagnostic>,
    ) -> Result<CloudWorkerBootstrapRecord, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        let lease = self.lease(&mutation.lease_id)?;
        lease.validate(mutation)?;
        let record = self
            .bootstrap_records
            .get_mut(&mutation.lease_id)
            .ok_or(CloudWorkerOwnerError::UnknownBootstrapRecord)?;
        if !bootstrap_matches_mutation(record, mutation) {
            return Err(CloudWorkerOwnerError::BootstrapLeaseMismatch);
        }
        record.settle(phase, retry_class, diagnostic)?;
        Ok(record.clone())
    }

    pub fn attach_bootstrap_process_receipt(
        &mut self,
        mutation: &CloudWorkerLeaseMutation,
        receipt: CloudWorkerProcessReceipt,
    ) -> Result<CloudWorkerBootstrapRecord, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        let lease = self.lease(&mutation.lease_id)?;
        lease.validate(mutation)?;
        let record = self
            .bootstrap_records
            .get_mut(&mutation.lease_id)
            .ok_or(CloudWorkerOwnerError::UnknownBootstrapRecord)?;
        if !bootstrap_matches_mutation(record, mutation) {
            return Err(CloudWorkerOwnerError::BootstrapLeaseMismatch);
        }
        record.attach_process_receipt(receipt)?;
        Ok(record.clone())
    }

    pub fn settle_lease(
        &mut self,
        mutation: &CloudWorkerLeaseMutation,
        next: CloudWorkerLifecycle,
        outcome: CloudWorkerMutationOutcome,
    ) -> Result<CloudWorkerLease, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        let lease = self
            .leases
            .get_mut(&mutation.lease_id)
            .ok_or(CloudWorkerOwnerError::UnknownLease)?;
        lease.validate(mutation)?;
        lease.apply_outcome(next, outcome, mutation.replay_key.clone())?;
        Ok(lease.clone())
    }

    /// Settles a runtime-control mutation using the action's own lifecycle
    /// contract. Producers cannot substitute an unrelated next state; an
    /// ambiguous result is always retained as explicit recovery state.
    pub fn settle_runtime_control(
        &mut self,
        mutation: &CloudWorkerLeaseMutation,
        action: CloudWorkerRuntimeControlAction,
        outcome: CloudWorkerMutationOutcome,
    ) -> Result<CloudWorkerLease, CloudWorkerOwnerError> {
        let current = self.lease(&mutation.lease_id)?.lifecycle;
        let next = match outcome {
            CloudWorkerMutationOutcome::Completed => action.completed_lifecycle(current)?,
            CloudWorkerMutationOutcome::Submitted
            | CloudWorkerMutationOutcome::Failed
            | CloudWorkerMutationOutcome::Cancelled => current,
            CloudWorkerMutationOutcome::OutcomeUnknown => CloudWorkerLifecycle::OutcomeUnknown,
        };
        self.settle_lease(mutation, next, outcome)
    }

    pub fn register_warm_image(
        &mut self,
        image: CloudWorkerWarmImage,
    ) -> Result<(), CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        if self.warm_images.len() >= self.config.max_warm_images {
            return Err(CloudWorkerOwnerError::WarmImageLimitReached);
        }
        image.validate_persisted()?;
        if self.warm_images.contains_key(&image.image_id) {
            return Err(CloudWorkerOwnerError::DuplicateWarmImage);
        }
        self.warm_images.insert(image.image_id.clone(), image);
        Ok(())
    }

    pub fn warm_image(
        &self,
        image_id: &str,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerOwnerError> {
        let image = self
            .warm_images
            .get(image_id)
            .ok_or(CloudWorkerOwnerError::UnknownWarmImage)?;
        image.validate_persisted()?;
        Ok(image.clone())
    }

    /// Returns only warm-image rows that require an explicit recovery
    /// decision. The projection is deterministic so callers can present or
    /// reconcile the same rows without treating a broad inventory as a
    /// recovery instruction.
    pub fn warm_image_recovery_candidates(&self) -> Vec<CloudWorkerWarmImage> {
        let mut candidates = self
            .warm_images
            .values()
            .filter(|image| {
                image.cleanup_debt
                    || matches!(
                        image.state,
                        super::CloudWorkerWarmImageState::CaptureUncertain
                            | super::CloudWorkerWarmImageState::Retiring
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.image_id.cmp(&right.image_id));
        candidates
    }

    /// Selects one exact recovery row by image ID. A healthy available image
    /// is not a recovery target, even when the caller knows its ID.
    pub fn warm_image_recovery_candidate(
        &self,
        image_id: &str,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerOwnerError> {
        let image = self.warm_image(image_id)?;
        if image.cleanup_debt
            || matches!(
                image.state,
                super::CloudWorkerWarmImageState::CaptureUncertain
                    | super::CloudWorkerWarmImageState::Retiring
            )
        {
            return Ok(image);
        }
        Err(CloudWorkerOwnerError::WarmImageNotRecoverable)
    }

    pub fn apply_warm_image(
        &mut self,
        image_id: &str,
        expected_generation: CloudWorkerWarmImageGeneration,
        operation: CloudWorkerWarmImageOperation,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        let image = self
            .warm_images
            .get_mut(image_id)
            .ok_or(CloudWorkerOwnerError::UnknownWarmImage)?;
        image.compare_and_apply(expected_generation, operation)?;
        Ok(image.clone())
    }

    pub fn mark_warm_image_capture_uncertain(
        &mut self,
        image_id: &str,
        expected_generation: CloudWorkerWarmImageGeneration,
    ) -> Result<CloudWorkerWarmImage, CloudWorkerOwnerError> {
        self.ensure_enabled()?;
        let image = self
            .warm_images
            .get_mut(image_id)
            .ok_or(CloudWorkerOwnerError::UnknownWarmImage)?;
        image.mark_capture_uncertain(expected_generation)?;
        Ok(image.clone())
    }

    fn ensure_enabled(&self) -> Result<(), CloudWorkerOwnerError> {
        if !self.config.enabled {
            return Err(CloudWorkerOwnerError::Disabled);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CloudWorkerOwnerError {
    #[error("cloud-worker provisioning owner is disabled")]
    Disabled,
    #[error("cloud-worker owner is unavailable")]
    Unavailable,
    #[error("cloud-worker leased capability is stale")]
    StaleCapability,
    #[error("maximum lease count must be greater than zero")]
    InvalidLeaseLimit,
    #[error("maximum warm-image count must be greater than zero")]
    InvalidWarmImageLimit,
    #[error("maximum artifact count must be greater than zero")]
    InvalidArtifactLimit,
    #[error("lease id must be non-empty")]
    InvalidLeaseId,
    #[error("lease already exists")]
    DuplicateLease,
    #[error("lease limit reached")]
    LeaseLimitReached,
    #[error("warm-image limit reached")]
    WarmImageLimitReached,
    #[error("bootstrap record limit reached")]
    BootstrapRecordLimitReached,
    #[error("artifact limit reached")]
    ArtifactLimitReached,
    #[error("lease was not found")]
    UnknownLease,
    #[error("warm image already exists")]
    DuplicateWarmImage,
    #[error("warm image was not found")]
    UnknownWarmImage,
    #[error("warm image does not require recovery")]
    WarmImageNotRecoverable,
    #[error("bootstrap record already exists")]
    DuplicateBootstrapRecord,
    #[error("bootstrap record was not found")]
    UnknownBootstrapRecord,
    #[error("bootstrap record does not match its lease")]
    BootstrapLeaseMismatch,
    #[error("artifact already exists")]
    DuplicateArtifact,
    #[error("artifact was not found")]
    UnknownArtifact,
    #[error("artifact is not verified or does not match its admitted size")]
    ArtifactNotVerified,
    #[error("unsupported cloud-worker snapshot schema version: {0}")]
    UnsupportedSnapshotVersion(u32),
    #[error("cloud-worker snapshot contains invalid or duplicate records")]
    InvalidSnapshot,
    #[error("migration receipt operation id is empty or already recorded")]
    DuplicateMigrationReceipt,
    #[error("migration receipt fields are invalid")]
    InvalidMigrationReceipt,
    #[error("migration receipt limit reached")]
    MigrationReceiptLimitReached,
    #[error(transparent)]
    Lease(#[from] CloudWorkerLeaseError),
    #[error(transparent)]
    WarmImage(#[from] CloudWorkerWarmImageError),
    #[error(transparent)]
    Bootstrap(#[from] CloudWorkerNodeBootstrapError),
    #[error(transparent)]
    Artifact(#[from] CloudWorkerArtifactAdmissionError),
    #[error(transparent)]
    RuntimeControl(#[from] CloudWorkerRuntimeControlError),
    #[error(transparent)]
    Auxiliary(#[from] CloudWorkerAuxiliaryError),
}

fn artifact_key(artifact: &CloudWorkerArtifactAdmission) -> String {
    match &artifact.source {
        super::CloudWorkerArtifactSource::Https { digest, .. }
        | super::CloudWorkerArtifactSource::ManagedLocal { digest } => digest.0.clone(),
    }
}

fn bootstrap_matches_lease(
    record: &CloudWorkerBootstrapRecord,
    lease: &CloudWorkerLease,
) -> bool {
    record.request.lease_id == lease.lease_id
        && record.request.scope == lease.scope
        && record.request.owner_epoch == lease.owner_epoch
        && record.request.generation == lease.generation
        && record.request.runtime == lease.runtime
}

fn bootstrap_matches_mutation(
    record: &CloudWorkerBootstrapRecord,
    mutation: &CloudWorkerLeaseMutation,
) -> bool {
    record.request.lease_id == mutation.lease_id
        && record.request.owner_epoch == mutation.owner_epoch
        && record.request.generation == mutation.generation
        && record.request.runtime == mutation.runtime
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> CloudWorkerProvisioningOwner {
        CloudWorkerProvisioningOwner::new(CloudWorkerProvisioningConfig {
            enabled: true,
            max_leases: 1,
            max_warm_images: 1,
            max_artifacts: 1,
        })
        .unwrap()
    }

    fn runtime() -> CloudWorkerRuntimeIdentity {
        CloudWorkerRuntimeIdentity::new("provider", "profile", "runtime").unwrap()
    }

    #[test]
    fn disabled_owner_has_no_mutation_side_effect() {
        let mut owner = CloudWorkerProvisioningOwner::new(Default::default()).unwrap();
        assert_eq!(owner.status(), CloudWorkerOwnerStatus::Disabled);
        assert_eq!(
            owner.admit(
                CloudWorkerLeaseScope::new("account", "device").unwrap(),
                LeaseId("lease-1".into()),
                runtime(),
            ),
            Err(CloudWorkerOwnerError::Disabled)
        );
    }

    #[test]
    fn owner_is_the_single_in_memory_lease_writer() {
        let mut owner = owner();
        let lease = owner
            .admit(
                CloudWorkerLeaseScope::new("account", "device").unwrap(),
                LeaseId("lease-1".into()),
                runtime(),
            )
            .unwrap();
        let mutation = lease.mutation(
            super::super::OwnerEpoch(0),
            lease.generation,
            lease.runtime.clone(),
            super::super::ReplayKey("request-1".into()),
        );
        let settled = owner
            .settle_lease(
                &mutation,
                CloudWorkerLifecycle::Preparing,
                CloudWorkerMutationOutcome::Completed,
            )
            .unwrap();
        assert_eq!(settled.lifecycle, CloudWorkerLifecycle::Preparing);
    }

    #[test]
    fn owner_enforces_lease_capacity() {
        let mut owner = owner();
        let scope = CloudWorkerLeaseScope::new("account", "device").unwrap();
        owner
            .admit(scope.clone(), LeaseId("lease-1".into()), runtime())
            .unwrap();
        assert_eq!(
            owner.admit(scope, LeaseId("lease-2".into()), runtime()),
            Err(CloudWorkerOwnerError::LeaseLimitReached)
        );
    }

    #[test]
    fn owner_rejects_zero_warm_image_capacity() {
        assert_eq!(
            CloudWorkerProvisioningOwner::new(CloudWorkerProvisioningConfig {
                enabled: true,
                max_leases: 1,
                max_warm_images: 0,
                max_artifacts: 1,
            })
            .unwrap_err(),
            CloudWorkerOwnerError::InvalidWarmImageLimit
        );
    }

    #[test]
    fn profile_suggestion_is_owner_bound_and_disabled_by_default() {
        let disabled = CloudWorkerProvisioningOwner::new(Default::default()).unwrap();
        assert_eq!(
            disabled.suggest_profile("aws", "standard"),
            Err(CloudWorkerOwnerError::Disabled)
        );

        let enabled = owner();
        let suggestion = enabled.suggest_profile("aws", "standard").unwrap();
        assert_eq!(suggestion.provider_display_id, "aws");
        assert_eq!(suggestion.machine_class, "standard");
    }

    #[test]
    fn owner_caches_catalog_only_by_exact_binary_identity() {
        let mut owner = owner();
        let identity = CloudWorkerBinaryIdentity::new("sha256:provider").unwrap();
        let catalog = CloudWorkerCatalog {
            providers: vec![super::super::CloudWorkerProvider {
                id: "local".into(),
                profiles: vec![super::super::CloudWorkerProfile {
                    id: "default".into(),
                    machines: vec![super::super::CloudWorkerMachine {
                        os: super::super::CloudWorkerOs::Linux,
                        arch: "x86_64".into(),
                        resources: super::super::CloudWorkerResourceLimits {
                            cpu_cores: 2,
                            memory_bytes: 1024,
                        },
                    }],
                }],
            }],
        }
        .validate()
        .unwrap();
        let first = owner
            .resolve_catalog(identity.clone(), || Ok(catalog.clone()))
            .unwrap();
        let second = owner
            .resolve_catalog(identity, || {
                Err(super::super::CloudWorkerCatalogError::EmptyCatalog)
            })
            .unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn owner_rejects_invalid_direct_lease_records() {
        let mut owner = owner();
        let invalid_scope = CloudWorkerLeaseScope {
            account_scope: "account\nunsafe".into(),
            device_scope: "device".into(),
        };
        assert_eq!(
            owner.admit(
                invalid_scope,
                LeaseId("lease-unsafe".into()),
                runtime(),
            ),
            Err(CloudWorkerOwnerError::Lease(
                CloudWorkerLeaseError::InvalidPersistedRecord
            ))
        );
    }

    #[test]
    fn owner_can_restore_a_pre_mutation_snapshot() {
        let mut owner = owner();
        let before = owner.snapshot();
        owner
            .admit(
                CloudWorkerLeaseScope::new("account", "device").unwrap(),
                LeaseId("lease-1".into()),
                runtime(),
            )
            .unwrap();
        owner.restore_snapshot(before).unwrap();
        assert_eq!(
            owner.lease(&LeaseId("lease-1".into())),
            Err(CloudWorkerOwnerError::UnknownLease)
        );
    }

    #[test]
    fn runtime_control_completion_returns_read_capability_to_enrolled() {
        let mut owner = owner();
        let lease = owner
            .admit(
                CloudWorkerLeaseScope::new("account", "device").unwrap(),
                LeaseId("lease-1".into()),
                runtime(),
            )
            .unwrap();
        let mutation = owner
            .admit_runtime_control(
                &lease.lease_id,
                CloudWorkerRuntimeControlAction::Heartbeat,
                super::super::ReplayKey("heartbeat-1".into()),
            )
            .unwrap();
        let settled = owner
            .settle_runtime_control(
                &mutation,
                CloudWorkerRuntimeControlAction::Heartbeat,
                CloudWorkerMutationOutcome::Completed,
            )
            .unwrap();
        assert_eq!(settled.lifecycle, CloudWorkerLifecycle::Enrolled);
        assert_eq!(settled.generation, super::super::LeaseGeneration(1));
    }

    #[test]
    fn recovery_candidates_are_sorted_and_exclude_healthy_images() {
        let mut owner = CloudWorkerProvisioningOwner::new(CloudWorkerProvisioningConfig {
            enabled: true,
            max_leases: 1,
            max_warm_images: 3,
            max_artifacts: 1,
        })
        .unwrap();
        owner
            .register_warm_image(CloudWorkerWarmImage::new("healthy").unwrap())
            .unwrap();
        owner
            .register_warm_image(CloudWorkerWarmImage::new("zeta").unwrap())
            .unwrap();
        owner
            .register_warm_image(CloudWorkerWarmImage::new("alpha").unwrap())
            .unwrap();
        owner
            .mark_warm_image_capture_uncertain("zeta", CloudWorkerWarmImageGeneration(0))
            .unwrap();
        owner
            .mark_warm_image_capture_uncertain("alpha", CloudWorkerWarmImageGeneration(0))
            .unwrap();

        let candidates = owner.warm_image_recovery_candidates();
        assert_eq!(
            candidates
                .iter()
                .map(|image| image.image_id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );
        assert_eq!(
            owner.warm_image_recovery_candidate("healthy"),
            Err(CloudWorkerOwnerError::WarmImageNotRecoverable)
        );
    }
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(|character| character.is_control())
}
