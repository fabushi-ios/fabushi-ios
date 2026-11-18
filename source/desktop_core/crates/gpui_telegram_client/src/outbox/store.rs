use crate::cache::sqlite_cache::{db_err, SqliteClientCache};
use crate::net::RichSendMessageRequest;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_proto::message::MessageContent;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboxStatus {
    Pending,
    InFlight,
    Delivered,
    Failed,
}

impl OutboxStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutboxStatus::Pending => "Pending",
            OutboxStatus::InFlight => "InFlight",
            OutboxStatus::Delivered => "Delivered",
            OutboxStatus::Failed => "Failed",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s.trim() {
            "InFlight" | "Sending" => OutboxStatus::InFlight,
            "Delivered" | "Sent" => OutboxStatus::Delivered,
            "Failed" => OutboxStatus::Failed,
            _ => OutboxStatus::Pending,
        }
    }

    pub fn can_transition_to(self, next: OutboxStatus) -> bool {
        matches!(
            (self, next),
            (OutboxStatus::Pending, OutboxStatus::InFlight)
                | (OutboxStatus::Pending, OutboxStatus::Failed)
                | (OutboxStatus::InFlight, OutboxStatus::Delivered)
                | (OutboxStatus::InFlight, OutboxStatus::Pending)
                | (OutboxStatus::InFlight, OutboxStatus::Failed)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxMessageRecord {
    pub client_nonce: u64,
    pub chat_id: i64,
    pub payload_json: String,
    pub text_preview: String,
    pub status: OutboxStatus,
    pub retry_count: u32,
    pub next_retry_at_ms: i64,
    pub last_error: Option<String>,
    pub server_message_id: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl OutboxMessageRecord {
    pub fn decode_request(&self) -> Result<RichSendMessageRequest> {
        serde_json::from_str(&self.payload_json)
            .map_err(|e| TelegramError::Serialization(e.to_string()))
    }
}

pub(crate) fn preview_from_request(req: &RichSendMessageRequest) -> String {
    if let Some(ref t) = req.text {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return trimmed.chars().take(120).collect();
        }
    }
    match req.content.as_ref() {
        Some(MessageContent::Text { text, .. }) => text.trim().chars().take(120).collect(),
        Some(MessageContent::Encrypted(_)) => "[Encrypted message]".to_string(),
        Some(MessageContent::Poll(p)) => format!("[Poll: {}]", p.question),
        Some(MessageContent::GenerativeCard(g)) => format!("[Card: {}]", g.title),
        Some(MessageContent::Photo { .. }) => "[Photo]".to_string(),
        Some(MessageContent::Video { .. }) => "[Video]".to_string(),
        Some(MessageContent::Document { file_name, .. }) => format!("[File: {file_name}]"),
        Some(_) => "[Media]".to_string(),
        None => String::new(),
    }
}

#[derive(Clone)]
pub struct OutboxStore {
    cache: Arc<SqliteClientCache>,
}

impl std::fmt::Debug for OutboxStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxStore").finish()
    }
}

impl OutboxStore {
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

    fn row_to_record(r: &rusqlite::Row<'_>) -> rusqlite::Result<OutboxMessageRecord> {
        let status_raw: String = r.get(4)?;
        Ok(OutboxMessageRecord {
            client_nonce: r.get::<_, i64>(0)?.max(0) as u64,
            chat_id: r.get(1)?,
            payload_json: r.get(2)?,
            text_preview: r.get(3)?,
            status: OutboxStatus::from_db_str(&status_raw),
            retry_count: r.get::<_, i64>(5)?.max(0) as u32,
            next_retry_at_ms: r.get(6)?,
            last_error: r.get(7)?,
            server_message_id: r.get(8)?,
            created_at_ms: r.get(9)?,
            updated_at_ms: r.get(10)?,
        })
    }

    pub fn enqueue(
        &self,
        req: &RichSendMessageRequest,
        nonce: u64,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        self.insert_with_status(req, nonce, OutboxStatus::Pending, now_ms)
    }

    pub fn enqueue_in_flight(
        &self,
        req: &RichSendMessageRequest,
        nonce: u64,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        self.insert_with_status(req, nonce, OutboxStatus::InFlight, now_ms)
    }

    fn insert_with_status(
        &self,
        req: &RichSendMessageRequest,
        nonce: u64,
        initial_status: OutboxStatus,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        if req.chat_id <= 0 {
            return Err(TelegramError::Validation("Outbox chat_id must be positive".into()));
        }
        if nonce == 0 || nonce > i64::MAX as u64 {
            return Err(TelegramError::Validation("client_nonce out of valid i64 range".into()));
        }
        let has_content = req.content.is_some()
            || req.text.as_deref().is_some_and(|t| !t.trim().is_empty())
            || req.forward_from_msg_id.is_some();
        if !has_content {
            return Err(TelegramError::Validation("Outbox message must have text or content".into()));
        }
        let payload_json = serde_json::to_string(req)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let text_preview = preview_from_request(req);
        let conn = self.cache.conn.lock();
        conn.execute(
            r#"INSERT INTO outbox_messages (
                   client_nonce, chat_id, payload_json, text_preview, status,
                   retry_count, next_retry_at_ms, last_error, server_message_id,
                   created_at_ms, updated_at_ms
               ) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, NULL, NULL, ?6, ?6)"#,
            params![nonce as i64, req.chat_id, payload_json, text_preview, initial_status.as_str(), now_ms],
        )
        .map_err(db_err)?;

        conn.query_row(
            r#"SELECT client_nonce, chat_id, payload_json, text_preview, status,
                      retry_count, next_retry_at_ms, last_error, server_message_id,
                      created_at_ms, updated_at_ms
               FROM outbox_messages WHERE client_nonce = ?1"#,
            params![nonce as i64],
            Self::row_to_record,
        )
        .map_err(db_err)
    }

    pub fn recover_in_flight(&self, now_ms: i64) -> Result<usize> {
        let conn = self.cache.conn.lock();
        let n = conn
            .execute(
                r#"UPDATE outbox_messages
                   SET status = 'Pending', next_retry_at_ms = 0, updated_at_ms = ?1
                   WHERE status IN ('InFlight', 'Sending')"#,
                params![now_ms],
            )
            .map_err(db_err)?;
        Ok(n)
    }

    pub fn claim_next_due(&self, now_ms: i64) -> Result<Option<OutboxMessageRecord>> {
        let conn = self.cache.conn.lock();
        let candidate: Option<i64> = conn
            .query_row(
                r#"SELECT client_nonce FROM outbox_messages
                   WHERE status = 'Pending' AND next_retry_at_ms <= ?1
                   ORDER BY client_nonce ASC LIMIT 1"#,
                params![now_ms],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_err)?;

        let Some(nonce_i64) = candidate else {
            return Ok(None);
        };
        let changed = conn
            .execute(
                r#"UPDATE outbox_messages
                   SET status = 'InFlight', updated_at_ms = ?1
                   WHERE client_nonce = ?2 AND status = 'Pending'"#,
                params![now_ms, nonce_i64],
            )
            .map_err(db_err)?;
        if changed == 0 {
            return Ok(None);
        }
        conn.query_row(
            r#"SELECT client_nonce, chat_id, payload_json, text_preview, status,
                      retry_count, next_retry_at_ms, last_error, server_message_id,
                      created_at_ms, updated_at_ms
               FROM outbox_messages WHERE client_nonce = ?1"#,
            params![nonce_i64],
            Self::row_to_record,
        )
        .optional()
        .map_err(db_err)
    }

    pub fn mark_delivered(
        &self,
        client_nonce: u64,
        server_message_id: i64,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        self.transition_record(
            client_nonce,
            OutboxStatus::Delivered,
            None,
            None,
            None,
            Some(server_message_id),
            now_ms,
        )
    }

    pub fn schedule_retry(
        &self,
        client_nonce: u64,
        retry_count: u32,
        next_retry_at_ms: i64,
        error: &str,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        self.transition_record(
            client_nonce,
            OutboxStatus::Pending,
            Some(retry_count),
            Some(next_retry_at_ms),
            Some(error.to_string()),
            None,
            now_ms,
        )
    }

    pub fn mark_failed(
        &self,
        client_nonce: u64,
        error: &str,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        self.transition_record(
            client_nonce,
            OutboxStatus::Failed,
            None,
            None,
            Some(error.to_string()),
            None,
            now_ms,
        )
    }

    fn transition_record(
        &self,
        client_nonce: u64,
        next_status: OutboxStatus,
        retry_count: Option<u32>,
        next_retry_at_ms: Option<i64>,
        last_error: Option<String>,
        server_message_id: Option<i64>,
        now_ms: i64,
    ) -> Result<OutboxMessageRecord> {
        let conn = self.cache.conn.lock();
        let current: OutboxMessageRecord = conn
            .query_row(
                r#"SELECT client_nonce, chat_id, payload_json, text_preview, status,
                          retry_count, next_retry_at_ms, last_error, server_message_id,
                          created_at_ms, updated_at_ms
                   FROM outbox_messages WHERE client_nonce = ?1"#,
                params![client_nonce as i64],
                Self::row_to_record,
            )
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| TelegramError::NotFound(format!("Outbox nonce {client_nonce} not found")))?;

        if !current.status.can_transition_to(next_status) {
            return Err(TelegramError::Validation(format!(
                "Invalid outbox status transition {:?} -> {:?}",
                current.status, next_status
            )));
        }

        let final_retry = retry_count.unwrap_or(current.retry_count);
        let final_next_retry = next_retry_at_ms.unwrap_or(current.next_retry_at_ms);
        let final_err = match next_status {
            OutboxStatus::Delivered => None,
            _ => last_error.or(current.last_error),
        };
        let final_msg_id = server_message_id.or(current.server_message_id);

        conn.execute(
            r#"UPDATE outbox_messages
               SET status = ?1,
                   retry_count = ?2,
                   next_retry_at_ms = ?3,
                   last_error = ?4,
                   server_message_id = ?5,
                   updated_at_ms = ?6
               WHERE client_nonce = ?7"#,
            params![
                next_status.as_str(),
                final_retry as i64,
                final_next_retry,
                final_err,
                final_msg_id,
                now_ms,
                client_nonce as i64,
            ],
        )
        .map_err(db_err)?;

        conn.query_row(
            r#"SELECT client_nonce, chat_id, payload_json, text_preview, status,
                      retry_count, next_retry_at_ms, last_error, server_message_id,
                      created_at_ms, updated_at_ms
               FROM outbox_messages WHERE client_nonce = ?1"#,
            params![client_nonce as i64],
            Self::row_to_record,
        )
        .map_err(db_err)
    }

    pub fn reset_all_pending_retry_times(&self, now_ms: i64) -> Result<usize> {
        let conn = self.cache.conn.lock();
        let n = conn
            .execute(
                r#"UPDATE outbox_messages
                   SET next_retry_at_ms = 0, updated_at_ms = ?1
                   WHERE status = 'Pending'"#,
                params![now_ms],
            )
            .map_err(db_err)?;
        Ok(n)
    }

    pub fn get(&self, client_nonce: u64) -> Result<Option<OutboxMessageRecord>> {
        let conn = self.cache.conn.lock();
        conn.query_row(
            r#"SELECT client_nonce, chat_id, payload_json, text_preview, status,
                      retry_count, next_retry_at_ms, last_error, server_message_id,
                      created_at_ms, updated_at_ms
               FROM outbox_messages WHERE client_nonce = ?1"#,
            params![client_nonce as i64],
            Self::row_to_record,
        )
        .optional()
        .map_err(db_err)
    }

    pub fn list_by_status(&self, status: Option<OutboxStatus>) -> Result<Vec<OutboxMessageRecord>> {
        let conn = self.cache.conn.lock();
        match status {
            Some(st) => {
                let mut stmt = conn
                    .prepare(
                        r#"SELECT client_nonce, chat_id, payload_json, text_preview, status,
                                  retry_count, next_retry_at_ms, last_error, server_message_id,
                                  created_at_ms, updated_at_ms
                           FROM outbox_messages WHERE status = ?1
                           ORDER BY client_nonce ASC"#,
                    )
                    .map_err(db_err)?;
                let rows = stmt
                    .query_map(params![st.as_str()], Self::row_to_record)
                    .map_err(db_err)?;
                rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
            }
            None => {
                let mut stmt = conn
                    .prepare(
                        r#"SELECT client_nonce, chat_id, payload_json, text_preview, status,
                                  retry_count, next_retry_at_ms, last_error, server_message_id,
                                  created_at_ms, updated_at_ms
                           FROM outbox_messages
                           ORDER BY client_nonce ASC"#,
                    )
                    .map_err(db_err)?;
                let rows = stmt.query_map([], Self::row_to_record).map_err(db_err)?;
                rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
            }
        }
    }

    pub fn pending_count(&self) -> usize {
        let conn = self.cache.conn.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM outbox_messages WHERE status IN ('Pending', 'InFlight')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as usize
    }

    pub fn max_nonce(&self) -> u64 {
        self.cache.max_outbox_nonce()
    }
}
