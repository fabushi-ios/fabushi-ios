use crate::db::rich_store::next_pts_in_tx;
use crate::db::{MessageRecord, ServerDatabase};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MessageFlags, MsgId, PeerId};
use gpui_telegram_proto::message::{DeleteTombstone, MessageContent, MessageItem};
use gpui_telegram_proto::sync::{PtsUpdate, UpdateEvent};
use rusqlite::{params, OptionalExtension};

pub fn messages_equal_semantically(a: &MessageItem, b: &MessageItem) -> bool {
    a.chat_id == b.chat_id
        && a.sender_id == b.sender_id
        && a.topic_id == b.topic_id
        && a.content == b.content
        && a.reply_to == b.reply_to
        && a.forward_info == b.forward_info
}

pub fn validate_incoming_ids(msg: &MessageItem) -> Result<()> {
    if msg.sender_id.0 == 0 || msg.sender_id.0 > i64::MAX as u64 {
        return Err(TelegramError::Validation("Invalid sender_id out of SQLite positive range".into()));
    }
    if let Some(nonce) = msg.client_nonce {
        if nonce > i64::MAX as u64 {
            return Err(TelegramError::Validation("client_nonce exceeds signed 64-bit SQLite range".into()));
        }
    }
    Ok(())
}

pub enum TxCommitOutcome {
    Committed { item: MessageItem, update: PtsUpdate },
    Duplicate { item: MessageItem, pts: u64, created_at_ms: i64 },
}

pub fn commit_incoming_message_tx(
    db: &ServerDatabase,
    mut msg: MessageItem,
    record_to_item: fn(&MessageRecord) -> Result<MessageItem>,
    check_conflict: bool,
) -> Result<TxCommitOutcome> {
    let now_ms = msg.date.timestamp_millis();
    db.pool().with_transaction(|tx| {
        if let Some(nonce) = msg.client_nonce {
            let existing: Option<(i64, i64, i64, i64)> = tx.query_row(
                "SELECT chat_id, message_id, pts, created_at_ms FROM message_dedup WHERE sender_id = ?1 AND client_nonce = ?2",
                params![msg.sender_id.0 as i64, nonce as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;
            if let Some((cid, mid, pts, ts)) = existing {
                let rec = tx.query_row(
                    r#"SELECT chat_id, message_id, sender_id, to_peer_raw, topic_id, reply_to_id,
                              pts, date_ms, content_json, media_type, flags_bitmask, client_nonce,
                              edit_date_ms, deleted_at_ms, reply_to_json, forward_info_json, reactions_json
                       FROM messages WHERE chat_id = ?1 AND message_id = ?2"#,
                    params![cid, mid],
                    |r| Ok(MessageRecord {
                        chat_id: ChatId(r.get(0)?), message_id: MsgId(r.get(1)?),
                        sender_id: gpui_telegram_core::types::UserId(r.get::<_, i64>(2)? as u64),
                        to_peer_raw: r.get::<_, i64>(3)? as u64,
                        topic_id: r.get::<_, Option<i64>>(4)?.map(gpui_telegram_core::types::TopicId),
                        reply_to_id: r.get::<_, Option<i64>>(5)?.map(MsgId),
                        pts: r.get::<_, i64>(6)? as u64, date_ms: r.get(7)?,
                        content_json: r.get(8)?, media_type: r.get(9)?,
                        flags_bitmask: r.get::<_, i64>(10)? as u32,
                        client_nonce: r.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                        edit_date_ms: r.get(12)?, deleted_at_ms: r.get(13)?,
                        reply_to_json: r.get(14)?, forward_info_json: r.get(15)?, reactions_json: r.get(16)?,
                    }),
                ).map_err(|e| TelegramError::Database(e.to_string()))?;
                let existing_item = record_to_item(&rec)?;
                if check_conflict && !messages_equal_semantically(&existing_item, &msg) {
                    return Err(TelegramError::Validation("client_nonce conflict with different message payload".into()));
                }
                return Ok(TxCommitOutcome::Duplicate { item: existing_item, pts: pts as u64, created_at_ms: ts });
            }
        }

        let max_existing: i64 = tx.query_row(
            "SELECT COALESCE(MAX(message_id), 0) FROM messages WHERE chat_id = ?1",
            params![msg.chat_id.0], |r| r.get(0),
        ).map_err(|e| TelegramError::Database(e.to_string()))?;

        if msg.id.0 <= 0 || msg.id.is_client() {
            let next_id: i64 = tx.query_row(
                r#"INSERT INTO chat_message_sequences (chat_id, next_msg_id, account_generation, updated_at_ms)
                   VALUES (?1, ?2, 1, ?3)
                   ON CONFLICT(chat_id) DO UPDATE SET
                       next_msg_id = MAX(chat_message_sequences.next_msg_id, ?4) + 1,
                       updated_at_ms = excluded.updated_at_ms
                   RETURNING next_msg_id"#,
                params![msg.chat_id.0, max_existing + 1, now_ms, max_existing],
                |r| r.get(0),
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            msg.id = MsgId(next_id);
        }

        let content_json = serde_json::to_string(&msg.content).map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let reply_json = msg.reply_to.as_ref().and_then(|r| serde_json::to_string(r).ok());
        let fwd_json = msg.forward_info.as_ref().and_then(|f| serde_json::to_string(f).ok());
        let react_json = if msg.reactions.is_empty() { None } else { serde_json::to_string(&msg.reactions).ok() };
        let media_type = match &msg.content {
            MessageContent::Photo { .. } => Some("photo"),
            MessageContent::Video { .. } => Some("video"),
            MessageContent::Voice(_) => Some("voice"),
            MessageContent::VideoNote(_) => Some("video_note"),
            MessageContent::Document { .. } => Some("document"),
            MessageContent::Poll(_) => Some("poll"),
            MessageContent::Todo(_) => Some("todo"),
            MessageContent::GenerativeCard(_) => Some("generative_card"),
            _ => None,
        };

        let event = UpdateEvent::NewMessage(msg.clone());
        let pts = next_pts_in_tx(tx, msg.chat_id, &event, now_ms)?;

        tx.execute(
            r#"INSERT INTO messages (
                chat_id, message_id, sender_id, to_peer_raw, topic_id, reply_to_id,
                pts, date_ms, content_json, media_type, flags_bitmask, client_nonce,
                edit_date_ms, deleted_at_ms, reply_to_json, forward_info_json, reactions_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, NULL, ?13, ?14, ?15)"#,
            params![
                msg.chat_id.0, msg.id.0, msg.sender_id.0 as i64, msg.to_peer.to_raw() as i64,
                msg.topic_id.map(|t| t.0), msg.reply_to.as_ref().map(|r| r.reply_to_msg_id.0),
                pts as i64, now_ms, content_json, media_type, msg.flags.to_bits() as i64,
                msg.client_nonce.map(|n| n as i64), reply_json, fwd_json, react_json,
            ],
        ).map_err(|e| TelegramError::Database(e.to_string()))?;

        tx.execute(
            r#"UPDATE chats SET pts = ?1,
               last_message_id = CASE WHEN ?2 > last_message_id THEN ?2 ELSE last_message_id END,
               last_message_date_ms = CASE WHEN ?2 >= last_message_id THEN ?3 ELSE last_message_date_ms END,
               updated_at_ms = ?4 WHERE chat_id = ?5"#,
            params![pts as i64, msg.id.0, now_ms, now_ms, msg.chat_id.0],
        ).map_err(|e| TelegramError::Database(e.to_string()))?;

        if let Some(tid) = msg.topic_id {
            tx.execute(
                "UPDATE forum_topics SET last_message_id = MAX(COALESCE(last_message_id, 0), ?1) WHERE chat_id = ?2 AND topic_id = ?3",
                params![msg.id.0, msg.chat_id.0, tid.0],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
        }

        if let Some(nonce) = msg.client_nonce {
            tx.execute(
                "INSERT INTO message_dedup (sender_id, client_nonce, message_id, chat_id, pts, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![msg.sender_id.0 as i64, nonce as i64, msg.id.0, msg.chat_id.0, pts as i64, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
        }

        ServerDatabase::increment_peer_unread_in_tx(tx, msg.chat_id, msg.sender_id, msg.topic_id, now_ms)?;

        Ok(TxCommitOutcome::Committed {
            item: msg.clone(),
            update: PtsUpdate { chat_id: msg.chat_id, pts, pts_count: 1, event, timestamp_ms: now_ms },
        })
    })
}

pub fn edit_message_tx(
    db: &ServerDatabase,
    chat_id: ChatId,
    message_id: MsgId,
    new_content: MessageContent,
    record_to_item: fn(&MessageRecord) -> Result<MessageItem>,
) -> Result<(PtsUpdate, MessageItem)> {
    let now = chrono::Utc::now();
    let now_ms = now.timestamp_millis();
    db.pool().with_transaction(|tx| {
        let rec = tx.query_row(
            r#"SELECT chat_id, message_id, sender_id, to_peer_raw, topic_id, reply_to_id,
                      pts, date_ms, content_json, media_type, flags_bitmask, client_nonce,
                      edit_date_ms, deleted_at_ms, reply_to_json, forward_info_json, reactions_json
               FROM messages WHERE chat_id = ?1 AND message_id = ?2 AND deleted_at_ms IS NULL"#,
            params![chat_id.0, message_id.0],
            |r| Ok(MessageRecord {
                chat_id: ChatId(r.get(0)?), message_id: MsgId(r.get(1)?),
                sender_id: gpui_telegram_core::types::UserId(r.get::<_, i64>(2)? as u64),
                to_peer_raw: r.get::<_, i64>(3)? as u64,
                topic_id: r.get::<_, Option<i64>>(4)?.map(gpui_telegram_core::types::TopicId),
                reply_to_id: r.get::<_, Option<i64>>(5)?.map(MsgId),
                pts: r.get::<_, i64>(6)? as u64, date_ms: r.get(7)?,
                content_json: r.get(8)?, media_type: r.get(9)?,
                flags_bitmask: r.get::<_, i64>(10)? as u32,
                client_nonce: r.get::<_, Option<i64>>(11)?.map(|v| v as u64),
                edit_date_ms: r.get(12)?, deleted_at_ms: r.get(13)?,
                reply_to_json: r.get(14)?, forward_info_json: r.get(15)?, reactions_json: r.get(16)?,
            }),
        ).optional().map_err(|e| TelegramError::Database(e.to_string()))?
            .ok_or_else(|| TelegramError::NotFound(format!("Message not found: {}:{}", chat_id, message_id)))?;
        let mut item = record_to_item(&rec)?;
        item.content = new_content.clone();
        item.edit_date = Some(now);
        item.flags = MessageFlags::from_bits(rec.flags_bitmask);
        item.flags.is_edited = true;
        let content_json = serde_json::to_string(&new_content).map_err(|e| TelegramError::Serialization(e.to_string()))?;
        let ev = UpdateEvent::EditMessage(item.clone());
        let pts = next_pts_in_tx(tx, chat_id, &ev, now_ms)?;
        tx.execute(
            "UPDATE messages SET content_json = ?1, edit_date_ms = ?2, pts = ?3, flags_bitmask = ?4 WHERE chat_id = ?5 AND message_id = ?6",
            params![content_json, now_ms, pts as i64, item.flags.to_bits() as i64, chat_id.0, message_id.0],
        ).map_err(|e| TelegramError::Database(e.to_string()))?;
        Ok((PtsUpdate { chat_id, pts, pts_count: 1, event: ev, timestamp_ms: now_ms }, item))
    })
}

pub fn delete_messages_tx(db: &ServerDatabase, chat_id: ChatId, message_ids: Vec<MsgId>, for_everyone: bool) -> Result<PtsUpdate> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    db.pool().with_transaction(|tx| {
        for &id in &message_ids {
            tx.execute("UPDATE messages SET deleted_at_ms = ?1 WHERE chat_id = ?2 AND message_id = ?3 AND deleted_at_ms IS NULL",
                params![now_ms, chat_id.0, id.0]).map_err(|e| TelegramError::Database(e.to_string()))?;
        }
        let tombstone = DeleteTombstone::new(PeerId::Chat(chat_id), message_ids, for_everyone);
        let ev = UpdateEvent::DeleteMessages(tombstone);
        let pts = next_pts_in_tx(tx, chat_id, &ev, now_ms)?;
        Ok(PtsUpdate { chat_id, pts, pts_count: 1, event: ev, timestamp_ms: now_ms })
    })
}
