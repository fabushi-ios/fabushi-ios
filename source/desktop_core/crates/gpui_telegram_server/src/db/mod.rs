pub mod acp_store;
pub mod agentic_store;
pub mod analytics_store;
pub mod auth_store;
pub mod chat_extras;
pub mod computer_store;
pub mod e2ee_store;
pub mod migrations;
pub mod miniapp_store;
pub mod poll_store;
pub mod pool;
pub mod rich_store;
pub mod schema;
pub mod schema_parity;
pub mod schema_parity_extra;
pub mod social_store;

pub use migrations::run_migrations;
pub use pool::SqlitePool;
pub use schema::*;

use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MsgId, TopicId, UserId};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) trait SqliteResultExt<T> {
    fn map_db(self) -> Result<T>;
}

impl<T> SqliteResultExt<T> for std::result::Result<T, rusqlite::Error> {
    fn map_db(self) -> Result<T> {
        self.map_err(|e| TelegramError::Database(e.to_string()))
    }
}

/// ServerDatabase provides the persistence abstraction for GPUI Telegram Server.
#[derive(Debug, Clone)]
pub struct ServerDatabase {
    pub(crate) pool: SqlitePool,
}

impl ServerDatabase {
    /// Opens a SQLite database at `path` and runs all schema migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let pool = SqlitePool::open(path)?;
        pool.with_conn(|conn| run_migrations(conn))?;
        Ok(Self { pool })
    }

    /// Opens an in-memory SQLite database and runs all schema migrations.
    pub fn open_in_memory() -> Result<Self> {
        let pool = SqlitePool::open_in_memory()?;
        pool.with_conn(|conn| run_migrations(conn))?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn is_inactive_member_role(role: &str) -> bool {
        matches!(
            role.trim().to_ascii_lowercase().as_str(),
            "kicked" | "banned" | "left"
        )
    }

    pub fn is_admin_member_role(role: &str) -> bool {
        matches!(
            role.trim().to_ascii_lowercase().as_str(),
            "owner" | "creator" | "admin" | "administrator"
        )
    }

    pub fn normalize_member_role(role: &str) -> Option<&'static str> {
        match role.trim().to_ascii_lowercase().as_str() {
            "owner" => Some("owner"),
            "creator" => Some("creator"),
            "admin" => Some("admin"),
            "administrator" => Some("administrator"),
            "member" => Some("member"),
            "restricted" => Some("restricted"),
            "left" => Some("left"),
            "kicked" => Some("kicked"),
            "banned" => Some("banned"),
            _ => None,
        }
    }

    // --- User Operations ---

    pub fn create_user(&self, user: &UserRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO users (
                    user_id, username, phone, first_name, last_name, bio,
                    password_hash, is_bot, is_verified, is_support, is_scam, is_fake,
                    profile_photo_file_id, account_generation, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                "#,
                params![
                    user.user_id.0 as i64,
                    user.username,
                    user.phone,
                    user.first_name,
                    user.last_name,
                    user.bio,
                    user.password_hash,
                    user.is_bot as i64,
                    user.is_verified as i64,
                    user.is_support as i64,
                    user.is_scam as i64,
                    user.is_fake as i64,
                    user.profile_photo_file_id,
                    user.account_generation as i64,
                    user.created_at_ms,
                    user.updated_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_user(&self, user_id: UserId) -> Result<Option<UserRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT user_id, username, phone, first_name, last_name, bio,
                           password_hash, is_bot, is_verified, is_support, is_scam, is_fake,
                           profile_photo_file_id, account_generation, created_at_ms, updated_at_ms
                    FROM users WHERE user_id = ?1
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![user_id.0 as i64]).map_db()?;

            if let Some(row) = rows.next().map_db()? {
                Ok(Some(UserRecord {
                    user_id: UserId(row.get::<_, i64>(0).map_db()? as u64),
                    username: row.get(1).map_db()?,
                    phone: row.get(2).map_db()?,
                    first_name: row.get(3).map_db()?,
                    last_name: row.get(4).map_db()?,
                    bio: row.get(5).map_db()?,
                    password_hash: row.get(6).map_db()?,
                    is_bot: row.get::<_, i64>(7).map_db()? != 0,
                    is_verified: row.get::<_, i64>(8).map_db()? != 0,
                    is_support: row.get::<_, i64>(9).map_db()? != 0,
                    is_scam: row.get::<_, i64>(10).map_db()? != 0,
                    is_fake: row.get::<_, i64>(11).map_db()? != 0,
                    profile_photo_file_id: row.get(12).map_db()?,
                    account_generation: row.get::<_, i64>(13).map_db()? as u64,
                    created_at_ms: row.get(14).map_db()?,
                    updated_at_ms: row.get(15).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn get_user_by_username(&self, username: &str) -> Result<Option<UserRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT user_id, username, phone, first_name, last_name, bio,
                           password_hash, is_bot, is_verified, is_support, is_scam, is_fake,
                           profile_photo_file_id, account_generation, created_at_ms, updated_at_ms
                    FROM users WHERE username = ?1 COLLATE NOCASE
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![username]).map_db()?;

            if let Some(row) = rows.next().map_db()? {
                Ok(Some(UserRecord {
                    user_id: UserId(row.get::<_, i64>(0).map_db()? as u64),
                    username: row.get(1).map_db()?,
                    phone: row.get(2).map_db()?,
                    first_name: row.get(3).map_db()?,
                    last_name: row.get(4).map_db()?,
                    bio: row.get(5).map_db()?,
                    password_hash: row.get(6).map_db()?,
                    is_bot: row.get::<_, i64>(7).map_db()? != 0,
                    is_verified: row.get::<_, i64>(8).map_db()? != 0,
                    is_support: row.get::<_, i64>(9).map_db()? != 0,
                    is_scam: row.get::<_, i64>(10).map_db()? != 0,
                    is_fake: row.get::<_, i64>(11).map_db()? != 0,
                    profile_photo_file_id: row.get(12).map_db()?,
                    account_generation: row.get::<_, i64>(13).map_db()? as u64,
                    created_at_ms: row.get(14).map_db()?,
                    updated_at_ms: row.get(15).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn get_max_user_id(&self) -> Result<u64> {
        self.pool.with_reader(|conn| {
            let max_id: Option<i64> = conn
                .query_row(
                    "SELECT MAX(user_id) FROM users",
                    [],
                    |row| row.get(0),
                )
                .map_db()?;
            Ok(max_id.unwrap_or(100_000) as u64)
        })
    }

    pub fn get_max_human_user_id(&self) -> Result<u64> {
        self.pool.with_reader(|conn| {
            let max_id: Option<i64> = conn
                .query_row(
                    "SELECT COALESCE(MAX(user_id), 100000) FROM users WHERE is_bot = 0 AND user_id < 500000",
                    [],
                    |row| row.get(0),
                )
                .map_db()?;
            Ok(max_id.unwrap_or(100_000) as u64)
        })
    }

    pub fn get_user_by_email(&self, email: &str) -> Result<Option<UserRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT user_id, username, phone, first_name, last_name, bio,
                           password_hash, is_bot, is_verified, is_support, is_scam, is_fake,
                           profile_photo_file_id, account_generation, created_at_ms, updated_at_ms
                    FROM users WHERE email = ?1 COLLATE NOCASE
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![email]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(UserRecord {
                    user_id: UserId(row.get::<_, i64>(0).map_db()? as u64),
                    username: row.get(1).map_db()?,
                    phone: row.get(2).map_db()?,
                    first_name: row.get(3).map_db()?,
                    last_name: row.get(4).map_db()?,
                    bio: row.get(5).map_db()?,
                    password_hash: row.get(6).map_db()?,
                    is_bot: row.get::<_, i64>(7).map_db()? != 0,
                    is_verified: row.get::<_, i64>(8).map_db()? != 0,
                    is_support: row.get::<_, i64>(9).map_db()? != 0,
                    is_scam: row.get::<_, i64>(10).map_db()? != 0,
                    is_fake: row.get::<_, i64>(11).map_db()? != 0,
                    profile_photo_file_id: row.get(12).map_db()?,
                    account_generation: row.get::<_, i64>(13).map_db()? as u64,
                    created_at_ms: row.get(14).map_db()?,
                    updated_at_ms: row.get(15).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn set_user_email_verified(&self, user_id: UserId, email: &str) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE users SET email = ?1, email_verified = 1 WHERE user_id = ?2",
                params![email, user_id.0 as i64],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn set_user_alipay_identity(&self, user_id: UserId, alipay_user_id: &str, nickname: &str) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE users SET alipay_user_id = ?1, alipay_nickname = ?2 WHERE user_id = ?3",
                params![alipay_user_id, nickname, user_id.0 as i64],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_user_by_alipay_id(&self, alipay_user_id: &str) -> Result<Option<UserRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT user_id, username, phone, first_name, last_name, bio,
                           password_hash, is_bot, is_verified, is_support, is_scam,
                           is_fake, profile_photo_file_id, account_generation,
                           created_at_ms, updated_at_ms
                    FROM users WHERE alipay_user_id = ?1
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![alipay_user_id]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(UserRecord {
                    user_id: UserId(row.get::<_, i64>(0).map_db()? as u64),
                    username: row.get(1).map_db()?,
                    phone: row.get(2).map_db()?,
                    first_name: row.get(3).map_db()?,
                    last_name: row.get(4).map_db()?,
                    bio: row.get(5).map_db()?,
                    password_hash: row.get(6).map_db()?,
                    is_bot: row.get::<_, i64>(7).map_db()? != 0,
                    is_verified: row.get::<_, i64>(8).map_db()? != 0,
                    is_support: row.get::<_, i64>(9).map_db()? != 0,
                    is_scam: row.get::<_, i64>(10).map_db()? != 0,
                    is_fake: row.get::<_, i64>(11).map_db()? != 0,
                    profile_photo_file_id: row.get(12).map_db()?,
                    account_generation: row.get::<_, i64>(13).map_db()? as u64,
                    created_at_ms: row.get(14).map_db()?,
                    updated_at_ms: row.get(15).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn save_email_challenge(&self, challenge: &EmailChallengeRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO account_email_challenges (
                    challenge_id, email, code_hash, attempts_left,
                    cooldown_until_ms, expires_at_ms, created_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(challenge_id) DO UPDATE SET
                    attempts_left = excluded.attempts_left,
                    cooldown_until_ms = excluded.cooldown_until_ms,
                    expires_at_ms = excluded.expires_at_ms
                "#,
                params![
                    challenge.challenge_id,
                    challenge.email,
                    challenge.code_hash,
                    challenge.attempts_left,
                    challenge.cooldown_until_ms,
                    challenge.expires_at_ms,
                    challenge.created_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_latest_email_challenge(&self, email: &str) -> Result<Option<EmailChallengeRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT challenge_id, email, code_hash, attempts_left,
                           cooldown_until_ms, expires_at_ms, created_at_ms
                    FROM account_email_challenges
                    WHERE email = ?1 COLLATE NOCASE
                    ORDER BY created_at_ms DESC
                    LIMIT 1
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![email]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(EmailChallengeRecord {
                    challenge_id: row.get(0).map_db()?,
                    email: row.get(1).map_db()?,
                    code_hash: row.get(2).map_db()?,
                    attempts_left: row.get(3).map_db()?,
                    cooldown_until_ms: row.get(4).map_db()?,
                    expires_at_ms: row.get(5).map_db()?,
                    created_at_ms: row.get(6).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn decrement_email_challenge_attempts(&self, challenge_id: &str) -> Result<i32> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE account_email_challenges SET attempts_left = MAX(0, attempts_left - 1) WHERE challenge_id = ?1",
                params![challenge_id],
            )
            .map_db()?;
            let remaining: i32 = conn
                .query_row(
                    "SELECT attempts_left FROM account_email_challenges WHERE challenge_id = ?1",
                    params![challenge_id],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(remaining)
        })
    }

    pub fn delete_email_challenge(&self, challenge_id: &str) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "DELETE FROM account_email_challenges WHERE challenge_id = ?1",
                params![challenge_id],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn create_oauth_attempt(&self, attempt: &OAuthAttemptRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO account_oauth_attempts (
                    attempt_id, provider, state, status,
                    session_json, delivered_at_ms, expires_at_ms, created_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(attempt_id) DO UPDATE SET
                    status = excluded.status,
                    session_json = excluded.session_json,
                    delivered_at_ms = excluded.delivered_at_ms
                "#,
                params![
                    attempt.attempt_id,
                    attempt.provider,
                    attempt.state,
                    attempt.status,
                    attempt.session_json,
                    attempt.delivered_at_ms,
                    attempt.expires_at_ms,
                    attempt.created_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_oauth_attempt(&self, attempt_id: &str) -> Result<Option<OAuthAttemptRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT attempt_id, provider, state, status,
                           session_json, delivered_at_ms, expires_at_ms, created_at_ms
                    FROM account_oauth_attempts WHERE attempt_id = ?1
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![attempt_id]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(OAuthAttemptRecord {
                    attempt_id: row.get(0).map_db()?,
                    provider: row.get(1).map_db()?,
                    state: row.get(2).map_db()?,
                    status: row.get(3).map_db()?,
                    session_json: row.get(4).map_db()?,
                    delivered_at_ms: row.get(5).map_db()?,
                    expires_at_ms: row.get(6).map_db()?,
                    created_at_ms: row.get(7).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn get_oauth_attempt_by_state(&self, state: &str) -> Result<Option<OAuthAttemptRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT attempt_id, provider, state, status,
                           session_json, delivered_at_ms, expires_at_ms, created_at_ms
                    FROM account_oauth_attempts WHERE state = ?1
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![state]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(OAuthAttemptRecord {
                    attempt_id: row.get(0).map_db()?,
                    provider: row.get(1).map_db()?,
                    state: row.get(2).map_db()?,
                    status: row.get(3).map_db()?,
                    session_json: row.get(4).map_db()?,
                    delivered_at_ms: row.get(5).map_db()?,
                    expires_at_ms: row.get(6).map_db()?,
                    created_at_ms: row.get(7).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn update_oauth_attempt_status(&self, attempt_id: &str, status: &str) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE account_oauth_attempts SET status = ?1 WHERE attempt_id = ?2",
                params![status, attempt_id],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn complete_oauth_attempt(&self, attempt_id: &str, session_json: &str) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let rows = conn
                .execute(
                    "UPDATE account_oauth_attempts SET status = 'completed', session_json = ?1 WHERE attempt_id = ?2 AND status = 'pending'",
                    params![session_json, attempt_id],
                )
                .map_db()?;
            Ok(rows > 0)
        })
    }

    pub fn claim_oauth_attempt_session(
        &self,
        attempt_id: &str,
        delivered_at_ms: i64,
    ) -> Result<Option<String>> {
        self.pool.with_conn(|conn| {
            let tx = conn.transaction().map_db()?;
            let session_json: Option<String> = tx
                .query_row(
                    "SELECT session_json FROM account_oauth_attempts WHERE attempt_id = ?1 AND status = 'completed' AND delivered_at_ms IS NULL",
                    params![attempt_id],
                    |row| row.get(0),
                )
                .optional()
                .map_db()?;

            if let Some(json_str) = session_json {
                let changes = tx
                    .execute(
                        "UPDATE account_oauth_attempts SET session_json = NULL, delivered_at_ms = ?1 WHERE attempt_id = ?2 AND delivered_at_ms IS NULL",
                        params![delivered_at_ms, attempt_id],
                    )
                    .map_db()?;

                if changes > 0 {
                    tx.commit().map_db()?;
                    return Ok(Some(json_str));
                }
            }
            Ok(None)
        })
    }

    // --- Session Operations ---

    pub fn create_session(&self, session: &SessionRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO sessions (
                    session_id, user_id, device_id, client_type, app_version,
                    ip_address, location_desc, account_generation, token_hash,
                    created_at_ms, last_active_ms, expires_at_ms, revoked_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                ON CONFLICT(session_id) DO UPDATE SET
                    last_active_ms = excluded.last_active_ms,
                    expires_at_ms = excluded.expires_at_ms,
                    revoked_at_ms = excluded.revoked_at_ms
                "#,
                params![
                    session.session_id,
                    session.user_id.0 as i64,
                    session.device_id,
                    session.client_type,
                    session.app_version,
                    session.ip_address,
                    session.location_desc,
                    session.account_generation as i64,
                    session.token_hash,
                    session.created_at_ms,
                    session.last_active_ms,
                    session.expires_at_ms,
                    session.revoked_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT session_id, user_id, device_id, client_type, app_version,
                           ip_address, location_desc, account_generation, token_hash,
                           created_at_ms, last_active_ms, expires_at_ms, revoked_at_ms
                    FROM sessions WHERE session_id = ?1
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![session_id]).map_db()?;

            if let Some(row) = rows.next().map_db()? {
                Ok(Some(SessionRecord {
                    session_id: row.get(0).map_db()?,
                    user_id: UserId(row.get::<_, i64>(1).map_db()? as u64),
                    device_id: row.get(2).map_db()?,
                    client_type: row.get(3).map_db()?,
                    app_version: row.get(4).map_db()?,
                    ip_address: row.get(5).map_db()?,
                    location_desc: row.get(6).map_db()?,
                    account_generation: row.get::<_, i64>(7).map_db()? as u64,
                    token_hash: row.get(8).map_db()?,
                    created_at_ms: row.get(9).map_db()?,
                    last_active_ms: row.get(10).map_db()?,
                    expires_at_ms: row.get(11).map_db()?,
                    revoked_at_ms: row.get(12).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn revoke_session(&self, session_id: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET revoked_at_ms = ?1 WHERE session_id = ?2",
                params![now_ms, session_id],
            )
            .map_db()?;
            Ok(())
        })
    }

    // --- Chat Operations ---

    fn insert_chat_conn(conn: &rusqlite::Connection, chat: &ChatRecord) -> Result<()> {
        conn.execute(
            r#"
            INSERT INTO chats (
                chat_id, chat_type, peer_type, title, username, about,
                creator_id, is_forum, flags, photo_file_id, slowmode_seconds,
                linked_chat_id, pinned_message_id, pts, last_message_id,
                last_message_date_ms, unread_count, account_generation,
                created_at_ms, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
            "#,
            params![
                chat.chat_id.0,
                chat.chat_type as i64,
                chat.peer_type as i64,
                chat.title,
                chat.username,
                chat.about,
                chat.creator_id.0 as i64,
                chat.is_forum as i64,
                chat.flags as i64,
                chat.photo_file_id,
                chat.slowmode_seconds as i64,
                chat.linked_chat_id.map(|c| c.0),
                chat.pinned_message_id.map(|m| m.0),
                chat.pts as i64,
                chat.last_message_id.0,
                chat.last_message_date_ms,
                chat.unread_count as i64,
                chat.account_generation as i64,
                chat.created_at_ms,
                chat.updated_at_ms,
            ],
        )
        .map_db()?;
        Ok(())
    }

    pub fn create_chat(&self, chat: &ChatRecord) -> Result<()> {
        self.pool.with_conn(|conn| Self::insert_chat_conn(conn, chat))
    }

    /// Atomically creates a chat and its initial member rows in one SQLite transaction.
    pub fn create_chat_with_members(&self, chat: &ChatRecord, members: &[ChatMemberRecord]) -> Result<()> {
        self.pool.with_transaction(|tx| {
            Self::insert_chat_conn(tx, chat)?;
            for member in members {
                Self::upsert_chat_member_conn(tx, member)?;
            }
            Ok(())
        })
    }

    /// Increments per-member `read_states.unread_count` for active peers other than the sender.
    pub(crate) fn increment_peer_unread_in_tx(
        tx: &rusqlite::Transaction<'_>,
        chat_id: ChatId,
        sender_id: UserId,
        topic_id: Option<TopicId>,
        now_ms: i64,
    ) -> Result<()> {
        let mut st_mem = tx
            .prepare("SELECT user_id FROM chat_members WHERE chat_id = ?1 AND user_id != ?2 AND LOWER(TRIM(role)) NOT IN ('kicked', 'banned', 'left')")
            .map_db()?;
        let peers: Vec<i64> = st_mem
            .query_map(params![chat_id.0, sender_id.0 as i64], |r| r.get(0))
            .map_db()?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_db()?;
        let topic_key = topic_id.map(|t| t.0).unwrap_or(0);
        for peer_uid in peers {
            tx.execute(
                r#"INSERT INTO read_states (chat_id, user_id, topic_id, inbox_max_id, outbox_max_id, unread_count, updated_at_ms)
                   VALUES (?1, ?2, ?3, 0, 0, 1, ?4)
                   ON CONFLICT(chat_id, user_id, topic_id) DO UPDATE SET
                       unread_count = read_states.unread_count + 1, updated_at_ms = excluded.updated_at_ms"#,
                params![chat_id.0, peer_uid, topic_key, now_ms],
            )
            .map_db()?;
        }
        Ok(())
    }

    fn row_to_chat_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatRecord> {
        Ok(ChatRecord {
            chat_id: ChatId(row.get(0)?),
            chat_type: row.get::<_, i64>(1)? as u8,
            peer_type: row.get::<_, i64>(2)? as u8,
            title: row.get(3)?,
            username: row.get(4)?,
            about: row.get(5)?,
            creator_id: UserId(row.get::<_, i64>(6)? as u64),
            is_forum: row.get::<_, i64>(7)? != 0,
            flags: row.get::<_, i64>(8)? as u32,
            photo_file_id: row.get(9)?,
            slowmode_seconds: row.get::<_, i64>(10)? as u32,
            linked_chat_id: row.get::<_, Option<i64>>(11)?.map(ChatId),
            pinned_message_id: row.get::<_, Option<i64>>(12)?.map(MsgId),
            pts: row.get::<_, i64>(13)? as u64,
            last_message_id: MsgId(row.get(14)?),
            last_message_date_ms: row.get(15)?,
            unread_count: row.get::<_, i64>(16)? as u32,
            account_generation: row.get::<_, i64>(17)? as u64,
            created_at_ms: row.get(18)?,
            updated_at_ms: row.get(19)?,
        })
    }

    pub fn get_chat(&self, chat_id: ChatId) -> Result<Option<ChatRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, chat_type, peer_type, title, username, about,
                           creator_id, is_forum, flags, photo_file_id, slowmode_seconds,
                           linked_chat_id, pinned_message_id, pts, last_message_id,
                           last_message_date_ms, unread_count, account_generation,
                           created_at_ms, updated_at_ms
                    FROM chats WHERE chat_id = ?1
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![chat_id.0]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(Self::row_to_chat_record(row).map_db()?))
            } else {
                Ok(None)
            }
        })
    }

    pub fn get_chat_by_username(&self, username: &str) -> Result<Option<ChatRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, chat_type, peer_type, title, username, about,
                           creator_id, is_forum, flags, photo_file_id, slowmode_seconds,
                           linked_chat_id, pinned_message_id, pts, last_message_id,
                           last_message_date_ms, unread_count, account_generation,
                           created_at_ms, updated_at_ms
                    FROM chats WHERE username = ?1 COLLATE NOCASE
                    "#,
                )
                .map_db()?;
            let mut rows = stmt.query(params![username]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(Self::row_to_chat_record(row).map_db()?))
            } else {
                Ok(None)
            }
        })
    }

    pub fn is_user_in_chat(&self, chat_id: ChatId, user_id: UserId) -> Result<bool> {
        let Some(chat) = self.get_chat(chat_id)? else { return Ok(false); };
        if chat.creator_id == user_id { return Ok(true); }
        Ok(self.get_chat_member(chat_id, user_id)?.map(|m| !Self::is_inactive_member_role(&m.role)).unwrap_or(false))
    }

    // --- Chat Member Operations ---

    fn upsert_chat_member_conn(conn: &rusqlite::Connection, member: &ChatMemberRecord) -> Result<()> {
        conn.execute(
            r#"
            INSERT INTO chat_members (
                chat_id, user_id, role, permissions_bitmask, admin_title,
                admin_rights, banned_rights, until_date_ms, joined_at_ms,
                invited_by, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(chat_id, user_id) DO UPDATE SET
                role = excluded.role,
                permissions_bitmask = excluded.permissions_bitmask,
                admin_title = excluded.admin_title,
                admin_rights = excluded.admin_rights,
                banned_rights = excluded.banned_rights,
                until_date_ms = excluded.until_date_ms,
                updated_at_ms = excluded.updated_at_ms
            "#,
            params![
                member.chat_id.0,
                member.user_id.0 as i64,
                member.role,
                member.permissions_bitmask as i64,
                member.admin_title,
                member.admin_rights as i64,
                member.banned_rights as i64,
                member.until_date_ms,
                member.joined_at_ms,
                member.invited_by.map(|u| u.0 as i64),
                member.updated_at_ms,
            ],
        )
        .map_db()?;
        Ok(())
    }

    pub fn add_chat_member(&self, member: &ChatMemberRecord) -> Result<()> {
        self.pool.with_conn(|conn| Self::upsert_chat_member_conn(conn, member))
    }

    pub fn get_chat_member(&self, chat_id: ChatId, user_id: UserId) -> Result<Option<ChatMemberRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, user_id, role, permissions_bitmask, admin_title,
                           admin_rights, banned_rights, until_date_ms, joined_at_ms,
                           invited_by, updated_at_ms
                    FROM chat_members WHERE chat_id = ?1 AND user_id = ?2
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![chat_id.0, user_id.0 as i64]).map_db()?;

            if let Some(row) = rows.next().map_db()? {
                Ok(Some(ChatMemberRecord {
                    chat_id: ChatId(row.get(0).map_db()?),
                    user_id: UserId(row.get::<_, i64>(1).map_db()? as u64),
                    role: row.get(2).map_db()?,
                    permissions_bitmask: row.get::<_, i64>(3).map_db()? as u32,
                    admin_title: row.get(4).map_db()?,
                    admin_rights: row.get::<_, i64>(5).map_db()? as u32,
                    banned_rights: row.get::<_, i64>(6).map_db()? as u32,
                    until_date_ms: row.get(7).map_db()?,
                    joined_at_ms: row.get(8).map_db()?,
                    invited_by: row.get::<_, Option<i64>>(9).map_db()?.map(|id| UserId(id as u64)),
                    updated_at_ms: row.get(10).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    // --- Forum Topics Operations ---

    pub fn create_forum_topic(&self, topic: &ForumTopicRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO forum_topics (
                    chat_id, topic_id, title, icon_color, icon_emoji_id,
                    creator_id, is_pinned, is_closed, is_hidden,
                    last_message_id, unread_count, created_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(chat_id, topic_id) DO UPDATE SET
                    title = excluded.title,
                    icon_color = excluded.icon_color,
                    icon_emoji_id = excluded.icon_emoji_id,
                    is_pinned = excluded.is_pinned,
                    is_closed = excluded.is_closed,
                    is_hidden = excluded.is_hidden,
                    last_message_id = excluded.last_message_id,
                    unread_count = excluded.unread_count
                "#,
                params![
                    topic.chat_id.0,
                    topic.topic_id.0,
                    topic.title,
                    topic.icon_color as i64,
                    topic.icon_emoji_id.map(|id| id as i64),
                    topic.creator_id.0 as i64,
                    topic.is_pinned as i64,
                    topic.is_closed as i64,
                    topic.is_hidden as i64,
                    topic.last_message_id.map(|m| m.0),
                    topic.unread_count as i64,
                    topic.created_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn list_forum_topics(&self, chat_id: ChatId) -> Result<Vec<ForumTopicRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, topic_id, title, icon_color, icon_emoji_id,
                           creator_id, is_pinned, is_closed, is_hidden,
                           last_message_id, unread_count, created_at_ms
                    FROM forum_topics
                    WHERE chat_id = ?1
                    ORDER BY is_pinned DESC, topic_id ASC
                    "#,
                )
                .map_db()?;

            let rows = stmt
                .query_map(params![chat_id.0], |row| {
                    Ok(ForumTopicRecord {
                        chat_id: ChatId(row.get(0)?),
                        topic_id: TopicId(row.get(1)?),
                        title: row.get(2)?,
                        icon_color: row.get::<_, i64>(3)? as u32,
                        icon_emoji_id: row.get::<_, Option<i64>>(4)?.map(|id| id as u64),
                        creator_id: UserId(row.get::<_, i64>(5)? as u64),
                        is_pinned: row.get::<_, i64>(6)? != 0,
                        is_closed: row.get::<_, i64>(7)? != 0,
                        is_hidden: row.get::<_, i64>(8)? != 0,
                        last_message_id: row.get::<_, Option<i64>>(9)?.map(MsgId),
                        unread_count: row.get::<_, i64>(10)? as u32,
                        created_at_ms: row.get(11)?,
                    })
                })
                .map_db()?;

            let mut topics = Vec::new();
            for r in rows {
                topics.push(r.map_db()?);
            }
            Ok(topics)
        })
    }

    // --- Message Operations ---

    pub fn save_message(&self, msg: &MessageRecord) -> Result<()> {
        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"
                INSERT INTO messages (
                    chat_id, message_id, sender_id, to_peer_raw, topic_id,
                    reply_to_id, pts, date_ms, content_json, media_type,
                    flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms,
                    reply_to_json, forward_info_json, reactions_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                "#,
                params![
                    msg.chat_id.0,
                    msg.message_id.0,
                    msg.sender_id.0 as i64,
                    msg.to_peer_raw as i64,
                    msg.topic_id.map(|t| t.0),
                    msg.reply_to_id.map(|r| r.0),
                    msg.pts as i64,
                    msg.date_ms,
                    msg.content_json,
                    msg.media_type,
                    msg.flags_bitmask as i64,
                    msg.client_nonce.map(|n| n as i64),
                    msg.edit_date_ms,
                    msg.deleted_at_ms,
                    msg.reply_to_json,
                    msg.forward_info_json,
                    msg.reactions_json,
                ],
            )
            .map_db()?;

            // Synchronously update chat dialog summary fields
            let _ = tx.execute(
                r#"
                UPDATE chats SET
                    pts = MAX(pts, ?1),
                    last_message_id = MAX(last_message_id, ?2),
                    last_message_date_ms = MAX(last_message_date_ms, ?3),
                    updated_at_ms = MAX(updated_at_ms, ?3)
                WHERE chat_id = ?4
                "#,
                params![
                    msg.pts as i64,
                    msg.message_id.0,
                    msg.date_ms,
                    msg.chat_id.0,
                ],
            );

            Ok(())
        })
    }

    pub fn update_message(&self, msg: &MessageRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            let count = conn.execute(
                r#"
                UPDATE messages SET
                    pts = ?1,
                    content_json = ?2,
                    media_type = ?3,
                    flags_bitmask = ?4,
                    edit_date_ms = ?5,
                    deleted_at_ms = ?6,
                    reactions_json = ?7
                WHERE chat_id = ?8 AND message_id = ?9
                "#,
                params![
                    msg.pts as i64,
                    msg.content_json,
                    msg.media_type,
                    msg.flags_bitmask as i64,
                    msg.edit_date_ms,
                    msg.deleted_at_ms,
                    msg.reactions_json,
                    msg.chat_id.0,
                    msg.message_id.0,
                ],
            )
            .map_db()?;
            if count == 0 {
                return Err(TelegramError::NotFound(format!(
                    "Message {}:{} not found",
                    msg.chat_id, msg.message_id
                )));
            }
            Ok(())
        })
    }

    pub fn get_message(&self, chat_id: ChatId, message_id: MsgId) -> Result<Option<MessageRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, message_id, sender_id, to_peer_raw, topic_id,
                           reply_to_id, pts, date_ms, content_json, media_type,
                           flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms,
                           reply_to_json, forward_info_json, reactions_json
                    FROM messages WHERE chat_id = ?1 AND message_id = ?2
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![chat_id.0, message_id.0]).map_db()?;

            if let Some(row) = rows.next().map_db()? {
                Ok(Some(MessageRecord {
                    chat_id: ChatId(row.get(0).map_db()?),
                    message_id: MsgId(row.get(1).map_db()?),
                    sender_id: UserId(row.get::<_, i64>(2).map_db()? as u64),
                    to_peer_raw: row.get::<_, i64>(3).map_db()? as u64,
                    topic_id: row.get::<_, Option<i64>>(4).map_db()?.map(TopicId),
                    reply_to_id: row.get::<_, Option<i64>>(5).map_db()?.map(MsgId),
                    pts: row.get::<_, i64>(6).map_db()? as u64,
                    date_ms: row.get(7).map_db()?,
                    content_json: row.get(8).map_db()?,
                    media_type: row.get(9).map_db()?,
                    flags_bitmask: row.get::<_, i64>(10).map_db()? as u32,
                    client_nonce: row.get::<_, Option<i64>>(11).map_db()?.map(|n| n as u64),
                    edit_date_ms: row.get(12).map_db()?,
                    deleted_at_ms: row.get(13).map_db()?,
                    reply_to_json: row.get(14).map_db()?,
                    forward_info_json: row.get(15).map_db()?,
                    reactions_json: row.get(16).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn next_message_id(&self, chat_id: ChatId) -> Result<MsgId> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_transaction(|tx| {
            let next_id: i64 = tx
                .query_row(
                    r#"
                    INSERT INTO chat_message_sequences (chat_id, next_msg_id, account_generation, updated_at_ms)
                    VALUES (
                        ?1,
                        COALESCE((SELECT MAX(message_id) FROM messages WHERE chat_id = ?1), 0) + 1,
                        1,
                        ?2
                    )
                    ON CONFLICT(chat_id) DO UPDATE SET
                        next_msg_id = chat_message_sequences.next_msg_id + 1,
                        updated_at_ms = excluded.updated_at_ms
                    RETURNING next_msg_id
                    "#,
                    params![chat_id.0, now_ms],
                    |row| row.get(0),
                )
                .map_err(|e| TelegramError::Database(format!("Failed to increment next_message_id: {}", e)))?;
            Ok(MsgId(next_id))
        })
    }

    // --- Deduplication Table Operations ---

    pub fn find_dedup_message(&self, sender_id: u64, nonce: u64) -> Result<Option<MessageDedupRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT sender_id, client_nonce, message_id, chat_id, pts, created_at_ms
                    FROM message_dedup WHERE sender_id = ?1 AND client_nonce = ?2
                    "#,
                )
                .map_db()?;

            let mut rows = stmt.query(params![sender_id as i64, nonce as i64]).map_db()?;

            if let Some(row) = rows.next().map_db()? {
                Ok(Some(MessageDedupRecord {
                    sender_id: UserId(row.get::<_, i64>(0).map_db()? as u64),
                    client_nonce: row.get::<_, i64>(1).map_db()? as u64,
                    message_id: MsgId(row.get(2).map_db()?),
                    chat_id: ChatId(row.get(3).map_db()?),
                    pts: row.get::<_, i64>(4).map_db()? as u64,
                    created_at_ms: row.get(5).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn insert_dedup_record(&self, record: &MessageDedupRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO message_dedup (sender_id, client_nonce, message_id, chat_id, pts, created_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT(sender_id, client_nonce) DO NOTHING
                "#,
                params![
                    record.sender_id.0 as i64,
                    record.client_nonce as i64,
                    record.message_id.0,
                    record.chat_id.0,
                    record.pts as i64,
                    record.created_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    // --- PTS State Queries ---

    pub fn get_current_pts(&self, chat_id: ChatId) -> Result<u64> {
        self.pool.with_reader(|conn| {
            let pts: i64 = conn
                .query_row(
                    "SELECT pts FROM pts_sequences WHERE chat_id = ?1",
                    params![chat_id.0],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(pts as u64)
        })
    }

    pub fn get_journal_floor_pts(&self, chat_id: ChatId) -> Result<u64> {
        self.pool.with_reader(|conn| {
            let floor: Option<i64> = conn
                .query_row(
                    "SELECT MIN(pts) FROM pts_updates WHERE chat_id = ?1",
                    params![chat_id.0],
                    |row| row.get(0),
                )
                .unwrap_or(None);

            if let Some(f) = floor {
                Ok(f as u64)
            } else {
                let current: i64 = conn
                    .query_row(
                        "SELECT pts FROM pts_sequences WHERE chat_id = ?1",
                        params![chat_id.0],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                Ok(current as u64)
            }
        })
    }

    // --- Read States ---

    pub fn update_read_state(&self, state: &ReadStateRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO read_states (
                    chat_id, user_id, topic_id, inbox_max_id, outbox_max_id, unread_count, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(chat_id, user_id, topic_id) DO UPDATE SET
                    inbox_max_id = MAX(read_states.inbox_max_id, excluded.inbox_max_id),
                    outbox_max_id = MAX(read_states.outbox_max_id, excluded.outbox_max_id),
                    unread_count = excluded.unread_count,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![
                    state.chat_id.0,
                    state.user_id.0 as i64,
                    state.topic_id.0,
                    state.inbox_max_id.0,
                    state.outbox_max_id.0,
                    state.unread_count as i64,
                    state.updated_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_read_state(
        &self,
        chat_id: ChatId,
        user_id: UserId,
        topic_id: TopicId,
    ) -> Result<Option<ReadStateRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, user_id, topic_id, inbox_max_id, outbox_max_id, unread_count, updated_at_ms
                    FROM read_states WHERE chat_id = ?1 AND user_id = ?2 AND topic_id = ?3
                    "#,
                )
                .map_db()?;

            let mut rows = stmt
                .query(params![chat_id.0, user_id.0 as i64, topic_id.0])
                .map_db()?;

            if let Some(row) = rows.next().map_db()? {
                Ok(Some(ReadStateRecord {
                    chat_id: ChatId(row.get(0).map_db()?),
                    user_id: UserId(row.get::<_, i64>(1).map_db()? as u64),
                    topic_id: TopicId(row.get(2).map_db()?),
                    inbox_max_id: MsgId(row.get(3).map_db()?),
                    outbox_max_id: MsgId(row.get(4).map_db()?),
                    unread_count: row.get::<_, i64>(5).map_db()? as u32,
                    updated_at_ms: row.get(6).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    // ========================================================================
    // --- Milestone 1: Messaging, Chats & Membership Operations ---
    // ========================================================================

    /// Allocates the next sequential ChatId.
    pub fn next_chat_id(&self) -> Result<ChatId> {
        self.pool.with_reader(|conn| {
            let max_id: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(chat_id), 0) FROM chats WHERE chat_id < 500000000",
                    rusqlite::params![],
                    |row| row.get(0),
                )
                .map_db()?;
            Ok(ChatId(max_id + 1))
        })
    }

    /// Atomically creates a chat, records the creator with full owner permissions,
    /// optionally adds additional member IDs, and initializes the PTS sequence.
    pub fn create_chat_with_creator(
        &self,
        creator_id: UserId,
        title: &str,
        member_ids: &[UserId],
        now_ms: i64,
    ) -> Result<ChatRecord> {
        let chat_id = self.next_chat_id()?;
        let chat = ChatRecord {
            chat_id,
            chat_type: 1, // Group
            peer_type: 2, // Chat
            title: title.to_string(),
            username: None,
            about: None,
            creator_id,
            is_forum: false,
            flags: 0,
            photo_file_id: None,
            slowmode_seconds: 0,
            linked_chat_id: None,
            pinned_message_id: None,
            pts: 0,
            last_message_id: MsgId(0),
            last_message_date_ms: 0,
            unread_count: 0,
            account_generation: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };

        self.pool.with_transaction(|tx| {
            tx.execute(
                r#"
                INSERT INTO chats (
                    chat_id, chat_type, peer_type, title, username, about,
                    creator_id, is_forum, flags, photo_file_id, slowmode_seconds,
                    linked_chat_id, pinned_message_id, pts, last_message_id,
                    last_message_date_ms, unread_count, account_generation,
                    created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
                "#,
                params![
                    chat.chat_id.0,
                    chat.chat_type as i64,
                    chat.peer_type as i64,
                    chat.title,
                    chat.username,
                    chat.about,
                    chat.creator_id.0 as i64,
                    chat.is_forum as i64,
                    chat.flags as i64,
                    chat.photo_file_id,
                    chat.slowmode_seconds as i64,
                    chat.linked_chat_id.map(|c| c.0),
                    chat.pinned_message_id.map(|m| m.0),
                    chat.pts as i64,
                    chat.last_message_id.0,
                    chat.last_message_date_ms,
                    chat.unread_count as i64,
                    chat.account_generation as i64,
                    chat.created_at_ms,
                    chat.updated_at_ms,
                ],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;

            // Insert creator as owner in chat_members
            tx.execute(
                r#"
                INSERT INTO chat_members (
                    chat_id, user_id, role, permissions_bitmask, admin_title,
                    admin_rights, banned_rights, until_date_ms, joined_at_ms,
                    invited_by, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                params![
                    chat.chat_id.0,
                    chat.creator_id.0 as i64,
                    "owner",
                    0xFFFFi64,
                    Some("Owner"),
                    0xFFFFFFFFi64,
                    0i64,
                    Option::<i64>::None,
                    now_ms,
                    Option::<i64>::None,
                    now_ms,
                ],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;

            // Insert any extra member IDs
            for &member_id in member_ids {
                if member_id != creator_id {
                    tx.execute(
                        r#"
                        INSERT INTO chat_members (
                            chat_id, user_id, role, permissions_bitmask, admin_title,
                            admin_rights, banned_rights, until_date_ms, joined_at_ms,
                            invited_by, updated_at_ms
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                        "#,
                        params![
                            chat.chat_id.0,
                            member_id.0 as i64,
                            "member",
                            0xFFFFi64,
                            Option::<String>::None,
                            0i64,
                            0i64,
                            Option::<i64>::None,
                            now_ms,
                            Some(creator_id.0 as i64),
                            now_ms,
                        ],
                    )
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                }
            }

            // Initialize pts_sequences
            tx.execute(
                r#"
                INSERT INTO pts_sequences (chat_id, pts, account_generation, updated_at_ms)
                VALUES (?1, 0, 1, ?2)
                ON CONFLICT(chat_id) DO NOTHING
                "#,
                params![chat.chat_id.0, now_ms],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;

            Ok(chat)
        })
    }

    /// Retrieves all user IDs belonging to a chat, ordered by join time.
    pub fn get_chat_members(&self, chat_id: ChatId) -> Result<Vec<UserId>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT DISTINCT user_id FROM (
                        SELECT user_id, joined_at_ms FROM chat_members WHERE chat_id = ?1 AND LOWER(TRIM(role)) NOT IN ('kicked', 'banned', 'left')
                        UNION ALL
                        SELECT creator_id AS user_id, created_at_ms AS joined_at_ms FROM chats WHERE chat_id = ?1
                    )
                    ORDER BY joined_at_ms ASC
                    "#,
                )
                .map_db()?;

            let rows = stmt
                .query_map(params![chat_id.0], |row| {
                    Ok(UserId(row.get::<_, i64>(0)? as u64))
                })
                .map_db()?;

            let mut members = Vec::new();
            for r in rows {
                members.push(r.map_db()?);
            }
            Ok(members)
        })
    }

    /// Alias for get_chat_members.
    pub fn get_chat_member_ids(&self, chat_id: ChatId) -> Result<Vec<UserId>> {
        self.get_chat_members(chat_id)
    }

    /// Lists dialog records for all chats where `user_id` is a member or creator.
    pub fn list_user_chats(&self, user_id: UserId) -> Result<Vec<ChatRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT c.chat_id, c.chat_type, c.peer_type, c.title, c.username, c.about,
                           c.creator_id, c.is_forum, c.flags, c.photo_file_id, c.slowmode_seconds,
                           c.linked_chat_id, c.pinned_message_id, c.pts, c.last_message_id,
                           c.last_message_date_ms,
                           COALESCE(rs.unread_count, c.unread_count, 0) AS unread_count,
                           c.account_generation,
                           c.created_at_ms, c.updated_at_ms
                    FROM chats c
                    JOIN (
                        SELECT chat_id FROM chat_members WHERE user_id = ?1 AND LOWER(TRIM(role)) NOT IN ('kicked', 'banned', 'left')
                        UNION
                        SELECT chat_id FROM chats WHERE creator_id = ?1
                    ) member_chats ON member_chats.chat_id = c.chat_id
                    LEFT JOIN read_states rs ON rs.chat_id = c.chat_id AND rs.user_id = ?1 AND rs.topic_id = 0
                    LEFT JOIN user_chat_settings ucs ON ucs.chat_id = c.chat_id AND ucs.user_id = ?1
                    ORDER BY COALESCE(ucs.is_pinned, 0) DESC,
                             COALESCE(ucs.pin_order, 0) ASC,
                             CASE WHEN c.last_message_id > 0 THEN c.last_message_date_ms ELSE 0 END DESC,
                             c.chat_id ASC
                    "#,
                )
                .map_db()?;

            let rows = stmt
                .query_map(rusqlite::params![user_id.0 as i64], |row| {
                    Ok(ChatRecord {
                        chat_id: ChatId(row.get(0)?),
                        chat_type: row.get::<_, i64>(1)? as u8,
                        peer_type: row.get::<_, i64>(2)? as u8,
                        title: row.get(3)?,
                        username: row.get(4)?,
                        about: row.get(5)?,
                        creator_id: UserId(row.get::<_, i64>(6)? as u64),
                        is_forum: row.get::<_, i64>(7)? != 0,
                        flags: row.get::<_, i64>(8)? as u32,
                        photo_file_id: row.get(9)?,
                        slowmode_seconds: row.get::<_, i64>(10)? as u32,
                        linked_chat_id: row.get::<_, Option<i64>>(11)?.map(ChatId),
                        pinned_message_id: row.get::<_, Option<i64>>(12)?.map(MsgId),
                        pts: row.get::<_, i64>(13)? as u64,
                        last_message_id: MsgId(row.get(14)?),
                        last_message_date_ms: row.get(15)?,
                        unread_count: row.get::<_, i64>(16)? as u32,
                        account_generation: row.get::<_, i64>(17)? as u64,
                        created_at_ms: row.get(18)?,
                        updated_at_ms: row.get(19)?,
                    })
                })
                .map_db()?;

            let mut chats = Vec::new();
            for r in rows {
                chats.push(r.map_db()?);
            }
            Ok(chats)
        })
    }

    /// Lists dialog summaries for all chats where `user_id` is a member or creator.
    pub fn list_user_chat_summaries(&self, user_id: UserId) -> Result<Vec<ChatSummary>> {
        let chats = self.list_user_chats(user_id)?;
        let mut summaries = Vec::with_capacity(chats.len());
        for c in chats {
            let kind = match c.chat_type {
                0 => "direct",
                1 => "group",
                2 => "channel",
                3 => "saved",
                4 => "secret",
                _ => "group",
            };
            let timestamp = if c.last_message_date_ms > 0 {
                c.last_message_date_ms
            } else {
                c.created_at_ms
            };
            summaries.push(ChatSummary {
                chat_id: c.chat_id.0,
                title: c.title,
                kind: kind.to_string(),
                chat_type: c.chat_type,
                creator_id: c.creator_id.0,
                pts: c.pts,
                unread_count: c.unread_count,
                last_message: String::new(),
                last_message_id: c.last_message_id.0,
                timestamp,
                updated_at_ms: c.updated_at_ms,
                is_pinned: false,
                is_muted: false,
                current_pts: c.pts,
                draft: None,
            });
        }
        Ok(summaries)
    }

    /// Lists messages in a chat with backward pagination and deleted message filtering.
    /// Returns items sorted chronologically (oldest to newest within the requested page).
    pub fn list_messages(
        &self,
        chat_id: ChatId,
        offset_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<MessageRecord>> {
        let fetch_limit = limit.clamp(1, 100);

        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, message_id, sender_id, to_peer_raw, topic_id,
                           reply_to_id, pts, date_ms, content_json, media_type,
                           flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms,
                           reply_to_json, forward_info_json, reactions_json
                    FROM messages
                    WHERE chat_id = ?1
                      AND (?2 IS NULL OR ?2 <= 0 OR message_id < ?2)
                      AND deleted_at_ms IS NULL
                    ORDER BY message_id DESC
                    LIMIT ?3
                    "#,
                )
                .map_db()?;

            let rows = stmt
                .query_map(
                    params![chat_id.0, offset_id, fetch_limit as i64],
                    |row| {
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
                    },
                )
                .map_db()?;

            let mut items = Vec::new();
            for r in rows {
                items.push(r.map_db()?);
            }
            items.reverse();
            Ok(items)
        })
    }

    // ========================================================================
    // --- Milestone 3: Monetization & Payment Database Operations ---
    // ========================================================================

    pub fn create_order(&self, order: &OrderRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO orders (
                    order_id, user_id, username, plan, amount, original_amount,
                    is_admin_order, status, platform, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                params![
                    order.order_id,
                    order.user_id.0 as i64,
                    order.username,
                    order.plan,
                    order.amount,
                    order.original_amount,
                    order.is_admin_order as i64,
                    order.status,
                    order.platform,
                    order.created_at_ms,
                    order.updated_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_order(&self, order_id: &str) -> Result<Option<OrderRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT order_id, user_id, username, plan, amount, original_amount,
                           is_admin_order, status, platform, created_at_ms, updated_at_ms
                    FROM orders WHERE order_id = ?1
                    "#,
                )
                .map_db()?;
            let mut rows = stmt.query(params![order_id]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(OrderRecord {
                    order_id: row.get(0).map_db()?,
                    user_id: UserId(row.get::<_, i64>(1).map_db()? as u64),
                    username: row.get(2).map_db()?,
                    plan: row.get(3).map_db()?,
                    amount: row.get(4).map_db()?,
                    original_amount: row.get(5).map_db()?,
                    is_admin_order: row.get::<_, i64>(6).map_db()? != 0,
                    status: row.get(7).map_db()?,
                    platform: row.get(8).map_db()?,
                    created_at_ms: row.get(9).map_db()?,
                    updated_at_ms: row.get(10).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn update_order_status(&self, order_id: &str, status: &str) -> Result<bool> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let count = conn
                .execute(
                    "UPDATE orders SET status = ?1, updated_at_ms = ?2 WHERE order_id = ?3",
                    params![status, now_ms, order_id],
                )
                .map_db()?;
            Ok(count > 0)
        })
    }

    pub fn create_redeem_code(&self, code: &str, days: u32, plan_type: &str) -> Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO redeem_codes (code, days, plan_type, used, created_at_ms)
                VALUES (?1, ?2, ?3, 0, ?4)
                "#,
                params![code, days as i64, plan_type, now_ms],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_redeem_code(&self, code: &str) -> Result<Option<RedeemCodeRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT code, days, plan_type, used, used_by_user_id, used_at_ms, created_at_ms
                    FROM redeem_codes WHERE code = ?1
                    "#,
                )
                .map_db()?;
            let mut rows = stmt.query(params![code]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(RedeemCodeRecord {
                    code: row.get(0).map_db()?,
                    days: row.get::<_, i64>(1).map_db()? as u32,
                    plan_type: row.get(2).map_db()?,
                    used: row.get::<_, i64>(3).map_db()? != 0,
                    used_by_user_id: row.get::<_, Option<i64>>(4).map_db()?.map(|u| UserId(u as u64)),
                    used_at_ms: row.get(5).map_db()?,
                    created_at_ms: row.get(6).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    /// Atomically redeems a code with anti-double-spend concurrency barrier.
    pub fn use_redeem_code_atomic(
        &self,
        user_id: UserId,
        username: &str,
        code: &str,
    ) -> Result<UseRedeemCodeResult> {
        let trimmed_code = code.trim().to_uppercase();
        if trimmed_code.len() < 8 || trimmed_code.len() > 64 || !trimmed_code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(TelegramError::Validation("兑换码格式无效".into()));
        }

        let now_ms = chrono::Utc::now().timestamp_millis();

        self.pool.with_transaction(|tx| {
            // Concurrency race defense: Insert claim first; fails if already claimed by any user.
            let claim_res = tx.execute(
                "INSERT INTO redeem_claims (code, user_id, username, claimed_at_ms) VALUES (?1, ?2, ?3, ?4)",
                params![trimmed_code, user_id.0 as i64, username, now_ms],
            );
            if claim_res.is_err() {
                return Err(TelegramError::Validation("兑换码不存在或已使用".into()));
            }

            // Check redeem code existence and state
            let mut stmt = tx
                .prepare("SELECT days, plan_type, used FROM redeem_codes WHERE code = ?1")
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let mut rows = stmt
                .query(params![trimmed_code])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let (days, plan_type, used): (i64, String, i64) = match rows.next().map_err(|e| TelegramError::Database(e.to_string()))? {
                Some(r) => (
                    r.get(0).map_err(|e| TelegramError::Database(e.to_string()))?,
                    r.get(1).map_err(|e| TelegramError::Database(e.to_string()))?,
                    r.get(2).map_err(|e| TelegramError::Database(e.to_string()))?,
                ),
                None => return Err(TelegramError::NotFound("兑换码不存在或已使用".into())),
            };

            if used != 0 {
                return Err(TelegramError::Validation("兑换码不存在或已使用".into()));
            }

            // Mark code as used
            tx.execute(
                "UPDATE redeem_codes SET used = 1, used_by_user_id = ?1, used_at_ms = ?2 WHERE code = ?3 AND used = 0",
                params![user_id.0 as i64, now_ms, trimmed_code],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;

            // Fetch current expiry to stack duration
            let current_expiry: Option<i64> = tx
                .query_row(
                    "SELECT expires_at_ms FROM user_memberships WHERE user_id = ?1",
                    params![user_id.0 as i64],
                    |row| row.get(0),
                )
                .unwrap_or(None);

            let start_ms = match current_expiry {
                Some(exp) if exp > now_ms => exp,
                _ => now_ms,
            };
            let duration_ms = days * 86_400_000;
            let new_expiry_ms = start_ms + duration_ms;

            // Upsert user_memberships
            tx.execute(
                r#"
                INSERT INTO user_memberships (user_id, is_active, membership_type, expires_at_ms, updated_at_ms)
                VALUES (?1, 1, ?2, ?3, ?4)
                ON CONFLICT(user_id) DO UPDATE SET
                    is_active = 1,
                    membership_type = excluded.membership_type,
                    expires_at_ms = excluded.expires_at_ms,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![user_id.0 as i64, plan_type, new_expiry_ms, now_ms],
            )
            .map_err(|e| TelegramError::Database(e.to_string()))?;

            Ok(UseRedeemCodeResult {
                success: true,
                message: format!("兑换成功！获得{}天会员", days),
                days_added: days as u32,
                expires_at_ms: new_expiry_ms,
                plan_type,
            })
        })
    }

    pub fn get_user_membership(&self, user_id: UserId) -> Result<UserMembershipRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT user_id, is_active, membership_type, expires_at_ms, updated_at_ms
                    FROM user_memberships WHERE user_id = ?1
                    "#,
                )
                .map_db()?;
            let mut rows = stmt.query(params![user_id.0 as i64]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                let is_active_raw: i64 = row.get(1).map_db()?;
                let expires_at_ms: Option<i64> = row.get(3).map_db()?;
                let is_active = is_active_raw != 0 && expires_at_ms.map(|e| e > now_ms).unwrap_or(false);
                Ok(UserMembershipRecord {
                    user_id: UserId(row.get::<_, i64>(0).map_db()? as u64),
                    is_active,
                    membership_type: row.get(2).map_db()?,
                    expires_at_ms,
                    updated_at_ms: row.get(4).map_db()?,
                })
            } else {
                Ok(UserMembershipRecord {
                    user_id,
                    is_active: false,
                    membership_type: "free".into(),
                    expires_at_ms: None,
                    updated_at_ms: now_ms,
                })
            }
        })
    }

    pub fn fulfill_order_membership(
        &self,
        order_id: &str,
        provider_trade_no: Option<&str>,
    ) -> Result<UserMembershipRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let order = self.get_order(order_id)?.ok_or_else(|| TelegramError::NotFound("Order not found".into()))?;

        if order.status == "PAID" {
            return self.get_user_membership(order.user_id);
        }

        let days: i64 = match order.plan.as_str() {
            "yearly" => 365,
            "quarterly" => 90,
            _ => 30, // monthly default
        };
        let duration_ms = days * 86_400_000;

        self.pool.with_transaction(|tx| {
            let trade_no = provider_trade_no.unwrap_or(order_id);
            tx.execute(
                "INSERT INTO payment_receipts (order_id, provider, provider_trade_no, amount, received_at_ms) VALUES (?1, 'alipay', ?2, ?3, ?4) ON CONFLICT(order_id) DO NOTHING",
                params![order_id, trade_no, order.amount, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;

            let current_expiry: Option<i64> = tx
                .query_row(
                    "SELECT expires_at_ms FROM user_memberships WHERE user_id = ?1",
                    params![order.user_id.0 as i64],
                    |row| row.get(0),
                )
                .unwrap_or(None);

            let start_ms = match current_expiry {
                Some(exp) if exp > now_ms => exp,
                _ => now_ms,
            };
            let new_expiry_ms = start_ms + duration_ms;

            tx.execute(
                r#"
                INSERT INTO user_memberships (user_id, is_active, membership_type, expires_at_ms, updated_at_ms)
                VALUES (?1, 1, ?2, ?3, ?4)
                ON CONFLICT(user_id) DO UPDATE SET
                    is_active = 1,
                    membership_type = excluded.membership_type,
                    expires_at_ms = excluded.expires_at_ms,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                params![order.user_id.0 as i64, order.plan, new_expiry_ms, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;

            tx.execute(
                "INSERT INTO purchase_history (user_id, order_id, plan, amount, valid_from_ms, valid_to_ms, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![order.user_id.0 as i64, order_id, order.plan, order.amount, start_ms, new_expiry_ms, now_ms],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;

            tx.execute(
                "UPDATE orders SET status = 'PAID', updated_at_ms = ?1 WHERE order_id = ?2",
                params![now_ms, order_id],
            ).map_err(|e| TelegramError::Database(e.to_string()))?;

            Ok(UserMembershipRecord {
                user_id: order.user_id,
                is_active: true,
                membership_type: order.plan,
                expires_at_ms: Some(new_expiry_ms),
                updated_at_ms: now_ms,
            })
        })
    }

    // ========================================================================
    // --- Milestone 3: Telegram Core Feature Operations ---
    // ========================================================================

    pub fn get_chat_member_details(&self, chat_id: ChatId) -> Result<Vec<ChatMemberDetail>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT m.chat_id, m.user_id, u.username, u.first_name, u.last_name, u.profile_photo_file_id,
                           m.role, m.permissions_bitmask, m.admin_title, m.admin_rights, m.banned_rights, m.joined_at_ms
                    FROM chat_members m
                    LEFT JOIN users u ON u.user_id = m.user_id
                    WHERE m.chat_id = ?1 AND LOWER(TRIM(m.role)) NOT IN ('kicked', 'banned', 'left')
                    ORDER BY m.joined_at_ms ASC
                    "#,
                )
                .map_db()?;

            let rows = stmt
                .query_map(params![chat_id.0], |row| {
                    Ok(ChatMemberDetail {
                        chat_id: ChatId(row.get(0)?),
                        user_id: UserId(row.get::<_, i64>(1)? as u64),
                        username: row.get(2)?,
                        first_name: row.get::<_, Option<String>>(3)?.unwrap_or_else(|| "User".into()),
                        last_name: row.get(4)?,
                        profile_photo_file_id: row.get(5)?,
                        role: row.get(6)?,
                        permissions_bitmask: row.get::<_, i64>(7)? as u32,
                        admin_title: row.get(8)?,
                        admin_rights: row.get::<_, i64>(9)? as u32,
                        banned_rights: row.get::<_, i64>(10)? as u32,
                        joined_at_ms: row.get(11)?,
                    })
                })
                .map_db()?;

            let mut members = Vec::new();
            for r in rows {
                members.push(r.map_db()?);
            }
            Ok(members)
        })
    }

    pub fn remove_chat_member(&self, chat_id: ChatId, user_id: UserId) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let count = conn
                .execute(
                    "DELETE FROM chat_members WHERE chat_id = ?1 AND user_id = ?2",
                    params![chat_id.0, user_id.0 as i64],
                )
                .map_db()?;
            Ok(count > 0)
        })
    }

    pub fn update_chat_info(
        &self,
        chat_id: ChatId,
        title: Option<&str>,
        about: Option<&str>,
        photo_file_id: Option<&str>,
        is_forum: Option<bool>,
    ) -> Result<ChatRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let mut sets = vec!["updated_at_ms = ?1".to_string()];
            let mut params_vec: Vec<rusqlite::types::Value> = vec![now_ms.into()];

            if let Some(t) = title {
                params_vec.push(t.to_string().into());
                sets.push(format!("title = ?{}", params_vec.len()));
            }
            if let Some(a) = about {
                params_vec.push(a.to_string().into());
                sets.push(format!("about = ?{}", params_vec.len()));
            }
            if let Some(p) = photo_file_id {
                params_vec.push(p.to_string().into());
                sets.push(format!("photo_file_id = ?{}", params_vec.len()));
            }
            if let Some(f) = is_forum {
                params_vec.push((if f { 1i64 } else { 0i64 }).into());
                sets.push(format!("is_forum = ?{}", params_vec.len()));
            }

            params_vec.push(chat_id.0.into());
            let query = format!(
                "UPDATE chats SET {} WHERE chat_id = ?{}",
                sets.join(", "),
                params_vec.len()
            );

            conn.execute(&query, rusqlite::params_from_iter(params_vec))
                .map_db()?;
            Ok(())
        })?;

        self.get_chat(chat_id)?
            .ok_or_else(|| TelegramError::NotFound("Chat not found".into()))
    }

    pub fn get_forum_topic(&self, chat_id: ChatId, topic_id: TopicId) -> Result<Option<ForumTopicRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, topic_id, title, icon_color, icon_emoji_id,
                           creator_id, is_pinned, is_closed, is_hidden, last_message_id,
                           unread_count, created_at_ms
                    FROM forum_topics
                    WHERE chat_id = ?1 AND topic_id = ?2
                    "#,
                )
                .map_db()?;
            let mut rows = stmt.query(params![chat_id.0, topic_id.0]).map_db()?;
            if let Some(row) = rows.next().map_db()? {
                Ok(Some(ForumTopicRecord {
                    chat_id: ChatId(row.get(0).map_db()?),
                    topic_id: TopicId(row.get(1).map_db()?),
                    title: row.get(2).map_db()?,
                    icon_color: row.get::<_, i64>(3).map_db()? as u32,
                    icon_emoji_id: row.get::<_, Option<i64>>(4).map_db()?.map(|v| v as u64),
                    creator_id: UserId(row.get::<_, i64>(5).map_db()? as u64),
                    is_pinned: row.get::<_, i64>(6).map_db()? != 0,
                    is_closed: row.get::<_, i64>(7).map_db()? != 0,
                    is_hidden: row.get::<_, i64>(8).map_db()? != 0,
                    last_message_id: row.get::<_, Option<i64>>(9).map_db()?.map(MsgId),
                    unread_count: row.get::<_, i64>(10).map_db()? as u32,
                    created_at_ms: row.get(11).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn update_forum_topic(
        &self,
        chat_id: ChatId,
        topic_id: TopicId,
        title: Option<&str>,
        icon_color: Option<u32>,
        icon_emoji_id: Option<u64>,
        is_pinned: Option<bool>,
        is_closed: Option<bool>,
        is_hidden: Option<bool>,
    ) -> Result<ForumTopicRecord> {
        self.pool.with_conn(|conn| {
            let mut sets = Vec::new();
            let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

            if let Some(t) = title {
                params_vec.push(t.to_string().into());
                sets.push(format!("title = ?{}", params_vec.len()));
            }
            if let Some(c) = icon_color {
                params_vec.push((c as i64).into());
                sets.push(format!("icon_color = ?{}", params_vec.len()));
            }
            if let Some(e) = icon_emoji_id {
                params_vec.push((e as i64).into());
                sets.push(format!("icon_emoji_id = ?{}", params_vec.len()));
            }
            if let Some(p) = is_pinned {
                params_vec.push((if p { 1i64 } else { 0i64 }).into());
                sets.push(format!("is_pinned = ?{}", params_vec.len()));
            }
            if let Some(c) = is_closed {
                params_vec.push((if c { 1i64 } else { 0i64 }).into());
                sets.push(format!("is_closed = ?{}", params_vec.len()));
            }
            if let Some(h) = is_hidden {
                params_vec.push((if h { 1i64 } else { 0i64 }).into());
                sets.push(format!("is_hidden = ?{}", params_vec.len()));
            }

            if !sets.is_empty() {
                params_vec.push(chat_id.0.into());
                let chat_idx = params_vec.len();
                params_vec.push(topic_id.0.into());
                let topic_idx = params_vec.len();
                let query = format!(
                    "UPDATE forum_topics SET {} WHERE chat_id = ?{} AND topic_id = ?{}",
                    sets.join(", "),
                    chat_idx,
                    topic_idx
                );
                conn.execute(&query, rusqlite::params_from_iter(params_vec))
                    .map_db()?;
            }
            Ok(())
        })?;

        self.get_forum_topic(chat_id, topic_id)?
            .ok_or_else(|| TelegramError::NotFound("Forum topic not found".into()))
    }

    pub fn delete_forum_topic(&self, chat_id: ChatId, topic_id: TopicId) -> Result<bool> {
        self.pool.with_conn(|conn| {
            let count = conn
                .execute(
                    "DELETE FROM forum_topics WHERE chat_id = ?1 AND topic_id = ?2",
                    params![chat_id.0, topic_id.0],
                )
                .map_db()?;
            Ok(count > 0)
        })
    }

    pub fn update_user_profile(
        &self,
        user_id: UserId,
        first_name: Option<&str>,
        last_name: Option<&str>,
        bio: Option<&str>,
        username: Option<&str>,
        photo_file_id: Option<&str>,
    ) -> Result<UserRecord> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        self.pool.with_conn(|conn| {
            let mut sets = vec!["updated_at_ms = ?1".to_string()];
            let mut params_vec: Vec<rusqlite::types::Value> = vec![now_ms.into()];

            if let Some(fn_str) = first_name {
                params_vec.push(fn_str.to_string().into());
                sets.push(format!("first_name = ?{}", params_vec.len()));
            }
            if let Some(ln_str) = last_name {
                params_vec.push(ln_str.to_string().into());
                sets.push(format!("last_name = ?{}", params_vec.len()));
            }
            if let Some(bio_str) = bio {
                params_vec.push(bio_str.to_string().into());
                sets.push(format!("bio = ?{}", params_vec.len()));
            }
            if let Some(un_str) = username {
                params_vec.push(un_str.to_string().into());
                sets.push(format!("username = ?{}", params_vec.len()));
            }
            if let Some(ph_str) = photo_file_id {
                params_vec.push(ph_str.to_string().into());
                sets.push(format!("profile_photo_file_id = ?{}", params_vec.len()));
            }

            params_vec.push((user_id.0 as i64).into());
            let query = format!(
                "UPDATE users SET {} WHERE user_id = ?{}",
                sets.join(", "),
                params_vec.len()
            );

            conn.execute(&query, rusqlite::params_from_iter(params_vec))
                .map_db()?;
            Ok(())
        })?;

        self.get_user(user_id)?
            .ok_or_else(|| TelegramError::NotFound("User not found".into()))
    }

    pub fn list_messages_with_topic(
        &self,
        chat_id: ChatId,
        topic_id: Option<TopicId>,
        offset_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<MessageRecord>> {
        let fetch_limit = limit.clamp(1, 100);

        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT chat_id, message_id, sender_id, to_peer_raw, topic_id,
                           reply_to_id, pts, date_ms, content_json, media_type,
                           flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms,
                           reply_to_json, forward_info_json, reactions_json
                    FROM messages
                    WHERE chat_id = ?1
                      AND (?2 IS NULL OR ?2 <= 0 OR message_id < ?2)
                      AND (?3 IS NULL OR topic_id = ?3)
                      AND deleted_at_ms IS NULL
                    ORDER BY message_id DESC
                    LIMIT ?4
                    "#,
                )
                .map_db()?;

            let rows = stmt
                .query_map(
                    params![
                        chat_id.0,
                        offset_id,
                        topic_id.map(|t| t.0),
                        fetch_limit as i64,
                    ],
                    |row| {
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
                    },
                )
                .map_db()?;

            let mut items = Vec::new();
            for r in rows {
                items.push(r.map_db()?);
            }
            Ok(items)
        })
    }

    pub fn save_device_push_token(&self, record: &crate::db::schema::DevicePushTokenRecord) -> Result<()> {
        self.pool.with_writer(|conn| {
            conn.execute(
                r#"
                INSERT INTO device_push_tokens (
                    token, user_id, platform, device_id, endpoint,
                    p256dh, auth_secret, app_version, created_at_ms, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(user_id, token) DO UPDATE SET
                    platform = excluded.platform,
                    device_id = excluded.device_id,
                    endpoint = excluded.endpoint,
                    p256dh = excluded.p256dh,
                    auth_secret = excluded.auth_secret,
                    app_version = excluded.app_version,
                    updated_at_ms = excluded.updated_at_ms
                "#,
                rusqlite::params![
                    record.token,
                    record.user_id.0 as i64,
                    record.platform,
                    record.device_id,
                    record.endpoint,
                    record.p256dh,
                    record.auth_secret,
                    record.app_version,
                    record.created_at_ms,
                    record.updated_at_ms,
                ],
            )
            .map_db()?;
            Ok(())
        })
    }

    pub fn get_user_push_tokens(&self, user_id: UserId) -> Result<Vec<crate::db::schema::DevicePushTokenRecord>> {
        self.pool.with_reader(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT token, user_id, platform, device_id, endpoint,
                           p256dh, auth_secret, app_version, created_at_ms, updated_at_ms
                    FROM device_push_tokens
                    WHERE user_id = ?1
                    ORDER BY updated_at_ms DESC
                    "#,
                )
                .map_db()?;

            let rows = stmt
                .query_map(rusqlite::params![user_id.0 as i64], |row| {
                    Ok(crate::db::schema::DevicePushTokenRecord {
                        token: row.get(0)?,
                        user_id: UserId(row.get::<_, i64>(1)? as u64),
                        platform: row.get(2)?,
                        device_id: row.get(3)?,
                        endpoint: row.get(4)?,
                        p256dh: row.get(5)?,
                        auth_secret: row.get(6)?,
                        app_version: row.get(7)?,
                        created_at_ms: row.get(8)?,
                        updated_at_ms: row.get(9)?,
                    })
                })
                .map_db()?;

            let mut list = Vec::new();
            for r in rows {
                list.push(r.map_db()?);
            }
            Ok(list)
        })
    }

    pub fn delete_device_push_token(&self, user_id: UserId, token: &str) -> Result<bool> {
        self.pool.with_writer(|conn| {
            let count = conn
                .execute(
                    "DELETE FROM device_push_tokens WHERE user_id = ?1 AND token = ?2",
                    rusqlite::params![user_id.0 as i64, token],
                )
                .map_db()?;
            Ok(count > 0)
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseRedeemCodeResult {
    pub success: bool,
    pub message: String,
    pub days_added: u32,
    pub expires_at_ms: i64,
    pub plan_type: String,
}

/// Cross-platform conversation dialog summary matching Swift `TelegramChatSummary`
/// and Desktop 4-pane dialog rows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChatSummary {
    #[serde(alias = "id")]
    pub chat_id: i64,
    pub title: String,
    #[serde(default)]
    pub kind: String, // "direct" | "group" | "channel" | "saved" | "secret"
    #[serde(default)]
    pub chat_type: u8,
    #[serde(default)]
    pub creator_id: u64,
    #[serde(default)]
    pub pts: u64,
    #[serde(default, alias = "unreadCount")]
    pub unread_count: u32,
    #[serde(default, alias = "lastMessage")]
    pub last_message: String,
    #[serde(default)]
    pub last_message_id: i64,
    #[serde(default)]
    pub timestamp: i64,
    #[serde(default)]
    pub updated_at_ms: i64,
    #[serde(default, alias = "isPinned")]
    pub is_pinned: bool,
    #[serde(default, alias = "isMuted")]
    pub is_muted: bool,
    #[serde(default, alias = "currentPts")]
    pub current_pts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
}

impl ChatSummary {
    pub fn new(chat_id: i64, title: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            chat_id,
            title: title.into(),
            kind: kind.into(),
            chat_type: 1,
            creator_id: 0,
            pts: 0,
            unread_count: 0,
            last_message: String::new(),
            last_message_id: 0,
            timestamp: 0,
            updated_at_ms: 0,
            is_pinned: false,
            is_muted: false,
            current_pts: 0,
            draft: None,
        }
    }
}
