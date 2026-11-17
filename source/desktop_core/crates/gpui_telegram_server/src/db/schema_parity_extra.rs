pub const MIGRATION_V24_MEDIA_STICKERS_STORIES_CALLS: &str = r#"
CREATE TABLE IF NOT EXISTS media_files (
    media_id TEXT PRIMARY KEY,
    uploader_id INTEGER NOT NULL REFERENCES users(user_id),
    chat_id INTEGER,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    width INTEGER,
    height INTEGER,
    duration_seconds INTEGER,
    waveform_json TEXT,
    thumbnail_data_url TEXT,
    is_voice INTEGER NOT NULL DEFAULT 0,
    is_video_note INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_media_files_uploader ON media_files(uploader_id);
CREATE INDEX IF NOT EXISTS idx_media_files_chat ON media_files(chat_id);

CREATE TABLE IF NOT EXISTS sticker_sets (
    set_id TEXT PRIMARY KEY,
    short_name TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    is_animated INTEGER NOT NULL DEFAULT 0,
    is_video INTEGER NOT NULL DEFAULT 0,
    is_official INTEGER NOT NULL DEFAULT 1,
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS stickers (
    sticker_id TEXT PRIMARY KEY,
    set_id TEXT NOT NULL REFERENCES sticker_sets(set_id) ON DELETE CASCADE,
    emoji TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    width INTEGER NOT NULL DEFAULT 512,
    height INTEGER NOT NULL DEFAULT 512,
    file_id TEXT,
    lottie_json TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE IF NOT EXISTS user_sticker_sets (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    set_id TEXT NOT NULL REFERENCES sticker_sets(set_id) ON DELETE CASCADE,
    installed_at_ms INTEGER NOT NULL,
    PRIMARY KEY (user_id, set_id)
) STRICT;

CREATE TABLE IF NOT EXISTS gifs_catalog (
    gif_id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    tags TEXT NOT NULL,
    width INTEGER NOT NULL DEFAULT 320,
    height INTEGER NOT NULL DEFAULT 240,
    duration_secs INTEGER NOT NULL DEFAULT 3,
    mime_type TEXT NOT NULL DEFAULT 'image/gif',
    url TEXT NOT NULL,
    preview_url TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS stories (
    story_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    media_id TEXT,
    media_url TEXT,
    media_type TEXT NOT NULL DEFAULT 'photo',
    caption TEXT,
    privacy TEXT NOT NULL DEFAULT 'everyone',
    view_count INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_stories_expires ON stories(expires_at_ms);

CREATE TABLE IF NOT EXISTS story_views (
    story_id TEXT NOT NULL REFERENCES stories(story_id) ON DELETE CASCADE,
    viewer_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    reaction TEXT,
    viewed_at_ms INTEGER NOT NULL,
    PRIMARY KEY (story_id, viewer_user_id)
) STRICT;

CREATE TABLE IF NOT EXISTS calls (
    call_id TEXT PRIMARY KEY,
    chat_id INTEGER,
    caller_id INTEGER NOT NULL REFERENCES users(user_id),
    callee_id INTEGER NOT NULL REFERENCES users(user_id),
    is_video INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL,
    duration_secs INTEGER NOT NULL DEFAULT 0,
    rating INTEGER,
    feedback TEXT,
    started_at_ms INTEGER NOT NULL,
    ended_at_ms INTEGER
) STRICT;
CREATE INDEX IF NOT EXISTS idx_calls_participants ON calls(caller_id, callee_id, started_at_ms DESC);
"#;

pub const MIGRATION_V25_WALLET_ACP: &str = r#"
CREATE TABLE IF NOT EXISTS wallet_balances (
    user_id INTEGER PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    stars_balance INTEGER NOT NULL DEFAULT 0,
    ton_nano INTEGER NOT NULL DEFAULT 0,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS wallet_transactions (
    tx_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    peer_user_id INTEGER,
    kind TEXT NOT NULL,
    amount_stars INTEGER NOT NULL,
    description TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_wallet_tx_user ON wallet_transactions(user_id, created_at_ms DESC);

CREATE TABLE IF NOT EXISTS acp_tasks (
    task_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS acp_approval_gates (
    gate_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    decision TEXT,
    payload_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS mcp_servers_db (
    server_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    status TEXT NOT NULL,
    tools_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS computer_leases_db (
    lease_id TEXT PRIMARY KEY,
    owner_user_id INTEGER NOT NULL,
    session_id TEXT,
    status TEXT NOT NULL,
    actions_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS talk_sessions_db (
    session_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    status TEXT NOT NULL,
    transcript_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const MIGRATION_V26_PEER_BLOCKS_TTL_USER_SETTINGS: &str = r#"
ALTER TABLE chats ADD COLUMN message_ttl_seconds INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS blocked_users (
    owner_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    blocked_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    blocked_at_ms INTEGER NOT NULL,
    PRIMARY KEY (owner_user_id, blocked_user_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_blocked_users_owner ON blocked_users(owner_user_id, blocked_at_ms DESC);

CREATE TABLE IF NOT EXISTS user_privacy_settings (
    user_id INTEGER PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    settings_json TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
) STRICT;
"#;

pub const MIGRATION_V27_BOTFATHER_MINIAPPS: &str = r#"
CREATE TABLE IF NOT EXISTS bots_registry (
    bot_user_id INTEGER PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    owner_user_id INTEGER NOT NULL,
    username TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name TEXT NOT NULL,
    bot_token TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL DEFAULT '',
    about TEXT NOT NULL DEFAULT '',
    ai_persona TEXT NOT NULL DEFAULT '',
    menu_button_text TEXT NOT NULL DEFAULT '',
    menu_button_url TEXT NOT NULL DEFAULT '',
    is_official INTEGER NOT NULL DEFAULT 0,
    is_inline INTEGER NOT NULL DEFAULT 0,
    allow_attach_menu INTEGER NOT NULL DEFAULT 0,
    attach_peer_types_json TEXT NOT NULL DEFAULT '["users","bots","groups","channels"]',
    commands_json TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_bots_registry_owner ON bots_registry(owner_user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS botfather_sessions (
    user_id INTEGER PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    chat_id INTEGER NOT NULL,
    state TEXT NOT NULL,
    draft_json TEXT NOT NULL DEFAULT '{}',
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS mini_apps (
    app_id TEXT PRIMARY KEY,
    bot_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    bot_username TEXT NOT NULL,
    short_name TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT 'productivity',
    icon_emoji TEXT NOT NULL DEFAULT '🧩',
    web_url TEXT NOT NULL,
    ui_schema_json TEXT NOT NULL DEFAULT '{}',
    peer_types_json TEXT NOT NULL DEFAULT '["users","bots","groups","channels"]',
    is_published INTEGER NOT NULL DEFAULT 1,
    is_attach_enabled INTEGER NOT NULL DEFAULT 0,
    install_count INTEGER NOT NULL DEFAULT 0,
    created_by_nl INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(bot_username COLLATE NOCASE, short_name COLLATE NOCASE)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_mini_apps_bot ON mini_apps(bot_user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_mini_apps_published ON mini_apps(is_published, install_count DESC);

CREATE TABLE IF NOT EXISTS user_mini_apps (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    app_id TEXT NOT NULL REFERENCES mini_apps(app_id) ON DELETE CASCADE,
    bot_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    companion_chat_id INTEGER NOT NULL,
    allow_write_access INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL,
    last_launched_at INTEGER NOT NULL,
    PRIMARY KEY(user_id, app_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_user_mini_apps_user ON user_mini_apps(user_id, last_launched_at DESC);
CREATE INDEX IF NOT EXISTS idx_user_mini_apps_bot ON user_mini_apps(user_id, bot_user_id);

CREATE TABLE IF NOT EXISTS mini_app_sessions (
    query_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    app_id TEXT NOT NULL REFERENCES mini_apps(app_id) ON DELETE CASCADE,
    bot_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    chat_id INTEGER NOT NULL,
    launch_mode TEXT NOT NULL DEFAULT 'direct',
    start_param TEXT NOT NULL DEFAULT '',
    init_data_raw TEXT NOT NULL,
    auth_date INTEGER NOT NULL,
    state_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_mini_app_sessions_user_app ON mini_app_sessions(user_id, app_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS mini_app_cloud_storage (
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    bot_user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    storage_key TEXT NOT NULL,
    storage_value TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(user_id, bot_user_id, storage_key)
) STRICT;
"#;

pub const MIGRATION_V29_FABUSHI_BOT_PROFILES: &str = r#"
ALTER TABLE bots_registry ADD COLUMN agent_id TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN avatar_shape TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN avatar_color TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN preferred_model TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN thinking_level TEXT NOT NULL DEFAULT 'medium';
ALTER TABLE bots_registry ADD COLUMN approval_mode TEXT NOT NULL DEFAULT 'prompt';
ALTER TABLE bots_registry ADD COLUMN toolset_preset TEXT NOT NULL DEFAULT 'full';
"#;

pub const MIGRATION_V31_TELEGRAM_MINIAPPS_PARITY: &str = r#"
ALTER TABLE bots_registry ADD COLUMN menu_button_type TEXT NOT NULL DEFAULT 'default';
ALTER TABLE bots_registry ADD COLUMN has_main_app INTEGER NOT NULL DEFAULT 0;
ALTER TABLE bots_registry ADD COLUMN main_app_short_name TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN privacy_policy_url TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN has_settings INTEGER NOT NULL DEFAULT 0;
ALTER TABLE bots_registry ADD COLUMN can_join_groups INTEGER NOT NULL DEFAULT 1;
ALTER TABLE bots_registry ADD COLUMN webhook_url TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN webhook_secret_token TEXT NOT NULL DEFAULT '';
ALTER TABLE bots_registry ADD COLUMN is_standard_bot INTEGER NOT NULL DEFAULT 0;

ALTER TABLE mini_apps ADD COLUMN header_color TEXT NOT NULL DEFAULT '';
ALTER TABLE mini_apps ADD COLUMN background_color TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS user_bot_permissions (
    user_id INTEGER NOT NULL,
    bot_user_id INTEGER NOT NULL,
    allows_write_to_pm INTEGER NOT NULL DEFAULT 0,
    app_tos_accepted_at INTEGER NOT NULL DEFAULT 0,
    has_started_chat INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(user_id, bot_user_id)
) STRICT;

CREATE TABLE IF NOT EXISTS mini_app_usage_events (
    user_id INTEGER NOT NULL,
    app_id TEXT NOT NULL,
    bot_user_id INTEGER NOT NULL,
    last_launched_at INTEGER NOT NULL,
    launch_count INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY(user_id, app_id)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_mini_app_usage_user ON mini_app_usage_events(user_id, last_launched_at DESC);
CREATE INDEX IF NOT EXISTS idx_mini_app_usage_bot_time ON mini_app_usage_events(bot_user_id, last_launched_at DESC);

CREATE TABLE IF NOT EXISTS bot_api_updates (
    update_id INTEGER PRIMARY KEY AUTOINCREMENT,
    bot_user_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_bot_api_updates_bot ON bot_api_updates(bot_user_id, update_id ASC);

CREATE TABLE IF NOT EXISTS mini_app_hosted_bundles (
    app_id TEXT PRIMARY KEY,
    html_content TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
"#;
