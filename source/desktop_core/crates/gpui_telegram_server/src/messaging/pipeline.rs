use crate::db::{MessageDedupRecord, MessageRecord, ServerDatabase};
use crate::messaging::dedup::{CachedNonceEntry, MessageNonceDedup};
use crate::sync::pts_engine::PtsEngine;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MessageFlags, MsgId, PeerId};
use gpui_telegram_proto::message::{DeleteTombstone, MessageContent, MessageItem};
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
    pub fn process_incoming_message(&self, mut msg: MessageItem) -> Result<PtsUpdate> {
        // Tier 1: Check in-memory LRU cache
        if let Some(nonce) = msg.client_nonce {
            if let Some(cached) = self.dedup.get(msg.sender_id.0, nonce) {
                return Ok(PtsUpdate {
                    chat_id: cached.message.chat_id,
                    pts: cached.pts,
                    pts_count: 0,
                    event: UpdateEvent::NewMessage(cached.message),
                    timestamp_ms: cached.timestamp_ms,
                });
            }

            // Tier 2: Check SQLite durable unique constraint table
            if let Some(dedup_row) = self.db.find_dedup_message(msg.sender_id.0, nonce)? {
                if let Some(existing_record) = self.db.get_message(dedup_row.chat_id, dedup_row.message_id)? {
                    let existing_item = Self::record_to_item(&existing_record)?;
                    let cached_entry = CachedNonceEntry {
                        message: existing_item.clone(),
                        pts: dedup_row.pts,
                        timestamp_ms: dedup_row.created_at_ms,
                    };
                    self.dedup.insert(msg.sender_id.0, nonce, cached_entry);

                    return Ok(PtsUpdate {
                        chat_id: existing_item.chat_id,
                        pts: dedup_row.pts,
                        pts_count: 0,
                        event: UpdateEvent::NewMessage(existing_item),
                        timestamp_ms: dedup_row.created_at_ms,
                    });
                }
            }
        }

        // New message path: assign server message ID if client or unassigned
        if msg.id.0 <= 0 || msg.id.is_client() {
            msg.id = self.db.next_message_id(msg.chat_id)?;
        }

        let now_ms = msg.date.timestamp_millis();
        let content_json = serde_json::to_string(&msg.content)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;

        // Advance PTS atomically in SQLite transaction
        let (pts, update) = self.pts_engine.advance_pts(
            msg.chat_id,
            1,
            UpdateEvent::NewMessage(msg.clone()),
        )?;

        // Persist message record to SQLite
        let record = MessageRecord {
            chat_id: msg.chat_id,
            message_id: msg.id,
            sender_id: msg.sender_id,
            to_peer_raw: msg.to_peer.to_raw(),
            topic_id: msg.topic_id,
            reply_to_id: msg.reply_to.as_ref().map(|r| r.reply_to_msg_id),
            pts,
            date_ms: now_ms,
            content_json,
            media_type: None,
            flags_bitmask: msg.flags.to_bits(),
            client_nonce: msg.client_nonce,
            edit_date_ms: None,
            deleted_at_ms: None,
            reply_to_json: msg.reply_to.as_ref().and_then(|r| serde_json::to_string(r).ok()),
            forward_info_json: msg.forward_info.as_ref().and_then(|f| serde_json::to_string(f).ok()),
            reactions_json: if msg.reactions.is_empty() {
                None
            } else {
                serde_json::to_string(&msg.reactions).ok()
            },
        };
        self.db.save_message(&record)?;

        // If client_nonce is present, record in durable dedup table and memory LRU
        if let Some(nonce) = msg.client_nonce {
            let dedup_record = MessageDedupRecord {
                sender_id: msg.sender_id,
                client_nonce: nonce,
                message_id: msg.id,
                chat_id: msg.chat_id,
                pts,
                created_at_ms: now_ms,
            };
            self.db.insert_dedup_record(&dedup_record)?;

            self.dedup.insert(
                msg.sender_id.0,
                nonce,
                CachedNonceEntry {
                    message: msg.clone(),
                    pts,
                    timestamp_ms: now_ms,
                },
            );
        }

        Ok(update)
    }

    /// Edits an existing message and advances PTS.
    pub fn edit_message(
        &self,
        chat_id: ChatId,
        message_id: MsgId,
        new_content: MessageContent,
    ) -> Result<PtsUpdate> {
        let existing = self
            .db
            .get_message(chat_id, message_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Message not found: {}:{}", chat_id, message_id)))?;

        let mut item = Self::record_to_item(&existing)?;
        item.content = new_content.clone();
        item.edit_date = Some(chrono::Utc::now());
        item.flags.is_edited = true;

        let content_json = serde_json::to_string(&new_content)
            .map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let now_ms = chrono::Utc::now().timestamp_millis();

        let (pts, update) = self.pts_engine.advance_pts(
            chat_id,
            1,
            UpdateEvent::EditMessage(item.clone()),
        )?;

        let mut record = existing;
        record.content_json = content_json;
        record.edit_date_ms = Some(now_ms);
        record.pts = pts;
        record.flags_bitmask = item.flags.to_bits();
        self.db.save_message(&record)?;

        Ok(update)
    }

    /// Soft-deletes messages and records a DeleteMessages update.
    pub fn delete_messages(
        &self,
        chat_id: ChatId,
        message_ids: Vec<MsgId>,
        for_everyone: bool,
    ) -> Result<PtsUpdate> {
        let now_ms = chrono::Utc::now().timestamp_millis();

        for &id in &message_ids {
            if let Some(mut rec) = self.db.get_message(chat_id, id)? {
                rec.deleted_at_ms = Some(now_ms);
                self.db.save_message(&rec)?;
            }
        }

        let tombstone = DeleteTombstone::new(PeerId::Chat(chat_id), message_ids, for_everyone);
        let (_, update) = self.pts_engine.advance_pts(
            chat_id,
            1,
            UpdateEvent::DeleteMessages(tombstone),
        )?;

        Ok(update)
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
        })
    }
}
