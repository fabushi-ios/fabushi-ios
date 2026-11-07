use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Map, Value};

use crate::storage::sqlite_busy::{is_sqlite_corrupt_error, is_sqlite_io_error};
use crate::storage::sqlite_recovery::{
    copy_salvageable_sqlite_rows, open_sqlite_for_salvage, quarantine_corrupt_sqlite_db,
};
use crate::storage::store_db::DB_BUSY_TIMEOUT_MS;
use crate::transcript_mutation_events::publish_transcript_mutation;

use super::agent_db_schema::AGENT_DB_SCHEMA;
use super::session_diagnostics::{SessionDiagnostic, report_session_diagnostic};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SalvageCounts {
    pub kv: usize,
    pub blobs: usize,
    pub transcript: usize,
    pub topics: usize,
    pub members: usize,
    pub pins: usize,
}

impl SalvageCounts {
    pub fn total(self) -> usize {
        self.kv + self.blobs + self.transcript + self.topics + self.members + self.pins
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorruptionRecoveredEvent {
    pub outcome: &'static str,
    pub quarantine_path: Option<PathBuf>,
    pub salvaged: SalvageCounts,
}

pub type CorruptionRecoveredCallback =
    Arc<dyn Fn(&CorruptionRecoveredEvent) + Send + Sync + 'static>;

#[derive(Clone)]
pub struct DbRecoveryOptions {
    pub recover_on_corruption: bool,
    pub reject_if_corrupt: bool,
    pub verify_integrity_on_open: bool,
    pub busy_timeout_ms: u64,
    pub on_corruption_recovered: Option<CorruptionRecoveredCallback>,
}

impl Default for DbRecoveryOptions {
    fn default() -> Self {
        Self {
            recover_on_corruption: true,
            reject_if_corrupt: false,
            verify_integrity_on_open: true,
            busy_timeout_ms: DB_BUSY_TIMEOUT_MS,
            on_corruption_recovered: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentDbRecoveryError {
    #[error("agent store sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("agent store filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store.db failed integrity check for {0}")]
    Integrity(String),
}

fn diagnostic(
    kind: &str,
    agent_id: &str,
    metadata: impl IntoIterator<Item = (String, Value)>,
) {
    let mut values = metadata
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    values.insert("agentId".into(), Value::String(agent_id.to_string()));
    report_session_diagnostic(&SessionDiagnostic {
        family: "store_db".into(),
        kind: kind.into(),
        metadata: values,
    });
}

pub fn init_configured_db(
    db_path: &Path,
    agent_dir_name: &str,
    options: &DbRecoveryOptions,
) -> Result<Connection, AgentDbRecoveryError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Connection::open(db_path)?;
    db.busy_timeout(Duration::from_millis(options.busy_timeout_ms))?;
    if let Err(error) = db.pragma_update(None, "journal_mode", "WAL") {
        diagnostic(
            "wal_unavailable",
            agent_dir_name,
            [("errorClass".into(), Value::String(error.to_string()))],
        );
    }
    if let Err(error) = db.pragma_update(None, "synchronous", "NORMAL") {
        diagnostic(
            "wal_unavailable",
            agent_dir_name,
            [("errorClass".into(), Value::String(error.to_string()))],
        );
    }
    db.execute_batch(AGENT_DB_SCHEMA)?;
    let has_fence_state_known = db
        .query_row(
            "SELECT 1 FROM pragma_table_info('cron_session_runs') WHERE name = 'fence_state_known' LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if !has_fence_state_known {
        db.execute_batch(
            "ALTER TABLE cron_session_runs ADD COLUMN fence_state_known INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    let _ = ensure_conversation_schema_version(&db);
    Ok(db)
}

pub fn ensure_conversation_schema_version(db: &Connection) -> Result<(), rusqlite::Error> {
    let current = db.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if current != 2 {
        db.pragma_update(None, "user_version", 2)?;
    }
    Ok(())
}

pub fn is_store_db_healthy(db: &Connection) -> bool {
    db.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map(|value| value == "ok")
        .unwrap_or(false)
}

/// Reject an existing corrupt main database before journal-mode/schema writes.
/// A WAL or rollback journal may contain the current snapshot, so those stores
/// are left to the existing WAL-aware opener; this probe is never their authority.
pub(super) fn probe_human_conversation_store(
    db_path: &Path,
    busy_timeout_ms: u64,
) -> Result<(), AgentDbRecoveryError> {
    // A raw open/read/close can release another SQLite connection's POSIX
    // locks. Only inspect metadata here; let SQLite manage all file handles.
    let main_len = match std::fs::symlink_metadata(db_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(AgentDbRecoveryError::Integrity(db_path.display().to_string()));
        }
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    let mut has_retained_sidecar = false;
    let mut has_snapshot_sidecar = false;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut path = db_path.as_os_str().to_os_string();
        path.push(suffix);
        match std::fs::symlink_metadata(PathBuf::from(path)) {
            Ok(metadata) => {
                has_retained_sidecar = true;
                has_snapshot_sidecar |= suffix != "-shm" && metadata.len() > 0;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error.into()),
        }
    }
    if main_len == 0 && !has_retained_sidecar {
        return Ok(());
    }
    if main_len < 100 {
        // Missing/empty mains with retained sidecars need dedicated recovery.
        // SQLite may otherwise delete those files while opening a new store.
        return Err(AgentDbRecoveryError::Integrity(db_path.display().to_string()));
    }
    let mut uri = url::Url::from_file_path(db_path).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid Human store path")
    })?;
    uri.query_pairs_mut().append_pair("mode", "ro").append_pair("immutable", "1");
    let db = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
    )?;
    db.busy_timeout(Duration::from_millis(busy_timeout_ms))?;
    // Exercise SQLite's page and b-tree validation before any writable
    // journal-mode/schema initialization. This also rejects full-length
    // non-SQLite files that PRAGMA schema_version alone can treat as version 0.
    // The probe uses SQLite's own handle throughout: opening/closing a raw file
    // descriptor here can release another connection's POSIX record locks.
    let status = db.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
    if status != "ok" {
        return Err(AgentDbRecoveryError::Integrity(db_path.display().to_string()));
    }
    Ok(())
}

fn sqlite_error(error: &AgentDbRecoveryError) -> Option<&rusqlite::Error> {
    match error {
        AgentDbRecoveryError::Sqlite(error) => Some(error),
        AgentDbRecoveryError::Io(_) | AgentDbRecoveryError::Integrity(_) => None,
    }
}

pub fn open_configured_db(
    db_path: &Path,
    agent_dir_name: &str,
    options: &DbRecoveryOptions,
    has_other_live_handles: bool,
) -> Result<Connection, AgentDbRecoveryError> {
    let recover = options.recover_on_corruption && !has_other_live_handles;
    let mut opened = None;
    let mut last_error = None;

    for attempt in 0..2 {
        match init_configured_db(db_path, agent_dir_name, options) {
            Ok(db) => {
                opened = Some(db);
                break;
            }
            Err(error) => {
                let retry_io = sqlite_error(&error).is_some_and(is_sqlite_io_error) && attempt == 0;
                if retry_io {
                    diagnostic(
                        "open_io_retry",
                        agent_dir_name,
                        [("errorClass".into(), Value::String(error.to_string()))],
                    );
                    last_error = Some(error);
                    continue;
                }
                last_error = Some(error);
                break;
            }
        }
    }

    let Some(db) = opened else {
        let error = last_error.expect("configured db open records an error");
        if recover && sqlite_error(&error).is_some_and(is_sqlite_corrupt_error) {
            return recover_corrupt_store_db(db_path, agent_dir_name, options, &error.to_string());
        }
        return Err(error);
    };

    let verify = (recover || options.reject_if_corrupt) && options.verify_integrity_on_open;
    if verify && !is_store_db_healthy(&db) {
        drop(db);
        if recover {
            diagnostic(
                "quick_check_failed",
                agent_dir_name,
                Vec::<(String, Value)>::new(),
            );
            return recover_corrupt_store_db(
                db_path,
                agent_dir_name,
                options,
                "PRAGMA quick_check failed",
            );
        }
        return Err(AgentDbRecoveryError::Integrity(agent_dir_name.to_string()));
    }
    Ok(db)
}

pub fn quarantine_corrupt_db(db_path: &Path, agent_dir_name: &str) -> Option<PathBuf> {
    let result = quarantine_corrupt_sqlite_db(db_path, None, false, 1, 50);
    if result.rename_error_code.is_none() {
        return result.quarantine_path;
    }
    let error_class = result
        .rename_error_code
        .clone()
        .unwrap_or_else(|| "unknown".into());
    if result.quarantine_path.is_none() {
        diagnostic(
            "quarantine_rename_failed",
            agent_dir_name,
            [("errorClass".into(), Value::String(error_class))],
        );
        return None;
    }
    diagnostic(
        "quarantine_copied",
        agent_dir_name,
        [("errorClass".into(), Value::String(error_class))],
    );
    result.quarantine_path
}

pub fn salvage_store_db(source_path: &Path, fresh_db: &Connection) -> SalvageCounts {
    let mut counts = SalvageCounts {
        kv: 0,
        blobs: 0,
        transcript: 0,
        topics: 0,
        members: 0,
        pins: 0,
    };
    let Some(source) = open_sqlite_for_salvage(source_path, None) else {
        return counts;
    };

    counts.kv = copy_salvageable_sqlite_rows(
        &source,
        "SELECT key, value FROM kv",
        |row| {
            let key = row.get::<_, String>(0)?;
            let value = row.get::<_, String>(1)?;
            fresh_db
                .execute(
                    "INSERT OR IGNORE INTO kv (key, value) VALUES (?1, ?2)",
                    params![key, value],
                )
                .map(|_| ())
        },
    );
    counts.blobs = copy_salvageable_sqlite_rows(
        &source,
        "SELECT id, data FROM blobs",
        |row| {
            let id = row.get::<_, String>(0)?;
            let data = row.get::<_, Vec<u8>>(1)?;
            fresh_db
                .execute(
                    "INSERT OR IGNORE INTO blobs (id, data) VALUES (?1, ?2)",
                    params![id, data],
                )
                .map(|_| ())
        },
    );
    counts.transcript = copy_salvageable_sqlite_rows(
        &source,
        "SELECT seq, id, entry FROM transcript_entries",
        |row| {
            let seq = row.get::<_, i64>(0)?;
            let id = row.get::<_, String>(1)?;
            let entry = row.get::<_, String>(2)?;
            fresh_db
                .execute(
                    "INSERT OR IGNORE INTO transcript_entries (seq, id, entry) VALUES (?1, ?2, ?3)",
                    params![seq, id, entry],
                )
                .map(|_| ())
        },
    );
    counts.topics = copy_salvageable_sqlite_rows(
        &source,
        "SELECT topic_id, title, icon, creator_id, created_at_ms, pinned, closed, hidden, last_message_id, unread_count FROM conversation_topics",
        |row| {
            let topic_id = row.get::<_, String>(0)?;
            let title = row.get::<_, String>(1)?;
            let icon = row.get::<_, Option<String>>(2)?;
            let creator_id = row.get::<_, String>(3)?;
            let created_at_ms = row.get::<_, i64>(4)?;
            let pinned = row.get::<_, i64>(5)?;
            let closed = row.get::<_, i64>(6)?;
            let hidden = row.get::<_, i64>(7)?;
            let last_message_id = row.get::<_, Option<String>>(8)?;
            let unread_count = row.get::<_, i64>(9)?;
            fresh_db
                .execute(
                    "INSERT OR IGNORE INTO conversation_topics (
                        topic_id, title, icon, creator_id, created_at_ms,
                        pinned, closed, hidden, last_message_id, unread_count
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        topic_id,
                        title,
                        icon,
                        creator_id,
                        created_at_ms,
                        pinned,
                        closed,
                        hidden,
                        last_message_id,
                        unread_count,
                    ],
                )
                .map(|_| ())
        },
    );
    counts.members = copy_salvageable_sqlite_rows(
        &source,
        "SELECT actor_id, status, admin_title, admin_rights, restrictions, joined_at_ms, invited_by, updated_at_ms FROM conversation_members",
        |row| {
            let actor_id = row.get::<_, String>(0)?;
            let status = row.get::<_, String>(1)?;
            let admin_title = row.get::<_, Option<String>>(2)?;
            let admin_rights = row.get::<_, Option<String>>(3)?;
            let restrictions = row.get::<_, Option<String>>(4)?;
            let joined_at_ms = row.get::<_, i64>(5)?;
            let invited_by = row.get::<_, Option<String>>(6)?;
            let updated_at_ms = row.get::<_, i64>(7)?;
            fresh_db
                .execute(
                    "INSERT OR IGNORE INTO conversation_members (
                        actor_id, status, admin_title, admin_rights,
                        restrictions, joined_at_ms, invited_by, updated_at_ms
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        actor_id,
                        status,
                        admin_title,
                        admin_rights,
                        restrictions,
                        joined_at_ms,
                        invited_by,
                        updated_at_ms,
                    ],
                )
                .map(|_| ())
        },
    );
    counts.pins = copy_salvageable_sqlite_rows(
        &source,
        "SELECT child_scope, message_id, pinned_at_ms, pinned_by FROM conversation_pins",
        |row| {
            let child_scope = row.get::<_, String>(0)?;
            let message_id = row.get::<_, String>(1)?;
            let pinned_at_ms = row.get::<_, i64>(2)?;
            let pinned_by = row.get::<_, String>(3)?;
            fresh_db
                .execute(
                    "INSERT OR IGNORE INTO conversation_pins (
                        child_scope, message_id, pinned_at_ms, pinned_by
                    ) VALUES (?1, ?2, ?3, ?4)",
                    params![child_scope, message_id, pinned_at_ms, pinned_by],
                )
                .map(|_| ())
        },
    );
    counts
}

pub fn recover_corrupt_store_db(
    db_path: &Path,
    agent_dir_name: &str,
    options: &DbRecoveryOptions,
    cause: &str,
) -> Result<Connection, AgentDbRecoveryError> {
    diagnostic(
        "corrupt_recovering",
        agent_dir_name,
        [("errorClass".into(), Value::String(cause.to_string()))],
    );
    let quarantine_path = quarantine_corrupt_db(db_path, agent_dir_name);
    let fresh_db = init_configured_db(db_path, agent_dir_name, options)?;
    let salvaged = quarantine_path
        .as_deref()
        .map(|path| salvage_store_db(path, &fresh_db))
        .unwrap_or(SalvageCounts {
            kv: 0,
            blobs: 0,
            transcript: 0,
            topics: 0,
            members: 0,
            pins: 0,
        });
    let outcome = if salvaged.total() > 0 {
        "recovered"
    } else {
        "reset"
    };
    diagnostic(
        "recovery_outcome",
        agent_dir_name,
        [
            ("outcome".into(), Value::String(outcome.into())),
            (
                "quarantine".into(),
                Value::String(if quarantine_path.is_some() {
                    "preserved".into()
                } else {
                    "none".into()
                }),
            ),
            ("salvagedKv".into(), Value::from(salvaged.kv as u64)),
            ("salvagedBlobs".into(), Value::from(salvaged.blobs as u64)),
            (
                "salvagedTranscript".into(),
                Value::from(salvaged.transcript as u64),
            ),
            ("salvagedTopics".into(), Value::from(salvaged.topics as u64)),
            ("salvagedMembers".into(), Value::from(salvaged.members as u64)),
            ("salvagedPins".into(), Value::from(salvaged.pins as u64)),
        ],
    );
    if let Some(callback) = options.on_corruption_recovered.as_ref() {
        let event = CorruptionRecoveredEvent {
            outcome,
            quarantine_path: quarantine_path.clone(),
            salvaged,
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            callback(&event)
        }));
    }

    let mut mutation = Map::new();
    mutation.insert("kind".into(), Value::String("agent-needs-reindex".into()));
    mutation.insert("agentId".into(), Value::String(agent_dir_name.to_string()));
    publish_transcript_mutation(&mutation);
    Ok(fresh_db)
}
