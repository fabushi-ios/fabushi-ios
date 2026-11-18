pub const MIGRATION_V21_USER_AUTH_PROFILES: &str = r#"
ALTER TABLE users ADD COLUMN emoji_status TEXT;
CREATE TABLE IF NOT EXISTS account_phone_challenges (
    challenge_id TEXT PRIMARY KEY,
    phone TEXT NOT NULL,
    phone_code_hash TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    attempts_left INTEGER NOT NULL DEFAULT 5,
    cooldown_until_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_phone_challenges_phone ON account_phone_challenges(phone);

CREATE TABLE IF NOT EXISTS two_factor_auth (
    user_id INTEGER PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    password_hash TEXT NOT NULL,
    hint TEXT,
    recovery_email TEXT,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS two_factor_challenges (
    challenge_token TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    attempts_left INTEGER NOT NULL DEFAULT 5,
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS passkey_credentials (
    credential_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    public_key TEXT NOT NULL,
    sign_count INTEGER NOT NULL DEFAULT 0,
    device_name TEXT,
    created_at_ms INTEGER NOT NULL,
    last_used_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_passkeys_user ON passkey_credentials(user_id);

CREATE TABLE IF NOT EXISTS passkey_challenges (
    challenge_id TEXT PRIMARY KEY,
    user_id INTEGER,
    kind TEXT NOT NULL,
    challenge_b64 TEXT NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS qr_login_sessions (
    pair_token TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    user_id INTEGER REFERENCES users(user_id) ON DELETE CASCADE,
    session_json TEXT,
    expires_at_ms INTEGER NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const MIGRATION_V22_CONTACTS_FOLDERS_CHATS: &str = r#"
CREATE TABLE IF NOT EXISTS contacts (
    owner_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    contact_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    first_name TEXT NOT NULL,
    last_name TEXT,
    phone TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (owner_user_id, contact_user_id)
) STRICT;

CREATE TABLE IF NOT EXISTS chat_invites (
    invite_id TEXT PRIMARY KEY,
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    code TEXT NOT NULL UNIQUE,
    creator_id INTEGER NOT NULL REFERENCES users(user_id),
    title TEXT,
    usage_limit INTEGER,
    usage_count INTEGER NOT NULL DEFAULT 0,
    expire_date_ms INTEGER,
    request_needed INTEGER NOT NULL DEFAULT 0,
    is_revoked INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_chat_invites_chat ON chat_invites(chat_id);

CREATE TABLE IF NOT EXISTS chat_folders (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    folder_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    emoticon TEXT,
    pinned_chat_ids_json TEXT NOT NULL DEFAULT '[]',
    included_chat_ids_json TEXT NOT NULL DEFAULT '[]',
    excluded_chat_ids_json TEXT NOT NULL DEFAULT '[]',
    include_contacts INTEGER NOT NULL DEFAULT 0,
    include_non_contacts INTEGER NOT NULL DEFAULT 0,
    include_groups INTEGER NOT NULL DEFAULT 0,
    include_channels INTEGER NOT NULL DEFAULT 0,
    include_bots INTEGER NOT NULL DEFAULT 0,
    exclude_muted INTEGER NOT NULL DEFAULT 0,
    exclude_read INTEGER NOT NULL DEFAULT 0,
    exclude_archived INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, folder_id)
) STRICT;

CREATE TABLE IF NOT EXISTS user_chat_settings (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    is_archived INTEGER NOT NULL DEFAULT 0,
    is_pinned INTEGER NOT NULL DEFAULT 0,
    pin_order INTEGER NOT NULL DEFAULT 0,
    is_muted INTEGER NOT NULL DEFAULT 0,
    mute_until_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, chat_id)
) STRICT;

CREATE TABLE IF NOT EXISTS pinned_messages (
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL,
    pinned_by INTEGER NOT NULL REFERENCES users(user_id),
    pinned_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, message_id)
) STRICT;

CREATE TABLE IF NOT EXISTS drafts (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    topic_id INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL DEFAULT '',
    entities_json TEXT NOT NULL DEFAULT '[]',
    reply_to_msg_id INTEGER,
    no_webpage INTEGER NOT NULL DEFAULT 0,
    date_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, chat_id, topic_id)
) STRICT;
"#;

pub const MIGRATION_V23_REACTIONS_POLLS_MSG_EXTRAS: &str = r#"
ALTER TABLE messages ADD COLUMN inline_keyboard_json TEXT;
ALTER TABLE messages ADD COLUMN scheduled_at_ms INTEGER;
ALTER TABLE messages ADD COLUMN view_count INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS message_reactions (
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    emoji TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (chat_id, message_id, user_id, emoji)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_message_reactions_msg ON message_reactions(chat_id, message_id);

CREATE TABLE IF NOT EXISTS polls (
    poll_id INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id INTEGER NOT NULL REFERENCES chats(chat_id) ON DELETE CASCADE,
    message_id INTEGER NOT NULL DEFAULT 0,
    creator_id INTEGER NOT NULL REFERENCES users(user_id),
    question TEXT NOT NULL,
    is_anonymous INTEGER NOT NULL DEFAULT 1,
    multiple_choice INTEGER NOT NULL DEFAULT 0,
    is_quiz INTEGER NOT NULL DEFAULT 0,
    correct_option_id INTEGER,
    solution TEXT,
    is_closed INTEGER NOT NULL DEFAULT 0,
    close_period INTEGER,
    close_date INTEGER,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_polls_chat_msg ON polls(chat_id, message_id);

CREATE TABLE IF NOT EXISTS poll_options (
    poll_id INTEGER NOT NULL REFERENCES polls(poll_id) ON DELETE CASCADE,
    option_id INTEGER NOT NULL,
    text TEXT NOT NULL,
    vote_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (poll_id, option_id)
) STRICT;

CREATE TABLE IF NOT EXISTS poll_votes (
    poll_id INTEGER NOT NULL REFERENCES polls(poll_id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    option_id INTEGER NOT NULL,
    voted_at_ms INTEGER NOT NULL,
    PRIMARY KEY (poll_id, user_id, option_id)
) STRICT;
"#;
