pub mod migrations;
pub mod pool;
pub mod schema;

pub use migrations::run_migrations;
pub use pool::SqlitePool;
pub use schema::*;

use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MsgId, TopicId, UserId};
use rusqlite::params;
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
    pool: SqlitePool,
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
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
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

    pub fn create_chat(&self, chat: &ChatRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
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
        })
    }

    pub fn get_chat(&self, chat_id: ChatId) -> Result<Option<ChatRecord>> {
        self.pool.with_conn(|conn| {
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
                Ok(Some(ChatRecord {
                    chat_id: ChatId(row.get(0).map_db()?),
                    chat_type: row.get::<_, i64>(1).map_db()? as u8,
                    peer_type: row.get::<_, i64>(2).map_db()? as u8,
                    title: row.get(3).map_db()?,
                    username: row.get(4).map_db()?,
                    about: row.get(5).map_db()?,
                    creator_id: UserId(row.get::<_, i64>(6).map_db()? as u64),
                    is_forum: row.get::<_, i64>(7).map_db()? != 0,
                    flags: row.get::<_, i64>(8).map_db()? as u32,
                    photo_file_id: row.get(9).map_db()?,
                    slowmode_seconds: row.get::<_, i64>(10).map_db()? as u32,
                    linked_chat_id: row.get::<_, Option<i64>>(11).map_db()?.map(ChatId),
                    pinned_message_id: row.get::<_, Option<i64>>(12).map_db()?.map(MsgId),
                    pts: row.get::<_, i64>(13).map_db()? as u64,
                    last_message_id: MsgId(row.get(14).map_db()?),
                    last_message_date_ms: row.get(15).map_db()?,
                    unread_count: row.get::<_, i64>(16).map_db()? as u32,
                    account_generation: row.get::<_, i64>(17).map_db()? as u64,
                    created_at_ms: row.get(18).map_db()?,
                    updated_at_ms: row.get(19).map_db()?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    // --- Chat Member Operations ---

    pub fn add_chat_member(&self, member: &ChatMemberRecord) -> Result<()> {
        self.pool.with_conn(|conn| {
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
        })
    }

    pub fn get_chat_member(&self, chat_id: ChatId, user_id: UserId) -> Result<Option<ChatMemberRecord>> {
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO messages (
                    chat_id, message_id, sender_id, to_peer_raw, topic_id,
                    reply_to_id, pts, date_ms, content_json, media_type,
                    flags_bitmask, client_nonce, edit_date_ms, deleted_at_ms,
                    reply_to_json, forward_info_json, reactions_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                ON CONFLICT(chat_id, message_id) DO UPDATE SET
                    pts = excluded.pts,
                    content_json = excluded.content_json,
                    media_type = excluded.media_type,
                    flags_bitmask = excluded.flags_bitmask,
                    edit_date_ms = excluded.edit_date_ms,
                    deleted_at_ms = excluded.deleted_at_ms,
                    reactions_json = excluded.reactions_json
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
            Ok(())
        })
    }

    pub fn get_message(&self, chat_id: ChatId, message_id: MsgId) -> Result<Option<MessageRecord>> {
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
            let max_id: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(message_id), 0) FROM messages WHERE chat_id = ?1",
                    params![chat_id.0],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(MsgId(max_id + 1))
        })
    }

    // --- Deduplication Table Operations ---

    pub fn find_dedup_message(&self, sender_id: u64, nonce: u64) -> Result<Option<MessageDedupRecord>> {
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
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
        self.pool.with_conn(|conn| {
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
}
