use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CONVERSATION_BLOB_MIGRATION_UNSTARTED: i64 = 0;
pub const CONVERSATION_BLOB_ADOPTION_COMPLETE: i64 = 1;
pub const CONVERSATION_BLOB_RECOVERY_REBUILT: i64 = 2;

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
    let value = conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
    Ok(value == "ok")
}

fn configure_connection(
    conn: &Connection,
    busy_timeout_ms: u64,
) -> Result<(), ConversationBlobRecoveryError> {
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
        .map_err(|error| ConversationBlobRecoveryError::new(
            "SAND_BLOB_RECOVERY_SOURCE_UNAVAILABLE",
            format!("cannot open {}: {error}", path.display()),
        ))?;
    configure_connection(&conn, busy_timeout_ms)?;
    Ok(conn)
}

fn recovery_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn recover_corrupt_database(
    path: &Path,
    busy_timeout_ms: u64,
) -> Result<Connection, ConversationBlobRecoveryError> {
    let quarantine = PathBuf::from(format!("{}.corrupt-{}", path.display(), recovery_stamp()));
    fs::rename(path, &quarantine).map_err(|error| {
        ConversationBlobRecoveryError::new(
            "SAND_BLOB_RECOVERY_QUARANTINE_UNREADABLE",
            format!(
                "cannot quarantine corrupt blob database {}: {error}",
                path.display()
            ),
        )
    })?;

    let mut fresh = open_configured_conversation_blob_db(path, busy_timeout_ms)?;
    let mut salvaged = 0usize;

    if let Ok(source) = Connection::open_with_flags(&quarantine, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        if let Ok(mut statement) = source.prepare("SELECT id, data FROM blobs") {
            if let Ok(rows) = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            }) {
                let tx = fresh.transaction()?;
                for row in rows.flatten() {
                    if tx
                        .execute(
                            "INSERT OR IGNORE INTO blobs (id, data) VALUES (?1, ?2)",
                            params![row.0, row.1],
                        )
                        .is_ok()
                    {
                        salvaged += 1;
                    }
                }
                tx.commit()?;
            }
        }
    }

    fresh.execute_batch(&format!(
        "PRAGMA user_version = {};",
        CONVERSATION_BLOB_RECOVERY_REBUILT
    ))?;
    let _ = fresh.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    let _ = salvaged;
    Ok(fresh)
}

impl ConversationBlobDb {
    pub fn open(
        path: impl Into<PathBuf>,
        busy_timeout_ms: u64,
    ) -> Result<Self, ConversationBlobRecoveryError> {
        let path = path.into();
        match open_configured_conversation_blob_db(&path, busy_timeout_ms) {
            Ok(conn) => {
                if run_quick_check(&conn)? {
                    Ok(Self { path, conn })
                } else {
                    drop(conn);
                    let conn = recover_corrupt_database(&path, busy_timeout_ms)?;
                    Ok(Self { path, conn })
                }
            }
            Err(error) => Err(error),
        }
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
