use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use super::{CloudWorkerPersistentSnapshot, CloudWorkerProvisioningOwner};

pub const CLOUD_WORKER_MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct CloudWorkerProvisioningStore;

impl CloudWorkerProvisioningStore {
    pub fn backup_existing(
        &self,
        source: &Path,
        backup: &Path,
    ) -> Result<(), CloudWorkerStoreError> {
        if source == backup {
            return Err(CloudWorkerStoreError::InvalidBackupPath);
        }
        let source_metadata = fs::symlink_metadata(source)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    CloudWorkerStoreError::BackupSourceMissing
                } else {
                    CloudWorkerStoreError::Read(error)
                }
            })?;
        if source_metadata.file_type().is_symlink() || !source_metadata.file_type().is_file() {
            return Err(CloudWorkerStoreError::InvalidBackupSource);
        }
        match fs::symlink_metadata(backup) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CloudWorkerStoreError::InvalidBackupDestination);
            }
            Ok(_) => return Err(CloudWorkerStoreError::BackupAlreadyExists),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(CloudWorkerStoreError::Read(error)),
        }
        let parent = backup.parent().ok_or(CloudWorkerStoreError::NoParent)?;
        fs::create_dir_all(parent).map_err(CloudWorkerStoreError::Write)?;
        fs::copy(source, backup).map_err(CloudWorkerStoreError::Write)?;
        #[cfg(unix)]
        fs::set_permissions(
            backup,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .map_err(CloudWorkerStoreError::Write)?;
        File::open(backup)
            .and_then(|file| file.sync_all())
            .map_err(CloudWorkerStoreError::Write)?;
        sync_parent_directory(parent).map_err(CloudWorkerStoreError::DirectoryBarrierOutcomeUnknown)?;
        Ok(())
    }

    pub fn load(
        &self,
        path: &Path,
    ) -> Result<CloudWorkerPersistentSnapshot, CloudWorkerStoreError> {
        let mut file = File::open(path).map_err(CloudWorkerStoreError::Read)?;
        let mut bytes = Vec::new();
        file.take(CLOUD_WORKER_MAX_STATE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(CloudWorkerStoreError::Read)?;
        if bytes.len() as u64 > CLOUD_WORKER_MAX_STATE_BYTES {
            return Err(CloudWorkerStoreError::StateTooLarge {
                limit: CLOUD_WORKER_MAX_STATE_BYTES,
            });
        }
        serde_json::from_slice(&bytes).map_err(CloudWorkerStoreError::Decode)
    }

    pub fn save_atomic(
        &self,
        path: &Path,
        owner: &CloudWorkerProvisioningOwner,
    ) -> Result<(), CloudWorkerStoreError> {
        let parent = path.parent().ok_or(CloudWorkerStoreError::NoParent)?;
        fs::create_dir_all(parent).map_err(CloudWorkerStoreError::Write)?;
        let temporary = temporary_path(path);
        let bytes = serde_json::to_vec_pretty(&owner.snapshot())
            .map_err(CloudWorkerStoreError::Encode)?;
        if bytes.len() as u64 > CLOUD_WORKER_MAX_STATE_BYTES {
            return Err(CloudWorkerStoreError::StateTooLarge {
                limit: CLOUD_WORKER_MAX_STATE_BYTES,
            });
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(CloudWorkerStoreError::Write)?;
        #[cfg(unix)]
        if let Err(error) = fs::set_permissions(
            &temporary,
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        ) {
            let _ = fs::remove_file(&temporary);
            return Err(CloudWorkerStoreError::Write(error));
        }
        if let Err(error) = write_and_sync(&mut file, &bytes) {
            let _ = fs::remove_file(&temporary);
            return Err(CloudWorkerStoreError::Write(error));
        }
        drop(file);
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(CloudWorkerStoreError::Write(error));
        }
        // The rename makes the new snapshot visible, but the directory entry
        // is not durable until the parent directory is synced.  Treat a
        // failed directory barrier as a write failure so callers do not
        // mistake a crash-vulnerable publication for a durable checkpoint.
        sync_parent_directory(parent).map_err(CloudWorkerStoreError::DirectoryBarrierOutcomeUnknown)?;
        Ok(())
    }
}

fn write_and_sync(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> io::Result<()> {
    // Windows does not expose a portable directory-handle fsync equivalent
    // through the standard library. The file sync plus atomic rename remains
    // the strongest std-only publication primitive on that target.
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".tmp");
    PathBuf::from(temporary)
}

#[derive(Debug, thiserror::Error)]
pub enum CloudWorkerStoreError {
    #[error("cloud-worker state has no parent directory")]
    NoParent,
    #[error("could not read cloud-worker state: {0}")]
    Read(#[source] io::Error),
    #[error("could not write cloud-worker state atomically: {0}")]
    Write(#[source] io::Error),
    #[error("cloud-worker state directory durability barrier outcome is unknown: {0}")]
    DirectoryBarrierOutcomeUnknown(#[source] io::Error),
    #[error("could not encode cloud-worker state: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("could not decode cloud-worker state: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("cloud-worker state exceeds the {limit}-byte limit")]
    StateTooLarge { limit: u64 },
    #[error("cloud-worker owner lock is poisoned")]
    PoisonedOwner,
    #[error("backup source does not exist")]
    BackupSourceMissing,
    #[error("backup source must be a regular non-symlink file")]
    InvalidBackupSource,
    #[error("backup destination already exists")]
    BackupAlreadyExists,
    #[error("backup destination must not be a symlink")]
    InvalidBackupDestination,
    #[error("backup source and destination must differ")]
    InvalidBackupPath,
}
