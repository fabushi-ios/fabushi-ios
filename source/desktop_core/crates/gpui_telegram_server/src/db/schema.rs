use gpui_telegram_core::types::{ChatId, MsgId, TopicId, UserId};
use serde::{Deserialize, Serialize};

/// SQL DDL statements for the 10 core SQLite WAL tables and their indices.
pub const CREATE_USERS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    user_id INTEGER PRIMARY KEY,
    username TEXT UNIQUE COLLATE NOCASE,
    phone TEXT UNIQUE,
    first_name TEXT NOT NULL,
    last_name TEXT,
    bio TEXT,
    password_hash TEXT,
    is_bot INTEGER NOT NULL DEFAULT 0,
    is_verified INTEGER NOT NULL DEFAULT 0,
    is_support INTEGER NOT NULL DEFAULT 0,
    is_scam INTEGER NOT NULL DEFAULT 0,
    is_fake INTEGER NOT NULL DEFAULT 0,
    profile_photo_file_id TEXT,
    account_generation INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);
CREATE INDEX IF NOT EXISTS idx_users_phone ON users(phone);
"#;

pub const CREATE_SESSIONS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    client_type TEXT NOT NULL,
    app_version TEXT NOT NULL,
    ip_address TEXT,
    location_desc TEXT,
    account_generation INTEGER NOT NULL DEFAULT 1,
    token_hash TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    last_active_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    revoked_at_ms INTEGER
) STRICT;
CREATE INDEX IF NOT EXISTS idx_sessions_user_active ON sessions(user_id, expires_at_ms) WHERE revoked_at_ms IS NULL;
CREATE INDEX IF NOT EXISTS idx_sessions_user_device ON sessions(user_id, device_id);
"#;

pub const CREATE_CHATS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS chats (
    chat_id INTEGER PRIMARY KEY,
    chat_type INTEGER NOT NULL DEFAULT 0,
    peer_type INTEGER NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    username TEXT UNIQUE COLLATE NOCASE,
    about TEXT,
    creator_id INTEGER NOT NULL REFERENCES users(user_id),
    is_forum INTEGER NOT NULL DEFAULT 0,
    flags INTEGER NOT NULL DEFAULT 0,
    photo_file_id TEXT,
    slowmode_seconds INTEGER NOT NULL DEFAULT 0,
    linked_chat_id INTEGER,
    pinned_message_id INTEGER,
    pts INTEGER NOT NULL DEFAULT 0,
    last_message_id INTEGER NOT NULL DEFAULT 0,
    last_message_date_ms INTEGER NOT NULL DEFAULT 0,
    unread_count INTEGER NOT NULL DEFAULT 0,
    account_generation INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_chats_username ON chats(username);
CREATE INDEX IF NOT EXISTS idx_chats_creator ON chats(creator_id);
"#;

pub const CREATE_CHAT_MEMBERS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS chat_members (
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    permissions_bitmask INTEGER NOT NULL DEFAULT 0,
    admin_title TEXT,
    admin_rights INTEGER NOT NULL DEFAULT 0,
    banned_rights INTEGER NOT NULL DEFAULT 0,
    until_date_ms INTEGER,
    joined_at_ms INTEGER NOT NULL,
    invited_by INTEGER REFERENCES users(user_id),
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, user_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_chat_members_user ON chat_members(user_id);
"#;

pub const CREATE_FORUM_TOPICS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS forum_topics (
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    topic_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    icon_color INTEGER NOT NULL DEFAULT 0,
    icon_emoji_id INTEGER,
    creator_id INTEGER NOT NULL REFERENCES users(user_id),
    is_pinned INTEGER NOT NULL DEFAULT 0,
    is_closed INTEGER NOT NULL DEFAULT 0,
    is_hidden INTEGER NOT NULL DEFAULT 0,
    last_message_id INTEGER,
    unread_count INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, topic_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_forum_topics_list ON forum_topics(chat_id, is_pinned DESC, last_message_id DESC);
"#;

pub const CREATE_MESSAGES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL,
    sender_id INTEGER NOT NULL REFERENCES users(user_id),
    to_peer_raw INTEGER NOT NULL DEFAULT 0,
    topic_id INTEGER,
    reply_to_id INTEGER,
    pts INTEGER NOT NULL,
    date_ms INTEGER NOT NULL,
    content_json TEXT NOT NULL,
    media_type TEXT,
    flags_bitmask INTEGER NOT NULL DEFAULT 0,
    client_nonce INTEGER,
    edit_date_ms INTEGER,
    deleted_at_ms INTEGER,
    reply_to_json TEXT,
    forward_info_json TEXT,
    reactions_json TEXT,
    PRIMARY KEY (chat_id, message_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_messages_chat_pts ON messages(chat_id, pts);
CREATE INDEX IF NOT EXISTS idx_messages_chat_topic_date ON messages(chat_id, topic_id, date_ms DESC);
CREATE INDEX IF NOT EXISTS idx_messages_date ON messages(date_ms);
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_sender_nonce ON messages(sender_id, client_nonce)
    WHERE client_nonce IS NOT NULL;
"#;

pub const CREATE_PTS_SEQUENCES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS pts_sequences (
    chat_id INTEGER PRIMARY KEY,
    pts INTEGER NOT NULL DEFAULT 0,
    account_generation INTEGER NOT NULL DEFAULT 1,
    updated_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const CREATE_PTS_UPDATES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS pts_updates (
    sequence_id INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id INTEGER NOT NULL,
    pts INTEGER NOT NULL,
    pts_count INTEGER NOT NULL DEFAULT 1,
    date_ms INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    account_generation INTEGER NOT NULL DEFAULT 1,
    UNIQUE (chat_id, pts)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_pts_updates_query ON pts_updates(chat_id, pts ASC);
CREATE INDEX IF NOT EXISTS idx_pts_updates_cleanup ON pts_updates(date_ms);
"#;

pub const CREATE_MESSAGE_DEDUP_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS message_dedup (
    sender_id INTEGER NOT NULL,
    client_nonce INTEGER NOT NULL,
    message_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    pts INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (sender_id, client_nonce)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_message_dedup_ttl ON message_dedup(created_at_ms);
"#;

pub const CREATE_READ_STATES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS read_states (
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    topic_id INTEGER NOT NULL DEFAULT 0,
    inbox_max_id INTEGER NOT NULL DEFAULT 0,
    outbox_max_id INTEGER NOT NULL DEFAULT 0,
    unread_count INTEGER NOT NULL DEFAULT 0,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, user_id, topic_id)
) STRICT;
"#;

pub const ALL_MIGRATIONS: &[&str] = &[
    CREATE_USERS_TABLE,
    CREATE_SESSIONS_TABLE,
    CREATE_CHATS_TABLE,
    CREATE_CHAT_MEMBERS_TABLE,
    CREATE_FORUM_TOPICS_TABLE,
    CREATE_MESSAGES_TABLE,
    CREATE_PTS_SEQUENCES_TABLE,
    CREATE_PTS_UPDATES_TABLE,
    CREATE_MESSAGE_DEDUP_TABLE,
    CREATE_READ_STATES_TABLE,
];

// --- Database Row Representation Models ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRecord {
    pub user_id: UserId,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub first_name: String,
    pub last_name: Option<String>,
    pub bio: Option<String>,
    pub password_hash: Option<String>,
    pub is_bot: bool,
    pub is_verified: bool,
    pub is_support: bool,
    pub is_scam: bool,
    pub is_fake: bool,
    pub profile_photo_file_id: Option<String>,
    pub account_generation: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub user_id: UserId,
    pub device_id: String,
    pub client_type: String,
    pub app_version: String,
    pub ip_address: Option<String>,
    pub location_desc: Option<String>,
    pub account_generation: u64,
    pub token_hash: String,
    pub created_at_ms: i64,
    pub last_active_ms: i64,
    pub expires_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRecord {
    pub chat_id: ChatId,
    pub chat_type: u8,
    pub peer_type: u8,
    pub title: String,
    pub username: Option<String>,
    pub about: Option<String>,
    pub creator_id: UserId,
    pub is_forum: bool,
    pub flags: u32,
    pub photo_file_id: Option<String>,
    pub slowmode_seconds: u32,
    pub linked_chat_id: Option<ChatId>,
    pub pinned_message_id: Option<MsgId>,
    pub pts: u64,
    pub last_message_id: MsgId,
    pub last_message_date_ms: i64,
    pub unread_count: u32,
    pub account_generation: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMemberRecord {
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub role: String,
    pub permissions_bitmask: u32,
    pub admin_title: Option<String>,
    pub admin_rights: u32,
    pub banned_rights: u32,
    pub until_date_ms: Option<i64>,
    pub joined_at_ms: i64,
    pub invited_by: Option<UserId>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForumTopicRecord {
    pub chat_id: ChatId,
    pub topic_id: TopicId,
    pub title: String,
    pub icon_color: u32,
    pub icon_emoji_id: Option<u64>,
    pub creator_id: UserId,
    pub is_pinned: bool,
    pub is_closed: bool,
    pub is_hidden: bool,
    pub last_message_id: Option<MsgId>,
    pub unread_count: u32,
    pub created_at_ms: i64,
}

impl ForumTopicRecord {
    pub const GENERAL_TOPIC_ID: i64 = 1;

    pub fn is_general(&self) -> bool {
        self.topic_id.0 == Self::GENERAL_TOPIC_ID
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRecord {
    pub chat_id: ChatId,
    pub message_id: MsgId,
    pub sender_id: UserId,
    pub to_peer_raw: u64,
    pub topic_id: Option<TopicId>,
    pub reply_to_id: Option<MsgId>,
    pub pts: u64,
    pub date_ms: i64,
    pub content_json: String,
    pub media_type: Option<String>,
    pub flags_bitmask: u32,
    pub client_nonce: Option<u64>,
    pub edit_date_ms: Option<i64>,
    pub deleted_at_ms: Option<i64>,
    pub reply_to_json: Option<String>,
    pub forward_info_json: Option<String>,
    pub reactions_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtsSequenceRecord {
    pub chat_id: ChatId,
    pub pts: u64,
    pub account_generation: u64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtsUpdateRecord {
    pub sequence_id: i64,
    pub chat_id: ChatId,
    pub pts: u64,
    pub pts_count: u32,
    pub date_ms: i64,
    pub event_type: String,
    pub payload_json: String,
    pub account_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDedupRecord {
    pub sender_id: UserId,
    pub client_nonce: u64,
    pub message_id: MsgId,
    pub chat_id: ChatId,
    pub pts: u64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadStateRecord {
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub topic_id: TopicId,
    pub inbox_max_id: MsgId,
    pub outbox_max_id: MsgId,
    pub unread_count: u32,
    pub updated_at_ms: i64,
}
