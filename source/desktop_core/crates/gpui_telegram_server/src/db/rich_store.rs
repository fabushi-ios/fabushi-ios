use crate::db::ServerDatabase;
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MessageId, MsgId, UserId};
use gpui_telegram_proto::cards::{PollAnswer, PollCard, PollOption};
use gpui_telegram_proto::contracts::ReactionDetailDto;
use gpui_telegram_proto::message::{MessageContent, ReactionCount};
use gpui_telegram_proto::sync::{ReactionEvent, ReadHistoryEvent, UpdateEvent};
use rusqlite::{params, OptionalExtension, Transaction};

pub(crate) fn next_pts_in_tx(tx: &Transaction<'_>, chat_id: ChatId, event: &UpdateEvent, now_ms: i64) -> Result<u64> {
    tx.execute(
        "INSERT INTO pts_sequences (chat_id, pts, account_generation, updated_at_ms) VALUES (?1, 0, 1, ?2) ON CONFLICT(chat_id) DO NOTHING",
        params![chat_id.0, now_ms],
    ).map_err(|e| TelegramError::Database(e.to_string()))?;
    tx.execute("UPDATE pts_sequences SET pts = pts + 1, updated_at_ms = ?1 WHERE chat_id = ?2", params![now_ms, chat_id.0])
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let pts: i64 = tx.query_row("SELECT pts FROM pts_sequences WHERE chat_id = ?1", params![chat_id.0], |r| r.get(0))
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    let pts_u64 = pts as u64;
    let payload_json = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
    tx.execute(
        "INSERT INTO pts_updates (chat_id, pts, pts_count, date_ms, event_type, payload_json, account_generation) VALUES (?1, ?2, 1, ?3, ?4, ?5, 1)",
        params![chat_id.0, pts, now_ms, event_type_name(event), payload_json],
    ).map_err(|e| TelegramError::Database(e.to_string()))?;
    tx.execute("UPDATE chats SET pts = ?1, updated_at_ms = ?2 WHERE chat_id = ?3", params![pts, now_ms, chat_id.0])
        .map_err(|e| TelegramError::Database(e.to_string()))?;
    Ok(pts_u64)
}

fn event_type_name(ev: &UpdateEvent) -> &'static str {
    match ev {
        UpdateEvent::NewMessage(_) => "new_message",
        UpdateEvent::EditMessage(_) => "edit_message",
        UpdateEvent::DeleteMessages(_) => "delete_messages",
        UpdateEvent::ReactionUpdated(_) | UpdateEvent::MessageReactions(_) => "reaction_updated",
        UpdateEvent::PollUpdated { .. } => "poll_updated",
        UpdateEvent::ReadHistory(_) | UpdateEvent::ReadHistoryOut { .. } | UpdateEvent::ReadHistoryIn { .. } => "read_history",
        _ => "update",
    }
}

impl ServerDatabase {
    pub fn toggle_reaction_atomic(&self, chat_id: ChatId, message_id: MsgId, user_id: UserId, emoji: &str, action: &str) -> Result<(u64, Vec<ReactionDetailDto>, Vec<ReactionCount>)> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let exists_msg: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE chat_id = ?1 AND message_id = ?2 AND deleted_at_ms IS NULL)",
                params![chat_id.0, message_id.0], |r| r.get(0),
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            if !exists_msg { return Err(TelegramError::NotFound("Message not found".into())); }

            let has_mine: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM message_reactions WHERE chat_id = ?1 AND message_id = ?2 AND user_id = ?3 AND emoji = ?4)",
                params![chat_id.0, message_id.0, user_id.0 as i64, emoji], |r| r.get(0),
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            let add = match action {
                "add" => true,
                "remove" => false,
                _ => !has_mine,
            };
            if add {
                tx.execute(
                    "INSERT OR IGNORE INTO message_reactions (chat_id, message_id, user_id, emoji, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![chat_id.0, message_id.0, user_id.0 as i64, emoji, now_ms],
                ).map_err(|e| TelegramError::Database(e.to_string()))?;
            } else {
                tx.execute(
                    "DELETE FROM message_reactions WHERE chat_id = ?1 AND message_id = ?2 AND user_id = ?3 AND emoji = ?4",
                    params![chat_id.0, message_id.0, user_id.0 as i64, emoji],
                ).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            let details = Self::query_reactions_tx(tx, chat_id, message_id, user_id)?;
            let counts: Vec<ReactionCount> = details.iter().map(|d| ReactionCount::new(&d.emoji, d.count, d.chosen)).collect();
            let reactions_json = serde_json::to_string(&counts).unwrap_or_else(|_| "[]".into());
            tx.execute("UPDATE messages SET reactions_json = ?1 WHERE chat_id = ?2 AND message_id = ?3", params![reactions_json, chat_id.0, message_id.0])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let ev = UpdateEvent::ReactionUpdated(ReactionEvent::new(chat_id, MessageId(message_id.0), user_id, counts.clone()));
            let pts = next_pts_in_tx(tx, chat_id, &ev, now_ms)?;
            Ok((pts, details, counts))
        })
    }

    pub fn list_message_reactions(&self, chat_id: ChatId, message_id: MsgId, viewer: UserId) -> Result<Vec<ReactionDetailDto>> {
        self.pool.with_reader(|conn| Self::query_reactions_conn(conn, chat_id, message_id, viewer))
    }

    fn query_reactions_tx(tx: &Transaction<'_>, chat_id: ChatId, message_id: MsgId, viewer: UserId) -> Result<Vec<ReactionDetailDto>> {
        Self::query_reactions_conn(tx, chat_id, message_id, viewer)
    }

    fn query_reactions_conn(conn: &rusqlite::Connection, chat_id: ChatId, message_id: MsgId, viewer: UserId) -> Result<Vec<ReactionDetailDto>> {
        let mut stmt = conn.prepare("SELECT emoji, user_id FROM message_reactions WHERE chat_id = ?1 AND message_id = ?2 ORDER BY created_at_ms ASC")
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        let mut by_emoji: Vec<ReactionDetailDto> = Vec::new();
        let rows = stmt.query_map(params![chat_id.0, message_id.0], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))
            .map_err(|e| TelegramError::Database(e.to_string()))?;
        for row in rows {
            let (em, uid) = row.map_err(|e| TelegramError::Database(e.to_string()))?;
            if let Some(entry) = by_emoji.iter_mut().find(|d| d.emoji == em) {
                entry.count += 1;
                entry.user_ids.push(uid);
                if uid == viewer.0 { entry.chosen = true; }
            } else {
                by_emoji.push(ReactionDetailDto { emoji: em, count: 1, chosen: uid == viewer.0, user_ids: vec![uid] });
            }
        }
        Ok(by_emoji)
    }

    pub fn vote_poll_atomic(&self, poll_id: i64, user_id: UserId, option_ids: &[u8]) -> Result<(ChatId, MsgId, u64, PollCard)> {
        if option_ids.is_empty() {
            return Err(TelegramError::Validation("option_ids cannot be empty".into()));
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let row: Option<(i64, i64, String, i64, i64, i64, Option<i64>, Option<String>, i64, Option<i64>, Option<i64>)> = tx.query_row(
                "SELECT chat_id, message_id, question, is_anonymous, multiple_choice, is_quiz, correct_option_id, solution, is_closed, close_period, close_date FROM polls WHERE poll_id = ?1",
                params![poll_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?)),
            ).optional().map_err(|e| TelegramError::Database(e.to_string()))?;
            let (cid, mid, question, is_anon, mult, is_quiz, correct_opt, solution, is_closed, close_period, close_date) =
                row.ok_or_else(|| TelegramError::NotFound("Poll not found".into()))?;
            if is_closed != 0 { return Err(TelegramError::Validation("Poll is closed".into())); }
            if mult == 0 && option_ids.len() > 1 {
                return Err(TelegramError::Validation("Poll does not allow multiple answers".into()));
            }
            let existing_votes: Vec<u8> = {
                let mut st = tx.prepare("SELECT option_id FROM poll_votes WHERE poll_id = ?1 AND user_id = ?2")
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                let rs = st.query_map(params![poll_id, user_id.0 as i64], |r| Ok(r.get::<_, i64>(0)? as u8))
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                rs.collect::<std::result::Result<Vec<_>, _>>().map_err(|e| TelegramError::Database(e.to_string()))?
            };
            if is_quiz != 0 && !existing_votes.is_empty() {
                return Err(TelegramError::Validation("Quiz votes cannot be changed".into()));
            }
            tx.execute("DELETE FROM poll_votes WHERE poll_id = ?1 AND user_id = ?2", params![poll_id, user_id.0 as i64])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let retracting = is_quiz == 0 && mult == 0 && existing_votes == option_ids;
            if !retracting {
                for &oid in option_ids {
                    let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM poll_options WHERE poll_id = ?1 AND option_id = ?2)",
                        params![poll_id, oid as i64], |r| r.get(0)).map_err(|e| TelegramError::Database(e.to_string()))?;
                    if !valid { return Err(TelegramError::Validation(format!("Invalid poll option {}", oid))); }
                    tx.execute("INSERT OR IGNORE INTO poll_votes (poll_id, user_id, option_id, voted_at_ms) VALUES (?1, ?2, ?3, ?4)",
                        params![poll_id, user_id.0 as i64, oid as i64, now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
                }
            }
            tx.execute("UPDATE poll_options SET vote_count = (SELECT COUNT(*) FROM poll_votes WHERE poll_votes.poll_id = poll_options.poll_id AND poll_votes.option_id = poll_options.option_id) WHERE poll_id = ?1", params![poll_id])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let card = Self::build_poll_card_tx(tx, poll_id, user_id, question, is_anon != 0, mult != 0, is_quiz != 0, correct_opt.map(|v| v as u8), solution, is_closed != 0, close_period.map(|v| v as u32), close_date)?;
            let content_json = serde_json::to_string(&MessageContent::Poll(card.clone())).unwrap_or_default();
            tx.execute("UPDATE messages SET content_json = ?1 WHERE chat_id = ?2 AND message_id = ?3", params![content_json, cid, mid])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let ev = UpdateEvent::PollUpdated { chat_id: ChatId(cid), message_id: MessageId(mid), poll: card.clone() };
            let pts = next_pts_in_tx(tx, ChatId(cid), &ev, now_ms)?;
            Ok((ChatId(cid), MsgId(mid), pts, card))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_poll_card_tx(tx: &Transaction<'_>, poll_id: i64, viewer: UserId, question: String, is_anonymous: bool, multiple: bool, is_quiz: bool, correct_option_id: Option<u8>, solution: Option<String>, is_closed: bool, close_period: Option<u32>, close_date: Option<i64>) -> Result<PollCard> {
        Self::build_poll_card_conn(tx, poll_id, viewer, question, is_anonymous, multiple, is_quiz, correct_option_id, solution, is_closed, close_period, close_date)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_poll_card_conn(conn: &rusqlite::Connection, poll_id: i64, viewer: UserId, question: String, is_anonymous: bool, multiple: bool, is_quiz: bool, correct_option_id: Option<u8>, solution: Option<String>, is_closed: bool, close_period: Option<u32>, close_date: Option<i64>) -> Result<PollCard> {
        let mut stmt = conn.prepare(
            "SELECT po.option_id, po.text, po.vote_count, EXISTS(SELECT 1 FROM poll_votes pv WHERE pv.poll_id = po.poll_id AND pv.option_id = po.option_id AND pv.user_id = ?2) FROM poll_options po WHERE po.poll_id = ?1 ORDER BY po.option_id ASC"
        ).map_err(|e| TelegramError::Database(e.to_string()))?;
        let rows = stmt.query_map(params![poll_id, viewer.0 as i64], |r| Ok(PollOption {
            option_id: r.get::<_, i64>(0)? as u8, text: r.get(1)?, vote_count: r.get::<_, i64>(2)? as u32,
            is_chosen: r.get::<_, i64>(3)? != 0, percentage: 0.0,
        })).map_err(|e| TelegramError::Database(e.to_string()))?;
        let options = rows.collect::<std::result::Result<Vec<_>, _>>().map_err(|e| TelegramError::Database(e.to_string()))?;
        let reveal = !is_quiz || options.iter().any(|o| o.is_chosen) || is_closed;
        let mut card = PollCard {
            id: poll_id.to_string(), poll_id: poll_id as u64, question,
            answers: options.iter().map(PollAnswer::from).collect(),
            options, total_voters: 0, total_votes: 0, is_closed, is_anonymous,
            allows_multiple_answers: multiple, is_quiz,
            correct_option_id: if reveal { correct_option_id } else { None },
            solution: if reveal { solution } else { None },
            solution_entities: Vec::new(), close_period, close_date,
        };
        card.recalculate_percentages();
        Ok(card)
    }

    pub fn mark_chat_read_atomic(&self, chat_id: ChatId, reader_id: UserId, max_id: MsgId, topic_id: i64) -> Result<(MsgId, MsgId, u32, u64, Vec<UserId>)> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO read_states (chat_id, user_id, topic_id, inbox_max_id, outbox_max_id, unread_count, updated_at_ms) VALUES (?1, ?2, ?3, 0, 0, 0, ?4)",
                params![chat_id.0, reader_id.0 as i64, topic_id, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute(
                "UPDATE read_states SET inbox_max_id = MAX(inbox_max_id, ?1), updated_at_ms = ?2 WHERE chat_id = ?3 AND user_id = ?4 AND topic_id = ?5",
                params![max_id.0, now_ms, chat_id.0, reader_id.0 as i64, topic_id],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            let (inbox_max, outbox_max): (i64, i64) = tx.query_row(
                "SELECT inbox_max_id, outbox_max_id FROM read_states WHERE chat_id = ?1 AND user_id = ?2 AND topic_id = ?3",
                params![chat_id.0, reader_id.0 as i64, topic_id], |r| Ok((r.get(0)?, r.get(1)?)),
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            let unread: i64 = tx.query_row(
                "SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND deleted_at_ms IS NULL AND sender_id != ?2 AND message_id > ?3 AND (?4 = 0 OR topic_id = ?4)",
                params![chat_id.0, reader_id.0 as i64, inbox_max, topic_id], |r| r.get(0),
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute(
                "UPDATE read_states SET unread_count = ?1, updated_at_ms = ?2 WHERE chat_id = ?3 AND user_id = ?4 AND topic_id = ?5",
                params![unread, now_ms, chat_id.0, reader_id.0 as i64, topic_id],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;
            let mut st = tx.prepare("SELECT DISTINCT sender_id FROM messages WHERE chat_id = ?1 AND message_id <= ?2 AND sender_id != ?3")
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let senders: Vec<UserId> = st.query_map(params![chat_id.0, inbox_max, reader_id.0 as i64], |r| Ok(UserId(r.get::<_, i64>(0)? as u64)))
                .map_err(|e| TelegramError::Database(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>().map_err(|e| TelegramError::Database(e.to_string()))?;
            for s in &senders {
                tx.execute("INSERT OR IGNORE INTO read_states (chat_id, user_id, topic_id, inbox_max_id, outbox_max_id, unread_count, updated_at_ms) VALUES (?1, ?2, ?3, 0, ?4, 0, ?5)",
                    params![chat_id.0, s.0 as i64, topic_id, inbox_max, now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
                tx.execute("UPDATE read_states SET outbox_max_id = MAX(outbox_max_id, ?1), updated_at_ms = ?2 WHERE chat_id = ?3 AND user_id = ?4 AND topic_id = ?5",
                    params![inbox_max, now_ms, chat_id.0, s.0 as i64, topic_id]).map_err(|e| TelegramError::Database(e.to_string()))?;
            }
            let ev = UpdateEvent::ReadHistory(ReadHistoryEvent::new(chat_id, MessageId(inbox_max), false, unread as u32));
            let pts = next_pts_in_tx(tx, chat_id, &ev, now_ms)?;
            Ok((MsgId(inbox_max), MsgId(outbox_max), unread as u32, pts, senders))
        })
    }
}
