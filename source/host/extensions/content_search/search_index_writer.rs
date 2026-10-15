use crate::conversation_blob_db::ConversationBlobDb;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::search_index_db::{
    map_sqlite_error, open_search_index_db, search_media, search_messages, set_reconcile_done,
    SearchDbError, DB_BUSY_TIMEOUT_MS,
};

pub const INDEXED_BODY_MAX_CHARS: usize = 20_000;
const SOURCE_BLOB_PREFIX: &str = "content-search:v1:";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    pub id: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub timestamp_ms: i64,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

fn default_kind() -> String {
    "message".into()
}

impl IndexEntry {
    pub fn from_value(value: Value) -> Result<Self, SearchDbError> {
        serde_json::from_value(value)
            .map_err(|error| SearchDbError::Other(format!("decode content-search entry: {error}")))
    }

    fn text_field(&self, key: &str) -> Option<String> {
        self.fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    }

    fn integer_field(&self, key: &str) -> Option<i64> {
        self.fields.get(key).and_then(Value::as_i64)
    }
}

#[derive(Debug, Clone)]
pub enum SearchIndexJob {
    ReplaceScope { scope_id: String, entries: Vec<IndexEntry> },
    UpsertEntries { scope_id: String, entries: Vec<IndexEntry> },
    DeleteEntry { scope_id: String, entry_id: String },
    ClearScope { scope_id: String },
    ReindexScope { scope_id: String },
    Reconcile,
    SearchMessages { scope_id: String, query: String, limit: usize },
    SearchMedia { scope_id: String, query: String, limit: usize },
    Status,
}

pub struct SearchIndexWriter {
    index: Connection,
    source: ConversationBlobDb,
}

impl SearchIndexWriter {
    pub fn open(index_path: &Path, source_path: &Path) -> Result<Self, SearchDbError> {
        let index = open_search_index_db(index_path)?;
        let source = ConversationBlobDb::open(source_path, DB_BUSY_TIMEOUT_MS)
            .map_err(|error| SearchDbError::Other(format!("open content-search source: {error}")))?;
        Ok(Self { index, source })
    }

    pub fn apply(&mut self, job: SearchIndexJob) -> Result<Value, SearchDbError> {
        match job {
            SearchIndexJob::ReplaceScope { scope_id, entries } => {
                self.replace_scope(&scope_id, entries)?;
                Ok(json!({"ok": true}))
            }
            SearchIndexJob::UpsertEntries { scope_id, entries } => {
                self.upsert_entries(&scope_id, entries)?;
                Ok(json!({"ok": true}))
            }
            SearchIndexJob::DeleteEntry { scope_id, entry_id } => {
                self.delete_entry(&scope_id, &entry_id)?;
                Ok(json!({"ok": true}))
            }
            SearchIndexJob::ClearScope { scope_id } => {
                self.clear_scope(&scope_id)?;
                Ok(json!({"ok": true}))
            }
            SearchIndexJob::ReindexScope { scope_id } => {
                self.reindex_scope(&scope_id)?;
                Ok(json!({"ok": true}))
            }
            SearchIndexJob::Reconcile => {
                self.reconcile()?;
                Ok(json!({"ok": true, "reconciled": true}))
            }
            SearchIndexJob::SearchMessages { scope_id, query, limit } => Ok(json!({
                "results": search_messages(&self.index, &scope_id, &query, limit)?
            })),
            SearchIndexJob::SearchMedia { scope_id, query, limit } => Ok(json!({
                "results": search_media(&self.index, &scope_id, &query, limit)?
            })),
            SearchIndexJob::Status => Ok(json!({
                "ready": super::search_index_db::is_reconcile_done(&self.index)?,
                "sourceEntries": self.source.list_blob_index()
                    .map_err(|error| SearchDbError::Other(format!("list content-search source: {error}")))?
                    .len(),
            })),
        }
    }

    fn mark_pending(&self) -> Result<(), SearchDbError> {
        set_reconcile_done(&self.index, false)
    }

    fn replace_scope(&mut self, scope_id: &str, entries: Vec<IndexEntry>) -> Result<(), SearchDbError> {
        self.mark_pending()?;
        self.delete_source_scope(scope_id)?;
        for entry in &entries {
            self.persist_source_entry(scope_id, entry)?;
        }
        self.replace_index_scope(scope_id, &entries)?;
        set_reconcile_done(&self.index, true)
    }

    fn upsert_entries(&mut self, scope_id: &str, entries: Vec<IndexEntry>) -> Result<(), SearchDbError> {
        self.mark_pending()?;
        for entry in &entries {
            self.persist_source_entry(scope_id, entry)?;
        }
        let tx = self.index
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| map_sqlite_error("begin content-search upsert", error))?;
        for entry in &entries {
            upsert_index_entry(&tx, scope_id, entry)?;
        }
        let fingerprint = source_fingerprint(&self.source, scope_id)?;
        tx.execute(
            "INSERT INTO scopes(agent_id, fingerprint) VALUES(?1, ?2)
             ON CONFLICT(agent_id) DO UPDATE SET fingerprint = excluded.fingerprint",
            params![scope_id, fingerprint],
        )
        .map_err(|error| map_sqlite_error("persist content-search scope fingerprint", error))?;
        tx.commit()
            .map_err(|error| map_sqlite_error("commit content-search upsert", error))?;
        set_reconcile_done(&self.index, true)
    }

    fn delete_entry(&mut self, scope_id: &str, entry_id: &str) -> Result<(), SearchDbError> {
        self.mark_pending()?;
        self.source
            .delete_blob_hex(&source_blob_id(scope_id, entry_id))
            .map_err(|error| SearchDbError::Other(format!("delete content-search source entry: {error}")))?;
        let tx = self.index
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| map_sqlite_error("begin content-search delete", error))?;
        delete_index_entry(&tx, scope_id, entry_id)?;
        let fingerprint = source_fingerprint(&self.source, scope_id)?;
        tx.execute(
            "INSERT INTO scopes(agent_id, fingerprint) VALUES(?1, ?2)
             ON CONFLICT(agent_id) DO UPDATE SET fingerprint = excluded.fingerprint",
            params![scope_id, fingerprint],
        )
        .map_err(|error| map_sqlite_error("persist content-search scope fingerprint", error))?;
        tx.commit()
            .map_err(|error| map_sqlite_error("commit content-search delete", error))?;
        set_reconcile_done(&self.index, true)
    }

    fn clear_scope(&mut self, scope_id: &str) -> Result<(), SearchDbError> {
        self.mark_pending()?;
        self.delete_source_scope(scope_id)?;
        let tx = self.index
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| map_sqlite_error("begin content-search clear", error))?;
        clear_index_scope(&tx, scope_id)?;
        tx.execute("DELETE FROM scopes WHERE agent_id = ?1", [scope_id])
            .map_err(|error| map_sqlite_error("delete content-search scope", error))?;
        tx.commit()
            .map_err(|error| map_sqlite_error("commit content-search clear", error))?;
        set_reconcile_done(&self.index, true)
    }

    fn reindex_scope(&mut self, scope_id: &str) -> Result<(), SearchDbError> {
        self.mark_pending()?;
        let entries = source_entries_for_scope(&self.source, scope_id)?;
        self.replace_index_scope(scope_id, &entries)?;
        set_reconcile_done(&self.index, true)
    }

    fn reconcile(&mut self) -> Result<(), SearchDbError> {
        self.mark_pending()?;
        let source_scopes = all_source_scopes(&self.source)?;
        let indexed_scopes = indexed_scope_fingerprints(&self.index)?;
        for stale in indexed_scopes.keys()
            .filter(|scope| !source_scopes.contains(*scope))
            .cloned()
            .collect::<Vec<_>>()
        {
            let tx = self.index
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| map_sqlite_error("begin stale content-search cleanup", error))?;
            clear_index_scope(&tx, &stale)?;
            tx.execute("DELETE FROM scopes WHERE agent_id = ?1", [&stale])
                .map_err(|error| map_sqlite_error("delete stale content-search scope", error))?;
            tx.commit()
                .map_err(|error| map_sqlite_error("commit stale content-search cleanup", error))?;
        }
        for scope_id in source_scopes {
            let fingerprint = source_fingerprint(&self.source, &scope_id)?;
            if indexed_scopes.get(&scope_id) == Some(&fingerprint) {
                continue;
            }
            let entries = source_entries_for_scope(&self.source, &scope_id)?;
            self.replace_index_scope(&scope_id, &entries)?;
        }
        set_reconcile_done(&self.index, true)
    }

    fn replace_index_scope(&mut self, scope_id: &str, entries: &[IndexEntry]) -> Result<(), SearchDbError> {
        let fingerprint = source_fingerprint(&self.source, scope_id)?;
        let tx = self.index
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| map_sqlite_error("begin content-search reindex", error))?;
        clear_index_scope(&tx, scope_id)?;
        for entry in entries {
            upsert_index_entry(&tx, scope_id, entry)?;
        }
        tx.execute(
            "INSERT INTO scopes(agent_id, fingerprint) VALUES(?1, ?2)
             ON CONFLICT(agent_id) DO UPDATE SET fingerprint = excluded.fingerprint",
            params![scope_id, fingerprint],
        )
        .map_err(|error| map_sqlite_error("persist reindexed scope fingerprint", error))?;
        tx.commit()
            .map_err(|error| map_sqlite_error("commit content-search reindex", error))
    }

    fn persist_source_entry(&self, scope_id: &str, entry: &IndexEntry) -> Result<(), SearchDbError> {
        let bytes = serde_json::to_vec(entry)
            .map_err(|error| SearchDbError::Other(format!("encode content-search source entry: {error}")))?;
        self.source
            .set_blob_hex(&source_blob_id(scope_id, &entry.id), &bytes)
            .map_err(|error| SearchDbError::Other(format!("persist content-search source entry: {error}")))
    }

    fn delete_source_scope(&self, scope_id: &str) -> Result<(), SearchDbError> {
        let prefix = source_blob_prefix(scope_id);
        for entry in self.source
            .list_blob_index()
            .map_err(|error| SearchDbError::Other(format!("list content-search source entries: {error}")))?
        {
            if entry.id.starts_with(&prefix) {
                self.source
                    .delete_blob_hex(&entry.id)
                    .map_err(|error| SearchDbError::Other(format!("delete content-search source scope: {error}")))?;
            }
        }
        Ok(())
    }
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_to_bytes(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}

fn source_blob_prefix(scope_id: &str) -> String {
    format!("{SOURCE_BLOB_PREFIX}{}:", bytes_to_hex(scope_id.as_bytes()))
}

fn source_blob_id(scope_id: &str, entry_id: &str) -> String {
    format!("{}{}", source_blob_prefix(scope_id), bytes_to_hex(entry_id.as_bytes()))
}

fn decode_source_scope(blob_id: &str) -> Option<String> {
    let rest = blob_id.strip_prefix(SOURCE_BLOB_PREFIX)?;
    let (scope_hex, _) = rest.split_once(':')?;
    String::from_utf8(hex_to_bytes(scope_hex)?).ok()
}

fn all_source_scopes(source: &ConversationBlobDb) -> Result<BTreeSet<String>, SearchDbError> {
    Ok(source
        .list_blob_index()
        .map_err(|error| SearchDbError::Other(format!("list content-search source scopes: {error}")))?
        .into_iter()
        .filter_map(|entry| decode_source_scope(&entry.id))
        .collect())
}

fn source_entries_for_scope(
    source: &ConversationBlobDb,
    scope_id: &str,
) -> Result<Vec<IndexEntry>, SearchDbError> {
    let prefix = source_blob_prefix(scope_id);
    let mut entries = Vec::new();
    for row in source
        .list_blob_index()
        .map_err(|error| SearchDbError::Other(format!("list content-search source scope: {error}")))?
        .into_iter()
        .filter(|row| row.id.starts_with(&prefix))
    {
        let Some(bytes) = source
            .get_blob_hex(&row.id)
            .map_err(|error| SearchDbError::Other(format!("read content-search source entry: {error}")))?
        else { continue };
        entries.push(
            serde_json::from_slice(&bytes)
                .map_err(|error| SearchDbError::Other(format!("decode content-search source entry: {error}")))?,
        );
    }
    Ok(entries)
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn source_fingerprint(source: &ConversationBlobDb, scope_id: &str) -> Result<String, SearchDbError> {
    let prefix = source_blob_prefix(scope_id);
    let mut rows = source
        .list_blob_index()
        .map_err(|error| SearchDbError::Other(format!("fingerprint content-search source: {error}")))?
        .into_iter()
        .filter(|row| row.id.starts_with(&prefix))
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    let mut hash = 0xcbf29ce484222325u64;
    let mut count = 0u64;
    for row in rows {
        hash = fnv1a(hash, row.id.as_bytes());
        if let Some(bytes) = source
            .get_blob_hex(&row.id)
            .map_err(|error| SearchDbError::Other(format!("fingerprint content-search body: {error}")))?
        {
            hash = fnv1a(hash, &bytes);
        }
        count += 1;
    }
    Ok(format!("{count}:{hash:016x}"))
}

fn indexed_scope_fingerprints(index: &Connection) -> Result<BTreeMap<String, String>, SearchDbError> {
    let mut statement = index
        .prepare("SELECT agent_id, fingerprint FROM scopes")
        .map_err(|error| map_sqlite_error("prepare indexed scope list", error))?;
    let rows = statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|error| map_sqlite_error("query indexed scope list", error))?;
    rows.collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|error| map_sqlite_error("collect indexed scope list", error))
}

fn capped_body(body: &str) -> String {
    body.chars().take(INDEXED_BODY_MAX_CHARS).collect()
}

fn message_projection(entry: &IndexEntry) -> Option<(String, String)> {
    let body = entry
        .text_field("body")
        .or_else(|| entry.text_field("text"))
        .or_else(|| entry.text_field("content"))?;
    let role = match entry.text_field("role").as_deref() {
        Some("user") => "user",
        _ => "assistant",
    };
    Some((role.into(), capped_body(&body)))
}

fn extension(file_name: &str) -> String {
    file_name
        .rsplit_once('.')
        .map(|(_, value)| value.to_ascii_lowercase())
        .unwrap_or_default()
}

fn media_kind(file_name: &str, mime_type: Option<&str>) -> String {
    let ext = extension(file_name);
    let mime = mime_type.unwrap_or_default().to_ascii_lowercase();
    if mime.starts_with("image/") || matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg") {
        "image"
    } else if mime.starts_with("video/") || matches!(ext.as_str(), "mp4" | "mov" | "m4v" | "webm") {
        "video"
    } else if mime.starts_with("audio/") || matches!(ext.as_str(), "mp3" | "m4a" | "wav" | "ogg" | "flac") {
        "audio"
    } else if ext == "pdf" {
        "pdf"
    } else if matches!(ext.as_str(), "md" | "markdown") {
        "markdown"
    } else if matches!(ext.as_str(), "csv" | "tsv" | "xls" | "xlsx") {
        "table"
    } else if ext == "json" {
        "json"
    } else if matches!(ext.as_str(), "txt" | "log" | "rtf") || mime.starts_with("text/") {
        "text"
    } else if matches!(ext.as_str(), "doc" | "docx" | "pages" | "ppt" | "pptx" | "key") {
        "document"
    } else if matches!(ext.as_str(), "zip" | "tar" | "gz" | "bz2" | "7z" | "rar") {
        "archive"
    } else {
        "file"
    }
    .into()
}

fn delete_index_entry(conn: &Connection, scope_id: &str, entry_id: &str) -> Result<(), SearchDbError> {
    conn.execute(
        "DELETE FROM messages WHERE agent_id = ?1 AND entry_id = ?2",
        params![scope_id, entry_id],
    )
    .map_err(|error| map_sqlite_error("delete indexed message", error))?;
    conn.execute(
        "DELETE FROM media WHERE agent_id = ?1 AND entry_id = ?2",
        params![scope_id, entry_id],
    )
    .map_err(|error| map_sqlite_error("delete indexed media", error))?;
    Ok(())
}

fn clear_index_scope(conn: &Connection, scope_id: &str) -> Result<(), SearchDbError> {
    conn.execute("DELETE FROM messages WHERE agent_id = ?1", [scope_id])
        .map_err(|error| map_sqlite_error("clear indexed messages", error))?;
    conn.execute("DELETE FROM media WHERE agent_id = ?1", [scope_id])
        .map_err(|error| map_sqlite_error("clear indexed media", error))?;
    Ok(())
}

fn upsert_index_entry(conn: &Connection, scope_id: &str, entry: &IndexEntry) -> Result<(), SearchDbError> {
    delete_index_entry(conn, scope_id, &entry.id)?;
    if let Some((role, body)) = message_projection(entry) {
        conn.execute(
            "INSERT INTO messages(agent_id, entry_id, role, timestamp_ms, body)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![scope_id, entry.id, role, entry.timestamp_ms, body],
        )
        .map_err(|error| map_sqlite_error("upsert indexed message", error))?;
    }
    if let Some(file_name) = entry.text_field("fileName") {
        let mime_type = entry.text_field("mimeType");
        let kind = media_kind(&file_name, mime_type.as_deref());
        conn.execute(
            "INSERT INTO media(agent_id, entry_id, file_name, extension, mime_type, kind, timestamp_ms, width, height)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                scope_id, entry.id, file_name, extension(&file_name), mime_type, kind,
                entry.timestamp_ms, entry.integer_field("width"), entry.integer_field("height")
            ],
        )
        .map_err(|error| map_sqlite_error("upsert indexed media", error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-content-search-writer-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    fn entry(id: &str, body: &str) -> IndexEntry {
        IndexEntry {
            id: id.into(),
            kind: "message".into(),
            timestamp_ms: 7,
            fields: BTreeMap::from([
                ("role".into(), Value::String("user".into())),
                ("body".into(), Value::String(body.into())),
            ]),
        }
    }

    #[test]
    fn indexed_message_body_is_capped_at_twenty_thousand_characters() {
        let root = temp_root("cap");
        let mut writer = SearchIndexWriter::open(&root.join("search-index.db"), &root.join("source.db")).unwrap();
        writer.apply(SearchIndexJob::ReplaceScope {
            scope_id: "a\u{1f}c".into(),
            entries: vec![entry("m1", &"x".repeat(INDEXED_BODY_MAX_CHARS + 100))],
        }).unwrap();
        let len: i64 = writer.index.query_row("SELECT length(body) FROM messages", [], |row| row.get(0)).unwrap();
        assert_eq!(len as usize, INDEXED_BODY_MAX_CHARS);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn media_classification_preserves_desktop_categories() {
        assert_eq!(media_kind("photo.HEIC", None), "image");
        assert_eq!(media_kind("voice.bin", Some("audio/m4a")), "audio");
        assert_eq!(media_kind("table.csv", None), "table");
        assert_eq!(media_kind("archive.zip", None), "archive");
        assert_eq!(media_kind("unknown.bin", None), "file");
    }
}
