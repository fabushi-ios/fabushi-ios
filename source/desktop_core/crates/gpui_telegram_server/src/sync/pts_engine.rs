use crate::db::ServerDatabase;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::ChatId;
use gpui_telegram_proto::sync::{PtsUpdate, SyncGetDifferenceRequest, SyncGetDifferenceResponse, UpdateEvent};
use parking_lot::Mutex;
use rusqlite::params;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub const STRIPED_LOCK_BUCKETS: usize = 64;
pub const JOURNAL_RETENTION_UPDATES: i64 = 10_000;
pub const JOURNAL_RETENTION_MAX_AGE_MS: i64 = 30 * 24 * 3600 * 1000; // 30 days

/// Atomic monotonic contiguous PTS sequence engine backed by SQLite WAL persistence.
#[derive(Clone)]
pub struct PtsEngine {
    db: Arc<ServerDatabase>,
    striped_locks: Arc<Vec<Mutex<()>>>,
}

impl std::fmt::Debug for PtsEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtsEngine")
            .field("db", &self.db)
            .field("striped_buckets", &STRIPED_LOCK_BUCKETS)
            .finish()
    }
}

impl PtsEngine {
    /// Creates a new PtsEngine backed by `ServerDatabase`.
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        let mut locks = Vec::with_capacity(STRIPED_LOCK_BUCKETS);
        for _ in 0..STRIPED_LOCK_BUCKETS {
            locks.push(Mutex::new(()));
        }
        Self {
            db,
            striped_locks: Arc::new(locks),
        }
    }

    /// Creates an in-memory PtsEngine instance (ideal for tests).
    pub fn open_in_memory() -> Result<Self> {
        let db = Arc::new(ServerDatabase::open_in_memory()?);
        Ok(Self::new(db))
    }

    pub fn db(&self) -> &Arc<ServerDatabase> {
        &self.db
    }

    /// Acquires the striped lock bucket for `chat_id`.
    fn lock_chat(&self, chat_id: ChatId) -> parking_lot::MutexGuard<'_, ()> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        chat_id.hash(&mut hasher);
        let bucket = (hasher.finish() as usize) % STRIPED_LOCK_BUCKETS;
        self.striped_locks[bucket].lock()
    }

    /// Atomically increments the chat's monotonic PTS by `count` and records the update event.
    /// Invariant: Zero gaps, zero duplicates.
    pub fn advance_pts(&self, chat_id: ChatId, count: u64, event: UpdateEvent) -> Result<(u64, PtsUpdate)> {
        if count == 0 {
            let current = self.db.get_current_pts(chat_id)?;
            let update = PtsUpdate {
                chat_id,
                pts: current,
                pts_count: 0,
                event,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
            };
            return Ok((current, update));
        }

        let _guard = self.lock_chat(chat_id);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let payload_json = serde_json::to_string(&event)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;

        let event_type = match &event {
            UpdateEvent::NewMessage(_) => "new_message",
            UpdateEvent::EditMessage(_) => "edit_message",
            UpdateEvent::DeleteMessages(_) => "delete_messages",
            UpdateEvent::UserTyping(_) => "user_typing",
            UpdateEvent::ChatAction(_) => "chat_action",
            UpdateEvent::ReadHistory(_) => "read_history",
            UpdateEvent::MessageReactions(_) => "message_reactions",
            UpdateEvent::ReactionUpdated(_) => "reaction_updated",
            UpdateEvent::UserStatusChanged { .. } => "user_status_changed",
            UpdateEvent::PinnedMessageChanged { .. } => "pinned_message_changed",
            UpdateEvent::ReadHistoryOut { .. } => "read_history_out",
            UpdateEvent::ReadHistoryIn { .. } => "read_history_in",
        };

        let (new_pts, pts_update) = self.db.pool().with_transaction(|tx| {
            // 1. Atomically advance PTS in pts_sequences
            let current_pts: i64 = tx
                .query_row(
                    r#"
                    INSERT INTO pts_sequences (chat_id, pts, account_generation, updated_at_ms)
                    VALUES (?1, ?2, 1, ?3)
                    ON CONFLICT(chat_id) DO UPDATE SET
                        pts = pts_sequences.pts + excluded.pts,
                        updated_at_ms = excluded.updated_at_ms
                    RETURNING pts
                    "#,
                    params![chat_id.0, count as i64, now_ms],
                    |row| row.get(0),
                )
                .map_err(|e| TelegramError::Database(format!("Failed to increment pts: {}", e)))?;

            let new_pts = current_pts as u64;

            // 2. Insert into pts_updates
            tx.execute(
                r#"
                INSERT INTO pts_updates (chat_id, pts, pts_count, date_ms, event_type, payload_json, account_generation)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
                "#,
                params![
                    chat_id.0,
                    new_pts as i64,
                    count as i64,
                    now_ms,
                    event_type,
                    payload_json,
                ],
            )
            .map_err(|e| TelegramError::Database(format!("Failed to record pts update: {}", e)))?;

            // 3. Journal floor retention: prune updates older than 10,000 updates or 30 days
            let max_age_cutoff = now_ms - JOURNAL_RETENTION_MAX_AGE_MS;
            tx.execute(
                r#"
                DELETE FROM pts_updates
                WHERE chat_id = ?1
                  AND (pts <= ?2 - ?3 OR date_ms < ?4)
                "#,
                params![
                    chat_id.0,
                    new_pts as i64,
                    JOURNAL_RETENTION_UPDATES,
                    max_age_cutoff,
                ],
            )
            .map_err(|e| TelegramError::Database(format!("Failed to prune pts_updates: {}", e)))?;

            let update = PtsUpdate {
                chat_id,
                pts: new_pts,
                pts_count: count as u32,
                event: event.clone(),
                timestamp_ms: now_ms,
            };

            Ok((new_pts, update))
        })?;

        Ok((new_pts, pts_update))
    }

    /// Convenience wrapper for single-increment update events.
    pub fn append_update(&self, chat_id: ChatId, event: UpdateEvent) -> Result<PtsUpdate> {
        let (_, update) = self.advance_pts(chat_id, 1, event)?;
        Ok(update)
    }

    /// Returns the current monotonic PTS for `chat_id`.
    pub fn current_pts(&self, chat_id: ChatId) -> Result<u64> {
        self.db.get_current_pts(chat_id)
    }

    /// Returns the lowest retained PTS in the journal for `chat_id`.
    pub fn journal_floor_pts(&self, chat_id: ChatId) -> Result<u64> {
        self.db.get_journal_floor_pts(chat_id)
    }

    /// Delegates difference catch-up queries to `SyncDifferenceService`.
    pub fn get_difference(&self, req: &SyncGetDifferenceRequest) -> Result<SyncGetDifferenceResponse> {
        crate::sync::diff::SyncDifferenceService::new(self.db.clone()).get_difference(req)
    }

    /// Legacy catch-up query wrapper.
    pub fn get_difference_legacy(
        &self,
        chat_id: ChatId,
        from_pts: u64,
        limit: u32,
    ) -> Result<SyncGetDifferenceResponse> {
        let req = SyncGetDifferenceRequest::new(chat_id, from_pts, limit);
        self.get_difference(&req)
    }

    /// Manually prunes old journal entries preserving at least `keep_limit` latest updates.
    pub fn prune_journal(&self, chat_id: ChatId, keep_limit: usize, max_age_ms: i64) -> Result<usize> {
        let _guard = self.lock_chat(chat_id);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let cutoff_ms = now_ms - max_age_ms;

        self.db.pool().with_transaction(|tx| {
            let current_pts: i64 = tx
                .query_row(
                    "SELECT pts FROM pts_sequences WHERE chat_id = ?1",
                    params![chat_id.0],
                    |row| row.get(0),
                )
                .unwrap_or(0);

            let deleted = tx
                .execute(
                    r#"
                    DELETE FROM pts_updates
                    WHERE chat_id = ?1
                      AND (pts <= ?2 - ?3 OR date_ms < ?4)
                    "#,
                    params![chat_id.0, current_pts, keep_limit as i64, cutoff_ms],
                )
                .map_err(|e| TelegramError::Database(e.to_string()))?;

            Ok(deleted)
        })
    }
}
