use std::path::Component;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use sha2::{Digest, Sha256};

const MAX_ARCHIVE_MEMBER_CHARS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerArtifactKind {
    WorkerBinary,
    RuntimeBundle,
    PluginBundle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerArtifactDigest(pub String);

impl CloudWorkerArtifactDigest {
    pub fn new(value: impl Into<String>) -> Result<Self, CloudWorkerArtifactAdmissionError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CloudWorkerArtifactAdmissionError::InvalidDigest);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerArtifactSource {
    Https {
        url: String,
        digest: CloudWorkerArtifactDigest,
    },
    ManagedLocal {
        digest: CloudWorkerArtifactDigest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerArtifactPolicy {
    pub allowed_https_origins: Vec<String>,
    pub max_bytes: u64,
}

impl CloudWorkerArtifactPolicy {
    pub fn validate(&self) -> Result<(), CloudWorkerArtifactAdmissionError> {
        if self.max_bytes == 0 {
            return Err(CloudWorkerArtifactAdmissionError::InvalidSizeLimit);
        }
        for origin in &self.allowed_https_origins {
            canonical_https_origin(origin)?;
        }
        Ok(())
    }

    pub fn validate_source(
        &self,
        source: &CloudWorkerArtifactSource,
        size_bytes: u64,
    ) -> Result<(), CloudWorkerArtifactAdmissionError> {
        self.validate()?;
        if size_bytes > self.max_bytes {
            return Err(CloudWorkerArtifactAdmissionError::ArtifactTooLarge {
                bytes: size_bytes,
                max_bytes: self.max_bytes,
            });
        }
        let expected_digest = match source {
            CloudWorkerArtifactSource::Https { digest, .. }
            | CloudWorkerArtifactSource::ManagedLocal { digest } => digest,
        };
        CloudWorkerArtifactDigest::new(expected_digest.0.clone())
            .map_err(|_| CloudWorkerArtifactAdmissionError::InvalidDigest)?;
        if let CloudWorkerArtifactSource::Https { url, .. } = source {
            let parsed = Url::parse(url)
                .map_err(|_| CloudWorkerArtifactAdmissionError::InvalidOrigin)?;
            if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some()
            {
                return Err(CloudWorkerArtifactAdmissionError::InvalidOrigin);
            }
            let origin = parsed.origin().ascii_serialization();
            if !self
                .allowed_https_origins
                .iter()
                .any(|allowed| {
                    canonical_https_origin(allowed).ok().as_deref() == Some(origin.as_str())
                })
            {
                return Err(CloudWorkerArtifactAdmissionError::OriginNotAllowed(origin));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloudWorkerArtifactAdmissionState {
    Pending,
    Verified,
    Damaged,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerArtifactAdmission {
    pub kind: CloudWorkerArtifactKind,
    pub source: CloudWorkerArtifactSource,
    pub size_bytes: u64,
    pub state: CloudWorkerArtifactAdmissionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudWorkerVerifiedArtifact {
    pub digest: CloudWorkerArtifactDigest,
    pub size_bytes: u64,
}

impl CloudWorkerVerifiedArtifact {
    pub fn validate(&self) -> Result<(), CloudWorkerArtifactAdmissionError> {
        CloudWorkerArtifactDigest::new(self.digest.0.clone()).map(|_| ())
    }
}

pub fn verify_artifact_bytes(
    policy: &CloudWorkerArtifactPolicy,
    source: &CloudWorkerArtifactSource,
    bytes: &[u8],
) -> Result<CloudWorkerVerifiedArtifact, CloudWorkerArtifactAdmissionError> {
    policy.validate_source(source, bytes.len() as u64)?;
    let actual = CloudWorkerArtifactDigest::new(
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )?;
    let expected = match source {
        CloudWorkerArtifactSource::Https { digest, .. }
        | CloudWorkerArtifactSource::ManagedLocal { digest } => digest,
    };
    if &actual != expected {
        return Err(CloudWorkerArtifactAdmissionError::DigestMismatch {
            expected: expected.clone(),
            actual,
        });
    }
    Ok(CloudWorkerVerifiedArtifact {
        digest: actual,
        size_bytes: bytes.len() as u64,
    })
}

impl CloudWorkerArtifactAdmission {
    pub fn validate(
        &self,
        policy: &CloudWorkerArtifactPolicy,
    ) -> Result<(), CloudWorkerArtifactAdmissionError> {
        policy.validate_source(&self.source, self.size_bytes)
    }

    pub fn validate_persisted(&self) -> Result<(), CloudWorkerArtifactAdmissionError> {
        let source_digest = match &self.source {
            CloudWorkerArtifactSource::Https { url, digest } => {
                let parsed = Url::parse(url)
                    .map_err(|_| CloudWorkerArtifactAdmissionError::InvalidOrigin)?;
                if parsed.scheme() != "https"
                    || parsed.username() != ""
                    || parsed.password().is_some()
                {
                    return Err(CloudWorkerArtifactAdmissionError::InvalidOrigin);
                }
                digest
            }
            CloudWorkerArtifactSource::ManagedLocal { digest } => digest,
        };
        CloudWorkerArtifactDigest::new(source_digest.0.clone())?;
        Ok(())
    }

    pub fn settle(
        &mut self,
        next: CloudWorkerArtifactAdmissionState,
    ) -> Result<(), CloudWorkerArtifactAdmissionError> {
        let allowed = matches!(
            (self.state, next),
            (CloudWorkerArtifactAdmissionState::Pending,
                CloudWorkerArtifactAdmissionState::Verified)
                | (CloudWorkerArtifactAdmissionState::Pending,
                    CloudWorkerArtifactAdmissionState::Damaged)
                | (CloudWorkerArtifactAdmissionState::Pending,
                    CloudWorkerArtifactAdmissionState::OutcomeUnknown)
                | (CloudWorkerArtifactAdmissionState::OutcomeUnknown,
                    CloudWorkerArtifactAdmissionState::Pending)
        );
        if !allowed {
            return Err(CloudWorkerArtifactAdmissionError::InvalidStateTransition);
        }
        self.state = next;
        Ok(())
    }
}

pub fn validate_archive_member_path(path: &str) -> Result<(), CloudWorkerArtifactAdmissionError> {
    if path.is_empty()
        || path.chars().count() > MAX_ARCHIVE_MEMBER_CHARS
        || path.chars().any(|character| character.is_control())
        || path.contains('\\')
        || is_windows_drive_path(path)
    {
        return Err(CloudWorkerArtifactAdmissionError::UnsafeArchivePath);
    }
    let path = std::path::Path::new(path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(component, Component::ParentDir | Component::Prefix(_))
        })
    {
        return Err(CloudWorkerArtifactAdmissionError::UnsafeArchivePath);
    }
    Ok(())
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn canonical_https_origin(value: &str) -> Result<String, CloudWorkerArtifactAdmissionError> {
    let url = Url::parse(value).map_err(|_| CloudWorkerArtifactAdmissionError::InvalidOrigin)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || (url.path() != "" && url.path() != "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CloudWorkerArtifactAdmissionError::InvalidOrigin);
    }
    Ok(url.origin().ascii_serialization())
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CloudWorkerArtifactAdmissionError {
    #[error("artifact digest must be exactly 64 hexadecimal characters")]
    InvalidDigest,
    #[error("artifact size limit must be greater than zero")]
    InvalidSizeLimit,
    #[error("artifact origin must be an HTTPS origin with no path")]
    InvalidOrigin,
    #[error("artifact origin is not allowlisted: {0}")]
    OriginNotAllowed(String),
    #[error("artifact is {bytes} bytes, above the {max_bytes}-byte limit")]
    ArtifactTooLarge { bytes: u64, max_bytes: u64 },
    #[error("archive member path is unsafe")]
    UnsafeArchivePath,
    #[error("artifact digest mismatch: expected {expected:?}, actual {actual:?}")]
    DigestMismatch {
        expected: CloudWorkerArtifactDigest,
        actual: CloudWorkerArtifactDigest,
    },
    #[error("invalid artifact admission state transition")]
    InvalidStateTransition,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest() -> CloudWorkerArtifactDigest {
        CloudWorkerArtifactDigest::new("a".repeat(64)).unwrap()
    }

    fn policy() -> CloudWorkerArtifactPolicy {
        CloudWorkerArtifactPolicy {
            allowed_https_origins: vec!["https://artifacts.example".into()],
            max_bytes: 1024,
        }
    }

    #[test]
    fn accepts_an_allowlisted_https_source() {
        let source = CloudWorkerArtifactSource::Https {
            url: "https://artifacts.example/worker.tar".into(),
            digest: digest(),
        };
        assert_eq!(policy().validate_source(&source, 10), Ok(()));
    }

    #[test]
    fn rejects_credentials_in_artifact_urls() {
        let source = CloudWorkerArtifactSource::Https {
            url: "https://user:pass@artifacts.example/worker.tar".into(),
            digest: digest(),
        };
        assert!(matches!(
            policy().validate_source(&source, 10),
            Err(CloudWorkerArtifactAdmissionError::InvalidOrigin)
        ));
    }

    #[test]
    fn rejects_parent_archive_paths() {
        assert_eq!(
            validate_archive_member_path("worker/../secret"),
            Err(CloudWorkerArtifactAdmissionError::UnsafeArchivePath)
        );
    }

    #[test]
    fn rejects_windows_drive_archive_paths_on_unix_hosts() {
        assert_eq!(
            validate_archive_member_path("C:/Windows/System32/worker.exe"),
            Err(CloudWorkerArtifactAdmissionError::UnsafeArchivePath)
        );
        assert_eq!(
            validate_archive_member_path("C:relative-worker.exe"),
            Err(CloudWorkerArtifactAdmissionError::UnsafeArchivePath)
        );
    }

    #[test]
    fn verifies_bytes_against_the_declared_digest() {
        let bytes = b"worker";
        let actual = CloudWorkerArtifactDigest::new(
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
        .unwrap();
        let source = CloudWorkerArtifactSource::ManagedLocal { digest: actual };
        let verified = verify_artifact_bytes(&policy(), &source, bytes).unwrap();
        assert_eq!(verified.size_bytes, 6);
    }

    #[test]
    fn rejects_a_declared_digest_mismatch() {
        let source = CloudWorkerArtifactSource::ManagedLocal { digest: digest() };
        assert!(matches!(
            verify_artifact_bytes(&policy(), &source, b"worker"),
            Err(CloudWorkerArtifactAdmissionError::DigestMismatch { .. })
        ));
    }

    #[test]
    fn accepts_origin_without_a_trailing_slash() {
        assert_eq!(policy().validate(), Ok(()));
    }

    #[test]
    fn rejects_deserialized_invalid_digest_before_byte_verification() {
        let source = CloudWorkerArtifactSource::ManagedLocal {
            digest: CloudWorkerArtifactDigest("invalid".into()),
        };
        assert_eq!(
            policy().validate_source(&source, 10),
            Err(CloudWorkerArtifactAdmissionError::InvalidDigest)
        );
    }
}
