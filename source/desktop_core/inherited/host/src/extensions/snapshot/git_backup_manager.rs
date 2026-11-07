use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub const SNAPSHOT_EXPORT_ENVELOPE_VERSION: u32 = 1;
pub const SQLITE_SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
pub const RESTORE_NAMESPACE_PREFIX: &str = "restore_";
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_POSTGRES_SEQUENCE: u64 = 9_223_372_036_854_775_807;
pub const MAX_EXPORT_MANIFEST_BYTES: usize = 1024 * 1024;
const SQLITE_TABLE_BATCH_BYTES: usize = 1024 * 1024;
const GIT_BACKUP_PROJECTION_TABLES: &[&str] = &[
    "backup_runs",
    "session_transcript_fts_rows",
    "session_transcript_index_state",
];
pub const GIT_BACKUP_MANIFEST: &str = "manifest.json";
pub const GIT_BACKUP_SCHEMA: &str = "schema.sql";
pub const GIT_BACKUP_TABLES: &str = "tables";

pub fn validate_sqlite_table_name(value: &str) -> Result<(), String> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err("SQLite backup table name is empty".into());
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(format!(
            "SQLite backup table name is not filesystem-safe: {value}"
        ));
    }
    Ok(())
}

fn quote_sqlite_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteSchemaEntry {
    pub kind: String,
    pub name: String,
    pub sql: String,
}

pub fn render_sqlite_schema(
    mut entries: Vec<SqliteSchemaEntry>,
    user_version: u32,
) -> Result<String, String> {
    for entry in &entries {
        if !matches!(entry.kind.as_str(), "table" | "index" | "trigger") {
            return Err(format!(
                "unsupported SQLite schema entry kind: {}",
                entry.kind
            ));
        }
        if entry.sql.trim().is_empty() {
            return Err(format!("SQLite schema entry has no SQL: {}", entry.name));
        }
    }
    entries.sort_by_key(|entry| {
        (
            match entry.kind.as_str() {
                "table" => 0_u8,
                "index" => 1,
                _ => 2,
            },
            entry.name.clone(),
        )
    });
    let mut output = String::new();
    for entry in entries {
        output.push_str(entry.sql.trim_end());
        if !output.ends_with(';') {
            output.push(';');
        }
        output.push_str("\n\n");
    }
    output.push_str(&format!("-- PRAGMA user_version = {user_version}\n"));
    Ok(output)
}

pub fn read_sqlite_schema_entries(
    connection: &Connection,
) -> Result<Vec<SqliteSchemaEntry>, String> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, sql
             FROM sqlite_master
             WHERE type IN ('table', 'index', 'trigger')
               AND name NOT LIKE 'sqlite_%'
               AND sql IS NOT NULL
             ORDER BY CASE type
               WHEN 'table' THEN 0
               WHEN 'index' THEN 1
               ELSE 2
             END, name",
        )
        .map_err(|error| format!("prepare SQLite schema read: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(SqliteSchemaEntry {
                kind: row.get(0)?,
                name: row.get(1)?,
                sql: row.get(2)?,
            })
        })
        .map_err(|error| format!("read SQLite schema entries: {error}"))?;
    rows.map(|row| row.map_err(|error| format!("decode SQLite schema entry: {error}")))
        .collect()
}

pub fn read_sqlite_user_version(connection: &Connection) -> Result<u32, String> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| format!("read SQLite user_version: {error}"))?;
    u32::try_from(version).map_err(|_| format!("SQLite user_version is out of range: {version}"))
}

pub fn is_git_backup_projection_table(name: &str) -> bool {
    GIT_BACKUP_PROJECTION_TABLES.contains(&name)
}

pub fn select_sqlite_backup_tables(
    entries: &[SqliteSchemaEntry],
    virtual_tables: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let mut tables = entries
        .iter()
        .filter(|entry| entry.kind == "table")
        .filter(|entry| !excluded.contains(&entry.name))
        .filter(|entry| !is_git_backup_projection_table(&entry.name))
        .filter(|entry| {
            !virtual_tables.iter().any(|virtual_table| {
                entry.name == *virtual_table || entry.name.starts_with(&format!("{virtual_table}_"))
            })
        })
        .map(|entry| {
            validate_sqlite_table_name(&entry.name)?;
            Ok(entry.name.clone())
        })
        .collect::<Result<Vec<_>, String>>()?;
    tables.sort();
    tables.dedup();
    Ok(tables)
}

pub fn digest_sqlite_table(
    connection: &Connection,
    table: &str,
) -> Result<SqliteTableDigest, String> {
    validate_sqlite_table_name(table)?;
    let mut table_info = connection
        .prepare(&format!(
            "PRAGMA table_info({})",
            quote_sqlite_identifier(table)
        ))
        .map_err(|error| format!("prepare SQLite table info {table}: {error}"))?;
    let mut primary_keys = table_info
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })
        .map_err(|error| format!("read SQLite table info {table}: {error}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("decode SQLite table info {table}: {error}"))?;
    primary_keys.retain(|(_, position)| *position > 0);
    primary_keys.sort_by_key(|(_, position)| *position);
    let order_by = if primary_keys.is_empty() {
        "rowid".to_string()
    } else {
        primary_keys
            .iter()
            .map(|(name, _)| quote_sqlite_identifier(name))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut statement = connection
        .prepare(&format!(
            "SELECT * FROM {} ORDER BY {order_by}",
            quote_sqlite_identifier(table)
        ))
        .map_err(|error| format!("prepare SQLite table read {table}: {error}"))?;
    let column_names = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut rows = statement
        .query([])
        .map_err(|error| format!("query SQLite table {table}: {error}"))?;
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    while let Some(row) = rows
        .next()
        .map_err(|error| format!("iterate SQLite table {table}: {error}"))?
    {
        let values = column_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let value = match row
                    .get_ref(index)
                    .map_err(|error| format!("read SQLite cell {table}.{name}: {error}"))?
                {
                    rusqlite::types::ValueRef::Null => rusqlite::types::Value::Null,
                    rusqlite::types::ValueRef::Integer(value) => {
                        rusqlite::types::Value::Integer(value)
                    }
                    rusqlite::types::ValueRef::Real(value) => rusqlite::types::Value::Real(value),
                    rusqlite::types::ValueRef::Text(value) => {
                        let value = String::from_utf8(value.to_vec()).map_err(|error| {
                            format!("invalid SQLite text {table}.{name}: {error}")
                        })?;
                        rusqlite::types::Value::Text(value)
                    }
                    rusqlite::types::ValueRef::Blob(value) => {
                        rusqlite::types::Value::Blob(value.to_vec())
                    }
                };
                Ok((name.clone(), value))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let encoded = encode_sqlite_row(values)?;
        hash.update(encoded);
        count = count
            .checked_add(1)
            .ok_or_else(|| "SQLite backup row count overflow".to_string())?;
    }
    Ok(SqliteTableDigest {
        rows: count,
        sha256: format!("{:x}", hash.finalize()),
    })
}

/// Materialize a deterministic schema-v1 portable backup from a live SQLite
/// owner. The caller owns publication of the returned artifact directory.
pub fn export_sqlite_backup_artifacts(
    connection: &Connection,
    output_dir: impl AsRef<Path>,
    identity: GitBackupIdentity,
    excluded_tables: &BTreeSet<String>,
) -> Result<GitBackupManifest, String> {
    let output_dir = output_dir.as_ref();
    git_backup_scope_path(&identity)?;
    if fs::symlink_metadata(output_dir).is_ok() {
        return Err(format!(
            "SQLite backup output already exists: {}",
            output_dir.display()
        ));
    }
    let parent = output_dir
        .parent()
        .ok_or_else(|| "SQLite backup output has no parent directory".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("create SQLite backup output parent: {error}"))?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("inspect SQLite backup output parent: {error}"))?;
    if !parent_metadata.is_dir() {
        return Err("SQLite backup output parent is not a directory".into());
    }
    let staging = parent.join(format!(
        ".{}-staging-{}",
        output_dir
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "SQLite backup output name is invalid".to_string())?,
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(staging.join(GIT_BACKUP_TABLES))
        .map_err(|error| format!("create SQLite backup staging directory: {error}"))?;
    let result = (|| {
        let transaction = connection
            .unchecked_transaction()
            .map_err(|error| format!("begin SQLite backup snapshot: {error}"))?;
        let user_version = read_sqlite_user_version(&transaction)?;
        let schema_entries = read_sqlite_schema_entries(&transaction)?;
        let schema = render_sqlite_schema(schema_entries.clone(), user_version)?;
        let mut schema_file = File::create(staging.join(GIT_BACKUP_SCHEMA))
            .map_err(|error| format!("create SQLite backup schema: {error}"))?;
        schema_file
            .write_all(schema.as_bytes())
            .and_then(|_| schema_file.sync_all())
            .map_err(|error| format!("write SQLite backup schema: {error}"))?;
        let virtual_tables = schema_entries
            .iter()
            .filter(|entry| entry.kind == "table" && entry.sql.contains("VIRTUAL TABLE"))
            .map(|entry| entry.name.clone())
            .collect::<BTreeSet<_>>();
        let tables =
            select_sqlite_backup_tables(&schema_entries, &virtual_tables, excluded_tables)?;
        let mut manifest_tables = BTreeMap::new();
        for table in tables {
            let mut file = File::create(
                staging
                    .join(GIT_BACKUP_TABLES)
                    .join(format!("{table}.jsonl")),
            )
            .map_err(|error| format!("create SQLite backup table {table}: {error}"))?;
            let mut table_info = transaction
                .prepare(&format!(
                    "PRAGMA table_info({})",
                    quote_sqlite_identifier(&table)
                ))
                .map_err(|error| format!("prepare SQLite table info {table}: {error}"))?;
            let mut primary_keys = table_info
                .query_map([], |row| {
                    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
                })
                .map_err(|error| format!("read SQLite table info {table}: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("decode SQLite table info {table}: {error}"))?;
            primary_keys.retain(|(_, position)| *position > 0);
            primary_keys.sort_by_key(|(_, position)| *position);
            drop(table_info);
            let order_by = if primary_keys.is_empty() {
                "rowid".to_string()
            } else {
                primary_keys
                    .iter()
                    .map(|(name, _)| quote_sqlite_identifier(name))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT * FROM {} ORDER BY {order_by}",
                    quote_sqlite_identifier(&table)
                ))
                .map_err(|error| format!("prepare SQLite backup table {table}: {error}"))?;
            let columns = statement
                .column_names()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            let mut rows = statement
                .query([])
                .map_err(|error| format!("query SQLite backup table {table}: {error}"))?;
            let mut hash = Sha256::new();
            let mut count = 0_u64;
            while let Some(row) = rows
                .next()
                .map_err(|error| format!("iterate SQLite backup table {table}: {error}"))?
            {
                let values = columns
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        let value = match row
                            .get_ref(index)
                            .map_err(|error| format!("read SQLite cell {table}.{name}: {error}"))?
                        {
                            rusqlite::types::ValueRef::Null => rusqlite::types::Value::Null,
                            rusqlite::types::ValueRef::Integer(value) => {
                                rusqlite::types::Value::Integer(value)
                            }
                            rusqlite::types::ValueRef::Real(value) => {
                                rusqlite::types::Value::Real(value)
                            }
                            rusqlite::types::ValueRef::Text(value) => rusqlite::types::Value::Text(
                                String::from_utf8(value.to_vec()).map_err(|error| {
                                    format!("invalid SQLite text {table}.{name}: {error}")
                                })?,
                            ),
                            rusqlite::types::ValueRef::Blob(value) => {
                                rusqlite::types::Value::Blob(value.to_vec())
                            }
                        };
                        Ok((name.clone(), value))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let encoded = encode_sqlite_row(values)?;
                hash.update(&encoded);
                file.write_all(&encoded)
                    .map_err(|error| format!("write SQLite backup table {table}: {error}"))?;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| "SQLite backup row count overflow".to_string())?;
            }
            file.sync_all()
                .map_err(|error| format!("sync SQLite backup table {table}: {error}"))?;
            manifest_tables.insert(
                table,
                GitBackupTableEntry {
                    rows: count,
                    sha256: format!("{:x}", hash.finalize()),
                },
            );
        }
        let manifest = GitBackupManifest {
            schema_version: 1,
            identity,
            postgres_source: None,
            user_version: user_version as u64,
            excluded_tables: excluded_tables.iter().cloned().collect(),
            excluded_config_state_key_prefixes: Vec::new(),
            tables: manifest_tables,
        };
        let value = serde_json::to_value(&manifest)
            .map_err(|error| format!("serialize SQLite backup manifest: {error}"))?;
        let bytes = serde_json::to_vec_pretty(&value)
            .map_err(|error| format!("serialize SQLite backup manifest: {error}"))?;
        let mut manifest_file = File::create(staging.join(GIT_BACKUP_MANIFEST))
            .map_err(|error| format!("create SQLite backup manifest: {error}"))?;
        manifest_file
            .write_all(&bytes)
            .and_then(|_| manifest_file.sync_all())
            .map_err(|error| format!("write SQLite backup manifest: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("commit SQLite backup snapshot: {error}"))?;
        fs::rename(&staging, output_dir)
            .map_err(|error| format!("publish SQLite backup staging directory: {error}"))?;
        Ok(manifest)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotExportEnvelope {
    pub version: u32,
    pub snapshot_id: String,
    pub created_at_ms: u64,
    pub manifest: Value,
    pub checksum: String,
}

pub use super::git_backup_manager_ocrm::*;

/// Encode a SQLite cell using the portable Git-backup representation.
pub fn encode_sqlite_value(value: rusqlite::types::Value) -> Result<Value, String> {
    use rusqlite::types::Value as SqliteValue;
    match value {
        SqliteValue::Null => Ok(Value::Null),
        SqliteValue::Integer(value) => {
            if (-(MAX_SAFE_INTEGER as i64)..=MAX_SAFE_INTEGER as i64).contains(&value) {
                Ok(Value::Number(value.into()))
            } else {
                Ok(serde_json::json!({ "$int": value.to_string() }))
            }
        }
        SqliteValue::Real(value) if value.is_finite() => Ok(serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| "SQLite REAL cannot be represented as JSON".to_string())?),
        SqliteValue::Real(_) => {
            Err("Git backup cannot encode a non-finite SQLite REAL value".into())
        }
        SqliteValue::Text(value) => Ok(Value::String(value)),
        SqliteValue::Blob(value) => Ok(serde_json::json!({
            "$hex": value.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        })),
    }
}

pub fn decode_sqlite_value(value: &Value) -> Result<rusqlite::types::Value, String> {
    use rusqlite::types::Value as SqliteValue;
    match value {
        Value::Null => Ok(SqliteValue::Null),
        Value::String(value) => Ok(SqliteValue::Text(value.clone())),
        Value::Number(value) => {
            if let Some(integer) = value.as_i64() {
                Ok(SqliteValue::Integer(integer))
            } else if let Some(real) = value.as_f64() {
                if real.is_finite() {
                    Ok(SqliteValue::Real(real))
                } else {
                    Err("SQLite backup number is non-finite".into())
                }
            } else {
                Err("SQLite backup number is not representable".into())
            }
        }
        Value::Object(object) if object.len() == 1 && object.contains_key("$int") => {
            let Some(Value::String(value)) = object.get("$int") else {
                return Err("SQLite backup $int must be a string".into());
            };
            let value = value
                .parse::<i64>()
                .map_err(|error| format!("SQLite backup $int is out of range: {error}"))?;
            Ok(SqliteValue::Integer(value))
        }
        Value::Object(object) if object.len() == 1 && object.contains_key("$hex") => {
            let Some(Value::String(value)) = object.get("$hex") else {
                return Err("SQLite backup $hex must be a string".into());
            };
            if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("SQLite backup $hex is invalid".into());
            }
            let mut bytes = Vec::with_capacity(value.len() / 2);
            for pair in value.as_bytes().chunks_exact(2) {
                let high = (pair[0] as char)
                    .to_digit(16)
                    .ok_or_else(|| "SQLite backup $hex is invalid".to_string())?;
                let low = (pair[1] as char)
                    .to_digit(16)
                    .ok_or_else(|| "SQLite backup $hex is invalid".to_string())?;
                bytes.push(((high << 4) | low) as u8);
            }
            Ok(SqliteValue::Blob(bytes))
        }
        Value::Object(_) | Value::Array(_) | Value::Bool(_) => {
            Err("SQLite backup row contains an invalid encoded value".into())
        }
    }
}

pub fn restore_sqlite_rows(
    connection: &mut Connection,
    table: &str,
    columns: &[String],
    rows: impl IntoIterator<Item = Result<Vec<Value>, String>>,
    expected_digest: &SqliteTableDigest,
) -> Result<SqliteTableDigest, String> {
    validate_sqlite_table_name(table)?;
    if columns.is_empty()
        || columns
            .iter()
            .any(|column| column.is_empty() || column.contains('\0'))
    {
        return Err("SQLite restore columns are invalid".into());
    }
    let quoted_columns = columns
        .iter()
        .map(|column| format!("\"{}\"", column.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    let placeholders = (0..columns.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO \"{table}\" ({}) VALUES ({placeholders})",
        quoted_columns.join(", ")
    );
    let transaction = connection
        .transaction()
        .map_err(|error| format!("begin SQLite restore transaction: {error}"))?;
    let mut insert = transaction
        .prepare(&sql)
        .map_err(|error| format!("prepare SQLite restore insert {table}: {error}"))?;
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    for row in rows {
        let values = row?;
        if values.len() != columns.len() {
            return Err(format!(
                "SQLite restore row has {} values, expected {}",
                values.len(),
                columns.len()
            ));
        }
        let decoded = values
            .iter()
            .map(decode_sqlite_value)
            .collect::<Result<Vec<_>, _>>()?;
        insert
            .execute(rusqlite::params_from_iter(decoded.iter()))
            .map_err(|error| format!("insert SQLite restore row {table}: {error}"))?;
        let encoded = encode_sqlite_row(columns.iter().cloned().zip(decoded))?;
        hash.update(encoded);
        count = count
            .checked_add(1)
            .ok_or_else(|| "SQLite restore row count overflow".to_string())?;
    }
    drop(insert);
    let digest = SqliteTableDigest {
        rows: count,
        sha256: format!("{:x}", hash.finalize()),
    };
    if &digest != expected_digest {
        return Err("SQLite restore digest does not match the admitted manifest".into());
    }
    transaction
        .commit()
        .map_err(|error| format!("commit SQLite restore transaction: {error}"))?;
    Ok(digest)
}

/// Encode one deterministic JSONL row from SQLite columns.
pub fn encode_sqlite_row(
    columns: impl IntoIterator<Item = (String, rusqlite::types::Value)>,
) -> Result<Vec<u8>, String> {
    let mut encoded = BTreeMap::new();
    for (name, value) in columns {
        if encoded.contains_key(&name) {
            return Err(format!("duplicate SQLite column in backup row: {name}"));
        }
        encoded.insert(name, encode_sqlite_value(value)?);
    }
    let mut bytes = serde_json::to_vec(&encoded)
        .map_err(|error| format!("serialize SQLite backup row: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Bounded JSONL buffering equivalent to the upstream table serializer.
#[derive(Debug, Default)]
pub struct SqliteJsonlBatch {
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteTableDigest {
    pub rows: u64,
    pub sha256: String,
}

/// Hash the exact JSONL bytes that would be published for a table.
pub fn digest_sqlite_rows<I>(rows: I) -> Result<SqliteTableDigest, String>
where
    I: IntoIterator<Item = Result<Vec<u8>, String>>,
{
    let mut hash = Sha256::new();
    let mut count = 0_u64;
    for row in rows {
        let row = row?;
        if row.is_empty() || row.last() != Some(&b'\n') {
            return Err("SQLite backup row must be a non-empty JSONL line".into());
        }
        hash.update(&row);
        count = count
            .checked_add(1)
            .ok_or_else(|| "SQLite backup row count overflow".to_string())?;
    }
    Ok(SqliteTableDigest {
        rows: count,
        sha256: format!("{:x}", hash.finalize()),
    })
}

impl SqliteJsonlBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one complete row. Returns a flushed batch once the threshold is
    /// reached; a single oversized row is retained as one bounded unit.
    pub fn push(&mut self, row: &[u8]) -> Option<Vec<u8>> {
        self.bytes.extend_from_slice(row);
        if self.bytes.len() >= SQLITE_TABLE_BATCH_BYTES {
            return Some(std::mem::take(&mut self.bytes));
        }
        None
    }

    pub fn finish(&mut self) -> Option<Vec<u8>> {
        if self.bytes.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.bytes))
        }
    }

    pub fn pending_bytes(&self) -> usize {
        self.bytes.len()
    }
}

pub fn create_export_envelope(snapshot_id: &str, manifest: Value) -> SnapshotExportEnvelope {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let manifest_bytes = serde_json::to_vec(&manifest).unwrap_or_default();
    let checksum = format!("{:x}", Sha256::digest(&manifest_bytes));

    SnapshotExportEnvelope {
        version: SNAPSHOT_EXPORT_ENVELOPE_VERSION,
        snapshot_id: snapshot_id.to_string(),
        created_at_ms: now_ms,
        manifest,
        checksum,
    }
}

pub fn verify_export_envelope(envelope: &SnapshotExportEnvelope) -> Result<(), String> {
    if envelope.version != SNAPSHOT_EXPORT_ENVELOPE_VERSION {
        return Err(format!(
            "Unsupported snapshot envelope version: {}",
            envelope.version
        ));
    }

    let manifest_bytes = serde_json::to_vec(&envelope.manifest)
        .map_err(|e| format!("Manifest serialization failed: {e}"))?;
    let expected_checksum = format!("{:x}", Sha256::digest(&manifest_bytes));

    if envelope.checksum.eq_ignore_ascii_case(&expected_checksum) {
        Ok(())
    } else {
        Err(format!(
            "Snapshot checksum mismatch: expected {}, got {}",
            expected_checksum, envelope.checksum
        ))
    }
}
