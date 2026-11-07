use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use uuid::Uuid;

use super::sqlite_snapshot::{
    SqliteSnapshotFailure, SqliteSnapshotOperation, SqliteSnapshotPathStage,
    classify_sqlite_snapshot_db_failure, classify_sqlite_snapshot_io_failure,
    copy_locked_sqlite_db, sqlite_vacuum_into,
};

pub const CHROME_SESSION_CATEGORY_NAME: &str = "chrome-session";
pub const CHROME_SESSION_DB_DIR: &str = "/home/box/chrome-profile/Default";
pub const CHROME_SESSION_DB_REL_DIR: &str = "home/box/chrome-profile/Default";
pub const CHROME_SESSION_DB_NAMES: &[&str] =
    &["Cookies", "Login Data", "Login Data For Account", "Web Data"];
pub const CHROME_AUTH_STATE_REL_DIRS: &[&str] =
    &["Local Storage", "Session Storage", "IndexedDB", "Service Worker"];
pub const CHROME_AUTH_STATE_CACHE_EXCLUDE_NAMES: &[&str] = &["CacheStorage", "ScriptCache"];
pub const CHROME_SESSION_STAGE_MAX_ATTEMPTS: usize = 1;
pub const CHROME_SESSION_STAGE_RETRY_DELAY_MS: u64 = 150;
pub const CHROME_SESSION_STAGE_VACUUM_BUSY_TIMEOUT_MS: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedChromeFile {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeStageFailure {
    pub db: String,
    pub phase: String,
    pub snapshot: SqliteSnapshotFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeStageReport {
    pub staged: usize,
    pub skipped: usize,
    pub skipped_db_names: Vec<String>,
    pub error_class: Option<String>,
    pub failure: Option<ChromeStageFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeSessionStageTelemetry {
    pub level: &'static str,
    pub metadata: BTreeMap<String, String>,
}

fn sqlite_snapshot_operation_name(operation: SqliteSnapshotOperation) -> &'static str {
    match operation {
        SqliteSnapshotOperation::PrepareStaged => "prepare_staged",
        SqliteSnapshotOperation::ReadSourceMain => "read_source_main",
        SqliteSnapshotOperation::WriteStagedMain => "write_staged_main",
        SqliteSnapshotOperation::CopySourceSidecars => "copy_source_sidecars",
        SqliteSnapshotOperation::VerifySourceStability => "verify_source_stability",
        SqliteSnapshotOperation::OpenStaged => "open_staged",
        SqliteSnapshotOperation::CheckpointStaged => "checkpoint_staged",
        SqliteSnapshotOperation::SetStagedJournalMode => "set_staged_journal_mode",
        SqliteSnapshotOperation::QuickCheckStaged => "quick_check_staged",
        SqliteSnapshotOperation::CloseStaged => "close_staged",
        SqliteSnapshotOperation::CleanupStaged => "cleanup_staged",
        SqliteSnapshotOperation::VacuumInto => "vacuum_into",
    }
}

fn sqlite_snapshot_path_stage_name(path_stage: SqliteSnapshotPathStage) -> &'static str {
    match path_stage {
        SqliteSnapshotPathStage::SourceMain => "source_main",
        SqliteSnapshotPathStage::StagedMain => "staged_main",
        SqliteSnapshotPathStage::StagedSidecars => "staged_sidecars",
        SqliteSnapshotPathStage::SourceAndStagedSidecars => "source_and_staged_sidecars",
        SqliteSnapshotPathStage::SourceOrStagedMain => "source_or_staged_main",
    }
}

pub fn chrome_session_stage_telemetry(report: &ChromeStageReport) -> ChromeSessionStageTelemetry {
    let mut metadata = BTreeMap::from([
        ("staged".into(), report.staged.to_string()),
        ("skipped".into(), report.skipped.to_string()),
    ]);
    if !report.skipped_db_names.is_empty() {
        metadata.insert("skipped_dbs".into(), report.skipped_db_names.join(","));
    }
    if let Some(error_class) = report.error_class.as_ref() {
        metadata.insert("error_class".into(), error_class.clone());
    }
    if let Some(failure) = report.failure.as_ref() {
        metadata.insert("failure_db".into(), failure.db.clone());
        metadata.insert("failure_phase".into(), failure.phase.clone());
        metadata.insert(
            "failure_operation".into(),
            sqlite_snapshot_operation_name(failure.snapshot.operation).into(),
        );
        metadata.insert(
            "failure_path_stage".into(),
            sqlite_snapshot_path_stage_name(failure.snapshot.path_stage).into(),
        );
        metadata.insert("failure_cause".into(), failure.snapshot.cause.clone());
        if let Some(errno) = failure.snapshot.errno.as_ref() {
            metadata.insert("errno".into(), errno.clone());
        }
        if let Some(sqlite_code) = failure.snapshot.sqlite_code {
            metadata.insert("sqlite_code".into(), sqlite_code.to_string());
        }
    }
    ChromeSessionStageTelemetry {
        level: if report.skipped > 0 { "warn" } else { "info" },
        metadata,
    }
}

#[derive(Debug)]
pub struct StagedChromeSession {
    pub files: Vec<StagedChromeFile>,
    pub skipped: usize,
    staging_dir: PathBuf,
}

impl StagedChromeSession {
    pub fn cleanup(self) -> std::io::Result<()> {
        match fs::remove_dir_all(self.staging_dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeStageRetryPolicy {
    pub max_attempts: usize,
    pub delay_ms: u64,
}

impl Default for ChromeStageRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: CHROME_SESSION_STAGE_MAX_ATTEMPTS,
            delay_ms: CHROME_SESSION_STAGE_RETRY_DELAY_MS,
        }
    }
}

pub fn is_chrome_session_stage_retryable_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("busy") || message.contains("locked")
}

pub fn is_chrome_session_stage_retryable_failure(failure: &SqliteSnapshotFailure) -> bool {
    failure.cause == "busy"
        || is_chrome_session_stage_retryable_message(&failure.error_class)
}

fn temporary_stage_dir() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "sand-chrome-session-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir(&path)?;
    Ok(path)
}

fn file_mode(path: &Path) -> std::io::Result<u32> {
    let metadata = fs::metadata(path)?;
    #[cfg(unix)]
    {
        Ok(metadata.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        Ok(if metadata.permissions().readonly() { 0o444 } else { 0o666 })
    }
}

fn vacuum_with_retry<V, L>(
    src: &Path,
    dest: &Path,
    policy: ChromeStageRetryPolicy,
    vacuum: &mut V,
    log: &mut L,
) -> Result<(), SqliteSnapshotFailure>
where
    V: FnMut(&Path, &Path) -> Result<(), SqliteSnapshotFailure>,
    L: FnMut(String),
{
    let attempts = policy.max_attempts.max(1);
    for attempt in 1..=attempts {
        if attempt > 1 {
            if let Err(error) = fs::remove_file(dest) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    log(format!("retry pre-clean failed: {error}"));
                }
            }
        }
        match vacuum(src, dest) {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < attempts && is_chrome_session_stage_retryable_failure(&error) =>
            {
                if policy.delay_ms > 0 {
                    thread::sleep(Duration::from_millis(policy.delay_ms));
                }
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("at least one chrome stage attempt is always executed")
}

pub fn stage_box_chrome_session() -> std::io::Result<StagedChromeSession> {
    stage_box_chrome_session_with_report(|_| {})
}

pub fn stage_box_chrome_session_with_report<R>(
    report: R,
) -> std::io::Result<StagedChromeSession>
where
    R: FnMut(ChromeStageReport),
{
    stage_box_chrome_session_with(
        Path::new(CHROME_SESSION_DB_DIR),
        CHROME_SESSION_DB_REL_DIR,
        CHROME_SESSION_DB_NAMES,
        ChromeStageRetryPolicy::default(),
        |src, dest| {
            sqlite_vacuum_into(src, dest, CHROME_SESSION_STAGE_VACUUM_BUSY_TIMEOUT_MS)
                .map_err(|error| {
                    classify_sqlite_snapshot_db_failure(
                        &error,
                        SqliteSnapshotOperation::VacuumInto,
                        SqliteSnapshotPathStage::SourceOrStagedMain,
                    )
                })
        },
        |src, dest| {
            let mut failure = None;
            if copy_locked_sqlite_db(src, dest, |value| failure = Some(value)) {
                Ok(())
            } else {
                Err(failure.unwrap_or(SqliteSnapshotFailure {
                    operation: SqliteSnapshotOperation::PrepareStaged,
                    path_stage: SqliteSnapshotPathStage::SourceOrStagedMain,
                    cause: "unknown".into(),
                    error_class: "unknown".into(),
                    errno: None,
                    sqlite_code: None,
                }))
            }
        },
        |_| {},
        report,
    )
}

pub fn stage_box_chrome_session_with<V, C, L, R>(
    session_db_dir: &Path,
    session_db_rel_dir: &str,
    session_db_names: &[&str],
    retry_policy: ChromeStageRetryPolicy,
    mut vacuum: V,
    mut copy_locked: C,
    mut log: L,
    mut report: R,
) -> std::io::Result<StagedChromeSession>
where
    V: FnMut(&Path, &Path) -> Result<(), SqliteSnapshotFailure>,
    C: FnMut(&Path, &Path) -> Result<(), SqliteSnapshotFailure>,
    L: FnMut(String),
    R: FnMut(ChromeStageReport),
{
    let dir = temporary_stage_dir()?;
    let mut files = Vec::new();
    let mut skipped_db_names = Vec::new();
    let mut copied_db_names = Vec::new();
    let mut last_error_class = None;
    let mut last_failure = None;

    for name in session_db_names {
        let src = session_db_dir.join(name);
        if !src.exists() {
            continue;
        }
        let dest = dir.join(name);
        let mode = match file_mode(&src) {
            Ok(mode) => mode,
            Err(error) => {
                skipped_db_names.push((*name).to_string());
                let failure = classify_sqlite_snapshot_io_failure(
                    &error,
                    SqliteSnapshotOperation::ReadSourceMain,
                    SqliteSnapshotPathStage::SourceMain,
                );
                last_error_class = Some(failure.error_class.clone());
                log(format!(
                    "stage skipped {name} because its mode could not be read: {error}"
                ));
                continue;
            }
        };

        match vacuum_with_retry(
            &src,
            &dest,
            retry_policy,
            &mut vacuum,
            &mut log,
        ) {
            Ok(()) => files.push(StagedChromeFile {
                rel_path: format!("{session_db_rel_dir}/{name}"),
                abs_path: dest,
                mode,
            }),
            Err(vacuum_failure) => {
                let busy = is_chrome_session_stage_retryable_failure(&vacuum_failure);
                if busy {
                    match copy_locked(&src, &dest) {
                        Ok(()) => {
                            files.push(StagedChromeFile {
                                rel_path: format!("{session_db_rel_dir}/{name}"),
                                abs_path: dest,
                                mode,
                            });
                            copied_db_names.push((*name).to_string());
                            continue;
                        }
                        Err(raw_failure) => {
                            last_error_class = Some(raw_failure.error_class.clone());
                            last_failure = Some(ChromeStageFailure {
                                db: (*name).to_string(),
                                phase: "raw_copy".into(),
                                snapshot: raw_failure,
                            });
                        }
                    }
                } else {
                    last_error_class = Some(vacuum_failure.error_class.clone());
                    last_failure = Some(ChromeStageFailure {
                        db: (*name).to_string(),
                        phase: "vacuum".into(),
                        snapshot: vacuum_failure,
                    });
                }

                skipped_db_names.push((*name).to_string());
                log(format!("stage skipped {name} after retries"));
            }
        }
    }

    if !copied_db_names.is_empty() {
        log(format!(
            "staged {} exclusively-locked db(s) via raw-copy fallback: {}",
            copied_db_names.len(),
            copied_db_names.join(", ")
        ));
    }

    if !files.is_empty() || !skipped_db_names.is_empty() {
        report(ChromeStageReport {
            staged: files.len(),
            skipped: skipped_db_names.len(),
            skipped_db_names: skipped_db_names.clone(),
            error_class: last_error_class,
            failure: last_failure,
        });
    }

    Ok(StagedChromeSession {
        files,
        skipped: skipped_db_names.len(),
        staging_dir: dir,
    })
}
