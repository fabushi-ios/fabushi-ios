use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CONVERSATION_BLOB_MIGRATION_UNSTARTED: i64 = 0;
pub const CONVERSATION_BLOB_ADOPTION_COMPLETE: i64 = 1;
pub const CONVERSATION_BLOB_RECOVERY_REBUILT: i64 = 2;

const RECOVERY_RETRY_DELAY_MS: u64 = 50;

const CONVERSATION_BLOB_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS blobs (
  id TEXT PRIMARY KEY,
  data BLOB NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS checkpoint_roots (
  id TEXT PRIMARY KEY,
  written_at_ms INTEGER NOT NULL
) STRICT;
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationBlobMigrationState {
    Unstarted,
    AdoptionComplete,
    RecoveryRebuilt,
    Unknown,
}

impl ConversationBlobMigrationState {
    fn from_user_version(value: i64) -> Self {
        match value {
            CONVERSATION_BLOB_MIGRATION_UNSTARTED => Self::Unstarted,
            CONVERSATION_BLOB_ADOPTION_COMPLETE => Self::AdoptionComplete,
            CONVERSATION_BLOB_RECOVERY_REBUILT => Self::RecoveryRebuilt,
            _ => Self::Unknown,
        }
    }

    fn user_version(self) -> i64 {
        match self {
            Self::Unstarted => CONVERSATION_BLOB_MIGRATION_UNSTARTED,
            Self::AdoptionComplete => CONVERSATION_BLOB_ADOPTION_COMPLETE,
            Self::RecoveryRebuilt => CONVERSATION_BLOB_RECOVERY_REBUILT,
            Self::Unknown => -1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationBlobRecoveryError {
    pub code: &'static str,
    pub message: String,
}

impl ConversationBlobRecoveryError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConversationBlobRecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ConversationBlobRecoveryError {}

impl From<rusqlite::Error> for ConversationBlobRecoveryError {
    fn from(value: rusqlite::Error) -> Self {
        Self::new("SAND_BLOB_DB_ERROR", value.to_string())
    }
}

impl From<std::io::Error> for ConversationBlobRecoveryError {
    fn from(value: std::io::Error) -> Self {
        Self::new("SAND_BLOB_IO_ERROR", value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobIndexEntry {
    pub id: String,
    pub len: u64,
}

#[derive(Debug)]
pub struct ConversationBlobDb {
    path: PathBuf,
    conn: Connection,
}

pub fn read_conversation_blob_migration_state(
    conn: &Connection,
) -> ConversationBlobMigrationState {
    let version = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .unwrap_or(-1);
    ConversationBlobMigrationState::from_user_version(version)
}

pub fn run_quick_check(conn: &Connection) -> Result<bool, ConversationBlobRecoveryError> {
    run_quick_check_raw(conn).map_err(|error| map_sqlite_error("quick_check", error))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationBlobRecoveryOutcome {
    Recovered,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationBlobRecoveryInfo {
    pub agent_id: String,
    pub outcome: ConversationBlobRecoveryOutcome,
    pub quarantine_path: Option<PathBuf>,
    pub salvaged_blobs: usize,
    pub salvaged_checkpoint_roots: usize,
}

pub struct ConversationBlobDbOptions<'a> {
    pub db_path: &'a Path,
    pub busy_timeout_ms: u64,
    pub agent_id: &'a str,
    pub log: Option<&'a dyn Fn(&str)>,
    pub on_recovery: Option<&'a dyn Fn(&ConversationBlobRecoveryInfo)>,
}

impl<'a> ConversationBlobDbOptions<'a> {
    pub fn new(db_path: &'a Path, busy_timeout_ms: u64) -> Self {
        Self {
            db_path,
            busy_timeout_ms,
            agent_id: "unknown",
            log: None,
            on_recovery: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbHealth {
    Healthy,
    Corrupt,
    Unavailable,
}

#[derive(Debug, Clone)]
struct PendingConversationBlobRecovery {
    pending_path: PathBuf,
    quarantine_path: PathBuf,
}

fn sqlite_primary_code(error: &rusqlite::Error) -> Option<i32> {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) => Some(failure.extended_code & 0xff),
        _ => None,
    }
}

fn is_sqlite_corrupt_error(error: &rusqlite::Error) -> bool {
    matches!(sqlite_primary_code(error), Some(11) | Some(26))
}

fn is_sqlite_io_error(error: &rusqlite::Error) -> bool {
    sqlite_primary_code(error) == Some(10)
}

fn is_sqlite_cant_open_error(error: &rusqlite::Error) -> bool {
    sqlite_primary_code(error) == Some(14)
}

fn map_sqlite_error(context: &str, error: rusqlite::Error) -> ConversationBlobRecoveryError {
    if is_sqlite_corrupt_error(&error) {
        ConversationBlobRecoveryError::new(
            "SAND_BLOB_RECOVERY_SOURCE_CORRUPT",
            format!("{context}: {error}"),
        )
    } else if is_sqlite_io_error(&error) || is_sqlite_cant_open_error(&error) {
        ConversationBlobRecoveryError::new(
            "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE",
            format!("{context}: {error}"),
        )
    } else {
        ConversationBlobRecoveryError::new("SAND_BLOB_DB_ERROR", format!("{context}: {error}"))
    }
}

fn run_quick_check_raw(conn: &Connection) -> Result<bool, rusqlite::Error> {
    let value = conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
    Ok(value == "ok")
}

fn configure_connection_raw(conn: &Connection, busy_timeout_ms: u64) -> Result<(), rusqlite::Error> {
    conn.busy_timeout(Duration::from_millis(busy_timeout_ms))?;
    let _ = conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;");
    conn.execute_batch(CONVERSATION_BLOB_SCHEMA)?;
    Ok(())
}

pub fn open_configured_conversation_blob_db(
    path: &Path,
    busy_timeout_ms: u64,
) -> Result<Connection, ConversationBlobRecoveryError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)
        .map_err(|error| map_sqlite_error(&format!("cannot open {}", path.display()), error))?;
    configure_connection_raw(&conn, busy_timeout_ms)
        .map_err(|error| map_sqlite_error(&format!("cannot configure {}", path.display()), error))?;
    Ok(conn)
}

fn recovery_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn recovery_attempts(busy_timeout_ms: u64) -> usize {
    busy_timeout_ms
        .div_ceil(RECOVERY_RETRY_DELAY_MS)
        .max(1)
        .min(64) as usize
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), suffix))
}

fn remove_file_if_exists(path: &Path) -> Result<(), ConversationBlobRecoveryError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_sqlite_sidecars(path: &Path) {
    let _ = remove_file_if_exists(&path_with_suffix(path, "-wal"));
    let _ = remove_file_if_exists(&path_with_suffix(path, "-shm"));
}

fn remove_sqlite_db(path: &Path, attempts: usize) -> Result<(), ConversationBlobRecoveryError> {
    remove_sqlite_sidecars(path);
    for attempt in 0..attempts.max(1) {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt + 1 < attempts.max(1) => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(RECOVERY_RETRY_DELAY_MS));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn get_health(conn: &Connection) -> DbHealth {
    match run_quick_check_raw(conn) {
        Ok(true) => DbHealth::Healthy,
        Ok(false) => DbHealth::Corrupt,
        Err(error) if is_sqlite_corrupt_error(&error) => DbHealth::Corrupt,
        Err(_) => DbHealth::Unavailable,
    }
}

fn get_health_on_disk(
    path: &Path,
    busy_timeout_ms: u64,
) -> Result<DbHealth, ConversationBlobRecoveryError> {
    for attempt in 0..2 {
        let result = (|| -> Result<DbHealth, rusqlite::Error> {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            conn.busy_timeout(Duration::from_millis(busy_timeout_ms))?;
            Ok(get_health(&conn))
        })();
        match result {
            Ok(health) => return Ok(health),
            Err(error)
                if attempt == 0
                    && (is_sqlite_io_error(&error) || is_sqlite_cant_open_error(&error)) =>
            {
                remove_sqlite_sidecars(path);
            }
            Err(error) if is_sqlite_corrupt_error(&error) => return Ok(DbHealth::Corrupt),
            Err(_) => return Ok(DbHealth::Unavailable),
        }
    }
    Ok(DbHealth::Unavailable)
}

fn quarantine_path_for_marker(db_path: &Path, marker_name: &str) -> PathBuf {
    let db_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if marker_name == format!("{db_name}.pending") {
        return db_path.to_path_buf();
    }
    let suffix = if marker_name.ends_with(".intent") {
        ".intent"
    } else {
        ".pending"
    };
    let base = marker_name.strip_suffix(suffix).unwrap_or(marker_name);
    db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(base)
}

fn find_pending_recovery(
    db_path: &Path,
) -> Result<Option<PendingConversationBlobRecovery>, ConversationBlobRecoveryError> {
    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let db_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let prefix = format!("{db_name}.corrupt-");
    let in_place_marker = format!("{db_name}.pending");
    let mut names = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    let mut marker_names = names
        .iter()
        .filter(|name| {
            name.starts_with(&prefix)
                && (name.ends_with(".intent") || name.ends_with(".pending"))
        })
        .cloned()
        .collect::<Vec<_>>();
    marker_names.sort();
    if names.iter().any(|name| name == &in_place_marker) {
        marker_names.insert(0, in_place_marker.clone());
    }
    let selected = marker_names.last().cloned();
    let active_replacement = selected
        .as_deref()
        .map(|name| path_with_suffix(&quarantine_path_for_marker(db_path, name), ".replacement"));

    for name in names.drain(..) {
        let is_replacement = (name.starts_with(&prefix)
            || name == format!("{db_name}.replacement"))
            && name.ends_with(".replacement");
        if !is_replacement {
            continue;
        }
        let replacement = dir.join(&name);
        if active_replacement.as_ref() != Some(&replacement) {
            remove_sqlite_db(&replacement, 1)?;
        }
    }

    let Some(selected) = selected else {
        return Ok(None);
    };
    for stale in marker_names.iter().take(marker_names.len().saturating_sub(1)) {
        remove_file_if_exists(&dir.join(stale))?;
        remove_sqlite_db(
            &path_with_suffix(&quarantine_path_for_marker(db_path, stale), ".replacement"),
            1,
        )?;
    }
    Ok(Some(PendingConversationBlobRecovery {
        pending_path: dir.join(&selected),
        quarantine_path: quarantine_path_for_marker(db_path, &selected),
    }))
}

fn open_recovery_target(
    options: &ConversationBlobDbOptions<'_>,
    target_path: &Path,
) -> Result<Connection, ConversationBlobRecoveryError> {
    let attempts = recovery_attempts(options.busy_timeout_ms);
    let mut retried_without_sidecars = false;
    for _ in 0..3 {
        match open_configured_conversation_blob_db(target_path, options.busy_timeout_ms) {
            Ok(conn) => match get_health(&conn) {
                DbHealth::Healthy => return Ok(conn),
                DbHealth::Unavailable => {
                    return Err(ConversationBlobRecoveryError::new(
                        "SAND_BLOB_RECOVERY_TARGET_UNHEALTHY",
                        "conversation blob recovery target is temporarily unavailable",
                    ));
                }
                DbHealth::Corrupt => {
                    drop(conn);
                    remove_sqlite_db(target_path, attempts)?;
                }
            },
            Err(error)
                if error.code == "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE"
                    && !retried_without_sidecars =>
            {
                retried_without_sidecars = true;
                remove_sqlite_sidecars(target_path);
            }
            Err(error)
                if error.code == "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE"
                    || error.code == "SAND_BLOB_RECOVERY_SOURCE_CORRUPT" =>
            {
                remove_sqlite_db(target_path, attempts)?;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ConversationBlobRecoveryError::new(
        "SAND_BLOB_RECOVERY_TARGET_UNHEALTHY",
        "failed to create a healthy conversation blob recovery target",
    ))
}

fn install_completed_replacement(
    options: &ConversationBlobDbOptions<'_>,
    replacement_path: &Path,
    pending_path: Option<&Path>,
) -> Result<Option<Connection>, ConversationBlobRecoveryError> {
    match get_health_on_disk(replacement_path, options.busy_timeout_ms)? {
        DbHealth::Unavailable => {
            return Err(ConversationBlobRecoveryError::new(
                "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE",
                "completed conversation blob replacement is temporarily unavailable",
            ));
        }
        DbHealth::Corrupt => return Ok(None),
        DbHealth::Healthy => {}
    }
    let replacement = open_configured_conversation_blob_db(
        replacement_path,
        options.busy_timeout_ms,
    )?;
    if !matches!(
        read_conversation_blob_migration_state(&replacement),
        ConversationBlobMigrationState::RecoveryRebuilt
            | ConversationBlobMigrationState::AdoptionComplete
    ) {
        return Ok(None);
    }
    replacement.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE;")?;
    drop(replacement);
    remove_sqlite_sidecars(replacement_path);
    remove_sqlite_db(options.db_path, recovery_attempts(options.busy_timeout_ms))?;
    fs::rename(replacement_path, options.db_path)?;
    let installed = open_configured_conversation_blob_db(options.db_path, options.busy_timeout_ms)?;
    if get_health(&installed) != DbHealth::Healthy {
        return Err(ConversationBlobRecoveryError::new(
            "SAND_BLOB_RECOVERY_INSTALL_UNHEALTHY",
            "installed completed conversation blob replacement is unhealthy",
        ));
    }
    if let Some(marker) = pending_path {
        remove_file_if_exists(marker)?;
    }
    Ok(Some(installed))
}

fn salvage_quarantine_rows(
    quarantine_path: &Path,
    fresh: &mut Connection,
) -> Result<(usize, usize), ConversationBlobRecoveryError> {
    let source = match Connection::open_with_flags(quarantine_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(source) => source,
        Err(error) if is_sqlite_corrupt_error(&error) => return Ok((0, 0)),
        Err(error) if is_sqlite_io_error(&error) || is_sqlite_cant_open_error(&error) => {
            return Err(ConversationBlobRecoveryError::new(
                "SAND_BLOB_RECOVERY_QUARANTINE_UNREADABLE",
                format!("quarantined conversation blob database is unavailable: {error}"),
            ));
        }
        Err(_) => return Ok((0, 0)),
    };

    let tx = fresh.transaction()?;
    let mut salvaged_blobs = 0usize;
    if let Ok(mut statement) = source.prepare("SELECT id, data FROM blobs") {
        if let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        }) {
            for row in rows.flatten() {
                if tx
                    .execute(
                        "INSERT OR IGNORE INTO blobs (id, data) VALUES (?1, ?2)",
                        params![row.0, row.1],
                    )
                    .is_ok()
                {
                    salvaged_blobs += 1;
                }
            }
        }
    }
    let mut salvaged_roots = 0usize;
    if let Ok(mut statement) = source.prepare("SELECT id, written_at_ms FROM checkpoint_roots") {
        if let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) {
            for row in rows.flatten() {
                if tx
                    .execute(
                        "INSERT OR IGNORE INTO checkpoint_roots (id, written_at_ms) VALUES (?1, ?2)",
                        params![row.0, row.1],
                    )
                    .is_ok()
                {
                    salvaged_roots += 1;
                }
            }
        }
    }
    tx.commit()?;
    Ok((salvaged_blobs, salvaged_roots))
}

fn finish_recovery(
    options: &ConversationBlobDbOptions<'_>,
    mut quarantine_path: Option<PathBuf>,
    pending_path: Option<PathBuf>,
) -> Result<Connection, ConversationBlobRecoveryError> {
    let attempts = recovery_attempts(options.busy_timeout_ms);
    let mut replacement_path =
        path_with_suffix(quarantine_path.as_deref().unwrap_or(options.db_path), ".replacement");

    if !options.db_path.exists() && replacement_path.exists() {
        if let Some(installed) = install_completed_replacement(
            options,
            &replacement_path,
            pending_path.as_deref(),
        )? {
            return Ok(installed);
        }
    }

    if options.db_path.exists() {
        match get_health_on_disk(options.db_path, options.busy_timeout_ms)? {
            DbHealth::Unavailable => {
                return Err(ConversationBlobRecoveryError::new(
                    "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE",
                    "conversation blob recovery source is temporarily unavailable",
                ));
            }
            DbHealth::Healthy => {
                let existing =
                    open_configured_conversation_blob_db(options.db_path, options.busy_timeout_ms)?;
                if let Some(marker) = pending_path.as_deref() {
                    remove_file_if_exists(marker)?;
                }
                if let Some(quarantine) = quarantine_path.as_deref() {
                    remove_sqlite_db(&path_with_suffix(quarantine, ".replacement"), 1)?;
                }
                return Ok(existing);
            }
            DbHealth::Corrupt => {}
        }

        if quarantine_path.as_deref() == Some(options.db_path) {
            // Source remains in place and will be salvaged read-only.
        } else if quarantine_path.as_ref().is_some_and(|path| !path.exists()) {
            let requested = quarantine_path.clone().expect("checked as some");
            match fs::rename(options.db_path, &requested) {
                Ok(()) => {
                    remove_sqlite_sidecars(options.db_path);
                    quarantine_path = Some(requested);
                }
                Err(_) => {
                    match fs::copy(options.db_path, &requested) {
                        Ok(_) => {
                            remove_sqlite_db(options.db_path, attempts)?;
                            quarantine_path = Some(requested);
                        }
                        Err(_) => {
                            quarantine_path = Some(options.db_path.to_path_buf());
                        }
                    }
                }
            }
            replacement_path =
                path_with_suffix(quarantine_path.as_deref().unwrap_or(options.db_path), ".replacement");
        } else {
            remove_sqlite_db(options.db_path, attempts)?;
        }
    }

    remove_sqlite_db(&replacement_path, 1)?;
    let mut fresh = open_recovery_target(options, &replacement_path)?;
    let (salvaged_blobs, salvaged_checkpoint_roots) = if let Some(quarantine) = quarantine_path.as_deref() {
        if quarantine.exists() {
            salvage_quarantine_rows(quarantine, &mut fresh)?
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };
    fresh.execute_batch(&format!(
        "PRAGMA user_version = {}; PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE;",
        CONVERSATION_BLOB_RECOVERY_REBUILT
    ))?;
    drop(fresh);
    remove_sqlite_sidecars(&replacement_path);
    remove_sqlite_db(options.db_path, attempts)?;
    fs::rename(&replacement_path, options.db_path)?;
    let installed = open_configured_conversation_blob_db(options.db_path, options.busy_timeout_ms)?;
    if get_health(&installed) != DbHealth::Healthy {
        return Err(ConversationBlobRecoveryError::new(
            "SAND_BLOB_RECOVERY_INSTALL_UNHEALTHY",
            "installed conversation blob recovery is unhealthy",
        ));
    }

    let info = ConversationBlobRecoveryInfo {
        agent_id: options.agent_id.to_string(),
        outcome: if salvaged_blobs > 0 || salvaged_checkpoint_roots > 0 {
            ConversationBlobRecoveryOutcome::Recovered
        } else {
            ConversationBlobRecoveryOutcome::Reset
        },
        quarantine_path: quarantine_path.clone(),
        salvaged_blobs,
        salvaged_checkpoint_roots,
    };
    if let Some(log) = options.log {
        log(&format!(
            "[agent-store-worker] conversation blob recovery: agent={} outcome={} salvaged_blobs={} salvaged_checkpoint_roots={}",
            options.agent_id,
            match info.outcome {
                ConversationBlobRecoveryOutcome::Recovered => "recovered",
                ConversationBlobRecoveryOutcome::Reset => "reset",
            },
            info.salvaged_blobs,
            info.salvaged_checkpoint_roots
        ));
    }
    if let Some(on_recovery) = options.on_recovery {
        on_recovery(&info);
    }
    if let Some(marker) = pending_path.as_deref() {
        remove_file_if_exists(marker)?;
    }
    Ok(installed)
}

fn recover_corrupt_database(
    options: &ConversationBlobDbOptions<'_>,
) -> Result<Connection, ConversationBlobRecoveryError> {
    let intended_quarantine =
        path_with_suffix(options.db_path, &format!(".corrupt-{}", recovery_stamp()));
    let intent_path = path_with_suffix(&intended_quarantine, ".intent");
    fs::write(&intent_path, b"")?;

    let quarantine_path = if options.db_path.exists() {
        match fs::rename(options.db_path, &intended_quarantine) {
            Ok(()) => {
                remove_sqlite_sidecars(options.db_path);
                intended_quarantine.clone()
            }
            Err(_) => match fs::copy(options.db_path, &intended_quarantine) {
                Ok(_) => {
                    remove_sqlite_db(
                        options.db_path,
                        recovery_attempts(options.busy_timeout_ms),
                    )?;
                    intended_quarantine.clone()
                }
                Err(_) => options.db_path.to_path_buf(),
            },
        }
    } else {
        intended_quarantine.clone()
    };
    let pending_path = path_with_suffix(&quarantine_path, ".pending");
    if intent_path != pending_path {
        fs::rename(&intent_path, &pending_path)?;
    }
    finish_recovery(options, Some(quarantine_path), Some(pending_path))
}

pub fn open_conversation_blob_db_with_options(
    options: &ConversationBlobDbOptions<'_>,
) -> Result<Connection, ConversationBlobRecoveryError> {
    if let Some(parent) = options.db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(pending) = find_pending_recovery(options.db_path)? {
        return finish_recovery(
            options,
            Some(pending.quarantine_path),
            Some(pending.pending_path),
        );
    }

    match open_configured_conversation_blob_db(options.db_path, options.busy_timeout_ms) {
        Ok(conn) => match get_health(&conn) {
            DbHealth::Healthy => Ok(conn),
            DbHealth::Corrupt => {
                drop(conn);
                recover_corrupt_database(options)
            }
            DbHealth::Unavailable => Err(ConversationBlobRecoveryError::new(
                "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE",
                "conversation blob database health check is temporarily unavailable",
            )),
        },
        Err(error) if error.code == "SAND_BLOB_RECOVERY_SOURCE_CORRUPT" => {
            recover_corrupt_database(options)
        }
        Err(error) if error.code == "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE" => {
            remove_sqlite_sidecars(options.db_path);
            match open_configured_conversation_blob_db(options.db_path, options.busy_timeout_ms) {
                Ok(conn) if get_health(&conn) == DbHealth::Healthy => Ok(conn),
                Ok(conn) if get_health(&conn) == DbHealth::Corrupt => {
                    drop(conn);
                    recover_corrupt_database(options)
                }
                Ok(_) => Err(error),
                Err(second) if second.code == "SAND_BLOB_RECOVERY_SOURCE_CORRUPT" => {
                    recover_corrupt_database(options)
                }
                Err(_) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

impl ConversationBlobDb {
    pub fn open(
        path: impl Into<PathBuf>,
        busy_timeout_ms: u64,
    ) -> Result<Self, ConversationBlobRecoveryError> {
        let path = path.into();
        Self::open_with_options(ConversationBlobDbOptions::new(&path, busy_timeout_ms))
    }

    pub fn open_with_options(
        options: ConversationBlobDbOptions<'_>,
    ) -> Result<Self, ConversationBlobRecoveryError> {
        let path = options.db_path.to_path_buf();
        let conn = open_conversation_blob_db_with_options(&options)?;
        Ok(Self { path, conn })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn quick_check(&self) -> Result<bool, ConversationBlobRecoveryError> {
        run_quick_check(&self.conn)
    }

    pub fn migration_state(&self) -> ConversationBlobMigrationState {
        read_conversation_blob_migration_state(&self.conn)
    }

    pub fn set_migration_state(
        &self,
        state: ConversationBlobMigrationState,
    ) -> Result<(), ConversationBlobRecoveryError> {
        if state == ConversationBlobMigrationState::Unknown {
            return Err(ConversationBlobRecoveryError::new(
                "SAND_BLOB_MIGRATION_STATE_INVALID",
                "cannot persist unknown migration state",
            ));
        }
        self.conn
            .execute_batch(&format!("PRAGMA user_version = {};", state.user_version()))?;
        Ok(())
    }

    pub fn get_blob_hex(
        &self,
        id: &str,
    ) -> Result<Option<Vec<u8>>, ConversationBlobRecoveryError> {
        Ok(self
            .conn
            .query_row("SELECT data FROM blobs WHERE id = ?1", [id], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .optional()?)
    }

    pub fn contains_blob_hex(&self, id: &str) -> Result<bool, ConversationBlobRecoveryError> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM blobs WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    pub fn set_blob_hex(
        &self,
        id: &str,
        data: &[u8],
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.conn.execute(
            "INSERT INTO blobs (id, data) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![id, data],
        )?;
        Ok(())
    }

    pub fn delete_blob_hex(&self, id: &str) -> Result<bool, ConversationBlobRecoveryError> {
        Ok(self.conn.execute("DELETE FROM blobs WHERE id = ?1", [id])? > 0)
    }

    pub fn clear_blobs(&self) -> Result<(), ConversationBlobRecoveryError> {
        self.conn.execute("DELETE FROM blobs", [])?;
        self.conn.execute("DELETE FROM checkpoint_roots", [])?;
        Ok(())
    }

    pub fn list_blob_index(&self) -> Result<Vec<BlobIndexEntry>, ConversationBlobRecoveryError> {
        let mut statement = self
            .conn
            .prepare("SELECT id, length(data) FROM blobs ORDER BY id")?;
        let rows = statement.query_map([], |row| {
            Ok(BlobIndexEntry {
                id: row.get(0)?,
                len: row.get::<_, i64>(1)?.max(0) as u64,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn mark_checkpoint_root(
        &self,
        id: &str,
        written_at_ms: u64,
    ) -> Result<(), ConversationBlobRecoveryError> {
        self.conn.execute(
            "INSERT INTO checkpoint_roots (id, written_at_ms) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET written_at_ms = excluded.written_at_ms",
            params![id, written_at_ms as i64],
        )?;
        Ok(())
    }

    pub fn latest_checkpoint_root(&self) -> Result<Option<String>, ConversationBlobRecoveryError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM checkpoint_roots ORDER BY written_at_ms DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?)
    }

    pub fn list_checkpoint_roots(&self) -> Result<Vec<String>, ConversationBlobRecoveryError> {
        let mut statement = self
            .conn
            .prepare("SELECT id FROM checkpoint_roots ORDER BY written_at_ms DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn unmark_checkpoint_root(&self, id: &str) -> Result<(), ConversationBlobRecoveryError> {
        self.conn
            .execute("DELETE FROM checkpoint_roots WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn checkpoint(&self) -> Result<(), ConversationBlobRecoveryError> {
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-ios-{name}-{}-{}",
            std::process::id(),
            recovery_stamp()
        ));
        fs::create_dir_all(&root).unwrap();
        root.join("conversation-blobs.sqlite")
    }

    #[test]
    fn resumes_intent_marker_after_crash_before_quarantine() {
        let path = temp_path("blob-recovery-intent");
        fs::write(&path, b"not-a-sqlite-database").unwrap();
        let quarantine = PathBuf::from(format!("{}.corrupt-intent", path.display()));
        let intent = path_with_suffix(&quarantine, ".intent");
        fs::write(&intent, b"").unwrap();

        let db = ConversationBlobDb::open(&path, 500).unwrap();
        assert!(db.quick_check().unwrap());
        assert_eq!(
            db.migration_state(),
            ConversationBlobMigrationState::RecoveryRebuilt
        );
        assert!(!intent.exists());
        assert!(quarantine.exists());
    }

    #[test]
    fn clears_stale_replacement_without_pending_marker() {
        let path = temp_path("blob-recovery-stale-replacement");
        {
            let db = ConversationBlobDb::open(&path, 500).unwrap();
            db.set_blob_hex("live", b"payload").unwrap();
        }
        let stale = PathBuf::from(format!("{}.corrupt-old.replacement", path.display()));
        fs::write(&stale, b"stale").unwrap();

        let reopened = ConversationBlobDb::open(&path, 500).unwrap();
        assert_eq!(
            reopened.get_blob_hex("live").unwrap().as_deref(),
            Some(b"payload".as_slice())
        );
        assert!(!stale.exists());
    }

    #[test]
    fn unavailable_source_is_not_quarantined_as_corruption() {
        let path = temp_path("blob-recovery-unavailable");
        fs::create_dir_all(&path).unwrap();

        let error = ConversationBlobDb::open(&path, 500).unwrap_err();
        assert_eq!(error.code, "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE");
        assert!(path.is_dir());
        let parent = path.parent().unwrap();
        let base = path.file_name().unwrap().to_string_lossy().to_string();
        let recovery_markers = fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(&format!("{base}.corrupt-")))
            .count();
        assert_eq!(recovery_markers, 0);
    }

    #[test]
    fn resumes_pending_recovery_and_installs_completed_replacement() {
        let path = temp_path("blob-recovery-resume");
        let quarantine = PathBuf::from(format!("{}.corrupt-resume", path.display()));
        let pending = path_with_suffix(&quarantine, ".pending");
        fs::write(&pending, b"").unwrap();

        let replacement = path_with_suffix(&quarantine, ".replacement");
        {
            let conn = open_configured_conversation_blob_db(&replacement, 500).unwrap();
            conn.execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                params!["abcd", b"replacement".as_slice()],
            )
            .unwrap();
            conn.execute_batch(&format!(
                "PRAGMA user_version = {}; PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode = DELETE;",
                CONVERSATION_BLOB_RECOVERY_REBUILT
            ))
            .unwrap();
        }

        let db = ConversationBlobDb::open(&path, 500).unwrap();
        assert_eq!(
            db.get_blob_hex("abcd").unwrap().as_deref(),
            Some(b"replacement".as_slice())
        );
        assert!(!pending.exists());
        assert!(!replacement.exists());
    }

    #[test]
    fn corrupt_source_recovers_through_intent_pending_replacement_state_machine() {
        let path = temp_path("blob-recovery-corrupt");
        fs::write(&path, b"not-a-sqlite-database").unwrap();
        let events = std::sync::Mutex::new(Vec::<ConversationBlobRecoveryInfo>::new());
        let callback = |info: &ConversationBlobRecoveryInfo| {
            events.lock().unwrap().push(info.clone());
        };
        let options = ConversationBlobDbOptions {
            db_path: &path,
            busy_timeout_ms: 500,
            agent_id: "agent-a",
            log: None,
            on_recovery: Some(&callback),
        };
        let db = ConversationBlobDb::open_with_options(options).unwrap();
        assert!(db.quick_check().unwrap());
        assert_eq!(
            db.migration_state(),
            ConversationBlobMigrationState::RecoveryRebuilt
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].agent_id, "agent-a");
        assert_eq!(events[0].outcome, ConversationBlobRecoveryOutcome::Reset);
        let parent = path.parent().unwrap();
        let base = path.file_name().unwrap().to_string_lossy().to_string();
        let markers = fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(&format!("{base}.corrupt-"))
                && (name.ends_with(".intent") || name.ends_with(".pending")))
            .count();
        assert_eq!(markers, 0);
    }

    #[test]
    fn opens_configured_database_and_persists_blob_and_root_metadata() {
        let path = temp_path("blob-db");
        {
            let db = ConversationBlobDb::open(&path, 500).unwrap();
            assert!(db.quick_check().unwrap());
            assert_eq!(
                db.migration_state(),
                ConversationBlobMigrationState::Unstarted
            );
            db.set_blob_hex("abcd", b"payload").unwrap();
            db.mark_checkpoint_root("abcd", 42).unwrap();
            db.set_migration_state(ConversationBlobMigrationState::AdoptionComplete)
                .unwrap();
        }
        let reopened = ConversationBlobDb::open(&path, 500).unwrap();
        assert_eq!(
            reopened.get_blob_hex("abcd").unwrap().as_deref(),
            Some(b"payload".as_slice())
        );
        assert_eq!(
            reopened.latest_checkpoint_root().unwrap().as_deref(),
            Some("abcd")
        );
        assert_eq!(
            reopened.migration_state(),
            ConversationBlobMigrationState::AdoptionComplete
        );
    }
}
