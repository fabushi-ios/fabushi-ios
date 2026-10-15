use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::agent_content_search::conversation_id_from_scope;

pub const SEARCH_INDEX_SCHEMA_VERSION: i64 = 1;
pub const SEARCH_INDEX_FILENAME: &str = "search-index.db";
pub const DB_BUSY_TIMEOUT_MS: u64 = 5_000;
pub const FTS_QUERY_MAX_TERMS: usize = 8;
pub const SNIPPET_CONTEXT_TOKENS: i32 = 16;
pub const META_RECONCILE_DONE: &str = "reconcile_done";

pub const SEARCH_INDEX_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS scopes (
  agent_id TEXT PRIMARY KEY,
  fingerprint TEXT NOT NULL
) STRICT;
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_id TEXT NOT NULL,
  entry_id TEXT NOT NULL,
  role TEXT NOT NULL CHECK(role IN ('user','assistant')),
  timestamp_ms INTEGER NOT NULL,
  body TEXT NOT NULL,
  UNIQUE(agent_id, entry_id)
) STRICT;
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  body,
  content='messages',
  content_rowid='id',
  tokenize='unicode61 remove_diacritics 2',
  prefix='2 3'
);
CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', old.id, old.body);
END;
CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, body) VALUES('delete', old.id, old.body);
  INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;
CREATE TABLE IF NOT EXISTS media (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_id TEXT NOT NULL,
  entry_id TEXT NOT NULL,
  file_name TEXT NOT NULL,
  extension TEXT NOT NULL,
  mime_type TEXT,
  kind TEXT NOT NULL,
  timestamp_ms INTEGER NOT NULL,
  width INTEGER,
  height INTEGER,
  UNIQUE(agent_id, entry_id)
) STRICT;
CREATE VIRTUAL TABLE IF NOT EXISTS media_fts USING fts5(
  file_name,
  content='media',
  content_rowid='id',
  tokenize='unicode61 remove_diacritics 2',
  prefix='2 3'
);
CREATE TRIGGER IF NOT EXISTS media_ai AFTER INSERT ON media BEGIN
  INSERT INTO media_fts(rowid, file_name) VALUES (new.id, new.file_name);
END;
CREATE TRIGGER IF NOT EXISTS media_ad AFTER DELETE ON media BEGIN
  INSERT INTO media_fts(media_fts, rowid, file_name) VALUES('delete', old.id, old.file_name);
END;
CREATE TRIGGER IF NOT EXISTS media_au AFTER UPDATE ON media BEGIN
  INSERT INTO media_fts(media_fts, rowid, file_name) VALUES('delete', old.id, old.file_name);
  INSERT INTO media_fts(rowid, file_name) VALUES (new.id, new.file_name);
END;
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchDbError {
    Corrupt(String),
    Rebuild(String),
    Unavailable(String),
    Other(String),
}

impl std::fmt::Display for SearchDbError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Corrupt(message)
            | Self::Rebuild(message)
            | Self::Unavailable(message)
            | Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SearchDbError {}

fn primary_code(error: &rusqlite::Error) -> Option<i32> {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) => Some(failure.extended_code & 0xff),
        _ => None,
    }
}

pub fn map_sqlite_error(context: &str, error: rusqlite::Error) -> SearchDbError {
    if matches!(primary_code(&error), Some(11) | Some(26)) {
        SearchDbError::Corrupt(format!("{context}: {error}"))
    } else if matches!(primary_code(&error), Some(10) | Some(14)) {
        SearchDbError::Unavailable(format!("{context}: {error}"))
    } else {
        SearchDbError::Other(format!("{context}: {error}"))
    }
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), suffix))
}

pub fn remove_search_index_files(path: &Path) -> Result<(), SearchDbError> {
    for candidate in [
        path.to_path_buf(),
        path_with_suffix(path, "-wal"),
        path_with_suffix(path, "-shm"),
        path_with_suffix(path, "-journal"),
    ] {
        match fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(SearchDbError::Other(format!(
                    "remove {}: {error}",
                    candidate.display()
                )));
            }
        }
    }
    Ok(())
}

pub fn open_search_index_db(path: &Path) -> Result<Connection, SearchDbError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| SearchDbError::Other(format!("create search-index directory: {error}")))?;
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|error| map_sqlite_error("open search-index database", error))?;
    conn.busy_timeout(Duration::from_millis(DB_BUSY_TIMEOUT_MS))
        .map_err(|error| map_sqlite_error("configure search-index busy timeout", error))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA auto_vacuum = INCREMENTAL;",
    )
    .map_err(|error| map_sqlite_error("configure search-index pragmas", error))?;
    let version = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(|error| map_sqlite_error("read search-index schema version", error))?;
    if version != 0 && version != SEARCH_INDEX_SCHEMA_VERSION {
        return Err(SearchDbError::Rebuild(format!(
            "search-index schema version {version} does not match {SEARCH_INDEX_SCHEMA_VERSION}"
        )));
    }
    conn.execute_batch(SEARCH_INDEX_SCHEMA)
        .map_err(|error| map_sqlite_error("install search-index schema", error))?;
    if version == 0 {
        conn.execute_batch(&format!(
            "PRAGMA user_version = {SEARCH_INDEX_SCHEMA_VERSION};"
        ))
        .map_err(|error| map_sqlite_error("stamp search-index schema version", error))?;
    }
    conn.query_row("SELECT count(*) FROM messages_fts", [], |_| Ok(()))
        .map_err(|error| map_sqlite_error("verify FTS5 availability", error))?;
    Ok(conn)
}

pub fn set_reconcile_done(conn: &Connection, done: bool) -> Result<(), SearchDbError> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![META_RECONCILE_DONE, if done { "1" } else { "0" }],
    )
    .map_err(|error| map_sqlite_error("persist search-index reconcile flag", error))?;
    Ok(())
}

pub fn is_reconcile_done(conn: &Connection) -> Result<bool, SearchDbError> {
    let value = conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            [META_RECONCILE_DONE],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| map_sqlite_error("read search-index reconcile flag", error))?;
    Ok(value.as_deref() == Some("1"))
}

pub fn format_fts_query(raw: &str) -> String {
    raw.split_whitespace()
        .filter(|term| !term.is_empty())
        .take(FTS_QUERY_MAX_TERMS)
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn search_messages(
    conn: &Connection,
    scope_id: &str,
    raw_query: &str,
    limit: usize,
) -> Result<Vec<Value>, SearchDbError> {
    if !is_reconcile_done(conn)? {
        return Ok(Vec::new());
    }
    let query = format_fts_query(raw_query);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = conn
        .prepare(
            "SELECT m.entry_id, m.timestamp_ms,
                    snippet(messages_fts, 0, '', '', ' … ', ?4) AS snippet,
                    bm25(messages_fts) AS score
             FROM messages_fts
             JOIN messages m ON m.id = messages_fts.rowid
             WHERE messages_fts MATCH ?1 AND m.agent_id = ?2
             ORDER BY score ASC, m.timestamp_ms DESC
             LIMIT ?3",
        )
        .map_err(|error| map_sqlite_error("prepare message search", error))?;
    let rows = statement
        .query_map(
            params![query, scope_id, limit as i64, SNIPPET_CONTEXT_TOKENS],
            |row| {
                let entry_id = row.get::<_, String>(0)?;
                Ok(json!({
                    "kind": "message",
                    "id": entry_id.clone(),
                    "entryId": entry_id,
                    "conversationId": conversation_id_from_scope(scope_id),
                    "timestampMs": row.get::<_, i64>(1)?,
                    "snippet": row.get::<_, String>(2)?,
                    "score": row.get::<_, f64>(3)?.round() as i64,
                }))
            },
        )
        .map_err(|error| map_sqlite_error("execute message search", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| map_sqlite_error("collect message search", error))
}

pub fn search_media(
    conn: &Connection,
    scope_id: &str,
    raw_query: &str,
    limit: usize,
) -> Result<Vec<Value>, SearchDbError> {
    if !is_reconcile_done(conn)? {
        return Ok(Vec::new());
    }
    let query = format_fts_query(raw_query);
    if query.is_empty() {
        let mut statement = conn
            .prepare(
                "SELECT entry_id, file_name, extension, mime_type, kind, timestamp_ms, width, height
                 FROM media WHERE agent_id = ?1
                 ORDER BY timestamp_ms DESC LIMIT ?2",
            )
            .map_err(|error| map_sqlite_error("prepare recent media search", error))?;
        let rows = statement
            .query_map(params![scope_id, limit as i64], |row| {
                Ok(json!({
                    "kind": "media",
                    "entryId": row.get::<_, String>(0)?,
                    "conversationId": conversation_id_from_scope(scope_id),
                    "fileName": row.get::<_, String>(1)?,
                    "extension": row.get::<_, String>(2)?,
                    "mimeType": row.get::<_, Option<String>>(3)?,
                    "mediaKind": row.get::<_, String>(4)?,
                    "timestampMs": row.get::<_, i64>(5)?,
                    "width": row.get::<_, Option<i64>>(6)?,
                    "height": row.get::<_, Option<i64>>(7)?,
                }))
            })
            .map_err(|error| map_sqlite_error("execute recent media search", error))?;
        return rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| map_sqlite_error("collect recent media search", error));
    }
    let mut statement = conn
        .prepare(
            "SELECT m.entry_id, m.file_name, m.extension, m.mime_type, m.kind,
                    m.timestamp_ms, m.width, m.height
             FROM media_fts
             JOIN media m ON m.id = media_fts.rowid
             WHERE media_fts MATCH ?1 AND m.agent_id = ?2
             ORDER BY bm25(media_fts) ASC, m.timestamp_ms DESC LIMIT ?3",
        )
        .map_err(|error| map_sqlite_error("prepare media search", error))?;
    let rows = statement
        .query_map(params![query, scope_id, limit as i64], |row| {
            Ok(json!({
                "kind": "media",
                "entryId": row.get::<_, String>(0)?,
                "conversationId": conversation_id_from_scope(scope_id),
                "fileName": row.get::<_, String>(1)?,
                "extension": row.get::<_, String>(2)?,
                "mimeType": row.get::<_, Option<String>>(3)?,
                "mediaKind": row.get::<_, String>(4)?,
                "timestampMs": row.get::<_, i64>(5)?,
                "width": row.get::<_, Option<i64>>(6)?,
                "height": row.get::<_, Option<i64>>(7)?,
            }))
        })
        .map_err(|error| map_sqlite_error("execute media search", error))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| map_sqlite_error("collect media search", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-content-search-db-{}-{}.sqlite",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn installs_strict_fts_schema_with_desktop_tokenizer_contract() {
        let path = temp_path();
        let conn = open_search_index_db(&path).unwrap();
        let sql = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'messages_fts'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(sql.contains("unicode61 remove_diacritics 2"));
        assert!(sql.contains("prefix='2 3'"));
        let strict = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'messages'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(strict.contains("STRICT"));
        drop(conn);
        let _ = remove_search_index_files(&path);
    }

    #[test]
    fn caps_fts_query_terms_and_escapes_quotes() {
        let query = format_fts_query("one two three four five six seven eight nine");
        assert_eq!(query.split_whitespace().count(), 8);
        assert!(format_fts_query("a\"b").contains("\"a\"\"b\"*"));
    }
}
