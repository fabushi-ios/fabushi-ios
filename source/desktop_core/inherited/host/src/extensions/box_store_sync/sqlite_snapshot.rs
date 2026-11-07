use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::storage::store_db::{DB_BUSY_TIMEOUT_MS, SQLITE_DB_SIDECAR_SUFFIXES};

pub const LOCKED_DB_COPY_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteSnapshotOperation {
    PrepareStaged,
    ReadSourceMain,
    WriteStagedMain,
    CopySourceSidecars,
    VerifySourceStability,
    OpenStaged,
    CheckpointStaged,
    SetStagedJournalMode,
    QuickCheckStaged,
    CloseStaged,
    CleanupStaged,
    VacuumInto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteSnapshotPathStage {
    SourceMain,
    StagedMain,
    StagedSidecars,
    SourceAndStagedSidecars,
    SourceOrStagedMain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteSnapshotFailure {
    pub operation: SqliteSnapshotOperation,
    pub path_stage: SqliteSnapshotPathStage,
    pub cause: String,
    pub error_class: String,
    pub errno: Option<String>,
    pub sqlite_code: Option<i32>,
}

#[derive(Debug)]
enum SnapshotError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    SourceChanged,
    QuickCheckFailed,
}

fn sqlite_code(error: &rusqlite::Error) -> Option<i32> {
    match error {
        rusqlite::Error::SqliteFailure(inner, _) => Some(inner.extended_code),
        _ => None,
    }
}

fn errno_name(error: &io::Error) -> Option<String> {
    if error.kind() == io::ErrorKind::StorageFull {
        return Some("ENOSPC".into());
    }
    error.raw_os_error().map(|code| format!("errno:{code}"))
}

fn classify_snapshot_error(
    error: &SnapshotError,
    operation: SqliteSnapshotOperation,
    path_stage: SqliteSnapshotPathStage,
) -> SqliteSnapshotFailure {
    let (sqlite_code, errno, cause, error_class) = match error {
        SnapshotError::Sqlite(error) => {
            let code = sqlite_code(error);
            let primary = code.map(|value| value & 0xff);
            let cause = match primary {
                Some(5 | 6) => "busy",
                Some(10) => "io",
                Some(11) => "corrupt",
                Some(14) => "cant_open",
                Some(_) => "sqlite",
                None => "error",
            };
            (code, None, cause.to_string(), "Error".to_string())
        }
        SnapshotError::Io(error) => (
            None,
            errno_name(error),
            if error.kind() == io::ErrorKind::StorageFull {
                "system"
            } else {
                "error"
            }
            .to_string(),
            "Error".to_string(),
        ),
        SnapshotError::SourceChanged => (
            None,
            None,
            "source_changed".to_string(),
            "none".to_string(),
        ),
        SnapshotError::QuickCheckFailed => (
            None,
            None,
            "quick_check_failed".to_string(),
            "none".to_string(),
        ),
    };

    let primary = sqlite_code.map(|value| value & 0xff);
    let capacity_failure =
        errno.as_deref() == Some("ENOSPC") || primary == Some(13);
    let mut resolved_path_stage = path_stage;
    if operation == SqliteSnapshotOperation::VacuumInto && capacity_failure {
        resolved_path_stage = SqliteSnapshotPathStage::StagedMain;
    } else if operation == SqliteSnapshotOperation::CopySourceSidecars && capacity_failure {
        resolved_path_stage = SqliteSnapshotPathStage::StagedSidecars;
    } else if operation == SqliteSnapshotOperation::VacuumInto
        && matches!(primary, Some(5 | 6 | 11))
    {
        resolved_path_stage = SqliteSnapshotPathStage::SourceMain;
    }

    SqliteSnapshotFailure {
        operation,
        path_stage: resolved_path_stage,
        cause,
        error_class,
        errno,
        sqlite_code,
    }
}


pub fn classify_sqlite_snapshot_io_failure(
    error: &io::Error,
    operation: SqliteSnapshotOperation,
    path_stage: SqliteSnapshotPathStage,
) -> SqliteSnapshotFailure {
    classify_snapshot_error(
        &SnapshotError::Io(io::Error::new(error.kind(), error.to_string())),
        operation,
        path_stage,
    )
}

pub fn classify_sqlite_snapshot_db_failure(
    error: &rusqlite::Error,
    operation: SqliteSnapshotOperation,
    path_stage: SqliteSnapshotPathStage,
) -> SqliteSnapshotFailure {
    let owned = match error {
        rusqlite::Error::SqliteFailure(inner, message) => {
            rusqlite::Error::SqliteFailure(*inner, message.clone())
        }
        other => rusqlite::Error::InvalidParameterName(other.to_string()),
    };
    classify_snapshot_error(
        &SnapshotError::Sqlite(owned),
        operation,
        path_stage,
    )
}

pub fn sqlite_vacuum_into(
    src_path: impl AsRef<Path>,
    dest_path: impl AsRef<Path>,
    busy_timeout_ms: u64,
) -> Result<(), rusqlite::Error> {
    let db = Connection::open_with_flags(
        src_path.as_ref(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    db.busy_timeout(Duration::from_millis(busy_timeout_ms))?;
    let escaped = dest_path
        .as_ref()
        .to_string_lossy()
        .replace('\'', "''");
    db.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
    Ok(())
}

pub fn sqlite_vacuum_into_default(
    src_path: impl AsRef<Path>,
    dest_path: impl AsRef<Path>,
) -> Result<(), rusqlite::Error> {
    sqlite_vacuum_into(src_path, dest_path, DB_BUSY_TIMEOUT_MS)
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), suffix))
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn cleanup_staged(dest_path: &Path, remove_main: bool) -> Result<(), SnapshotError> {
    for suffix in SQLITE_DB_SIDECAR_SUFFIXES {
        remove_if_exists(&sidecar_path(dest_path, suffix)).map_err(SnapshotError::Io)?;
    }
    if remove_main {
        remove_if_exists(dest_path).map_err(SnapshotError::Io)?;
    }
    Ok(())
}

pub fn copy_locked_sqlite_db<F>(
    src_path: impl AsRef<Path>,
    dest_path: impl AsRef<Path>,
    on_failure: F,
) -> bool
where
    F: FnMut(SqliteSnapshotFailure),
{
    copy_locked_sqlite_db_with_reader(src_path, dest_path, |path| fs::read(path), on_failure)
}

pub fn copy_locked_sqlite_db_with_reader<R, F>(
    src_path: impl AsRef<Path>,
    dest_path: impl AsRef<Path>,
    mut read_source: R,
    mut on_failure: F,
) -> bool
where
    R: FnMut(&Path) -> io::Result<Vec<u8>>,
    F: FnMut(SqliteSnapshotFailure),
{
    let src_path = src_path.as_ref();
    let dest_path = dest_path.as_ref();
    let mut last_failure = SqliteSnapshotFailure {
        operation: SqliteSnapshotOperation::PrepareStaged,
        path_stage: SqliteSnapshotPathStage::StagedMain,
        cause: "unknown".into(),
        error_class: "unknown".into(),
        errno: None,
        sqlite_code: None,
    };

    for _attempt in 1..=LOCKED_DB_COPY_ATTEMPTS {
        let mut verified = false;
        let mut attempt_failure: Option<SqliteSnapshotFailure> = None;

        let attempt = (|| -> Result<(), (SnapshotError, SqliteSnapshotOperation, SqliteSnapshotPathStage)> {
            cleanup_staged(dest_path, true).map_err(|error| (
                error,
                SqliteSnapshotOperation::PrepareStaged,
                SqliteSnapshotPathStage::StagedMain,
            ))?;

            let main_bytes = read_source(src_path).map_err(|error| (
                SnapshotError::Io(error),
                SqliteSnapshotOperation::ReadSourceMain,
                SqliteSnapshotPathStage::SourceMain,
            ))?;
            fs::write(dest_path, &main_bytes).map_err(|error| (
                SnapshotError::Io(error),
                SqliteSnapshotOperation::WriteStagedMain,
                SqliteSnapshotPathStage::StagedMain,
            ))?;

            for suffix in SQLITE_DB_SIDECAR_SUFFIXES {
                let source_sidecar = sidecar_path(src_path, suffix);
                if source_sidecar.exists() {
                    fs::copy(&source_sidecar, sidecar_path(dest_path, suffix)).map_err(|error| (
                        SnapshotError::Io(error),
                        SqliteSnapshotOperation::CopySourceSidecars,
                        SqliteSnapshotPathStage::SourceAndStagedSidecars,
                    ))?;
                }
            }

            let stable_bytes = read_source(src_path).map_err(|error| (
                SnapshotError::Io(error),
                SqliteSnapshotOperation::VerifySourceStability,
                SqliteSnapshotPathStage::SourceMain,
            ))?;
            if stable_bytes != main_bytes {
                return Err((
                    SnapshotError::SourceChanged,
                    SqliteSnapshotOperation::VerifySourceStability,
                    SqliteSnapshotPathStage::SourceMain,
                ));
            }

            let db = Connection::open(dest_path).map_err(|error| (
                SnapshotError::Sqlite(error),
                SqliteSnapshotOperation::OpenStaged,
                SqliteSnapshotPathStage::StagedMain,
            ))?;
            db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").map_err(|error| (
                SnapshotError::Sqlite(error),
                SqliteSnapshotOperation::CheckpointStaged,
                SqliteSnapshotPathStage::StagedMain,
            ))?;
            db.execute_batch("PRAGMA journal_mode=DELETE").map_err(|error| (
                SnapshotError::Sqlite(error),
                SqliteSnapshotOperation::SetStagedJournalMode,
                SqliteSnapshotPathStage::StagedMain,
            ))?;
            let quick_check: String = db
                .query_row("PRAGMA quick_check", [], |row| row.get(0))
                .map_err(|error| (
                    SnapshotError::Sqlite(error),
                    SqliteSnapshotOperation::QuickCheckStaged,
                    SqliteSnapshotPathStage::StagedMain,
                ))?;
            if quick_check != "ok" {
                return Err((
                    SnapshotError::QuickCheckFailed,
                    SqliteSnapshotOperation::QuickCheckStaged,
                    SqliteSnapshotPathStage::StagedMain,
                ));
            }
            db.close().map_err(|(_db, error)| (
                SnapshotError::Sqlite(error),
                SqliteSnapshotOperation::CloseStaged,
                SqliteSnapshotPathStage::StagedMain,
            ))?;
            Ok(())
        })();

        match attempt {
            Ok(()) => verified = true,
            Err((error, operation, path_stage)) => {
                attempt_failure = Some(classify_snapshot_error(&error, operation, path_stage));
            }
        }

        if let Err(error) = cleanup_staged(dest_path, !verified) {
            verified = false;
            attempt_failure.get_or_insert_with(|| {
                classify_snapshot_error(
                    &error,
                    SqliteSnapshotOperation::CleanupStaged,
                    SqliteSnapshotPathStage::SourceOrStagedMain,
                )
            });
        }

        if verified && attempt_failure.is_none() {
            return true;
        }
        if let Some(failure) = attempt_failure {
            last_failure = failure;
        }
    }

    on_failure(last_failure);
    false
}
