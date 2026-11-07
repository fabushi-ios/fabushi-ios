use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::extensions::snapshot::git_backup_manager::{
    export_sqlite_backup_artifacts, git_backup_artifact_files, GitBackupIdentity,
};

use super::box_store_manifest_format::{BoxStoreManifestEntry, BOX_STORE_BLOBS_PREFIX};
use super::box_store_vacuum_worker::{spawn_box_store_vacuum_job, BoxStoreVacuumJob};
use crate::storage::store_db::DB_BUSY_TIMEOUT_MS;

pub type BoxManifestMap = BTreeMap<String, BoxStoreManifestEntry>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotUploadOutcome {
    Uploaded,
    Unchanged,
    Oversize,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFailurePhase {
    Capture,
    BlobUpload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotUploadResult {
    pub outcome: SnapshotUploadOutcome,
    pub bytes_uploaded: u64,
    pub blob_upload_duration_ms: u64,
    pub failure_phase: Option<SnapshotFailurePhase>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotArtifact {
    pub rel_path: String,
    pub path: PathBuf,
    pub mode: u32,
}

pub trait StoreDbSnapshotRuntimePort: Send + Sync {
    fn put_bytes(
        &self,
        store_id: &str,
        key: &str,
        bytes: &[u8],
        content_addressed: bool,
    ) -> Result<(), String>;

    fn put_from_file(
        &self,
        store_id: &str,
        key: &str,
        path: &Path,
        sha: &str,
        size: u64,
    ) -> Result<(), String>;

    fn set_manifest_entry(
        &self,
        manifest: &mut BoxManifestMap,
        rel_path: &str,
        entry: BoxStoreManifestEntry,
    ) {
        manifest.insert(rel_path.to_string(), entry);
    }
}

pub struct StoreDbSnapshotUpload {
    runtime: Arc<dyn StoreDbSnapshotRuntimePort>,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
    max_object_bytes: u64,
    large_object_threshold: u64,
}

impl StoreDbSnapshotUpload {
    pub fn new(
        runtime: Arc<dyn StoreDbSnapshotRuntimePort>,
        now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
        log: Arc<dyn Fn(&str) + Send + Sync>,
        max_object_bytes: u64,
        large_object_threshold: u64,
    ) -> Self {
        Self {
            runtime,
            now_ms,
            log,
            max_object_bytes,
            large_object_threshold,
        }
    }

    pub fn elapsed_duration_ms(&self, started_at: u64) -> u64 {
        (self.now_ms)().saturating_sub(started_at)
    }

    /// Upload a pre-admitted multi-file snapshot through the existing
    /// BoxStore owner. All relative paths are validated before the first
    /// upload; individual uploads remain observable and are intentionally
    /// non-atomic because the object store has no batch transaction.
    pub fn upload_artifact_set(
        &self,
        store_id: &str,
        manifest: &mut BoxManifestMap,
        artifacts: &[SnapshotArtifact],
    ) -> Result<Vec<SnapshotUploadResult>, String> {
        let mut paths = std::collections::BTreeSet::new();
        for artifact in artifacts {
            let path = Path::new(&artifact.rel_path);
            if artifact.rel_path.is_empty()
                || path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
                || !paths.insert(artifact.rel_path.clone())
            {
                return Err(format!(
                    "snapshot artifact path is invalid or duplicated: {}",
                    artifact.rel_path
                ));
            }
            let metadata = fs::symlink_metadata(&artifact.path).map_err(|error| {
                format!(
                    "snapshot artifact source cannot be inspected {}: {error}",
                    artifact.path.display()
                )
            })?;
            if !metadata.is_file() {
                return Err(format!(
                    "snapshot artifact source must be a regular file: {}",
                    artifact.path.display()
                ));
            }
            #[cfg(unix)]
            if std::os::unix::fs::MetadataExt::nlink(&metadata) != 1 {
                return Err(format!(
                    "snapshot artifact source must not be hard-linked: {}",
                    artifact.path.display()
                ));
            }
        }
        Ok(artifacts
            .iter()
            .map(|artifact| {
                self.upload_agent_db_snapshot(
                    store_id,
                    manifest,
                    &artifact.rel_path,
                    &artifact.path,
                    artifact.mode,
                )
            })
            .collect())
    }

    /// Export a live SQLite owner and publish its portable artifacts through
    /// this existing BoxStore owner. The staging directory is always removed;
    /// callers must treat a partially successful result as non-atomic.
    pub fn export_and_upload_sqlite_backup(
        &self,
        store_id: &str,
        manifest: &mut BoxManifestMap,
        connection: &Connection,
        staging_dir: &Path,
        identity: GitBackupIdentity,
        excluded_tables: &BTreeSet<String>,
        relative_prefix: &str,
        mode: u32,
    ) -> Result<Vec<SnapshotUploadResult>, String> {
        let exported =
            export_sqlite_backup_artifacts(connection, staging_dir, identity, excluded_tables)?;
        let artifacts = git_backup_artifact_files(&exported)
            .into_iter()
            .map(|relative| SnapshotArtifact {
                rel_path: format!("{relative_prefix}/{}", relative.display()),
                path: staging_dir.join(&relative),
                mode,
            })
            .collect::<Vec<_>>();
        let mut staged_manifest = manifest.clone();
        let result = self
            .upload_artifact_set(store_id, &mut staged_manifest, &artifacts)
            .and_then(|results| {
                if results.iter().all(|result| {
                    matches!(
                        result.outcome,
                        SnapshotUploadOutcome::Uploaded | SnapshotUploadOutcome::Unchanged
                    )
                }) {
                    *manifest = staged_manifest;
                    Ok(results)
                } else {
                    Err("portable SQLite backup upload was partially unsuccessful".into())
                }
            });
        let cleanup = fs::remove_dir_all(staging_dir);
        if let Err(error) = cleanup {
            return Err(format!("remove SQLite backup staging directory: {error}"));
        }
        result
    }

    pub fn upload_agent_db_snapshot(
        &self,
        store_id: &str,
        manifest: &mut BoxManifestMap,
        rel_path: &str,
        snapshot_path: &Path,
        mode: u32,
    ) -> SnapshotUploadResult {
        let size = match fs::metadata(snapshot_path) {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                (self.log)(&format!(
                    "store.db {rel_path} snapshot stat failed: {error}"
                ));
                return capture_error();
            }
        };
        if size > self.max_object_bytes {
            (self.log)(&format!(
                "oversize {rel_path}: {size}B over {}B",
                self.max_object_bytes
            ));
            return SnapshotUploadResult {
                outcome: SnapshotUploadOutcome::Oversize,
                bytes_uploaded: 0,
                blob_upload_duration_ms: 0,
                failure_phase: Some(SnapshotFailurePhase::Capture),
            };
        }

        let (sha, bytes) = if size >= self.large_object_threshold {
            match sha256_file(snapshot_path) {
                Ok(sha) => (sha, None),
                Err(error) => {
                    (self.log)(&format!("read failed {rel_path}: {error}"));
                    return capture_error();
                }
            }
        } else {
            match fs::read(snapshot_path) {
                Ok(bytes) => {
                    let sha = sha256_bytes(&bytes);
                    (sha, Some(bytes))
                }
                Err(error) => {
                    (self.log)(&format!("read failed {rel_path}: {error}"));
                    return capture_error();
                }
            }
        };

        let entry = BoxStoreManifestEntry::File {
            sha: sha.clone(),
            size,
            mode: mode & 0o777,
        };
        if manifest_file_matches(manifest.get(rel_path), &sha, size) {
            self.runtime.set_manifest_entry(manifest, rel_path, entry);
            return SnapshotUploadResult {
                outcome: SnapshotUploadOutcome::Unchanged,
                bytes_uploaded: 0,
                blob_upload_duration_ms: 0,
                failure_phase: None,
            };
        }

        let upload_started_at = (self.now_ms)();
        let key = format!("{BOX_STORE_BLOBS_PREFIX}/{sha}");
        let upload = match bytes {
            Some(bytes) => self.runtime.put_bytes(store_id, &key, &bytes, true),
            None => self
                .runtime
                .put_from_file(store_id, &key, snapshot_path, &sha, size),
        };
        if let Err(error) = upload {
            (self.log)(&format!("upload failed {rel_path}: {error}"));
            return SnapshotUploadResult {
                outcome: SnapshotUploadOutcome::Error,
                bytes_uploaded: 0,
                blob_upload_duration_ms: self.elapsed_duration_ms(upload_started_at),
                failure_phase: Some(SnapshotFailurePhase::BlobUpload),
            };
        }

        self.runtime.set_manifest_entry(manifest, rel_path, entry);
        SnapshotUploadResult {
            outcome: SnapshotUploadOutcome::Uploaded,
            bytes_uploaded: size,
            blob_upload_duration_ms: self.elapsed_duration_ms(upload_started_at),
            failure_phase: None,
        }
    }

    pub fn run_vacuum_off_thread(&self, src_path: &Path, dest_path: &Path) -> Result<(), String> {
        let worker = spawn_box_store_vacuum_job(BoxStoreVacuumJob {
            src_path: src_path.to_path_buf(),
            dest_path: dest_path.to_path_buf(),
            busy_timeout_ms: DB_BUSY_TIMEOUT_MS,
        })
        .map_err(|error| format!("vacuum worker unavailable: {error}"))?;
        let result = worker
            .join()
            .map_err(|_| "vacuum worker panicked before completing".to_string())?;
        if result.ok {
            Ok(())
        } else {
            Err(result
                .message
                .unwrap_or_else(|| "vacuum worker failed without a diagnostic".to_string()))
        }
    }

    pub fn discard_snapshot_temp(&self, tmp_path: &Path, label: &str) {
        match fs::remove_file(tmp_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                (self.log)(&format!("temp cleanup failed {label}: {error}"));
            }
        }
    }
}

fn capture_error() -> SnapshotUploadResult {
    SnapshotUploadResult {
        outcome: SnapshotUploadOutcome::Error,
        bytes_uploaded: 0,
        blob_upload_duration_ms: 0,
        failure_phase: Some(SnapshotFailurePhase::Capture),
    }
}

fn manifest_file_matches(existing: Option<&BoxStoreManifestEntry>, sha: &str, size: u64) -> bool {
    match existing {
        Some(BoxStoreManifestEntry::File {
            sha: existing_sha,
            size: existing_size,
            ..
        })
        | Some(BoxStoreManifestEntry::LegacyFile {
            sha: existing_sha,
            size: existing_size,
        }) => existing_sha == sha && *existing_size == size,
        _ => false,
    }
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex_lower(&digest.finalize()))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod internal_tests {
    use super::*;

    #[test]
    fn file_matching_ignores_mode_like_the_frozen_service() {
        let entry = BoxStoreManifestEntry::File {
            sha: "abc".into(),
            size: 3,
            mode: 0o600,
        };
        assert!(manifest_file_matches(Some(&entry), "abc", 3));
        assert!(!manifest_file_matches(Some(&entry), "def", 3));
    }
}
