use crate::db::{MessageRecord, ServerDatabase};
use crate::messaging::atomic_tx::{
    commit_incoming_message_tx, delete_messages_tx, edit_message_tx, messages_equal_semantically,
    validate_incoming_ids, TxCommitOutcome,
};
use crate::messaging::dedup::{CachedNonceEntry, MessageNonceDedup};
use crate::sync::pts_engine::PtsEngine;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MessageFlags, MsgId, PeerId};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_proto::sync::{PtsUpdate, UpdateEvent};
use std::sync::Arc;

/// Two-tier messaging pipeline handling idempotent message deduplication and atomic PTS persistence.
#[derive(Clone)]
pub struct MessagePipeline {
    dedup: MessageNonceDedup,
    pts_engine: PtsEngine,
    db: Arc<ServerDatabase>,
}

impl std::fmt::Debug for MessagePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessagePipeline")
            .field("dedup", &self.dedup)
            .field("pts_engine", &self.pts_engine)
            .finish()
    }
}

impl MessagePipeline {
    /// Creates a new MessagePipeline wrapping deduplication and PTS engine.
    pub fn new(dedup: MessageNonceDedup, pts_engine: PtsEngine) -> Self {
        let db = pts_engine.db().clone();
        Self {
            dedup,
            pts_engine,
            db,
        }
    }

    /// Creates an in-memory MessagePipeline instance.
    pub fn open_in_memory() -> Result<Self> {
        let pts_engine = PtsEngine::open_in_memory()?;
        let dedup = MessageNonceDedup::default();
        Ok(Self::new(dedup, pts_engine))
    }

    pub fn dedup(&self) -> &MessageNonceDedup {
        &self.dedup
    }

    pub fn pts_engine(&self) -> &PtsEngine {
        &self.pts_engine
    }

    pub fn db(&self) -> &Arc<ServerDatabase> {
        &self.db
    }

    /// Processes an incoming message with two-tier deduplication and atomic PTS assignment.
    /// Idempotency guarantee: retrying with the same `(sender_id, client_nonce)` returns the
    /// existing message and PTS without allocating new sequence numbers or creating duplicates.
    /// Processes an incoming message synchronously with strict `client_nonce`
    /// payload-conflict enforcement across Tier-1 memory and Tier-2 SQLite.
    pub fn process_incoming_message(&self, msg: MessageItem) -> Result<PtsUpdate> {
        self.process_incoming_impl(msg, true)
    }

    /// Alias preserving explicit strict caller intent for HTTP/WebSocket handlers.
    pub fn process_incoming_message_strict(&self, msg: MessageItem) -> Result<PtsUpdate> {
        self.process_incoming_impl(msg, true)
    }

    fn process_incoming_impl(&self, msg: MessageItem, check_conflict: bool) -> Result<PtsUpdate> {
        validate_incoming_ids(&msg)?;

        // Tier 1: Check in-memory LRU cache (with optional strict semantic equality check)
        if let Some(nonce) = msg.client_nonce {
            if let Some(cached) = self.dedup.get(msg.sender_id.0, nonce) {
                if check_conflict && !messages_equal_semantically(&cached.message, &msg) {
                    return Err(TelegramError::Validation(
                        "client_nonce conflict with different message payload".into(),
                    ));
                }
                let event_ts_ms = cached.message.date.timestamp_millis();
                return Ok(PtsUpdate {
                    chat_id: cached.message.chat_id,
                    pts: cached.pts,
                    pts_count: 0,
                    event: UpdateEvent::NewMessage(cached.message),
                    timestamp_ms: event_ts_ms,
                });
            }
        }

        // Acquire chat striped lock bucket to serialize message_id and pts generation
        let _chat_guard = self.pts_engine.lock_chat(msg.chat_id);

        if let Some(nonce) = msg.client_nonce {
            if let Some(cached) = self.dedup.get(msg.sender_id.0, nonce) {
                if check_conflict && !messages_equal_semantically(&cached.message, &msg) {
                    return Err(TelegramError::Validation(
                        "client_nonce conflict with different message payload".into(),
                    ));
                }
                let event_ts_ms = cached.message.date.timestamp_millis();
                return Ok(PtsUpdate {
                    chat_id: cached.message.chat_id,
                    pts: cached.pts,
                    pts_count: 0,
                    event: UpdateEvent::NewMessage(cached.message),
                    timestamp_ms: event_ts_ms,
                });
            }
        }

        let outcome =
            commit_incoming_message_tx(&self.db, msg.clone(), Self::record_to_item, check_conflict)?;
        let cached_at_ms = chrono::Utc::now().timestamp_millis();

        // Insert into Tier 1 cache ONLY after SQLite transaction commits
        match outcome {
            TxCommitOutcome::Committed { item, update } => {
                if let Some(nonce) = item.client_nonce {
                    self.dedup.insert(
                        item.sender_id.0,
                        nonce,
                        CachedNonceEntry {
                            message: item,
                            pts: update.pts,
                            timestamp_ms: cached_at_ms,
                        },
                    );
                }
                Ok(update)
            }
            TxCommitOutcome::Duplicate {
                item,
                pts,
                created_at_ms,
            } => {
                if let Some(nonce) = item.client_nonce {
                    self.dedup.insert(
                        item.sender_id.0,
                        nonce,
                        CachedNonceEntry {
                            message: item.clone(),
                            pts,
                            timestamp_ms: cached_at_ms,
                        },
                    );
                }
                Ok(PtsUpdate {
                    chat_id: item.chat_id,
                    pts,
                    pts_count: 0,
                    event: UpdateEvent::NewMessage(item),
                    timestamp_ms: created_at_ms,
                })
            }
        }
    }

    /// Edits an existing message and advances PTS atomically in one transaction.
    pub fn edit_message(
        &self,
        chat_id: ChatId,
        message_id: MsgId,
        new_content: MessageContent,
    ) -> Result<PtsUpdate> {
        let _chat_guard = self.pts_engine.lock_chat(chat_id);
        let (update, _item) = edit_message_tx(&self.db, chat_id, message_id, new_content, Self::record_to_item)?;
        Ok(update)
    }

    /// Soft-deletes messages and records a DeleteMessages update atomically.
    pub fn delete_messages(
        &self,
        chat_id: ChatId,
        message_ids: Vec<MsgId>,
        for_everyone: bool,
    ) -> Result<PtsUpdate> {
        let _chat_guard = self.pts_engine.lock_chat(chat_id);
        delete_messages_tx(&self.db, chat_id, message_ids, for_everyone)
    }

    /// Converts a raw database record to a typed MessageItem.
    pub fn record_to_item(rec: &MessageRecord) -> Result<MessageItem> {
        let content: MessageContent = serde_json::from_str(&rec.content_json)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let to_peer = PeerId::from_raw(rec.to_peer_raw);
        let date = chrono::DateTime::from_timestamp_millis(rec.date_ms)
            .unwrap_or_else(chrono::Utc::now);
        let edit_date = rec.edit_date_ms.and_then(chrono::DateTime::from_timestamp_millis);
        let reply_to = rec
            .reply_to_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let forward_info = rec
            .forward_info_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());
        let reactions = rec
            .reactions_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        Ok(MessageItem {
            id: rec.message_id,
            chat_id: rec.chat_id,
            peer_id: to_peer,
            to_peer,
            sender_id: rec.sender_id,
            topic_id: rec.topic_id,
            flags: MessageFlags::from_bits(rec.flags_bitmask),
            date,
            edit_date,
            content,
            reply_to,
            forward_info,
            reactions,
            client_nonce: rec.client_nonce,
            inline_keyboard: None,
            ttl_seconds: None,
            expires_at_ms: None,
        })
    }
}
