use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;

use super::box_store_transfer::CategoryTransferSummary;

pub const BOX_STORE_WRITER_LOCK_FILE_NAME: &str = "box-store-sync.lock";

#[derive(Debug)]
pub struct BoxStoreWriterLock {
    file: File,
    path: PathBuf,
}

impl BoxStoreWriterLock {
    pub fn try_acquire(lock_path: impl AsRef<Path>, window_id: &str) -> Result<Option<Self>, String> {
        let lock_path = lock_path.as_ref();
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create box-store writer lock parent: {error}"))?;
        }
        assert_no_symlink_in_path(lock_path)?;

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(lock_path)
            .map_err(|error| format!("open box-store writer lock: {error}"))?;

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(error) => return Err(format!("acquire box-store writer lock: {error}")),
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(lock_path, fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("secure box-store writer lock: {error}"))?;
        }

        let acquired_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let owner = serde_json::json!({
            "pid": std::process::id(),
            "windowId": window_id,
            "acquiredAt": acquired_at,
        });
        file.set_len(0)
            .map_err(|error| format!("truncate box-store writer lock: {error}"))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| format!("seek box-store writer lock: {error}"))?;
        file.write_all(owner.to_string().as_bytes())
            .map_err(|error| format!("write box-store writer lock owner: {error}"))?;
        file.flush()
            .map_err(|error| format!("flush box-store writer lock owner: {error}"))?;

        Ok(Some(Self {
            file,
            path: lock_path.to_path_buf(),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BoxStoreWriterLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn assert_no_symlink_in_path(path: &Path) -> Result<(), String> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "box-store writer lock path contains symlink: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "inspect box-store writer lock path {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

pub const DEFAULT_MAX_OBJECT_BYTES: u64 = 32 * 1024 * 1024 * 1024;
pub const DEFAULT_COPY_IN_CONCURRENCY: usize = 128;
pub const DEFAULT_SNAPSHOT_OUT_CONCURRENCY: usize = 8;
pub const LARGE_OBJECT_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;
pub const DOWNLOAD_IN_FLIGHT_BYTE_BUDGET: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreCycleSummary {
    pub ok: bool,
    pub reason: Option<String>,
    pub store_id: Option<String>,
    pub duration_ms: u64,
    pub categories: Vec<CategoryTransferSummary>,
    pub manifest_entries: usize,
    pub store_db_entries: usize,
    pub store_db_complete: bool,
    pub agent_dir_entries: usize,
    pub total_files_uploaded: usize,
    pub total_bytes_uploaded: u64,
    pub total_failures: usize,
    pub metadata_failures: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreFlushEvaluation {
    pub ok: bool,
    pub manifest_entries: usize,
    pub store_db_entries: usize,
    pub agent_dir_entries: usize,
    pub store_db_complete: bool,
    pub files_uploaded: usize,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxStoreCycleTelemetry {
    pub level: &'static str,
    pub metadata: BTreeMap<String, String>,
}

pub fn evaluate_box_store_flush(
    summary: Option<&BoxStoreCycleSummary>,
    session_category_name: &str,
) -> BoxStoreFlushEvaluation {
    let Some(summary) = summary else {
        return failed("error", None);
    };
    if !summary.ok {
        return failed(summary.reason.as_deref().unwrap_or("error"), Some(summary));
    }
    if summary
        .categories
        .iter()
        .any(|category| category.name == "store.db")
        && !summary.store_db_complete
    {
        return failed("store-db-incomplete", Some(summary));
    }
    if summary.metadata_failures > 0 {
        return failed("filesystem-metadata-incomplete", Some(summary));
    }
    if summary
        .categories
        .iter()
        .find(|category| category.name == session_category_name)
        .is_some_and(|category| category.failures > 0)
    {
        return failed("chrome-session-stage-failed", Some(summary));
    }
    BoxStoreFlushEvaluation {
        ok: true,
        manifest_entries: summary.manifest_entries,
        store_db_entries: summary.store_db_entries,
        agent_dir_entries: summary.agent_dir_entries,
        store_db_complete: summary.store_db_complete,
        files_uploaded: summary.total_files_uploaded,
        reason: String::new(),
    }
}

fn failed(reason: &str, summary: Option<&BoxStoreCycleSummary>) -> BoxStoreFlushEvaluation {
    BoxStoreFlushEvaluation {
        ok: false,
        manifest_entries: summary.map_or(0, |value| value.manifest_entries),
        store_db_entries: summary.map_or(0, |value| value.store_db_entries),
        agent_dir_entries: summary.map_or(0, |value| value.agent_dir_entries),
        store_db_complete: summary.is_some_and(|value| value.store_db_complete),
        files_uploaded: summary.map_or(0, |value| value.total_files_uploaded),
        reason: reason.to_string(),
    }
}

pub fn box_store_sync_cycle_telemetry(
    summary: &BoxStoreCycleSummary,
) -> Option<BoxStoreCycleTelemetry> {
    if !summary.ok {
        if summary.reason.as_deref() == Some("in-flight") {
            return None;
        }
        let metadata = BTreeMap::from([
            ("ok".to_string(), "false".to_string()),
            (
                "reason".to_string(),
                if summary.reason.as_deref() == Some("locked") {
                    "locked".to_string()
                } else {
                    "error".to_string()
                },
            ),
            ("duration_ms".to_string(), summary.duration_ms.to_string()),
            (
                "manifest_entries".to_string(),
                summary.manifest_entries.to_string(),
            ),
            (
                "store_db_entries".to_string(),
                summary.store_db_entries.to_string(),
            ),
        ]);
        return Some(BoxStoreCycleTelemetry {
            level: "warn",
            metadata,
        });
    }

    let excluded_files = summary
        .categories
        .iter()
        .map(|category| category.excluded_files)
        .sum::<usize>();
    let excluded_bytes = summary
        .categories
        .iter()
        .map(|category| category.excluded_bytes)
        .sum::<u64>();
    let pruned_dirs = summary
        .categories
        .iter()
        .map(|category| category.pruned_dirs)
        .sum::<usize>();
    let mut metadata = BTreeMap::from([
        ("ok".to_string(), "true".to_string()),
        ("duration_ms".to_string(), summary.duration_ms.to_string()),
        (
            "files_uploaded".to_string(),
            summary.total_files_uploaded.to_string(),
        ),
        (
            "bytes_uploaded".to_string(),
            summary.total_bytes_uploaded.to_string(),
        ),
        (
            "manifest_entries".to_string(),
            summary.manifest_entries.to_string(),
        ),
        ("excluded_files".to_string(), excluded_files.to_string()),
        ("excluded_bytes".to_string(), excluded_bytes.to_string()),
        ("pruned_dirs".to_string(), pruned_dirs.to_string()),
        (
            "store_db_entries".to_string(),
            summary.store_db_entries.to_string(),
        ),
        (
            "store_id".to_string(),
            summary.store_id.clone().unwrap_or_default(),
        ),
        ("failures".to_string(), summary.total_failures.to_string()),
    ]);
    for category in &summary.categories {
        metadata.insert(
            format!("{}_uploaded", category.name),
            category.files_uploaded.to_string(),
        );
        metadata.insert(
            format!("{}_bytes", category.name),
            category.bytes_uploaded.to_string(),
        );
    }
    Some(BoxStoreCycleTelemetry {
        level: "info",
        metadata,
    })
}

pub fn telemetry_level(summary: &BoxStoreCycleSummary) -> Option<&'static str> {
    box_store_sync_cycle_telemetry(summary).map(|telemetry| telemetry.level)
}
