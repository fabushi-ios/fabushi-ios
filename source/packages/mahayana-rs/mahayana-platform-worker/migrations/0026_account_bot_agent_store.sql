-- Account-scoped Bot + Agent cloud state, modeled after the Grok-style
-- content-addressed Agent Store. Human contacts are intentionally absent:
-- Bot identity, Agent runtime identity, and contact identity are separate domains.

CREATE TABLE IF NOT EXISTS account_bot_profiles (
    account_user_id TEXT NOT NULL,
    bot_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    profile_json TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'manual',
    source_id TEXT NOT NULL DEFAULT 'manual',
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (account_user_id, bot_id)
);
CREATE INDEX IF NOT EXISTS account_bot_profiles_account_updated_idx
ON account_bot_profiles(account_user_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS account_agents (
    account_user_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    profile_json TEXT NOT NULL DEFAULT '{}',
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (account_user_id, agent_id)
);
CREATE INDEX IF NOT EXISTS account_agents_account_updated_idx
ON account_agents(account_user_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS account_agent_store_refs (
    account_user_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    blob_id TEXT NOT NULL,
    etag TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (account_user_id, agent_id, rel_path)
);
CREATE INDEX IF NOT EXISTS account_agent_store_refs_agent_path_idx
ON account_agent_store_refs(account_user_id, agent_id, rel_path);

CREATE TABLE IF NOT EXISTS account_state_events (
    revision INTEGER PRIMARY KEY AUTOINCREMENT,
    account_user_id TEXT NOT NULL,
    event_type TEXT NOT NULL,
    object_id TEXT NOT NULL,
    payload_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS account_state_events_account_revision_idx
ON account_state_events(account_user_id, revision ASC);

CREATE TABLE IF NOT EXISTS account_miniapp_cloud_storage (
    account_user_id TEXT NOT NULL,
    mini_app_id TEXT NOT NULL,
    storage_key TEXT NOT NULL,
    value_text TEXT NOT NULL,
    revision INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (account_user_id, mini_app_id, storage_key)
);
CREATE INDEX IF NOT EXISTS account_miniapp_cloud_storage_app_idx
ON account_miniapp_cloud_storage(account_user_id, mini_app_id, updated_at ASC);
