use crate::db::{ServerDatabase, SqliteResultExt};
use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, UserId};
use gpui_telegram_proto::contracts::{ChannelStatisticsDto, DailyStatPoint, WalletBalanceDto, WalletTransactionDto};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

impl ServerDatabase {
    pub fn get_wallet_balance(&self, user_id: UserId) -> Result<WalletBalanceDto> {
        if let Some(dto) = self.pool.with_reader(|conn| {
            conn.query_row(
                "SELECT user_id, stars_balance, ton_nano, updated_at_ms FROM wallet_balances WHERE user_id = ?1",
                params![user_id.0 as i64],
                |r| Ok(WalletBalanceDto {
                    user_id: r.get::<_, i64>(0)? as u64, stars_balance: r.get(1)?,
                    ton_nano: r.get(2)?, updated_at_ms: r.get(3)?,
                }),
            ).optional().map_db()
        })? {
            return Ok(dto);
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_writer(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO wallet_balances (user_id, stars_balance, ton_nano, updated_at_ms) VALUES (?1, 0, 0, ?2)",
                params![user_id.0 as i64, now_ms],
            ).map_db()?;
            conn.query_row(
                "SELECT user_id, stars_balance, ton_nano, updated_at_ms FROM wallet_balances WHERE user_id = ?1",
                params![user_id.0 as i64],
                |r| Ok(WalletBalanceDto {
                    user_id: r.get::<_, i64>(0)? as u64, stars_balance: r.get(1)?,
                    ton_nano: r.get(2)?, updated_at_ms: r.get(3)?,
                }),
            ).map_db()
        })
    }

    pub fn topup_wallet_stars(&self, user_id: UserId, amount: i64, description: &str) -> Result<(WalletBalanceDto, WalletTransactionDto)> {
        if amount <= 0 { return Err(TelegramError::Validation("Topup amount must be positive".into())); }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let tx_id = uuid::Uuid::new_v4().to_string();
        self.pool.with_transaction(|tx| {
            tx.execute("INSERT OR IGNORE INTO wallet_balances (user_id, stars_balance, ton_nano, updated_at_ms) VALUES (?1, 0, 0, ?2)", params![user_id.0 as i64, now_ms])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("UPDATE wallet_balances SET stars_balance = stars_balance + ?1, updated_at_ms = ?2 WHERE user_id = ?3", params![amount, now_ms, user_id.0 as i64])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("INSERT INTO wallet_transactions (tx_id, user_id, peer_user_id, kind, amount_stars, description, created_at_ms) VALUES (?1, ?2, NULL, 'topup', ?3, ?4, ?5)",
                params![tx_id, user_id.0 as i64, amount, description, now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
            let bal = tx.query_row("SELECT user_id, stars_balance, ton_nano, updated_at_ms FROM wallet_balances WHERE user_id = ?1", params![user_id.0 as i64],
                |r| Ok(WalletBalanceDto { user_id: r.get::<_, i64>(0)? as u64, stars_balance: r.get(1)?, ton_nano: r.get(2)?, updated_at_ms: r.get(3)? }))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let entry = WalletTransactionDto { tx_id, peer_user_id: None, kind: "topup".into(), amount_stars: amount, description: description.into(), created_at_ms: now_ms };
            Ok((bal, entry))
        })
    }

    pub fn transfer_wallet_stars(&self, from_user: UserId, to_user: UserId, amount: i64, description: &str) -> Result<(WalletBalanceDto, WalletTransactionDto)> {
        if amount <= 0 { return Err(TelegramError::Validation("Transfer amount must be positive".into())); }
        if from_user == to_user { return Err(TelegramError::Validation("Cannot transfer stars to self".into())); }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let tx_out_id = uuid::Uuid::new_v4().to_string();
        let tx_in_id = uuid::Uuid::new_v4().to_string();
        self.pool.with_transaction(|tx| {
            let peer_exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE user_id = ?1)", params![to_user.0 as i64], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            if !peer_exists { return Err(TelegramError::NotFound("Recipient user not found".into())); }
            tx.execute("INSERT OR IGNORE INTO wallet_balances (user_id, stars_balance, ton_nano, updated_at_ms) VALUES (?1, 0, 0, ?2)", params![from_user.0 as i64, now_ms])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("INSERT OR IGNORE INTO wallet_balances (user_id, stars_balance, ton_nano, updated_at_ms) VALUES (?1, 0, 0, ?2)", params![to_user.0 as i64, now_ms])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let current: i64 = tx.query_row("SELECT stars_balance FROM wallet_balances WHERE user_id = ?1", params![from_user.0 as i64], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            if current < amount { return Err(TelegramError::Validation("Insufficient Stars balance".into())); }
            tx.execute("UPDATE wallet_balances SET stars_balance = stars_balance - ?1, updated_at_ms = ?2 WHERE user_id = ?3", params![amount, now_ms, from_user.0 as i64])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("UPDATE wallet_balances SET stars_balance = stars_balance + ?1, updated_at_ms = ?2 WHERE user_id = ?3", params![amount, now_ms, to_user.0 as i64])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("INSERT INTO wallet_transactions (tx_id, user_id, peer_user_id, kind, amount_stars, description, created_at_ms) VALUES (?1, ?2, ?3, 'transfer_out', ?4, ?5, ?6)",
                params![tx_out_id, from_user.0 as i64, to_user.0 as i64, -amount, description, now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
            tx.execute("INSERT INTO wallet_transactions (tx_id, user_id, peer_user_id, kind, amount_stars, description, created_at_ms) VALUES (?1, ?2, ?3, 'transfer_in', ?4, ?5, ?6)",
                params![tx_in_id, to_user.0 as i64, from_user.0 as i64, amount, description, now_ms]).map_err(|e| TelegramError::Database(e.to_string()))?;
            let bal = tx.query_row("SELECT user_id, stars_balance, ton_nano, updated_at_ms FROM wallet_balances WHERE user_id = ?1", params![from_user.0 as i64],
                |r| Ok(WalletBalanceDto { user_id: r.get::<_, i64>(0)? as u64, stars_balance: r.get(1)?, ton_nano: r.get(2)?, updated_at_ms: r.get(3)? }))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let entry = WalletTransactionDto { tx_id: tx_out_id, peer_user_id: Some(to_user.0), kind: "transfer_out".into(), amount_stars: -amount, description: description.into(), created_at_ms: now_ms };
            Ok((bal, entry))
        })
    }

    pub fn list_wallet_transactions(&self, user_id: UserId) -> Result<Vec<WalletTransactionDto>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn.prepare("SELECT tx_id, peer_user_id, kind, amount_stars, description, created_at_ms FROM wallet_transactions WHERE user_id = ?1 ORDER BY created_at_ms DESC LIMIT 100").map_db()?;
            let rows = stmt.query_map(params![user_id.0 as i64], |r| Ok(WalletTransactionDto {
                tx_id: r.get(0)?,
                peer_user_id: r.get::<_, Option<i64>>(1)?.map(|v| v as u64),
                kind: r.get(2)?, amount_stars: r.get(3)?, description: r.get(4)?, created_at_ms: r.get(5)?,
            })).map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn compute_chat_statistics(&self, chat_id: ChatId) -> Result<ChannelStatisticsDto> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let seven_days_ms = now_ms - 7 * 86_400_000;
        let fourteen_days_ms = now_ms - 14 * 86_400_000;
        self.pool.with_reader(|conn| {
            let title: String = conn.query_row("SELECT title FROM chats WHERE chat_id = ?1", params![chat_id.0], |r| r.get(0))
                .optional().map_db()?.ok_or_else(|| TelegramError::NotFound("Chat not found".into()))?;
            let member_count: u64 = conn.query_row("SELECT COUNT(*) FROM chat_members WHERE chat_id = ?1", params![chat_id.0], |r| r.get::<_, i64>(0)).map_db()? as u64;
            let total_messages: u64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND deleted_at_ms IS NULL", params![chat_id.0], |r| r.get::<_, i64>(0)).map_db()? as u64;
            let messages_last_7d: u64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND deleted_at_ms IS NULL AND date_ms >= ?2", params![chat_id.0, seven_days_ms], |r| r.get::<_, i64>(0)).map_db()? as u64;
            let prev_7d: u64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE chat_id = ?1 AND deleted_at_ms IS NULL AND date_ms >= ?2 AND date_ms < ?3", params![chat_id.0, fourteen_days_ms, seven_days_ms], |r| r.get::<_, i64>(0)).map_db()? as u64;
            let total_reactions: u64 = conn.query_row("SELECT COUNT(*) FROM message_reactions WHERE chat_id = ?1", params![chat_id.0], |r| r.get::<_, i64>(0)).map_db()? as u64;
            let active_posters_7d: u64 = conn.query_row("SELECT COUNT(DISTINCT sender_id) FROM messages WHERE chat_id = ?1 AND deleted_at_ms IS NULL AND date_ms >= ?2", params![chat_id.0, seven_days_ms], |r| r.get::<_, i64>(0)).map_db()? as u64;
            let mut st_day = conn.prepare("SELECT date(date_ms / 1000, 'unixepoch') AS d, MIN(date_ms), COUNT(*), COUNT(DISTINCT sender_id) FROM messages WHERE chat_id = ?1 AND deleted_at_ms IS NULL GROUP BY d ORDER BY d ASC LIMIT 30").map_db()?;
            let daily_series = st_day.query_map(params![chat_id.0], |r| Ok(DailyStatPoint {
                day: r.get(0)?, timestamp_ms: r.get(1)?, messages: r.get::<_, i64>(2)? as u64, reactions: 0, active_senders: r.get::<_, i64>(3)? as u64,
            })).map_db()?.collect::<std::result::Result<Vec<_>, _>>().map_db()?;
            let mut st_em = conn.prepare("SELECT emoji, COUNT(*) AS c FROM message_reactions WHERE chat_id = ?1 GROUP BY emoji ORDER BY c DESC LIMIT 10").map_db()?;
            let top_emojis = st_em.query_map(params![chat_id.0], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)))
                .map_db()?.collect::<std::result::Result<Vec<_>, _>>().map_db()?;
            let growth_rate_pct = if prev_7d == 0 { if messages_last_7d > 0 { 100.0 } else { 0.0 } } else { ((messages_last_7d as f64 - prev_7d as f64) / prev_7d as f64) * 100.0 };
            Ok(ChannelStatisticsDto { chat_id: chat_id.0, title, member_count, total_messages, messages_last_7d, total_reactions, active_posters_7d, growth_rate_pct, daily_series, top_emojis })
        })
    }

    pub fn export_chat_payload(&self, chat_id: ChatId, limit: u32) -> Result<Value> {
        let chat = self.get_chat(chat_id)?.ok_or_else(|| TelegramError::NotFound("Chat not found".into()))?;
        let members = self.get_chat_member_details(chat_id)?;
        let messages = self.list_messages(chat_id, None, limit.clamp(1, 5000))?;
        let msg_values: Vec<Value> = messages.into_iter().filter(|m| m.deleted_at_ms.is_none()).map(|m| {
            let content: Value = serde_json::from_str(&m.content_json).unwrap_or(json!({}));
            json!({
                "message_id": m.message_id.0, "sender_id": m.sender_id.0, "date_ms": m.date_ms,
                "edit_date_ms": m.edit_date_ms, "reply_to_id": m.reply_to_id.map(|r| r.0),
                "topic_id": m.topic_id.map(|t| t.0), "content": content,
            })
        }).collect();
        Ok(json!({
            "chat": { "chat_id": chat.chat_id.0, "title": chat.title, "username": chat.username, "about": chat.about, "created_at_ms": chat.created_at_ms },
            "members_count": members.len(),
            "members": members,
            "messages_count": msg_values.len(),
            "messages": msg_values,
            "exported_at_ms": chrono::Utc::now().timestamp_millis(),
        }))
    }

    pub fn list_user_member_chat_ids(&self, user_id: UserId) -> Result<Vec<ChatId>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare("SELECT chat_id FROM chat_members WHERE user_id = ?1")
                .map_db()?;
            let rows = stmt
                .query_map(params![user_id.0 as i64], |r| Ok(ChatId(r.get(0)?)))
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }

    pub fn list_recent_messages_for_search(
        &self,
        limit: usize,
    ) -> Result<Vec<(i64, i64, u64, i64, String, Option<String>)>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare("SELECT message_id, chat_id, sender_id, date_ms, content_json FROM messages WHERE deleted_at_ms IS NULL ORDER BY date_ms DESC LIMIT ?1")
                .map_db()?;
            let rows = stmt
                .query_map(params![limit as i64], |r| {
                    let mid: i64 = r.get(0)?;
                    let cid: i64 = r.get(1)?;
                    let sid: u64 = r.get::<_, i64>(2)? as u64;
                    let date_ms: i64 = r.get(3)?;
                    let raw: String = r.get(4)?;
                    let val: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
                    let data = val.get("data");
                    let text = val
                        .get("text")
                        .or_else(|| {
                            data.and_then(|d| d.get("text").or_else(|| d.get("caption")))
                        })
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let media_type = val
                        .get("media_type")
                        .or_else(|| val.get("type"))
                        .and_then(|v| v.as_str())
                        .filter(|t| *t != "Text")
                        .map(|t| t.to_ascii_lowercase());
                    Ok((mid, cid, sid, date_ms, text, media_type))
                })
                .map_db()?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_db()
        })
    }
}
