use crate::cache::sqlite_cache::{db_err, SqliteClientCache};
use gpui_telegram_core::error::{Result, TelegramError};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    Pending,
    InProgress,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TransferStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransferStatus::Pending => "Pending",
            TransferStatus::InProgress => "InProgress",
            TransferStatus::Paused => "Paused",
            TransferStatus::Completed => "Completed",
            TransferStatus::Failed => "Failed",
            TransferStatus::Cancelled => "Cancelled",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s.trim() {
            "InProgress" => TransferStatus::InProgress,
            "Paused" => TransferStatus::Paused,
            "Completed" => TransferStatus::Completed,
            "Failed" => TransferStatus::Failed,
            "Cancelled" => TransferStatus::Cancelled,
            _ => TransferStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTransferRecord {
    pub transfer_id: String,
    pub upload_id: Option<String>,
    pub direction: String,
    pub file_path: String,
    pub filename: String,
    pub mime_type: String,
    pub total_size: u64,
    pub part_size: u32,
    pub total_parts: u32,
    pub file_sha256: String,
    pub chunk_hashes: Vec<String>,
    pub uploaded_parts: Vec<bool>,
    pub status: TransferStatus,
    pub server_file_id: Option<String>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone)]
pub struct FileTransferStore {
    cache: Arc<SqliteClientCache>,
}

impl std::fmt::Debug for FileTransferStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileTransferStore").finish()
    }
}

impl FileTransferStore {
    pub fn open(base_dir: impl AsRef<Path>) -> Result<Self> {
        let cache = Arc::new(SqliteClientCache::open(base_dir.as_ref())?);
        Ok(Self { cache })
    }

    pub fn in_memory() -> Result<Self> {
        let cache = Arc::new(SqliteClientCache::open_in_memory()?);
        Ok(Self { cache })
    }

    pub(crate) fn from_cache(cache: Arc<SqliteClientCache>) -> Self {
        Self { cache }
    }

    fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<FileTransferRecord> {
        let chunk_hashes_json: String = r.get(10)?;
        let uploaded_parts_json: String = r.get(11)?;
        let status_str: String = r.get(12)?;
        let chunk_hashes: Vec<String> = serde_json::from_str(&chunk_hashes_json).unwrap_or_default();
        let uploaded_parts: Vec<bool> =
            serde_json::from_str(&uploaded_parts_json).unwrap_or_default();
        Ok(FileTransferRecord {
            transfer_id: r.get(0)?,
            upload_id: r.get(1)?,
            direction: r.get(2)?,
            file_path: r.get(3)?,
            filename: r.get(4)?,
            mime_type: r.get(5)?,
            total_size: r.get::<_, i64>(6)?.max(0) as u64,
            part_size: r.get::<_, i64>(7)?.max(0) as u32,
            total_parts: r.get::<_, i64>(8)?.max(0) as u32,
            file_sha256: r.get(9)?,
            chunk_hashes,
            uploaded_parts,
            status: TransferStatus::from_db_str(&status_str),
            server_file_id: r.get(13)?,
            last_error: r.get(14)?,
            created_at_ms: r.get(15)?,
            updated_at_ms: r.get(16)?,
        })
    }

    pub fn insert(&self, rec: &FileTransferRecord) -> Result<()> {
        let chunk_hashes_json = serde_json::to_string(&rec.chunk_hashes)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let uploaded_parts_json = serde_json::to_string(&rec.uploaded_parts)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let conn = self.cache.conn.lock();
        conn.execute(
            r#"INSERT OR REPLACE INTO file_transfers (
                   transfer_id, upload_id, direction, file_path, filename, mime_type,
                   total_size, part_size, total_parts, file_sha256, chunk_hashes_json,
                   uploaded_parts_json, status, server_file_id, last_error,
                   created_at_ms, updated_at_ms
               ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"#,
            params![
                &rec.transfer_id,
                rec.upload_id.as_deref(),
                &rec.direction,
                &rec.file_path,
                &rec.filename,
                &rec.mime_type,
                rec.total_size as i64,
                rec.part_size as i64,
                rec.total_parts as i64,
                &rec.file_sha256,
                chunk_hashes_json,
                uploaded_parts_json,
                rec.status.as_str(),
                rec.server_file_id.as_deref(),
                rec.last_error.as_deref(),
                rec.created_at_ms,
                rec.updated_at_ms,
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn get(&self, transfer_id: &str) -> Result<Option<FileTransferRecord>> {
        let conn = self.cache.conn.lock();
        conn.query_row(
            r#"SELECT transfer_id, upload_id, direction, file_path, filename, mime_type,
                      total_size, part_size, total_parts, file_sha256, chunk_hashes_json,
                      uploaded_parts_json, status, server_file_id, last_error,
                      created_at_ms, updated_at_ms
               FROM file_transfers WHERE transfer_id = ?1"#,
            params![transfer_id],
            Self::row_to_record,
        )
        .optional()
        .map_err(db_err)
    }

    pub fn update_upload_session(
        &self,
        transfer_id: &str,
        upload_id: Option<&str>,
        status: TransferStatus,
        now_ms: i64,
    ) -> Result<()> {
        let conn = self.cache.conn.lock();
        conn.execute(
            r#"UPDATE file_transfers
               SET upload_id = ?1, status = ?2, last_error = NULL, updated_at_ms = ?3
               WHERE transfer_id = ?4"#,
            params![upload_id, status.as_str(), now_ms, transfer_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn update_uploaded_parts(
        &self,
        transfer_id: &str,
        uploaded_parts: &[bool],
        now_ms: i64,
    ) -> Result<()> {
        let json = serde_json::to_string(uploaded_parts)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let conn = self.cache.conn.lock();
        conn.execute(
            r#"UPDATE file_transfers
               SET uploaded_parts_json = ?1, updated_at_ms = ?2
               WHERE transfer_id = ?3"#,
            params![json, now_ms, transfer_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn mark_completed(
        &self,
        transfer_id: &str,
        server_file_id: &str,
        now_ms: i64,
    ) -> Result<()> {
        let conn = self.cache.conn.lock();
        conn.execute(
            r#"UPDATE file_transfers
               SET status = 'Completed', server_file_id = ?1, last_error = NULL, updated_at_ms = ?2
               WHERE transfer_id = ?3"#,
            params![server_file_id, now_ms, transfer_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn mark_status_with_error(
        &self,
        transfer_id: &str,
        status: TransferStatus,
        error: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        let conn = self.cache.conn.lock();
        conn.execute(
            r#"UPDATE file_transfers
               SET status = ?1, last_error = ?2, updated_at_ms = ?3
               WHERE transfer_id = ?4"#,
            params![status.as_str(), error, now_ms, transfer_id],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<FileTransferRecord>> {
        let conn = self.cache.conn.lock();
        let mut stmt = conn
            .prepare(
                r#"SELECT transfer_id, upload_id, direction, file_path, filename, mime_type,
                          total_size, part_size, total_parts, file_sha256, chunk_hashes_json,
                          uploaded_parts_json, status, server_file_id, last_error,
                          created_at_ms, updated_at_ms
                   FROM file_transfers ORDER BY updated_at_ms DESC"#,
            )
            .map_err(db_err)?;
        let rows = stmt.query_map([], Self::row_to_record).map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_err)
    }
}
