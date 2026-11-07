use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use super::{
    verify_artifact_bytes, CloudWorkerArtifactAdmissionError, CloudWorkerArtifactKind,
    CloudWorkerArtifactPolicy, CloudWorkerArtifactSource, CloudWorkerVerifiedArtifact,
};

/// A digest-addressed managed artifact publication boundary.
///
/// This type owns only local publication of already verified bytes. Network
/// acquisition, archive extraction, provider launch and lifecycle admission
/// remain outside this boundary.
#[derive(Debug, Clone)]
pub struct CloudWorkerArtifactPublisher {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudWorkerPublishedArtifact {
    pub proof: CloudWorkerVerifiedArtifact,
    pub path: PathBuf,
}

impl CloudWorkerArtifactPublisher {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, CloudWorkerArtifactStoreError> {
        let root = root.into();
        ensure_regular_directory(&root)?;
        Ok(Self { root })
    }

    pub fn publish_verified(
        &self,
        kind: CloudWorkerArtifactKind,
        policy: &CloudWorkerArtifactPolicy,
        source: &CloudWorkerArtifactSource,
        bytes: &[u8],
    ) -> Result<CloudWorkerPublishedArtifact, CloudWorkerArtifactStoreError> {
        let proof = verify_artifact_bytes(policy, source, bytes)?;
        let directory = self.root.join(kind_directory(&kind));
        ensure_regular_directory(&directory)?;
        let destination = directory.join(format!("{}.artifact", proof.digest.0));

        match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                    return Err(CloudWorkerArtifactStoreError::InvalidDestination);
                }
                let mut existing = Vec::new();
                File::open(&destination)
                    .map_err(CloudWorkerArtifactStoreError::Read)?
                    .read_to_end(&mut existing)
                    .map_err(CloudWorkerArtifactStoreError::Read)?;
                let existing_proof = verify_artifact_bytes(policy, source, &existing)
                    .map_err(|_| CloudWorkerArtifactStoreError::ExistingArtifactMismatch)?;
                if existing_proof == proof {
                    return Ok(CloudWorkerPublishedArtifact {
                        proof,
                        path: destination,
                    });
                }
                return Err(CloudWorkerArtifactStoreError::ExistingArtifactMismatch);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(CloudWorkerArtifactStoreError::Read(error)),
        }

        let temporary = temporary_path(&destination);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(CloudWorkerArtifactStoreError::Write)?;
        #[cfg(unix)]
        if let Err(error) = set_private_permissions(&temporary) {
            let _ = fs::remove_file(&temporary);
            return Err(CloudWorkerArtifactStoreError::Write(error));
        }
        if let Err(error) = write_and_sync(&mut file, bytes) {
            let _ = fs::remove_file(&temporary);
            return Err(CloudWorkerArtifactStoreError::Write(error));
        }
        drop(file);
        if let Err(error) = fs::rename(&temporary, &destination) {
            let _ = fs::remove_file(&temporary);
            return Err(CloudWorkerArtifactStoreError::Write(error));
        }
        sync_parent_directory(&directory)
            .map_err(CloudWorkerArtifactStoreError::DirectoryBarrierOutcomeUnknown)?;
        Ok(CloudWorkerPublishedArtifact {
            proof,
            path: destination,
        })
    }

    /// Re-resolves a previously published artifact without creating or
    /// repairing anything. The bytes must still verify against the original
    /// source and the expected proof, otherwise the caller must treat the
    /// managed artifact as unavailable or damaged.
    pub fn resolve_verified(
        &self,
        kind: CloudWorkerArtifactKind,
        policy: &CloudWorkerArtifactPolicy,
        source: &CloudWorkerArtifactSource,
        expected: &CloudWorkerVerifiedArtifact,
    ) -> Result<CloudWorkerPublishedArtifact, CloudWorkerArtifactStoreError> {
        expected.validate()?;
        let directory = self.root.join(kind_directory(&kind));
        ensure_existing_regular_directory(&directory)?;
        let destination = directory.join(format!("{}.artifact", expected.digest.0));
        let metadata = fs::symlink_metadata(&destination).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CloudWorkerArtifactStoreError::MissingArtifact
            } else {
                CloudWorkerArtifactStoreError::Read(error)
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(CloudWorkerArtifactStoreError::InvalidDestination);
        }
        let mut bytes = Vec::new();
        File::open(&destination)
            .map_err(CloudWorkerArtifactStoreError::Read)?
            .read_to_end(&mut bytes)
            .map_err(CloudWorkerArtifactStoreError::Read)?;
        let proof = verify_artifact_bytes(policy, source, &bytes)
            .map_err(|_| CloudWorkerArtifactStoreError::ExistingArtifactMismatch)?;
        if proof != *expected {
            return Err(CloudWorkerArtifactStoreError::ExistingArtifactMismatch);
        }
        Ok(CloudWorkerPublishedArtifact {
            proof,
            path: destination,
        })
    }
}

fn kind_directory(kind: &CloudWorkerArtifactKind) -> &'static str {
    match kind {
        CloudWorkerArtifactKind::WorkerBinary => "worker-binary",
        CloudWorkerArtifactKind::RuntimeBundle => "runtime-bundle",
        CloudWorkerArtifactKind::PluginBundle => "plugin-bundle",
    }
}

fn ensure_regular_directory(path: &Path) -> Result<(), CloudWorkerArtifactStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(CloudWorkerArtifactStoreError::InvalidDirectory)
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(CloudWorkerArtifactStoreError::InvalidDirectory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(CloudWorkerArtifactStoreError::Write)
        }
        Err(error) => Err(CloudWorkerArtifactStoreError::Read(error)),
    }
}

fn ensure_existing_regular_directory(path: &Path) -> Result<(), CloudWorkerArtifactStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(CloudWorkerArtifactStoreError::InvalidDirectory)
        }
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(CloudWorkerArtifactStoreError::InvalidDirectory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(CloudWorkerArtifactStoreError::MissingArtifact)
        }
        Err(error) => Err(CloudWorkerArtifactStoreError::Read(error)),
    }
}

fn temporary_path(destination: &Path) -> PathBuf {
    let mut path = destination.as_os_str().to_os_string();
    path.push(format!(".tmp-{}", std::process::id()));
    PathBuf::from(path)
}

fn write_and_sync(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CloudWorkerArtifactStoreError {
    #[error(transparent)]
    Admission(#[from] CloudWorkerArtifactAdmissionError),
    #[error("managed artifact root or destination must be a regular directory/file")]
    InvalidDirectory,
    #[error("managed artifact destination must be a regular non-symlink file")]
    InvalidDestination,
    #[error("managed artifact is missing")]
    MissingArtifact,
    #[error("managed artifact already exists with different verified bytes")]
    ExistingArtifactMismatch,
    #[error("could not read managed artifact: {0}")]
    Read(#[source] io::Error),
    #[error("could not publish managed artifact: {0}")]
    Write(#[source] io::Error),
    #[error("managed artifact directory barrier outcome is unknown: {0}")]
    DirectoryBarrierOutcomeUnknown(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn policy() -> CloudWorkerArtifactPolicy {
        CloudWorkerArtifactPolicy {
            allowed_https_origins: Vec::new(),
            max_bytes: 1024,
        }
    }

    fn source(bytes: &[u8]) -> CloudWorkerArtifactSource {
        let digest = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        CloudWorkerArtifactSource::ManagedLocal {
            digest: super::super::CloudWorkerArtifactDigest::new(digest).unwrap(),
        }
    }

    #[test]
    fn publishes_digest_addressed_bytes_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "cloud-worker-artifacts-{}",
            std::process::id()
        ));
        let publisher = CloudWorkerArtifactPublisher::new(root.clone()).unwrap();
        let bytes = b"worker";
        let first = publisher
            .publish_verified(
                CloudWorkerArtifactKind::WorkerBinary,
                &policy(),
                &source(bytes),
                bytes,
            )
            .unwrap();
        let second = publisher
            .publish_verified(
                CloudWorkerArtifactKind::WorkerBinary,
                &policy(),
                &source(bytes),
                bytes,
            )
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(std::fs::read(first.path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_a_symlink_destination() {
        let root = std::env::temp_dir().join(format!(
            "cloud-worker-artifacts-symlink-{}",
            std::process::id()
        ));
        let publisher = CloudWorkerArtifactPublisher::new(root.clone()).unwrap();
        let bytes = b"worker";
        let proof = verify_artifact_bytes(&policy(), &source(bytes), bytes).unwrap();
        let directory = root.join("worker-binary");
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join(format!("{}.artifact", proof.digest.0));
        let target = root.join("outside");
        std::fs::write(&target, b"outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &destination).unwrap();
        #[cfg(unix)]
        assert!(matches!(
            publisher.publish_verified(
                CloudWorkerArtifactKind::WorkerBinary,
                &policy(),
                &source(bytes),
                bytes,
            ),
            Err(CloudWorkerArtifactStoreError::InvalidDestination)
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
