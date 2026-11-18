use crate::db::rich_store::next_pts_in_tx;
use crate::db::{MessageRecord, ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MessageFlags, MessageId, MsgId, PeerId, TopicId, UserId};
use gpui_telegram_proto::cards::{PollAnswer, PollCard, PollOption};
use gpui_telegram_proto::message::{MessageContent, MessageItem};
use gpui_telegram_proto::sync::UpdateEvent;
use rusqlite::{params, OptionalExtension};

impl ServerDatabase {
    #[allow(clippy::too_many_arguments)]
    pub fn create_poll_atomic(
        &self,
        chat_id: ChatId,
        creator_id: UserId,
        question: &str,
        option_texts: &[String],
        is_anonymous: bool,
        multiple_choice: bool,
        is_quiz: bool,
        correct_option_id: Option<u8>,
        solution: Option<&str>,
        topic_id: Option<i64>,
    ) -> Result<(MsgId, u64, PollCard, MessageItem)> {
        if option_texts.len() < 2 || option_texts.len() > 10 {
            return Err(TelegramError::Validation("Polls require between 2 and 10 options".into()));
        }
        if is_quiz {
            match correct_option_id {
                Some(idx) if (idx as usize) < option_texts.len() => {}
                _ => return Err(TelegramError::Validation("Quiz requires valid correct_option_id".into())),
            }
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let max_existing: i64 = tx.query_row("SELECT COALESCE(MAX(message_id), 0) FROM messages WHERE chat_id = ?1", params![chat_id.0], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let msg_id_i64: i64 = tx.query_row(
                r#"INSERT INTO chat_message_sequences (chat_id, next_msg_id, account_generation, updated_at_ms)
                   VALUES (?1, ?2, 1, ?3)
                   ON CONFLICT(chat_id) DO UPDATE SET
                       next_msg_id = MAX(chat_message_sequences.next_msg_id, ?4) + 1,
                       updated_at_ms = excluded.updated_at_ms
                   RETURNING next_msg_id"#,
                params![chat_id.0, max_existing + 1, now_ms, max_existing],
                |r| r.get(0),
            ).map_err(|e| TelegramError::Database(e.to_string()))?;

            tx.execute(
                r#"INSERT INTO polls (chat_id, message_id, creator_id, question, is_anonymous, multiple_choice, is_quiz, correct_option_id, solution, is_closed, close_period, close_date, created_at_ms)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, NULL, NULL, ?10)"#,
                params![chat_id.0, msg_id_i64, creator_id.0 as i64, question, is_anonymous as i64, multiple_choice as i64, is_quiz as i64, correct_option_id.map(|v| v as i64), solution, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            let poll_id = tx.last_insert_rowid();

            let mut options = Vec::with_capacity(option_texts.len());
            for (idx, opt_text) in option_texts.iter().enumerate() {
                tx.execute("INSERT INTO poll_options (poll_id, option_id, text, vote_count) VALUES (?1, ?2, ?3, 0)",
                    params![poll_id, idx as i64, opt_text]).map_err(|e| TelegramError::Database(e.to_string()))?;
                options.push(PollOption::new(idx as u8, opt_text.clone()));
            }
            let card = PollCard {
                id: poll_id.to_string(), poll_id: poll_id as u64, question: question.to_string(),
                answers: options.iter().map(PollAnswer::from).collect(), options,
                total_voters: 0, total_votes: 0, is_closed: false, is_anonymous,
                allows_multiple_answers: multiple_choice, is_quiz,
                correct_option_id: if is_quiz { None } else { correct_option_id },
                solution: if is_quiz { None } else { solution.map(str::to_string) },
                solution_entities: Vec::new(), close_period: None, close_date: None,
            };
            let content = MessageContent::Poll(card.clone());
            let mut item = MessageItem::new(MessageId(msg_id_i64), chat_id, creator_id, PeerId::Chat(chat_id), content.clone());
            item.topic_id = topic_id.map(TopicId);
            item.flags = MessageFlags::default();
            let ev = UpdateEvent::NewMessage(item.clone());
            let pts = next_pts_in_tx(tx, chat_id, &ev, now_ms)?;
            let content_json = serde_json::to_string(&content).map_err(|e| TelegramError::Serialization(e.to_string()))?;
            tx.execute(
                r#"INSERT INTO messages (chat_id, message_id, sender_id, to_peer_raw, topic_id, reply_to_id, pts, date_ms, content_json, media_type, flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms, reply_to_json, forward_info_json, reactions_json)
                   VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, 'poll', 0, NULL, NULL, NULL, NULL, NULL, NULL)"#,
                params![chat_id.0, msg_id_i64, creator_id.0 as i64, PeerId::Chat(chat_id).to_raw() as i64, topic_id, pts as i64, now_ms, content_json],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("UPDATE chats SET last_message_id = MAX(last_message_id, ?1), last_message_date_ms = ?2, updated_at_ms = ?2 WHERE chat_id = ?3",
                params![msg_id_i64, now_ms, chat_id.0]).map_err(|e| TelegramError::Database(e.to_string()))?;
            Self::increment_peer_unread_in_tx(tx, chat_id, creator_id, topic_id.map(TopicId), now_ms)?;
            Ok((MsgId(msg_id_i64), pts, card, item))
        })
    }

    pub fn close_poll_atomic(&self, poll_id: i64, requester: UserId) -> Result<(ChatId, MsgId, u64, PollCard)> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let row: Option<(i64, i64, i64, String, i64, i64, i64, Option<i64>, Option<String>, Option<i64>, Option<i64>)> = tx.query_row(
                "SELECT chat_id, message_id, creator_id, question, is_anonymous, multiple_choice, is_quiz, correct_option_id, solution, close_period, close_date FROM polls WHERE poll_id = ?1",
                params![poll_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?)),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;
            let (cid, mid, owner, question, is_anon, mult, is_quiz, correct_opt, solution, cp, cd) =
                row.ok_or_else(|| TelegramError::NotFound("Poll not found".into()))?;
            if owner as u64 != requester.0 {
                return Err(TelegramError::PermissionDenied("Only poll creator can close poll".into()));
            }
            tx.execute("UPDATE polls SET is_closed = 1 WHERE poll_id = ?1", params![poll_id]).map_err(|e| TelegramError::Database(e.to_string()))?;
            let card = Self::build_poll_card_tx(tx, poll_id, requester, question, is_anon != 0, mult != 0, is_quiz != 0, correct_opt.map(|v| v as u8), solution, true, cp.map(|v| v as u32), cd)?;
            let content_json = serde_json::to_string(&MessageContent::Poll(card.clone())).unwrap_or_default();
            tx.execute("UPDATE messages SET content_json = ?1 WHERE chat_id = ?2 AND message_id = ?3", params![content_json, cid, mid])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let ev = UpdateEvent::PollUpdated { chat_id: ChatId(cid), message_id: MessageId(mid), poll: card.clone() };
            let pts = next_pts_in_tx(tx, ChatId(cid), &ev, now_ms)?;
            Ok((ChatId(cid), MsgId(mid), pts, card))
        })
    }

    pub fn get_poll_for_user(&self, poll_id: i64, viewer: UserId) -> Result<Option<(ChatId, PollCard)>> {
        self.pool.with_reader(|conn| {
            let row: Option<(i64, String, i64, i64, i64, Option<i64>, Option<String>, i64, Option<i64>, Option<i64>)> = conn.query_row(
                "SELECT chat_id, question, is_anonymous, multiple_choice, is_quiz, correct_option_id, solution, is_closed, close_period, close_date FROM polls WHERE poll_id = ?1",
                params![poll_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;
            match row {
                Some((cid, q, anon, mult, quiz, corr, sol, closed, cp, cd)) => {
                    let card = Self::build_poll_card_conn(conn, poll_id, viewer, q, anon != 0, mult != 0, quiz != 0, corr.map(|v| v as u8), sol, closed != 0, cp.map(|v| v as u32), cd)?;
                    Ok(Some((ChatId(cid), card)))
                }
                None => Ok(None),
            }
        })
    }

    pub fn save_message_extras(&self, chat_id: ChatId, message_id: MsgId, inline_keyboard_json: Option<&str>, scheduled_at_ms: Option<i64>) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE messages SET inline_keyboard_json = COALESCE(?1, inline_keyboard_json), scheduled_at_ms = COALESCE(?2, scheduled_at_ms) WHERE chat_id = ?3 AND message_id = ?4",
                params![inline_keyboard_json, scheduled_at_ms, chat_id.0, message_id.0],
            ).map_db()?;
            Ok(())
        })
    }

    pub fn get_message_extras(&self, chat_id: ChatId, message_id: MsgId) -> Result<(Option<String>, Option<i64>, u32)> {
        self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT inline_keyboard_json, scheduled_at_ms, view_count FROM messages WHERE chat_id = ?1 AND message_id = ?2",
                params![chat_id.0, message_id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u32)),
            ).optional().map_db().map(|o| o.unwrap_or((None, None, 0)))
        })
    }

    pub fn list_messages_page(
        &self,
        chat_id: ChatId,
        topic_id: Option<TopicId>,
        before_id: Option<i64>,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<MessageRecord>, bool)> {
        let fetch_limit = limit.clamp(1, 100) as usize;
        let query_limit = (fetch_limit + 1) as i64;
        self.pool.with_reader(|conn| {
            let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<MessageRecord> {
                Ok(MessageRecord {
                    chat_id: ChatId(row.get(0)?),
                    message_id: MsgId(row.get(1)?),
                    sender_id: UserId(row.get::<_, i64>(2)? as u64),
                    to_peer_raw: row.get::<_, i64>(3)? as u64,
                    topic_id: row.get::<_, Option<i64>>(4)?.map(TopicId),
                    reply_to_id: row.get::<_, Option<i64>>(5)?.map(MsgId),
                    pts: row.get::<_, i64>(6)? as u64,
                    date_ms: row.get(7)?,
                    content_json: row.get(8)?,
                    media_type: row.get(9)?,
                    flags_bitmask: row.get::<_, i64>(10)? as u32,
                    client_nonce: row.get::<_, Option<i64>>(11)?.map(|n| n as u64),
                    edit_date_ms: row.get(12)?,
                    deleted_at_ms: row.get(13)?,
                    reply_to_json: row.get(14)?,
                    forward_info_json: row.get(15)?,
                    reactions_json: row.get(16)?,
                })
            };
            let mut rows_vec = if let Some(after) = after_id {
                let mut stmt = conn.prepare(
                    "SELECT chat_id, message_id, sender_id, to_peer_raw, topic_id, reply_to_id, pts, date_ms, content_json, media_type, flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms, reply_to_json, forward_info_json, reactions_json
                     FROM messages WHERE chat_id = ?1 AND message_id > ?2 AND (?3 IS NULL OR topic_id = ?3) AND deleted_at_ms IS NULL
                     ORDER BY message_id ASC LIMIT ?4",
                ).map_db()?;
                let mapped = stmt.query_map(params![chat_id.0, after, topic_id.map(|t| t.0), query_limit], map_row).map_db()?;
                mapped.collect::<std::result::Result<Vec<_>, _>>().map_db()?
            } else {
                let mut stmt = conn.prepare(
                    "SELECT chat_id, message_id, sender_id, to_peer_raw, topic_id, reply_to_id, pts, date_ms, content_json, media_type, flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms, reply_to_json, forward_info_json, reactions_json
                     FROM messages WHERE chat_id = ?1 AND (?2 IS NULL OR message_id < ?2) AND (?3 IS NULL OR topic_id = ?3) AND deleted_at_ms IS NULL
                     ORDER BY message_id DESC LIMIT ?4",
                ).map_db()?;
                let mapped = stmt.query_map(params![chat_id.0, before_id, topic_id.map(|t| t.0), query_limit], map_row).map_db()?;
                mapped.collect::<std::result::Result<Vec<_>, _>>().map_db()?
            };
            let has_more = rows_vec.len() > fetch_limit;
            rows_vec.truncate(fetch_limit);
            if after_id.is_none() {
                rows_vec.reverse();
            }
            Ok((rows_vec, has_more))
        })
    }
}
