use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    CloudWorkerLeaseScope, CloudWorkerPlatformBridge, CloudWorkerProcessReceipt,
    CloudWorkerRuntimeIdentity, CloudWorkerVerifiedArtifact, LeaseGeneration, LeaseId, OwnerEpoch,
    ReplayKey,
};

const MAX_DIAGNOSTIC_CHARS: usize = 2048;
const MAX_DIAGNOSTICS: usize = 32;
const MAX_SECRET_REF_CHARS: usize = 256;
const MAX_SECRET_REFS: usize = 32;
const MAX_REPLAY_KEY_CHARS: usize = 256;

/// An opaque handle resolved by Fabushi's existing secret/credential owner.
/// The value is never a secret payload and must not be replaced by one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerSecretRef(String);

impl CloudWorkerSecretRef {
    pub fn new(value: impl Into<String>) -> Result<Self, CloudWorkerNodeBootstrapError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_SECRET_REF_CHARS
            || value.chars().any(|character| character.is_control())
        {
            return Err(CloudWorkerNodeBootstrapError::InvalidSecretRef);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerBootstrapRetryClass {
    Never,
    SafeReplay,
    ReconcileFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerBootstrapPhase {
    Pending,
    Preparing,
    Enrolling,
    Ready,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerBootstrapRequest {
    pub lease_id: LeaseId,
    pub owner_epoch: OwnerEpoch,
    pub generation: LeaseGeneration,
    pub scope: CloudWorkerLeaseScope,
    pub runtime: CloudWorkerRuntimeIdentity,
    pub artifact: CloudWorkerVerifiedArtifact,
    pub platform: CloudWorkerPlatformBridge,
    pub secret_refs: Vec<CloudWorkerSecretRef>,
    pub replay_key: ReplayKey,
}

impl CloudWorkerBootstrapRequest {
    pub fn validate(&self) -> Result<(), CloudWorkerNodeBootstrapError> {
        if self.lease_id.0.trim().is_empty()
            || self
                .lease_id
                .0
                .chars()
                .any(|character| character.is_control())
        {
            return Err(CloudWorkerNodeBootstrapError::InvalidLeaseIdentity);
        }
        CloudWorkerLeaseScope::new(
            self.scope.account_scope.clone(),
            self.scope.device_scope.clone(),
        )
        .map_err(|_| CloudWorkerNodeBootstrapError::InvalidScope)?;
        CloudWorkerRuntimeIdentity::new(
            self.runtime.provider.clone(),
            self.runtime.profile.clone(),
            self.runtime.runtime_id.clone(),
        )
        .map_err(|_| CloudWorkerNodeBootstrapError::InvalidRuntimeIdentity)?;
        self.platform
            .validate()
            .map_err(|_| CloudWorkerNodeBootstrapError::InvalidPlatformBridge)?;
        self.artifact
            .validate()
            .map_err(|_| CloudWorkerNodeBootstrapError::InvalidArtifactDigest)?;
        if self.secret_refs.is_empty() {
            return Err(CloudWorkerNodeBootstrapError::MissingSecretRef);
        }
        if self.secret_refs.len() > MAX_SECRET_REFS {
            return Err(CloudWorkerNodeBootstrapError::SecretRefLimitReached);
        }
        for secret_ref in &self.secret_refs {
            CloudWorkerSecretRef::new(secret_ref.0.clone())
                .map_err(|_| CloudWorkerNodeBootstrapError::InvalidSecretRef)?;
        }
        if self.replay_key.0.trim().is_empty() {
            return Err(CloudWorkerNodeBootstrapError::MissingReplayKey);
        }
        if self.replay_key.0.len() > MAX_REPLAY_KEY_CHARS
            || self
                .replay_key
                .0
                .chars()
                .any(|character| character.is_control())
        {
            return Err(CloudWorkerNodeBootstrapError::InvalidReplayKey);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerDiagnostic {
    pub code: String,
    pub message: String,
    pub redacted: bool,
}

impl CloudWorkerDiagnostic {
    pub fn redacted(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = crate::error_diagnostics::sanitize_error_diagnostic(&code.into());
        let message = crate::error_diagnostics::sanitize_error_diagnostic(&message.into());
        Self {
            code,
            message,
            redacted: true,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), CloudWorkerNodeBootstrapError> {
        if self.code.trim().is_empty()
            || self.code.chars().count() > MAX_DIAGNOSTIC_CHARS
            || self.code.chars().any(|character| character.is_control())
            || self.message.chars().count() > MAX_DIAGNOSTIC_CHARS
            || self.message.chars().any(|character| character.is_control())
            || !self.redacted
            || crate::error_diagnostics::sanitize_error_diagnostic(&self.code) != self.code
            || crate::error_diagnostics::sanitize_error_diagnostic(&self.message) != self.message
        {
            return Err(CloudWorkerNodeBootstrapError::InvalidDiagnostic);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerBootstrapRecord {
    pub request: CloudWorkerBootstrapRequest,
    pub phase: CloudWorkerBootstrapPhase,
    pub retry_class: CloudWorkerBootstrapRetryClass,
    pub runtime_identity: String,
    pub diagnostics: Vec<CloudWorkerDiagnostic>,
    #[serde(default)]
    pub process_receipt: Option<CloudWorkerProcessReceipt>,
}

impl CloudWorkerBootstrapRecord {
    pub fn pending(
        request: CloudWorkerBootstrapRequest,
    ) -> Result<Self, CloudWorkerNodeBootstrapError> {
        request.validate()?;
        Ok(Self {
            runtime_identity: request.runtime.runtime_id.clone(),
            request,
            phase: CloudWorkerBootstrapPhase::Pending,
            retry_class: CloudWorkerBootstrapRetryClass::ReconcileFirst,
            diagnostics: Vec::new(),
            process_receipt: None,
        })
    }

    pub fn attach_process_receipt(
        &mut self,
        receipt: CloudWorkerProcessReceipt,
    ) -> Result<(), CloudWorkerNodeBootstrapError> {
        if self.phase != CloudWorkerBootstrapPhase::Enrolling {
            return Err(CloudWorkerNodeBootstrapError::InvalidProcessReceiptPhase {
                phase: self.phase,
            });
        }
        receipt
            .validate()
            .map_err(|_| CloudWorkerNodeBootstrapError::InvalidProcessReceipt)?;
        self.process_receipt = Some(receipt);
        Ok(())
    }

    pub fn settle(
        &mut self,
        phase: CloudWorkerBootstrapPhase,
        retry_class: CloudWorkerBootstrapRetryClass,
        diagnostic: Option<CloudWorkerDiagnostic>,
    ) -> Result<(), CloudWorkerNodeBootstrapError> {
        validate_retry_class(phase, retry_class)?;
        if !is_allowed_transition(self.phase, phase) {
            return Err(CloudWorkerNodeBootstrapError::InvalidPhaseTransition {
                from: self.phase,
                to: phase,
            });
        }
        if matches!(phase, CloudWorkerBootstrapPhase::Ready) {
            if self.request.secret_refs.is_empty() {
                return Err(CloudWorkerNodeBootstrapError::MissingSecretRef);
            }
            if self.process_receipt.is_none() {
                return Err(CloudWorkerNodeBootstrapError::MissingProcessReceipt);
            }
        }
        if let Some(diagnostic) = diagnostic {
            if self.diagnostics.len() >= MAX_DIAGNOSTICS {
                return Err(CloudWorkerNodeBootstrapError::DiagnosticLimitReached);
            }
            diagnostic.validate()?;
            self.diagnostics.push(diagnostic);
        }
        self.phase = phase;
        self.retry_class = retry_class;
        Ok(())
    }

    pub fn validate_persisted(&self) -> Result<(), CloudWorkerNodeBootstrapError> {
        self.request.validate()?;
        validate_retry_class(self.phase, self.retry_class)?;
        if self.runtime_identity != self.request.runtime.runtime_id {
            return Err(CloudWorkerNodeBootstrapError::InvalidRuntimeIdentity);
        }
        if self.diagnostics.len() > MAX_DIAGNOSTICS {
            return Err(CloudWorkerNodeBootstrapError::DiagnosticLimitReached);
        }
        for diagnostic in &self.diagnostics {
            diagnostic.validate()?;
        }
        if let Some(receipt) = &self.process_receipt {
            receipt
                .validate()
                .map_err(|_| CloudWorkerNodeBootstrapError::InvalidProcessReceipt)?;
        }
        if self.phase == CloudWorkerBootstrapPhase::Ready && self.process_receipt.is_none() {
            return Err(CloudWorkerNodeBootstrapError::MissingProcessReceipt);
        }
        Ok(())
    }
}

fn validate_retry_class(
    phase: CloudWorkerBootstrapPhase,
    retry_class: CloudWorkerBootstrapRetryClass,
) -> Result<(), CloudWorkerNodeBootstrapError> {
    let valid = match phase {
        CloudWorkerBootstrapPhase::Ready => retry_class == CloudWorkerBootstrapRetryClass::Never,
        CloudWorkerBootstrapPhase::OutcomeUnknown => {
            retry_class == CloudWorkerBootstrapRetryClass::ReconcileFirst
        }
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(CloudWorkerNodeBootstrapError::InvalidRetryClass { phase, retry_class })
    }
}

fn is_allowed_transition(from: CloudWorkerBootstrapPhase, to: CloudWorkerBootstrapPhase) -> bool {
    matches!(
        (from, to),
        (
            CloudWorkerBootstrapPhase::Pending,
            CloudWorkerBootstrapPhase::Preparing
        ) | (
            CloudWorkerBootstrapPhase::Pending,
            CloudWorkerBootstrapPhase::OutcomeUnknown
        ) | (
            CloudWorkerBootstrapPhase::Preparing,
            CloudWorkerBootstrapPhase::Enrolling
        ) | (
            CloudWorkerBootstrapPhase::Preparing,
            CloudWorkerBootstrapPhase::Failed
        ) | (
            CloudWorkerBootstrapPhase::Preparing,
            CloudWorkerBootstrapPhase::Cancelled
        ) | (
            CloudWorkerBootstrapPhase::Preparing,
            CloudWorkerBootstrapPhase::OutcomeUnknown
        ) | (
            CloudWorkerBootstrapPhase::Enrolling,
            CloudWorkerBootstrapPhase::Ready
        ) | (
            CloudWorkerBootstrapPhase::Enrolling,
            CloudWorkerBootstrapPhase::Failed
        ) | (
            CloudWorkerBootstrapPhase::Enrolling,
            CloudWorkerBootstrapPhase::Cancelled
        ) | (
            CloudWorkerBootstrapPhase::Enrolling,
            CloudWorkerBootstrapPhase::OutcomeUnknown
        ) | (
            CloudWorkerBootstrapPhase::OutcomeUnknown,
            CloudWorkerBootstrapPhase::Preparing
        )
    )
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerNodeBootstrapError {
    #[error("secret reference is empty, too long, or contains control characters")]
    InvalidSecretRef,
    #[error("bootstrap scope is invalid")]
    InvalidScope,
    #[error("bootstrap lease identity is invalid")]
    InvalidLeaseIdentity,
    #[error("bootstrap runtime identity is invalid")]
    InvalidRuntimeIdentity,
    #[error("bootstrap platform bridge is invalid")]
    InvalidPlatformBridge,
    #[error("bootstrap artifact digest is invalid")]
    InvalidArtifactDigest,
    #[error("at least one secret reference is required")]
    MissingSecretRef,
    #[error("bootstrap secret-reference limit reached")]
    SecretRefLimitReached,
    #[error("bootstrap requires a replay key")]
    MissingReplayKey,
    #[error("bootstrap replay key is invalid")]
    InvalidReplayKey,
    #[error("bootstrap diagnostic is missing redaction or contains invalid text")]
    InvalidDiagnostic,
    #[error("bootstrap diagnostic limit reached")]
    DiagnosticLimitReached,
    #[error("retry class {retry_class:?} is invalid for bootstrap phase {phase:?}")]
    InvalidRetryClass {
        phase: CloudWorkerBootstrapPhase,
        retry_class: CloudWorkerBootstrapRetryClass,
    },
    #[error("bootstrap process receipt is invalid")]
    InvalidProcessReceipt,
    #[error("bootstrap process receipt cannot be attached in phase {phase:?}")]
    InvalidProcessReceiptPhase { phase: CloudWorkerBootstrapPhase },
    #[error("ready bootstrap record requires a process receipt")]
    MissingProcessReceipt,
    #[error("invalid bootstrap phase transition from {from:?} to {to:?}")]
    InvalidPhaseTransition {
        from: CloudWorkerBootstrapPhase,
        to: CloudWorkerBootstrapPhase,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::cloud_workers::{CloudWorkerDesktopSession, CloudWorkerPlatform};

    fn request() -> CloudWorkerBootstrapRequest {
        CloudWorkerBootstrapRequest {
            lease_id: LeaseId("lease-1".into()),
            owner_epoch: OwnerEpoch(3),
            generation: LeaseGeneration(7),
            scope: CloudWorkerLeaseScope::new("account", "device").unwrap(),
            runtime: CloudWorkerRuntimeIdentity::new("provider", "profile", "runtime").unwrap(),
            artifact: CloudWorkerVerifiedArtifact {
                digest: super::super::CloudWorkerArtifactDigest::new("a".repeat(64)).unwrap(),
                size_bytes: 1,
            },
            platform: CloudWorkerPlatformBridge::new(
                CloudWorkerPlatform::Linux,
                CloudWorkerDesktopSession::Headless,
                "linux-launcher",
            )
            .unwrap(),
            secret_refs: vec![CloudWorkerSecretRef::new("secret://bootstrap").unwrap()],
            replay_key: ReplayKey("request-1".into()),
        }
    }

    fn process_receipt() -> CloudWorkerProcessReceipt {
        CloudWorkerProcessReceipt {
            pid: 42,
            start_time: "Fri Oct 31 11:22:33 2026".into(),
            executable_path: "/opt/openclaw/node".into(),
            runtime_dir: "/var/lib/openclaw/runtime/lease-1".into(),
            state_dir: "/var/lib/openclaw/state/lease-1".into(),
            cli: "/opt/openclaw/bin/openclaw".into(),
            session_id: None,
            user_sid: None,
        }
    }

    #[test]
    fn keeps_bootstrap_credentials_as_opaque_refs() {
        let record = CloudWorkerBootstrapRecord::pending(request()).unwrap();
        assert_eq!(record.request.secret_refs[0].as_str(), "secret://bootstrap");
        assert_eq!(record.phase, CloudWorkerBootstrapPhase::Pending);
    }

    #[test]
    fn ambiguous_bootstrap_is_reconcile_first() {
        let mut record = CloudWorkerBootstrapRecord::pending(request()).unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::OutcomeUnknown,
                CloudWorkerBootstrapRetryClass::ReconcileFirst,
                Some(CloudWorkerDiagnostic::redacted(
                    "timeout",
                    "provider response lost",
                )),
            )
            .unwrap();
        assert_eq!(
            record.retry_class,
            CloudWorkerBootstrapRetryClass::ReconcileFirst
        );
        assert!(record.diagnostics[0].redacted);
    }

    #[test]
    fn rejects_raw_controlled_secret_values() {
        assert!(matches!(
            CloudWorkerSecretRef::new("secret\nvalue"),
            Err(CloudWorkerNodeBootstrapError::InvalidSecretRef)
        ));
    }

    #[test]
    fn readiness_requires_preparation_and_enrollment() {
        let mut record = CloudWorkerBootstrapRecord::pending(request()).unwrap();
        assert_eq!(
            record.settle(
                CloudWorkerBootstrapPhase::Ready,
                CloudWorkerBootstrapRetryClass::Never,
                None,
            ),
            Err(CloudWorkerNodeBootstrapError::InvalidPhaseTransition {
                from: CloudWorkerBootstrapPhase::Pending,
                to: CloudWorkerBootstrapPhase::Ready,
            })
        );
        record
            .settle(
                CloudWorkerBootstrapPhase::Preparing,
                CloudWorkerBootstrapRetryClass::SafeReplay,
                None,
            )
            .unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Enrolling,
                CloudWorkerBootstrapRetryClass::ReconcileFirst,
                None,
            )
            .unwrap();
        record.attach_process_receipt(process_receipt()).unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Ready,
                CloudWorkerBootstrapRetryClass::Never,
                None,
            )
            .unwrap();
        assert_eq!(record.phase, CloudWorkerBootstrapPhase::Ready);
    }

    #[test]
    fn ready_requires_an_enrolling_process_receipt() {
        let mut record = CloudWorkerBootstrapRecord::pending(request()).unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Preparing,
                CloudWorkerBootstrapRetryClass::SafeReplay,
                None,
            )
            .unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Enrolling,
                CloudWorkerBootstrapRetryClass::ReconcileFirst,
                None,
            )
            .unwrap();
        assert_eq!(
            record.settle(
                CloudWorkerBootstrapPhase::Ready,
                CloudWorkerBootstrapRetryClass::Never,
                None,
            ),
            Err(CloudWorkerNodeBootstrapError::MissingProcessReceipt)
        );
        record.attach_process_receipt(process_receipt()).unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Ready,
                CloudWorkerBootstrapRetryClass::Never,
                None,
            )
            .unwrap();
        assert_eq!(
            record.attach_process_receipt(process_receipt()),
            Err(CloudWorkerNodeBootstrapError::InvalidProcessReceiptPhase {
                phase: CloudWorkerBootstrapPhase::Ready,
            })
        );
    }

    #[test]
    fn retry_class_matches_ready_and_unknown_outcomes() {
        let mut record = CloudWorkerBootstrapRecord::pending(request()).unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Preparing,
                CloudWorkerBootstrapRetryClass::SafeReplay,
                None,
            )
            .unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Enrolling,
                CloudWorkerBootstrapRetryClass::ReconcileFirst,
                None,
            )
            .unwrap();
        assert_eq!(
            record.settle(
                CloudWorkerBootstrapPhase::Ready,
                CloudWorkerBootstrapRetryClass::ReconcileFirst,
                None,
            ),
            Err(CloudWorkerNodeBootstrapError::InvalidRetryClass {
                phase: CloudWorkerBootstrapPhase::Ready,
                retry_class: CloudWorkerBootstrapRetryClass::ReconcileFirst,
            })
        );
        record.attach_process_receipt(process_receipt()).unwrap();
        record
            .settle(
                CloudWorkerBootstrapPhase::Ready,
                CloudWorkerBootstrapRetryClass::Never,
                None,
            )
            .unwrap();

        let mut uncertain = CloudWorkerBootstrapRecord::pending(request()).unwrap();
        assert_eq!(
            uncertain.settle(
                CloudWorkerBootstrapPhase::OutcomeUnknown,
                CloudWorkerBootstrapRetryClass::Never,
                None,
            ),
            Err(CloudWorkerNodeBootstrapError::InvalidRetryClass {
                phase: CloudWorkerBootstrapPhase::OutcomeUnknown,
                retry_class: CloudWorkerBootstrapRetryClass::Never,
            })
        );
    }

    #[test]
    fn request_validation_does_not_trust_deserialized_digest() {
        let mut request = request();
        request.artifact.digest = super::super::CloudWorkerArtifactDigest("not-a-digest".into());
        assert_eq!(
            request.validate(),
            Err(CloudWorkerNodeBootstrapError::InvalidArtifactDigest)
        );
    }

    #[test]
    fn request_validation_rejects_an_invalid_deserialized_platform_bridge() {
        let mut request = request();
        request.platform.launcher_id = "launcher\nidentity".into();
        assert_eq!(
            request.validate(),
            Err(CloudWorkerNodeBootstrapError::InvalidPlatformBridge)
        );
    }

    #[test]
    fn settlement_rejects_unredacted_diagnostics() {
        let mut record = CloudWorkerBootstrapRecord::pending(request()).unwrap();
        assert_eq!(
            record.settle(
                CloudWorkerBootstrapPhase::Preparing,
                CloudWorkerBootstrapRetryClass::SafeReplay,
                Some(CloudWorkerDiagnostic {
                    code: "raw-provider-error".into(),
                    message: "secret payload".into(),
                    redacted: false,
                }),
            ),
            Err(CloudWorkerNodeBootstrapError::InvalidDiagnostic)
        );
        assert_eq!(record.phase, CloudWorkerBootstrapPhase::Pending);
    }

    #[test]
    fn redacted_diagnostics_use_the_host_secret_sanitizer_and_bound() {
        let message = format!(
            "Bearer token123 password=secret https://example.test/?token=query-secret {}",
            "x".repeat(MAX_DIAGNOSTIC_CHARS + 100)
        );
        let diagnostic = CloudWorkerDiagnostic::redacted("provider", message);
        assert!(diagnostic.redacted);
        assert!(!diagnostic.message.contains("token123"));
        assert!(!diagnostic.message.contains("query-secret"));
        assert!(!diagnostic.message.contains("password=secret"));
        assert!(diagnostic.message.chars().count() <= MAX_DIAGNOSTIC_CHARS);
        assert!(diagnostic.validate().is_ok());
    }

    #[test]
    fn redacted_diagnostic_codes_are_sanitized_too() {
        let diagnostic = CloudWorkerDiagnostic::redacted(
            "Bearer code-token password=code-secret",
            "provider failed",
        );
        assert!(!diagnostic.code.contains("code-token"));
        assert!(!diagnostic.code.contains("code-secret"));
        assert!(diagnostic.validate().is_ok());
    }

    #[test]
    fn rejects_deserialized_diagnostics_marked_redacted_but_not_sanitized() {
        let diagnostic = CloudWorkerDiagnostic {
            code: "provider".into(),
            message: "Bearer token123".into(),
            redacted: true,
        };
        assert_eq!(
            diagnostic.validate(),
            Err(CloudWorkerNodeBootstrapError::InvalidDiagnostic)
        );

        let code_leak = CloudWorkerDiagnostic {
            code: "Bearer code-token".into(),
            message: "provider failed".into(),
            redacted: true,
        };
        assert_eq!(
            code_leak.validate(),
            Err(CloudWorkerNodeBootstrapError::InvalidDiagnostic)
        );
    }

    #[test]
    fn request_validation_bounds_secret_reference_count() {
        let mut request = request();
        request.secret_refs = (0..33)
            .map(|index| CloudWorkerSecretRef::new(format!("secret://{index}")).unwrap())
            .collect();
        assert_eq!(
            request.validate(),
            Err(CloudWorkerNodeBootstrapError::SecretRefLimitReached)
        );
    }
}
