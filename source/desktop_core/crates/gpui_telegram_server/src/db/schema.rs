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

pub const CREATE_CHAT_MESSAGE_SEQUENCES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS chat_message_sequences (
    chat_id INTEGER PRIMARY KEY,
    next_msg_id INTEGER NOT NULL DEFAULT 1,
    account_generation INTEGER NOT NULL DEFAULT 1,
    updated_at_ms INTEGER NOT NULL
) STRICT;
INSERT OR IGNORE INTO chat_message_sequences (chat_id, next_msg_id, account_generation, updated_at_ms)
SELECT chat_id, COALESCE(MAX(message_id), 0), 1, unixepoch('now') * 1000
FROM messages
GROUP BY chat_id;
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

pub const CREATE_ORDERS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS orders (
    order_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id),
    username TEXT NOT NULL,
    plan TEXT NOT NULL,
    amount TEXT NOT NULL,
    original_amount TEXT,
    is_admin_order INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'PENDING',
    platform TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_orders_user ON orders(user_id);
"#;

pub const CREATE_REDEEM_CODES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS redeem_codes (
    code TEXT PRIMARY KEY,
    days INTEGER NOT NULL,
    plan_type TEXT NOT NULL,
    used INTEGER NOT NULL DEFAULT 0,
    used_by_user_id INTEGER,
    used_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const CREATE_REDEEM_CLAIMS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS redeem_claims (
    code TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    username TEXT NOT NULL,
    claimed_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_redeem_claims_user ON redeem_claims(user_id);
"#;

pub const CREATE_USER_MEMBERSHIPS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS user_memberships (
    user_id INTEGER PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    is_active INTEGER NOT NULL DEFAULT 0,
    membership_type TEXT NOT NULL DEFAULT 'free',
    expires_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const CREATE_PAYMENT_RECEIPTS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS payment_receipts (
    order_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    provider_trade_no TEXT UNIQUE,
    amount TEXT NOT NULL,
    received_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const CREATE_PURCHASE_HISTORY_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS purchase_history (
    history_id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    order_id TEXT NOT NULL,
    plan TEXT NOT NULL,
    amount TEXT NOT NULL,
    valid_from_ms INTEGER NOT NULL,
    valid_to_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const MIGRATION_V11_USERS_AUTH_EXTENSIONS: &str = r#"
ALTER TABLE users ADD COLUMN email TEXT;
ALTER TABLE users ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;
ALTER TABLE users ADD COLUMN alipay_user_id TEXT;
ALTER TABLE users ADD COLUMN alipay_nickname TEXT;
ALTER TABLE users ADD COLUMN alipay_avatar TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email ON users(email) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_users_alipay_user_id ON users(alipay_user_id) WHERE alipay_user_id IS NOT NULL;
"#;

pub const CREATE_ACCOUNT_EMAIL_CHALLENGES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS account_email_challenges (
    challenge_id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    attempts_left INTEGER NOT NULL DEFAULT 5,
    cooldown_until_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_email_challenges_email ON account_email_challenges(email);
"#;

pub const CREATE_ACCOUNT_OAUTH_ATTEMPTS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS account_oauth_attempts (
    attempt_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    state TEXT NOT NULL,
    status TEXT NOT NULL,
    session_json TEXT,
    delivered_at_ms INTEGER,
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_oauth_attempts_state ON account_oauth_attempts(state);
"#;

pub const MIGRATION_V32_DEVICE_PUSH_TOKENS: &str = r#"
CREATE TABLE IF NOT EXISTS device_push_tokens (
    token TEXT NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    platform TEXT NOT NULL,
    device_id TEXT,
    endpoint TEXT,
    p256dh TEXT,
    auth_secret TEXT,
    app_version TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, token)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_push_tokens_user ON device_push_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_push_tokens_platform ON device_push_tokens(platform);
"#;

pub const MIGRATION_V33_E2EE_PREKEYS: &str = r#"
CREATE TABLE IF NOT EXISTS e2ee_devices (
    user_id INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    device_name TEXT,
    identity_key TEXT NOT NULL,
    identity_signing_key TEXT NOT NULL,
    signed_prekey_id INTEGER NOT NULL,
    signed_prekey_pub TEXT NOT NULL,
    signed_prekey_sig TEXT NOT NULL,
    signed_prekey_ts_ms INTEGER NOT NULL,
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, device_id)
) STRICT;
CREATE TABLE IF NOT EXISTS e2ee_one_time_prekeys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    key_id INTEGER NOT NULL,
    public_key TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    consumed_at_ms INTEGER,
    UNIQUE(user_id, device_id, key_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_e2ee_otpk_avail
    ON e2ee_one_time_prekeys(user_id, device_id, id) WHERE consumed_at_ms IS NULL;
"#;

pub const MIGRATION_V34_AGENTIC_HERMES_STATE: &str = r#"
CREATE TABLE IF NOT EXISTS hermes_sessions (
    id TEXT PRIMARY KEY,
    scope_key TEXT NOT NULL DEFAULT 'host',
    chat_id INTEGER,
    bot_id TEXT,
    source TEXT NOT NULL DEFAULT 'telegram',
    model TEXT NOT NULL DEFAULT '',
    api_mode TEXT NOT NULL DEFAULT 'local_hermes',
    title TEXT,
    parent_session_id TEXT,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    message_count INTEGER NOT NULL DEFAULT 0,
    metadata_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_hermes_sessions_scope ON hermes_sessions(scope_key, chat_id, started_at DESC);

CREATE TABLE IF NOT EXISTS hermes_messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    tool_calls_json TEXT,
    tool_call_id TEXT,
    tool_name TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_hermes_messages_sess ON hermes_messages(session_id, seq ASC);

CREATE TABLE IF NOT EXISTS hermes_cron_jobs (
    id TEXT NOT NULL,
    scope_key TEXT NOT NULL DEFAULT 'host',
    name TEXT NOT NULL,
    schedule TEXT NOT NULL,
    timezone TEXT,
    target_chat_id INTEGER,
    prompt TEXT NOT NULL DEFAULT '',
    model TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    deliver_to TEXT,
    origin_chat_id INTEGER,
    next_run_at INTEGER,
    last_run_at INTEGER,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (scope_key, id)
);
CREATE INDEX IF NOT EXISTS idx_hermes_cron_scope ON hermes_cron_jobs(scope_key, enabled, next_run_at);

CREATE TABLE IF NOT EXISTS hermes_cron_history (
    id TEXT PRIMARY KEY,
    scope_key TEXT NOT NULL DEFAULT 'host',
    job_id TEXT NOT NULL,
    run_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    output_preview TEXT,
    error TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_hermes_cron_history_scope_job ON hermes_cron_history(scope_key, job_id, run_at DESC);

CREATE TABLE IF NOT EXISTS hermes_approvals (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    command TEXT NOT NULL,
    risk_level TEXT NOT NULL,
    pattern_name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    decided_by TEXT,
    decided_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS hermes_checkpoints (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_index INTEGER NOT NULL,
    file_path TEXT NOT NULL,
    original_content TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, turn_index, file_path)
);

CREATE TABLE IF NOT EXISTS hermes_kanban_boards (
    id TEXT NOT NULL,
    scope_key TEXT NOT NULL DEFAULT 'host',
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    columns_json TEXT NOT NULL DEFAULT '["Backlog","In Progress","Review","Done"]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope_key, id)
);

CREATE TABLE IF NOT EXISTS hermes_kanban_cards (
    id TEXT NOT NULL,
    scope_key TEXT NOT NULL DEFAULT 'host',
    board_id TEXT NOT NULL,
    column_name TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    priority TEXT NOT NULL DEFAULT 'medium',
    assignee TEXT,
    labels_json TEXT NOT NULL DEFAULT '[]',
    comments_json TEXT NOT NULL DEFAULT '[]',
    checklist_json TEXT NOT NULL DEFAULT '[]',
    position INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope_key, id)
);
CREATE INDEX IF NOT EXISTS idx_hermes_kanban_cards_board ON hermes_kanban_cards(scope_key, board_id, column_name, position ASC);

CREATE TABLE IF NOT EXISTS hermes_kv_state (
    namespace TEXT NOT NULL DEFAULT 'default',
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (namespace, key)
);

CREATE TABLE IF NOT EXISTS agentic_connections (
    scope_key TEXT NOT NULL,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,
    endpoint TEXT,
    masked_secret TEXT,
    connected INTEGER NOT NULL DEFAULT 1,
    updated_at_ms INTEGER NOT NULL,
    last_tested_at_ms INTEGER,
    PRIMARY KEY (scope_key, name)
);

CREATE TABLE IF NOT EXISTS agentic_desktop_state (
    scope_key TEXT PRIMARY KEY,
    active_destination TEXT NOT NULL DEFAULT 'chats',
    open_panels_json TEXT NOT NULL DEFAULT '["chat_stage"]',
    last_action TEXT,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agentic_ui_action_log (
    id TEXT PRIMARY KEY,
    scope_key TEXT NOT NULL,
    action_type TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agentic_ui_log_scope ON agentic_ui_action_log(scope_key, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS agentic_subagent_lineage (
    task_id TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    parent_session_id TEXT,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    node_json TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (scope_key, task_id)
);
CREATE INDEX IF NOT EXISTS idx_agentic_lineage_scope ON agentic_subagent_lineage(scope_key, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS memory_embeddings (
    doc_id TEXT NOT NULL,
    chat_id INTEGER NOT NULL,
    source_kind TEXT NOT NULL DEFAULT 'message',
    content TEXT NOT NULL DEFAULT '',
    dims INTEGER NOT NULL,
    embedding_json TEXT NOT NULL,
    linked_entities_json TEXT NOT NULL DEFAULT '[]',
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, doc_id)
);
CREATE INDEX IF NOT EXISTS idx_memory_embeddings_chat ON memory_embeddings(chat_id, updated_at_ms DESC);

CREATE TABLE IF NOT EXISTS memory_entities (
    entity_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'entity'
);
CREATE INDEX IF NOT EXISTS idx_memory_entities_name ON memory_entities(name COLLATE NOCASE);

CREATE TABLE IF NOT EXISTS memory_entity_edges (
    src_entity TEXT NOT NULL,
    dst_entity TEXT NOT NULL,
    relation TEXT NOT NULL DEFAULT 'mentions',
    chat_id INTEGER NOT NULL,
    doc_id TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 1.0,
    PRIMARY KEY (src_entity, dst_entity, relation, chat_id, doc_id)
);
CREATE INDEX IF NOT EXISTS idx_memory_entity_edges_chat ON memory_entity_edges(chat_id, src_entity);
"#;

pub const MIGRATION_V35_SCOPE_ISOLATED_KEYS: &str = r#"
CREATE TABLE IF NOT EXISTS hermes_cron_jobs_v35 (
    id TEXT NOT NULL,
    scope_key TEXT NOT NULL DEFAULT 'host',
    name TEXT NOT NULL,
    schedule TEXT NOT NULL,
    timezone TEXT,
    target_chat_id INTEGER,
    prompt TEXT NOT NULL DEFAULT '',
    model TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    deliver_to TEXT,
    origin_chat_id INTEGER,
    next_run_at INTEGER,
    last_run_at INTEGER,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (scope_key, id)
);
INSERT OR IGNORE INTO hermes_cron_jobs_v35 (id, scope_key, name, schedule, timezone, target_chat_id, prompt, model, enabled, deliver_to, origin_chat_id, next_run_at, last_run_at, created_at)
SELECT id, scope_key, name, schedule, timezone, target_chat_id, prompt, model, enabled, deliver_to, origin_chat_id, next_run_at, last_run_at, created_at FROM hermes_cron_jobs;
DROP TABLE hermes_cron_jobs;
ALTER TABLE hermes_cron_jobs_v35 RENAME TO hermes_cron_jobs;
CREATE INDEX IF NOT EXISTS idx_hermes_cron_scope ON hermes_cron_jobs(scope_key, enabled, next_run_at);

CREATE TABLE IF NOT EXISTS hermes_cron_history_v35 (
    id TEXT PRIMARY KEY,
    scope_key TEXT NOT NULL DEFAULT 'host',
    job_id TEXT NOT NULL,
    run_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    output_preview TEXT,
    error TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO hermes_cron_history_v35 (id, scope_key, job_id, run_at, status, output_preview, error, duration_ms)
SELECT id, COALESCE((SELECT scope_key FROM hermes_cron_jobs j WHERE j.id = hermes_cron_history.job_id LIMIT 1), 'host'), job_id, run_at, status, output_preview, error, duration_ms FROM hermes_cron_history;
DROP TABLE hermes_cron_history;
ALTER TABLE hermes_cron_history_v35 RENAME TO hermes_cron_history;
CREATE INDEX IF NOT EXISTS idx_hermes_cron_history_scope_job ON hermes_cron_history(scope_key, job_id, run_at DESC);

CREATE TABLE IF NOT EXISTS hermes_kanban_boards_v35 (
    id TEXT NOT NULL,
    scope_key TEXT NOT NULL DEFAULT 'host',
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    columns_json TEXT NOT NULL DEFAULT '["Backlog","In Progress","Review","Done"]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope_key, id)
);
INSERT OR IGNORE INTO hermes_kanban_boards_v35 (id, scope_key, name, description, columns_json, created_at, updated_at)
SELECT id, scope_key, name, description, columns_json, created_at, updated_at FROM hermes_kanban_boards;
DROP TABLE hermes_kanban_boards;
ALTER TABLE hermes_kanban_boards_v35 RENAME TO hermes_kanban_boards;

CREATE TABLE IF NOT EXISTS hermes_kanban_cards_v35 (
    id TEXT NOT NULL,
    scope_key TEXT NOT NULL DEFAULT 'host',
    board_id TEXT NOT NULL,
    column_name TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    priority TEXT NOT NULL DEFAULT 'medium',
    assignee TEXT,
    labels_json TEXT NOT NULL DEFAULT '[]',
    comments_json TEXT NOT NULL DEFAULT '[]',
    checklist_json TEXT NOT NULL DEFAULT '[]',
    position INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope_key, id)
);
INSERT OR IGNORE INTO hermes_kanban_cards_v35 (id, scope_key, board_id, column_name, title, description, priority, assignee, labels_json, comments_json, checklist_json, position, created_at, updated_at)
SELECT id, scope_key, board_id, column_name, title, description, priority, assignee, labels_json, comments_json, checklist_json, position, created_at, updated_at FROM hermes_kanban_cards;
DROP TABLE hermes_kanban_cards;
ALTER TABLE hermes_kanban_cards_v35 RENAME TO hermes_kanban_cards;
CREATE INDEX IF NOT EXISTS idx_hermes_kanban_cards_board ON hermes_kanban_cards(scope_key, board_id, column_name, position ASC);

CREATE TABLE IF NOT EXISTS agentic_subagent_lineage_v35 (
    task_id TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    parent_session_id TEXT,
    role TEXT NOT NULL,
    status TEXT NOT NULL,
    node_json TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (scope_key, task_id)
);
INSERT OR IGNORE INTO agentic_subagent_lineage_v35 (task_id, scope_key, parent_session_id, role, status, node_json, created_at_ms, updated_at_ms)
SELECT task_id, scope_key, parent_session_id, role, status, node_json, created_at_ms, updated_at_ms FROM agentic_subagent_lineage;
DROP TABLE agentic_subagent_lineage;
ALTER TABLE agentic_subagent_lineage_v35 RENAME TO agentic_subagent_lineage;
CREATE INDEX IF NOT EXISTS idx_agentic_lineage_scope ON agentic_subagent_lineage(scope_key, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS memory_embeddings_v35 (
    doc_id TEXT NOT NULL,
    chat_id INTEGER NOT NULL,
    source_kind TEXT NOT NULL DEFAULT 'message',
    content TEXT NOT NULL DEFAULT '',
    dims INTEGER NOT NULL,
    embedding_json TEXT NOT NULL,
    linked_entities_json TEXT NOT NULL DEFAULT '[]',
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, doc_id)
);
INSERT OR IGNORE INTO memory_embeddings_v35 (doc_id, chat_id, source_kind, content, dims, embedding_json, linked_entities_json, updated_at_ms)
SELECT doc_id, chat_id, source_kind, content, dims, embedding_json, linked_entities_json, updated_at_ms FROM memory_embeddings;
DROP TABLE memory_embeddings;
ALTER TABLE memory_embeddings_v35 RENAME TO memory_embeddings;
CREATE INDEX IF NOT EXISTS idx_memory_embeddings_chat ON memory_embeddings(chat_id, updated_at_ms DESC);

CREATE TABLE IF NOT EXISTS memory_entity_edges_v35 (
    src_entity TEXT NOT NULL,
    dst_entity TEXT NOT NULL,
    relation TEXT NOT NULL DEFAULT 'mentions',
    chat_id INTEGER NOT NULL,
    doc_id TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 1.0,
    PRIMARY KEY (src_entity, dst_entity, relation, chat_id, doc_id)
);
INSERT OR IGNORE INTO memory_entity_edges_v35 (src_entity, dst_entity, relation, chat_id, doc_id, weight)
SELECT src_entity, dst_entity, relation, chat_id, doc_id, weight FROM memory_entity_edges;
DROP TABLE memory_entity_edges;
ALTER TABLE memory_entity_edges_v35 RENAME TO memory_entity_edges;
CREATE INDEX IF NOT EXISTS idx_memory_entity_edges_chat ON memory_entity_edges(chat_id, src_entity);
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
    MIGRATION_V11_USERS_AUTH_EXTENSIONS,
    CREATE_ACCOUNT_EMAIL_CHALLENGES_TABLE,
    CREATE_ACCOUNT_OAUTH_ATTEMPTS_TABLE,
    CREATE_ORDERS_TABLE,
    CREATE_REDEEM_CODES_TABLE,
    CREATE_REDEEM_CLAIMS_TABLE,
    CREATE_USER_MEMBERSHIPS_TABLE,
    CREATE_PAYMENT_RECEIPTS_TABLE,
    CREATE_PURCHASE_HISTORY_TABLE,
    CREATE_CHAT_MESSAGE_SEQUENCES_TABLE,
    crate::db::schema_parity::MIGRATION_V21_USER_AUTH_PROFILES,
    crate::db::schema_parity::MIGRATION_V22_CONTACTS_FOLDERS_CHATS,
    crate::db::schema_parity::MIGRATION_V23_REACTIONS_POLLS_MSG_EXTRAS,
    crate::db::schema_parity_extra::MIGRATION_V24_MEDIA_STICKERS_STORIES_CALLS,
    crate::db::schema_parity_extra::MIGRATION_V25_WALLET_ACP,
    crate::db::schema_parity_extra::MIGRATION_V26_PEER_BLOCKS_TTL_USER_SETTINGS,
    crate::db::schema_parity_extra::MIGRATION_V27_BOTFATHER_MINIAPPS,
    crate::db::computer_store::MIGRATION_V28_COMPUTER_CONTROL,
    crate::db::schema_parity_extra::MIGRATION_V29_FABUSHI_BOT_PROFILES,
    crate::db::computer_store::MIGRATION_V30_TAILNET_MESH,
    crate::db::schema_parity_extra::MIGRATION_V31_TELEGRAM_MINIAPPS_PARITY,
    MIGRATION_V32_DEVICE_PUSH_TOKENS,
    MIGRATION_V33_E2EE_PREKEYS,
    MIGRATION_V34_AGENTIC_HERMES_STATE,
    MIGRATION_V35_SCOPE_ISOLATED_KEYS,
];

// --- Database Row Representation Models ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailChallengeRecord {
    pub challenge_id: String,
    pub email: String,
    pub code_hash: String,
    pub attempts_left: i32,
    pub cooldown_until_ms: i64,
    pub expires_at_ms: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthAttemptRecord {
    pub attempt_id: String,
    pub provider: String,
    pub state: String,
    pub status: String,
    pub session_json: Option<String>,
    pub delivered_at_ms: Option<i64>,
    pub expires_at_ms: i64,
    pub created_at_ms: i64,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRecord {
    pub order_id: String,
    pub user_id: UserId,
    pub username: String,
    pub plan: String,
    pub amount: String,
    pub original_amount: Option<String>,
    pub is_admin_order: bool,
    pub status: String,
    pub platform: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedeemCodeRecord {
    pub code: String,
    pub days: u32,
    pub plan_type: String,
    pub used: bool,
    pub used_by_user_id: Option<UserId>,
    pub used_at_ms: Option<i64>,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMembershipRecord {
    pub user_id: UserId,
    pub is_active: bool,
    pub membership_type: String,
    pub expires_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMemberDetail {
    pub chat_id: ChatId,
    pub user_id: UserId,
    pub username: Option<String>,
    pub first_name: String,
    pub last_name: Option<String>,
    pub profile_photo_file_id: Option<String>,
    pub role: String,
    pub permissions_bitmask: u32,
    pub admin_title: Option<String>,
    pub admin_rights: u32,
    pub banned_rights: u32,
    pub joined_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePushTokenRecord {
    pub token: String,
    pub user_id: UserId,
    pub platform: String,
    pub device_id: Option<String>,
    pub endpoint: Option<String>,
    pub p256dh: Option<String>,
    pub auth_secret: Option<String>,
    pub app_version: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}
