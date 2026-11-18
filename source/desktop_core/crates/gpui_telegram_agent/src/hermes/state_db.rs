use gpui_telegram_core::error::{Result, TelegramError};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    chat_id INTEGER,
    bot_id TEXT,
    source TEXT NOT NULL DEFAULT 'telegram',
    model TEXT NOT NULL DEFAULT '',
    api_mode TEXT NOT NULL DEFAULT 'local_hermes',
    title TEXT,
    parent_session_id TEXT,
    branch_from_seq INTEGER,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    message_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    total_cost_usd REAL NOT NULL DEFAULT 0.0,
    system_prompt_hash TEXT,
    active_toolsets_json TEXT NOT NULL DEFAULT '[]',
    metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_sessions_chat_bot
    ON sessions (chat_id, bot_id, ended_at, started_at DESC);

CREATE INDEX IF NOT EXISTS idx_sessions_parent
    ON sessions (parent_session_id);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    reasoning TEXT,
    tool_calls_json TEXT,
    tool_call_id TEXT,
    tool_name TEXT,
    token_count INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_messages_session_seq
    ON messages (session_id, seq ASC);

CREATE TABLE IF NOT EXISTS cron_jobs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    schedule TEXT NOT NULL,
    timezone TEXT,
    target_chat_id INTEGER,
    prompt TEXT NOT NULL,
    model TEXT,
    skills_json TEXT NOT NULL DEFAULT '[]',
    enabled INTEGER NOT NULL DEFAULT 1,
    deliver_to TEXT,
    origin_chat_id INTEGER,
    next_run_at INTEGER,
    last_run_at INTEGER,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cron_jobs_due
    ON cron_jobs (enabled, next_run_at);

CREATE TABLE IF NOT EXISTS cron_history (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES cron_jobs(id) ON DELETE CASCADE,
    run_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    output_preview TEXT,
    error TEXT,
    duration_ms INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_cron_history_job_run
    ON cron_history (job_id, run_at DESC);

CREATE TABLE IF NOT EXISTS approvals (
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

CREATE INDEX IF NOT EXISTS idx_approvals_session_status
    ON approvals (session_id, status, created_at ASC);

CREATE TABLE IF NOT EXISTS checkpoints (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_index INTEGER NOT NULL,
    file_path TEXT NOT NULL,
    original_content TEXT,
    created_at INTEGER NOT NULL,
    UNIQUE(session_id, turn_index, file_path)
);

CREATE INDEX IF NOT EXISTS idx_checkpoints_session_turn
    ON checkpoints (session_id, turn_index DESC);

CREATE TABLE IF NOT EXISTS kanban_boards (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    columns_json TEXT NOT NULL DEFAULT '["Backlog","In Progress","Review","Done"]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS kanban_cards (
    id TEXT PRIMARY KEY,
    board_id TEXT NOT NULL REFERENCES kanban_boards(id) ON DELETE CASCADE,
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
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_kanban_cards_board_col
    ON kanban_cards (board_id, column_name, position ASC);

CREATE TABLE IF NOT EXISTS kv_state (
    namespace TEXT NOT NULL DEFAULT 'default',
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (namespace, key)
);
"#;

const FTS5_SCHEMA_SQL: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS hermes_messages_fts USING fts5(
    message_id UNINDEXED,
    session_id UNINDEXED,
    role UNINDEXED,
    content,
    tokenize='unicode61'
);

CREATE TRIGGER IF NOT EXISTS trg_hermes_messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO hermes_messages_fts(message_id, session_id, role, content)
    VALUES (new.id, new.session_id, new.role, new.content);
END;

CREATE TRIGGER IF NOT EXISTS trg_hermes_messages_ad AFTER DELETE ON messages BEGIN
    DELETE FROM hermes_messages_fts WHERE message_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS trg_hermes_messages_au AFTER UPDATE ON messages BEGIN
    DELETE FROM hermes_messages_fts WHERE message_id = old.id;
    INSERT INTO hermes_messages_fts(message_id, session_id, role, content)
    VALUES (new.id, new.session_id, new.role, new.content);
END;
"#;

fn db_err(err: rusqlite::Error) -> TelegramError {
    TelegramError::Database(err.to_string())
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub chat_id: Option<i64>,
    pub bot_id: Option<String>,
    pub source: String,
    pub model: String,
    pub api_mode: String,
    pub title: Option<String>,
    pub parent_session_id: Option<String>,
    pub branch_from_seq: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub message_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_cost_usd: f64,
    pub system_prompt_hash: Option<String>,
    pub active_toolsets_json: String,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub id: Option<String>,
    pub chat_id: Option<i64>,
    pub bot_id: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub api_mode: Option<String>,
    pub title: Option<String>,
    pub parent_session_id: Option<String>,
    pub branch_from_seq: Option<i64>,
    pub system_prompt_hash: Option<String>,
    pub active_toolsets: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionPatch {
    pub model: Option<String>,
    pub api_mode: Option<String>,
    pub title: Option<Option<String>>,
    pub ended_at: Option<Option<i64>>,
    pub system_prompt_hash: Option<Option<String>>,
    pub active_toolsets: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionListFilter {
    pub chat_id: Option<i64>,
    pub bot_id: Option<String>,
    pub source: Option<String>,
    pub active_only: bool,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: String,
    pub session_id: String,
    pub seq: i64,
    pub role: String,
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_calls_json: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub token_count: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppendMessageRequest {
    pub id: Option<String>,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_calls_json: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub token_count: i64,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoTurnResult {
    pub session_id: String,
    pub removed_count: usize,
    pub removed_messages: Vec<MessageRecord>,
    pub remaining_message_count: i64,
    pub rolled_back_to_seq: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchBackend {
    Fts5,
    LikeFallback,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSearchHit {
    pub session_id: String,
    pub session_title: Option<String>,
    pub message_id: String,
    pub seq: i64,
    pub role: String,
    pub snippet: String,
    pub content: String,
    pub created_at: i64,
    pub backend: SearchBackend,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsageBreakdown {
    pub model: String,
    pub session_count: i64,
    pub message_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_sessions: i64,
    pub total_messages: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub by_model: Vec<ModelUsageBreakdown>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolUsageCount {
    pub tool_name: String,
    pub call_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInsights {
    pub window_days: Option<u32>,
    pub session_count: i64,
    pub active_session_count: i64,
    pub message_count: i64,
    pub avg_messages_per_session: f64,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub top_models: Vec<ModelUsageBreakdown>,
    pub top_tools: Vec<ToolUsageCount>,
    pub recent_session_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronJobRecord {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub timezone: Option<String>,
    pub target_chat_id: Option<i64>,
    pub prompt: String,
    pub model: Option<String>,
    pub skills_json: String,
    pub enabled: bool,
    pub deliver_to: Option<String>,
    pub origin_chat_id: Option<i64>,
    pub next_run_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateCronJobRequest {
    pub id: Option<String>,
    pub name: String,
    pub schedule: String,
    pub timezone: Option<String>,
    pub target_chat_id: Option<i64>,
    pub prompt: String,
    pub model: Option<String>,
    pub skills: Option<Vec<String>>,
    pub enabled: Option<bool>,
    pub deliver_to: Option<String>,
    pub origin_chat_id: Option<i64>,
    pub next_run_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronJobPatch {
    pub name: Option<String>,
    pub schedule: Option<String>,
    pub timezone: Option<Option<String>>,
    pub target_chat_id: Option<Option<i64>>,
    pub prompt: Option<String>,
    pub model: Option<Option<String>>,
    pub skills: Option<Vec<String>>,
    pub enabled: Option<bool>,
    pub deliver_to: Option<Option<String>>,
    pub next_run_at: Option<Option<i64>>,
    pub last_run_at: Option<Option<i64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronHistoryRecord {
    pub id: String,
    pub job_id: String,
    pub run_at: i64,
    pub status: String,
    pub output_preview: Option<String>,
    pub error: Option<String>,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordCronRunRequest {
    pub job_id: String,
    pub run_at: Option<i64>,
    pub status: String,
    pub output_preview: Option<String>,
    pub error: Option<String>,
    pub duration_ms: i64,
    pub next_run_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    ApprovedOnce,
    ApprovedSession,
    ApprovedPermanent,
    Denied,
}

impl ApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::ApprovedOnce => "approved_once",
            Self::ApprovedSession => "approved_session",
            Self::ApprovedPermanent => "approved_permanent",
            Self::Denied => "denied",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "approved_once" => Self::ApprovedOnce,
            "approved_session" => Self::ApprovedSession,
            "approved_permanent" | "always_allow" => Self::ApprovedPermanent,
            "denied" | "deny" => Self::Denied,
            _ => Self::Pending,
        }
    }

    pub fn is_approved(&self) -> bool {
        matches!(
            self,
            Self::ApprovedOnce | Self::ApprovedSession | Self::ApprovedPermanent
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub risk_level: String,
    pub pattern_name: String,
    pub status: ApprovalStatus,
    pub decided_by: Option<String>,
    pub decided_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApprovalRequest {
    pub id: Option<String>,
    pub session_id: String,
    pub command: String,
    pub risk_level: String,
    pub pattern_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRecord {
    pub id: String,
    pub session_id: String,
    pub turn_index: i64,
    pub file_path: String,
    pub original_content: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanBoardRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub columns: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanChecklistItem {
    pub id: String,
    pub text: String,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanCardRecord {
    pub id: String,
    pub board_id: String,
    pub column_name: String,
    pub title: String,
    pub description: String,
    pub priority: String,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub comments: Vec<KanbanComment>,
    pub checklist: Vec<KanbanChecklistItem>,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateKanbanCardRequest {
    pub id: Option<String>,
    pub board_id: String,
    pub column_name: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KanbanCardPatch {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub assignee: Option<Option<String>>,
    pub labels: Option<Vec<String>>,
    pub comments: Option<Vec<KanbanComment>>,
    pub checklist: Option<Vec<KanbanChecklistItem>>,
    pub column_name: Option<String>,
    pub position: Option<i64>,
}

/// SQLite-backed state store for Hermes sessions, messages, FTS5/CJK search,
/// cron jobs, approvals, file rollback checkpoints, Kanban, and key-value state.
#[derive(Clone)]
pub struct HermesStateStore {
    path: Option<PathBuf>,
    conn: Arc<Mutex<Connection>>,
    fts5_available: bool,
}

impl std::fmt::Debug for HermesStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HermesStateStore")
            .field("path", &self.path)
            .field("fts5_available", &self.fts5_available)
            .finish()
    }
}

impl HermesStateStore {
    /// Opens a file-backed SQLite state store with WAL mode enabled.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        if path_ref == Path::new(":memory:") {
            return Self::open_in_memory();
        }
        if path_ref.as_os_str().is_empty() {
            return Err(TelegramError::Database(
                "Hermes state DB path cannot be empty".into(),
            ));
        }
        if let Some(parent) = path_ref.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    TelegramError::Database(format!(
                        "Failed to create Hermes state DB directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
        }
        let conn = Connection::open(path_ref).map_err(db_err)?;
        Self::init_connection(conn, Some(path_ref.to_path_buf()))
    }

    /// Opens an in-memory SQLite state store (ideal for unit/integration tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(db_err)?;
        Self::init_connection(conn, None)
    }

    fn init_connection(conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA temp_store = MEMORY;
            "#,
        )
        .map_err(db_err)?;

        conn.execute_batch(SCHEMA_SQL).map_err(db_err)?;

        let fts5_available = conn.execute_batch(FTS5_SCHEMA_SQL).is_ok();

        Ok(Self {
            path,
            conn: Arc::new(Mutex::new(conn)),
            fts5_available,
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn fts5_available(&self) -> bool {
        self.fts5_available
    }

    pub fn journal_mode(&self) -> Result<String> {
        let conn = self.conn.lock();
        conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .map_err(db_err)
    }

    // =========================================================================
    // Sessions CRUD & Continuity
    // =========================================================================

    pub fn create_session(&self, req: CreateSessionRequest) -> Result<SessionRecord> {
        let id = req
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| new_id("sess"));
        let source = req.source.unwrap_or_else(|| "telegram".into());
        let model = req.model.unwrap_or_else(|| "local-hermes".into());
        let api_mode = req.api_mode.unwrap_or_else(|| "local_hermes".into());
        let started_at = now_ms();
        let active_toolsets_json =
            serde_json::to_string(&req.active_toolsets.unwrap_or_default())?;
        let metadata_json = match req.metadata {
            Some(v) => serde_json::to_string(&v)?,
            None => "{}".into(),
        };

        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO sessions (
                id, chat_id, bot_id, source, model, api_mode, title,
                parent_session_id, branch_from_seq, started_at, ended_at,
                message_count, input_tokens, output_tokens, total_cost_usd,
                system_prompt_hash, active_toolsets_json, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, 0, 0, 0, 0.0, ?11, ?12, ?13)
            "#,
            params![
                id,
                req.chat_id,
                req.bot_id,
                source,
                model,
                api_mode,
                req.title,
                req.parent_session_id,
                req.branch_from_seq,
                started_at,
                req.system_prompt_hash,
                active_toolsets_json,
                metadata_json,
            ],
        )
        .map_err(db_err)?;

        Self::get_session_conn(&conn, &id)?
            .ok_or_else(|| TelegramError::Internal("Inserted session missing".into()))
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>> {
        let conn = self.conn.lock();
        Self::get_session_conn(&conn, session_id)
    }

    /// Deterministically resolves the latest active (non-ended) session for `(chat_id, bot_id)`
    /// or creates a new session if none is active. Eliminates random ephemeral `conv_<uuid>` fragmentation.
    pub fn get_or_create_chat_session(
        &self,
        chat_id: i64,
        bot_id: &str,
        mut defaults: CreateSessionRequest,
    ) -> Result<SessionRecord> {
        {
            let conn = self.conn.lock();
            let existing_id: Option<String> = conn
                .query_row(
                    r#"
                    SELECT id FROM sessions
                    WHERE chat_id = ?1 AND bot_id = ?2 AND ended_at IS NULL
                    ORDER BY started_at DESC
                    LIMIT 1
                    "#,
                    params![chat_id, bot_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db_err)?;
            if let Some(id) = existing_id {
                if let Some(sess) = Self::get_session_conn(&conn, &id)? {
                    return Ok(sess);
                }
            }
        }

        defaults.chat_id = Some(chat_id);
        defaults.bot_id = Some(bot_id.to_string());
        if defaults.id.is_none() {
            let safe_bot: String = bot_id
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                .collect();
            defaults.id = Some(format!("chat_{chat_id}_{safe_bot}_{}", uuid::Uuid::new_v4().simple()));
        }
        self.create_session(defaults)
    }

    pub fn list_sessions(&self, filter: SessionListFilter) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT id, chat_id, bot_id, source, model, api_mode, title, parent_session_id, branch_from_seq, started_at, ended_at, message_count, input_tokens, output_tokens, total_cost_usd, system_prompt_hash, active_toolsets_json, metadata_json FROM sessions WHERE 1=1",
        );
        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(chat_id) = filter.chat_id {
            sql.push_str(" AND chat_id = ?");
            params_vec.push(chat_id.into());
        }
        if let Some(bot_id) = filter.bot_id {
            sql.push_str(" AND bot_id = ?");
            params_vec.push(bot_id.into());
        }
        if let Some(source) = filter.source {
            sql.push_str(" AND source = ?");
            params_vec.push(source.into());
        }
        if filter.active_only {
            sql.push_str(" AND ended_at IS NULL");
        }
        sql.push_str(" ORDER BY started_at DESC");
        let limit = filter.limit.unwrap_or(100) as i64;
        let offset = filter.offset.unwrap_or(0) as i64;
        sql.push_str(" LIMIT ? OFFSET ?");
        params_vec.push(limit.into());
        params_vec.push(offset.into());

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(params_vec), Self::row_to_session)
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn update_session(&self, session_id: &str, patch: SessionPatch) -> Result<SessionRecord> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let mut current = Self::get_session_conn(&tx, session_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Session '{session_id}' not found")))?;

        if let Some(model) = patch.model {
            current.model = model;
        }
        if let Some(api_mode) = patch.api_mode {
            current.api_mode = api_mode;
        }
        if let Some(title) = patch.title {
            current.title = title;
        }
        if let Some(ended_at) = patch.ended_at {
            current.ended_at = ended_at;
        }
        if let Some(hash) = patch.system_prompt_hash {
            current.system_prompt_hash = hash;
        }
        if let Some(toolsets) = patch.active_toolsets {
            current.active_toolsets_json = serde_json::to_string(&toolsets)?;
        }
        if let Some(metadata) = patch.metadata {
            current.metadata_json = serde_json::to_string(&metadata)?;
        }

        tx.execute(
            r#"
            UPDATE sessions SET
                model = ?1,
                api_mode = ?2,
                title = ?3,
                ended_at = ?4,
                system_prompt_hash = ?5,
                active_toolsets_json = ?6,
                metadata_json = ?7
            WHERE id = ?8
            "#,
            params![
                current.model,
                current.api_mode,
                current.title,
                current.ended_at,
                current.system_prompt_hash,
                current.active_toolsets_json,
                current.metadata_json,
                session_id,
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(current)
    }

    pub fn end_session(&self, session_id: &str) -> Result<()> {
        self.update_session(
            session_id,
            SessionPatch {
                ended_at: Some(Some(now_ms())),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    pub fn delete_session(&self, session_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        // Delete messages explicitly so FTS5 delete triggers fire cleanly.
        tx.execute("DELETE FROM messages WHERE session_id = ?1", params![session_id])
            .map_err(db_err)?;
        tx.execute("DELETE FROM checkpoints WHERE session_id = ?1", params![session_id])
            .map_err(db_err)?;
        tx.execute("DELETE FROM approvals WHERE session_id = ?1", params![session_id])
            .map_err(db_err)?;
        let affected = tx
            .execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(affected > 0)
    }

    pub fn record_token_usage(
        &self,
        session_id: &str,
        input_tokens: i64,
        output_tokens: i64,
        cost_usd: f64,
    ) -> Result<()> {
        let conn = self.conn.lock();
        let affected = conn
            .execute(
                r#"
                UPDATE sessions
                SET input_tokens = input_tokens + ?1,
                    output_tokens = output_tokens + ?2,
                    total_cost_usd = total_cost_usd + ?3
                WHERE id = ?4
                "#,
                params![input_tokens.max(0), output_tokens.max(0), cost_usd.max(0.0), session_id],
            )
            .map_err(db_err)?;
        if affected == 0 {
            return Err(TelegramError::NotFound(format!(
                "Session '{session_id}' not found"
            )));
        }
        Ok(())
    }

    fn get_session_conn(conn: &Connection, session_id: &str) -> Result<Option<SessionRecord>> {
        conn.query_row(
            r#"
            SELECT id, chat_id, bot_id, source, model, api_mode, title,
                   parent_session_id, branch_from_seq, started_at, ended_at,
                   message_count, input_tokens, output_tokens, total_cost_usd,
                   system_prompt_hash, active_toolsets_json, metadata_json
            FROM sessions WHERE id = ?1
            "#,
            params![session_id],
            Self::row_to_session,
        )
        .optional()
        .map_err(db_err)
    }

    fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
        Ok(SessionRecord {
            id: row.get(0)?,
            chat_id: row.get(1)?,
            bot_id: row.get(2)?,
            source: row.get(3)?,
            model: row.get(4)?,
            api_mode: row.get(5)?,
            title: row.get(6)?,
            parent_session_id: row.get(7)?,
            branch_from_seq: row.get(8)?,
            started_at: row.get(9)?,
            ended_at: row.get(10)?,
            message_count: row.get(11)?,
            input_tokens: row.get(12)?,
            output_tokens: row.get(13)?,
            total_cost_usd: row.get(14)?,
            system_prompt_hash: row.get(15)?,
            active_toolsets_json: row.get(16)?,
            metadata_json: row.get(17)?,
        })
    }

    // =========================================================================
    // Messages CRUD, Branching & /undo
    // =========================================================================

    pub fn append_message(&self, req: AppendMessageRequest) -> Result<MessageRecord> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;

        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                params![req.session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if !exists {
            return Err(TelegramError::NotFound(format!(
                "Session '{}' not found",
                req.session_id
            )));
        }

        let next_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?1",
                params![req.session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let id = req
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| new_id("msg"));
        let created_at = req.created_at.unwrap_or_else(now_ms);

        tx.execute(
            r#"
            INSERT INTO messages (
                id, session_id, seq, role, content, reasoning,
                tool_calls_json, tool_call_id, tool_name, token_count, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#,
            params![
                id,
                req.session_id,
                next_seq,
                req.role,
                req.content,
                req.reasoning,
                req.tool_calls_json,
                req.tool_call_id,
                req.tool_name,
                req.token_count,
                created_at,
            ],
        )
        .map_err(db_err)?;

        tx.execute(
            "UPDATE sessions SET message_count = (SELECT COUNT(*) FROM messages WHERE session_id = ?1) WHERE id = ?1",
            params![req.session_id],
        )
        .map_err(db_err)?;

        let record = tx
            .query_row(
                r#"
                SELECT id, session_id, seq, role, content, reasoning,
                       tool_calls_json, tool_call_id, tool_name, token_count, created_at
                FROM messages WHERE id = ?1
                "#,
                params![id],
                Self::row_to_message,
            )
            .map_err(db_err)?;

        tx.commit().map_err(db_err)?;
        Ok(record)
    }

    pub fn get_messages(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<MessageRecord>> {
        let conn = self.conn.lock();
        if let Some(lim) = limit {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, session_id, seq, role, content, reasoning,
                           tool_calls_json, tool_call_id, tool_name, token_count, created_at
                    FROM (
                        SELECT id, session_id, seq, role, content, reasoning,
                               tool_calls_json, tool_call_id, tool_name, token_count, created_at
                        FROM messages
                        WHERE session_id = ?1
                        ORDER BY seq DESC
                        LIMIT ?2
                    ) sub
                    ORDER BY seq ASC
                    "#,
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![session_id, lim as i64], Self::row_to_message)
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        } else {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, session_id, seq, role, content, reasoning,
                           tool_calls_json, tool_call_id, tool_name, token_count, created_at
                    FROM messages
                    WHERE session_id = ?1
                    ORDER BY seq ASC
                    "#,
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![session_id], Self::row_to_message)
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        }
    }

    pub fn get_messages_range(
        &self,
        session_id: &str,
        from_seq: i64,
        to_seq: i64,
    ) -> Result<Vec<MessageRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, session_id, seq, role, content, reasoning,
                       tool_calls_json, tool_call_id, tool_name, token_count, created_at
                FROM messages
                WHERE session_id = ?1 AND seq >= ?2 AND seq <= ?3
                ORDER BY seq ASC
                "#,
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![session_id, from_seq, to_seq], Self::row_to_message)
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Atomically replaces all messages in a session (e.g., after `/compress` compaction).
    pub fn replace_session_messages(
        &self,
        session_id: &str,
        messages: Vec<AppendMessageRequest>,
    ) -> Result<Vec<MessageRecord>> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;

        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if !exists {
            return Err(TelegramError::NotFound(format!(
                "Session '{session_id}' not found"
            )));
        }

        tx.execute("DELETE FROM messages WHERE session_id = ?1", params![session_id])
            .map_err(db_err)?;

        let mut inserted = Vec::with_capacity(messages.len());
        for (idx, req) in messages.into_iter().enumerate() {
            let seq = (idx + 1) as i64;
            let id = req
                .id
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| new_id("msg"));
            let created_at = req.created_at.unwrap_or_else(now_ms);
            tx.execute(
                r#"
                INSERT INTO messages (
                    id, session_id, seq, role, content, reasoning,
                    tool_calls_json, tool_call_id, tool_name, token_count, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                params![
                    id,
                    session_id,
                    seq,
                    req.role,
                    req.content,
                    req.reasoning,
                    req.tool_calls_json,
                    req.tool_call_id,
                    req.tool_name,
                    req.token_count,
                    created_at,
                ],
            )
            .map_err(db_err)?;
            inserted.push(MessageRecord {
                id,
                session_id: session_id.to_string(),
                seq,
                role: req.role,
                content: req.content,
                reasoning: req.reasoning,
                tool_calls_json: req.tool_calls_json,
                tool_call_id: req.tool_call_id,
                tool_name: req.tool_name,
                token_count: req.token_count,
                created_at,
            });
        }

        tx.execute(
            "UPDATE sessions SET message_count = ?1 WHERE id = ?2",
            params![inserted.len() as i64, session_id],
        )
        .map_err(db_err)?;

        tx.commit().map_err(db_err)?;
        Ok(inserted)
    }

    /// Forks a new session from `parent_session_id`, cloning messages up to `up_to_seq` (or all).
    pub fn branch_session(
        &self,
        parent_session_id: &str,
        up_to_seq: Option<i64>,
        new_title: Option<String>,
    ) -> Result<SessionRecord> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;

        let parent = Self::get_session_conn(&tx, parent_session_id)?.ok_or_else(|| {
            TelegramError::NotFound(format!("Parent session '{parent_session_id}' not found"))
        })?;

        let cutoff = match up_to_seq {
            Some(seq) => seq,
            None => tx
                .query_row(
                    "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE session_id = ?1",
                    params![parent_session_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?,
        };

        let branch_id = new_id("branch");
        let started_at = now_ms();
        let branch_title = new_title.or_else(|| {
            parent
                .title
                .as_ref()
                .map(|t| format!("{t} (branch @{cutoff})"))
                .or_else(|| Some(format!("Branch from {} @{cutoff}", parent.id)))
        });

        tx.execute(
            r#"
            INSERT INTO sessions (
                id, chat_id, bot_id, source, model, api_mode, title,
                parent_session_id, branch_from_seq, started_at, ended_at,
                message_count, input_tokens, output_tokens, total_cost_usd,
                system_prompt_hash, active_toolsets_json, metadata_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, 0, 0, 0, 0.0, ?11, ?12, ?13)
            "#,
            params![
                branch_id,
                parent.chat_id,
                parent.bot_id,
                parent.source,
                parent.model,
                parent.api_mode,
                branch_title,
                parent.id,
                cutoff,
                started_at,
                parent.system_prompt_hash,
                parent.active_toolsets_json,
                parent.metadata_json,
            ],
        )
        .map_err(db_err)?;

        let mut stmt = tx
            .prepare(
                r#"
                SELECT id, session_id, seq, role, content, reasoning,
                       tool_calls_json, tool_call_id, tool_name, token_count, created_at
                FROM messages
                WHERE session_id = ?1 AND seq <= ?2
                ORDER BY seq ASC
                "#,
            )
            .map_err(db_err)?;
        let parent_msgs = stmt
            .query_map(params![parent_session_id, cutoff], Self::row_to_message)
            .map_err(db_err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_err)?;
        drop(stmt);

        let copied_count = parent_msgs.len() as i64;
        for msg in parent_msgs {
            let cloned_id = new_id("msg");
            tx.execute(
                r#"
                INSERT INTO messages (
                    id, session_id, seq, role, content, reasoning,
                    tool_calls_json, tool_call_id, tool_name, token_count, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                "#,
                params![
                    cloned_id,
                    branch_id,
                    msg.seq,
                    msg.role,
                    msg.content,
                    msg.reasoning,
                    msg.tool_calls_json,
                    msg.tool_call_id,
                    msg.tool_name,
                    msg.token_count,
                    msg.created_at,
                ],
            )
            .map_err(db_err)?;
        }

        tx.execute(
            "UPDATE sessions SET message_count = ?1 WHERE id = ?2",
            params![copied_count, branch_id],
        )
        .map_err(db_err)?;

        let branch = Self::get_session_conn(&tx, &branch_id)?
            .ok_or_else(|| TelegramError::Internal("Branched session missing".into()))?;
        tx.commit().map_err(db_err)?;
        Ok(branch)
    }

    /// Lists direct child branches and sibling branches related to `session_id`.
    pub fn list_branches(&self, session_id: &str) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock();
        let parent_id = Self::get_session_conn(&conn, session_id)?
            .and_then(|s| s.parent_session_id);
        let root_id = parent_id.unwrap_or_else(|| session_id.to_string());
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, chat_id, bot_id, source, model, api_mode, title,
                       parent_session_id, branch_from_seq, started_at, ended_at,
                       message_count, input_tokens, output_tokens, total_cost_usd,
                       system_prompt_hash, active_toolsets_json, metadata_json
                FROM sessions
                WHERE id = ?1 OR parent_session_id = ?1 OR parent_session_id = ?2
                ORDER BY started_at ASC
                "#,
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![root_id, session_id], Self::row_to_session)
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Implements `/undo`: rolls back the last user message and every trailing assistant/tool message.
    pub fn undo_last_turn(&self, session_id: &str) -> Result<UndoTurnResult> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;

        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if !exists {
            return Err(TelegramError::NotFound(format!(
                "Session '{session_id}' not found"
            )));
        }

        let last_user_seq: Option<i64> = tx
            .query_row(
                "SELECT MAX(seq) FROM messages WHERE session_id = ?1 AND role = 'user'",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let Some(cutoff_seq) = last_user_seq else {
            let remaining: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            return Ok(UndoTurnResult {
                session_id: session_id.to_string(),
                removed_count: 0,
                removed_messages: Vec::new(),
                remaining_message_count: remaining,
                rolled_back_to_seq: 0,
            });
        };

        let mut stmt = tx
            .prepare(
                r#"
                SELECT id, session_id, seq, role, content, reasoning,
                       tool_calls_json, tool_call_id, tool_name, token_count, created_at
                FROM messages
                WHERE session_id = ?1 AND seq >= ?2
                ORDER BY seq ASC
                "#,
            )
            .map_err(db_err)?;
        let removed_messages = stmt
            .query_map(params![session_id, cutoff_seq], Self::row_to_message)
            .map_err(db_err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_err)?;
        drop(stmt);

        tx.execute(
            "DELETE FROM messages WHERE session_id = ?1 AND seq >= ?2",
            params![session_id, cutoff_seq],
        )
        .map_err(db_err)?;

        let remaining_message_count: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let rolled_back_to_seq: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM messages WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        tx.execute(
            "UPDATE sessions SET message_count = ?1 WHERE id = ?2",
            params![remaining_message_count, session_id],
        )
        .map_err(db_err)?;

        tx.commit().map_err(db_err)?;
        Ok(UndoTurnResult {
            session_id: session_id.to_string(),
            removed_count: removed_messages.len(),
            removed_messages,
            remaining_message_count,
            rolled_back_to_seq,
        })
    }

    fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRecord> {
        Ok(MessageRecord {
            id: row.get(0)?,
            session_id: row.get(1)?,
            seq: row.get(2)?,
            role: row.get(3)?,
            content: row.get(4)?,
            reasoning: row.get(5)?,
            tool_calls_json: row.get(6)?,
            tool_call_id: row.get(7)?,
            tool_name: row.get(8)?,
            token_count: row.get(9)?,
            created_at: row.get(10)?,
        })
    }

    // =========================================================================
    // Full-Text Search (FTS5 + CJK / Substring LIKE Fallback)
    // =========================================================================

    pub fn session_search(
        &self,
        query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionSearchHit>> {
        let trimmed = query.trim();
        if trimmed.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock();
        let has_cjk = contains_cjk(trimmed);

        if self.fts5_available && !has_cjk {
            if let Some(fts_query) = sanitize_fts5_query(trimmed) {
                let fts_hits = self.run_fts5_search(&conn, &fts_query, trimmed, session_id, limit);
                if let Ok(hits) = fts_hits {
                    if !hits.is_empty() {
                        return Ok(hits);
                    }
                }
            }
        }

        self.run_like_fallback_search(&conn, trimmed, session_id, limit)
    }

    fn run_fts5_search(
        &self,
        conn: &Connection,
        fts_query: &str,
        raw_query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionSearchHit>> {
        let mut sql = String::from(
            r#"
            SELECT m.session_id, s.title, m.id, m.seq, m.role, m.content, m.created_at
            FROM hermes_messages_fts f
            JOIN messages m ON m.id = f.message_id
            JOIN sessions s ON s.id = m.session_id
            WHERE hermes_messages_fts MATCH ?
            "#,
        );
        let mut params_vec: Vec<rusqlite::types::Value> = vec![fts_query.to_string().into()];
        if let Some(sid) = session_id {
            sql.push_str(" AND m.session_id = ?");
            params_vec.push(sid.to_string().into());
        }
        sql.push_str(" ORDER BY bm25(hermes_messages_fts) ASC, m.created_at DESC LIMIT ?");
        params_vec.push((limit as i64).into());

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(params_vec), |row| {
                let content: String = row.get(5)?;
                Ok(SessionSearchHit {
                    session_id: row.get(0)?,
                    session_title: row.get(1)?,
                    message_id: row.get(2)?,
                    seq: row.get(3)?,
                    role: row.get(4)?,
                    snippet: build_highlighted_snippet(&content, raw_query, 160),
                    content,
                    created_at: row.get(6)?,
                    backend: SearchBackend::Fts5,
                })
            })
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    fn run_like_fallback_search(
        &self,
        conn: &Connection,
        query: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SessionSearchHit>> {
        let escaped = escape_like(query);
        let pattern = format!("%{escaped}%");
        let mut sql = String::from(
            r#"
            SELECT m.session_id, s.title, m.id, m.seq, m.role, m.content, m.created_at
            FROM messages m
            JOIN sessions s ON s.id = m.session_id
            WHERE LOWER(m.content) LIKE LOWER(?) ESCAPE '\'
            "#,
        );
        let mut params_vec: Vec<rusqlite::types::Value> = vec![pattern.into()];
        if let Some(sid) = session_id {
            sql.push_str(" AND m.session_id = ?");
            params_vec.push(sid.to_string().into());
        }
        sql.push_str(" ORDER BY m.created_at DESC, m.seq DESC LIMIT ?");
        params_vec.push((limit as i64).into());

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(params_vec), |row| {
                let content: String = row.get(5)?;
                Ok(SessionSearchHit {
                    session_id: row.get(0)?,
                    session_title: row.get(1)?,
                    message_id: row.get(2)?,
                    seq: row.get(3)?,
                    role: row.get(4)?,
                    snippet: build_highlighted_snippet(&content, query, 160),
                    content,
                    created_at: row.get(6)?,
                    backend: SearchBackend::LikeFallback,
                })
            })
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    // =========================================================================
    // Usage Summary & Insights Aggregation
    // =========================================================================

    pub fn usage_summary(
        &self,
        session_id: Option<&str>,
        bot_id: Option<&str>,
    ) -> Result<UsageSummary> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            r#"
            SELECT model,
                   COUNT(*),
                   COALESCE(SUM(message_count), 0),
                   COALESCE(SUM(input_tokens), 0),
                   COALESCE(SUM(output_tokens), 0),
                   COALESCE(SUM(total_cost_usd), 0.0)
            FROM sessions
            WHERE 1=1
            "#,
        );
        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(sid) = session_id {
            sql.push_str(" AND id = ?");
            params_vec.push(sid.to_string().into());
        }
        if let Some(bid) = bot_id {
            sql.push_str(" AND bot_id = ?");
            params_vec.push(bid.to_string().into());
        }
        sql.push_str(" GROUP BY model ORDER BY SUM(input_tokens + output_tokens) DESC");

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let by_model = stmt
            .query_map(params_from_iter(params_vec), |row| {
                Ok(ModelUsageBreakdown {
                    model: row.get(0)?,
                    session_count: row.get(1)?,
                    message_count: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    total_cost_usd: row.get(5)?,
                })
            })
            .map_err(db_err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_err)?;

        let total_sessions = by_model.iter().map(|m| m.session_count).sum();
        let total_messages = by_model.iter().map(|m| m.message_count).sum();
        let total_input_tokens: i64 = by_model.iter().map(|m| m.input_tokens).sum();
        let total_output_tokens: i64 = by_model.iter().map(|m| m.output_tokens).sum();
        let total_cost_usd = by_model.iter().map(|m| m.total_cost_usd).sum();

        Ok(UsageSummary {
            total_sessions,
            total_messages,
            total_input_tokens,
            total_output_tokens,
            total_tokens: total_input_tokens + total_output_tokens,
            total_cost_usd,
            by_model,
        })
    }

    pub fn insights(&self, days: Option<u32>, bot_id: Option<&str>) -> Result<SessionInsights> {
        let cutoff_ms = days.map(|d| now_ms() - (d as i64) * 86_400_000);
        let conn = self.conn.lock();

        let mut where_clause = String::from(" WHERE 1=1");
        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(cutoff) = cutoff_ms {
            where_clause.push_str(" AND started_at >= ?");
            params_vec.push(cutoff.into());
        }
        if let Some(bid) = bot_id {
            where_clause.push_str(" AND bot_id = ?");
            params_vec.push(bid.to_string().into());
        }

        let summary_sql = format!(
            r#"
            SELECT COUNT(*),
                   SUM(CASE WHEN ended_at IS NULL THEN 1 ELSE 0 END),
                   COALESCE(SUM(message_count), 0),
                   COALESCE(SUM(input_tokens + output_tokens), 0),
                   COALESCE(SUM(total_cost_usd), 0.0)
            FROM sessions
            {where_clause}
            "#
        );
        let (session_count, active_session_count, message_count, total_tokens, total_cost_usd): (
            i64,
            Option<i64>,
            i64,
            i64,
            f64,
        ) = conn
            .query_row(&summary_sql, params_from_iter(params_vec.clone()), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .map_err(db_err)?;

        let model_sql = format!(
            r#"
            SELECT model,
                   COUNT(*),
                   COALESCE(SUM(message_count), 0),
                   COALESCE(SUM(input_tokens), 0),
                   COALESCE(SUM(output_tokens), 0),
                   COALESCE(SUM(total_cost_usd), 0.0)
            FROM sessions
            {where_clause}
            GROUP BY model
            ORDER BY SUM(input_tokens + output_tokens) DESC
            LIMIT 10
            "#
        );
        let mut model_stmt = conn.prepare(&model_sql).map_err(db_err)?;
        let top_models = model_stmt
            .query_map(params_from_iter(params_vec.clone()), |row| {
                Ok(ModelUsageBreakdown {
                    model: row.get(0)?,
                    session_count: row.get(1)?,
                    message_count: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    total_cost_usd: row.get(5)?,
                })
            })
            .map_err(db_err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_err)?;

        let mut tool_sql = String::from(
            r#"
            SELECT m.tool_name, COUNT(*)
            FROM messages m
            JOIN sessions s ON s.id = m.session_id
            WHERE m.tool_name IS NOT NULL AND m.tool_name != ''
            "#,
        );
        let mut tool_params: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(cutoff) = cutoff_ms {
            tool_sql.push_str(" AND s.started_at >= ?");
            tool_params.push(cutoff.into());
        }
        if let Some(bid) = bot_id {
            tool_sql.push_str(" AND s.bot_id = ?");
            tool_params.push(bid.to_string().into());
        }
        tool_sql.push_str(" GROUP BY m.tool_name ORDER BY COUNT(*) DESC LIMIT 15");
        let mut tool_stmt = conn.prepare(&tool_sql).map_err(db_err)?;
        let top_tools = tool_stmt
            .query_map(params_from_iter(tool_params), |row| {
                Ok(ToolUsageCount {
                    tool_name: row.get(0)?,
                    call_count: row.get(1)?,
                })
            })
            .map_err(db_err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_err)?;

        let recent_sql = format!(
            "SELECT id FROM sessions {where_clause} ORDER BY started_at DESC LIMIT 10"
        );
        let mut recent_stmt = conn.prepare(&recent_sql).map_err(db_err)?;
        let recent_session_ids = recent_stmt
            .query_map(params_from_iter(params_vec), |row| row.get(0))
            .map_err(db_err)?
            .collect::<std::result::Result<Vec<String>, _>>()
            .map_err(db_err)?;

        let avg_messages_per_session = if session_count > 0 {
            message_count as f64 / session_count as f64
        } else {
            0.0
        };

        Ok(SessionInsights {
            window_days: days,
            session_count,
            active_session_count: active_session_count.unwrap_or(0),
            message_count,
            avg_messages_per_session,
            total_tokens,
            total_cost_usd,
            top_models,
            top_tools,
            recent_session_ids,
        })
    }

    // =========================================================================
    // Cron Jobs & History CRUD
    // =========================================================================

    pub fn create_cron_job(&self, req: CreateCronJobRequest) -> Result<CronJobRecord> {
        if req.name.trim().is_empty() {
            return Err(TelegramError::Validation("Cron job name cannot be empty".into()));
        }
        if req.schedule.trim().is_empty() {
            return Err(TelegramError::Validation("Cron job schedule cannot be empty".into()));
        }
        let id = req
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| new_id("cron"));
        let skills_json = serde_json::to_string(&req.skills.unwrap_or_default())?;
        let enabled = req.enabled.unwrap_or(true);
        let created_at = now_ms();

        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO cron_jobs (
                id, name, schedule, timezone, target_chat_id, prompt, model,
                skills_json, enabled, deliver_to, origin_chat_id,
                next_run_at, last_run_at, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13)
            "#,
            params![
                id,
                req.name,
                req.schedule,
                req.timezone,
                req.target_chat_id,
                req.prompt,
                req.model,
                skills_json,
                enabled as i64,
                req.deliver_to,
                req.origin_chat_id,
                req.next_run_at,
                created_at,
            ],
        )
        .map_err(db_err)?;

        Self::get_cron_job_conn(&conn, &id)?
            .ok_or_else(|| TelegramError::Internal("Inserted cron job missing".into()))
    }

    pub fn get_cron_job(&self, job_id: &str) -> Result<Option<CronJobRecord>> {
        let conn = self.conn.lock();
        Self::get_cron_job_conn(&conn, job_id)
    }

    pub fn list_cron_jobs(&self, enabled_only: bool) -> Result<Vec<CronJobRecord>> {
        let conn = self.conn.lock();
        let sql = if enabled_only {
            "SELECT id, name, schedule, timezone, target_chat_id, prompt, model, skills_json, enabled, deliver_to, origin_chat_id, next_run_at, last_run_at, created_at FROM cron_jobs WHERE enabled = 1 ORDER BY created_at DESC"
        } else {
            "SELECT id, name, schedule, timezone, target_chat_id, prompt, model, skills_json, enabled, deliver_to, origin_chat_id, next_run_at, last_run_at, created_at FROM cron_jobs ORDER BY created_at DESC"
        };
        let mut stmt = conn.prepare(sql).map_err(db_err)?;
        let rows = stmt.query_map([], Self::row_to_cron_job).map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn update_cron_job(&self, job_id: &str, patch: CronJobPatch) -> Result<CronJobRecord> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let mut current = Self::get_cron_job_conn(&tx, job_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Cron job '{job_id}' not found")))?;

        if let Some(name) = patch.name {
            current.name = name;
        }
        if let Some(schedule) = patch.schedule {
            current.schedule = schedule;
        }
        if let Some(tz) = patch.timezone {
            current.timezone = tz;
        }
        if let Some(target_chat_id) = patch.target_chat_id {
            current.target_chat_id = target_chat_id;
        }
        if let Some(prompt) = patch.prompt {
            current.prompt = prompt;
        }
        if let Some(model) = patch.model {
            current.model = model;
        }
        if let Some(skills) = patch.skills {
            current.skills_json = serde_json::to_string(&skills)?;
        }
        if let Some(enabled) = patch.enabled {
            current.enabled = enabled;
        }
        if let Some(deliver_to) = patch.deliver_to {
            current.deliver_to = deliver_to;
        }
        if let Some(next_run_at) = patch.next_run_at {
            current.next_run_at = next_run_at;
        }
        if let Some(last_run_at) = patch.last_run_at {
            current.last_run_at = last_run_at;
        }

        tx.execute(
            r#"
            UPDATE cron_jobs SET
                name = ?1, schedule = ?2, timezone = ?3, target_chat_id = ?4,
                prompt = ?5, model = ?6, skills_json = ?7, enabled = ?8,
                deliver_to = ?9, next_run_at = ?10, last_run_at = ?11
            WHERE id = ?12
            "#,
            params![
                current.name,
                current.schedule,
                current.timezone,
                current.target_chat_id,
                current.prompt,
                current.model,
                current.skills_json,
                current.enabled as i64,
                current.deliver_to,
                current.next_run_at,
                current.last_run_at,
                job_id,
            ],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(current)
    }

    pub fn delete_cron_job(&self, job_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        tx.execute("DELETE FROM cron_history WHERE job_id = ?1", params![job_id])
            .map_err(db_err)?;
        let affected = tx
            .execute("DELETE FROM cron_jobs WHERE id = ?1", params![job_id])
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(affected > 0)
    }

    pub fn list_due_cron_jobs(&self, current_ms: i64) -> Result<Vec<CronJobRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, name, schedule, timezone, target_chat_id, prompt, model,
                       skills_json, enabled, deliver_to, origin_chat_id,
                       next_run_at, last_run_at, created_at
                FROM cron_jobs
                WHERE enabled = 1 AND next_run_at IS NOT NULL AND next_run_at <= ?1
                ORDER BY next_run_at ASC
                "#,
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![current_ms], Self::row_to_cron_job)
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn record_cron_run(&self, req: RecordCronRunRequest) -> Result<CronHistoryRecord> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;

        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM cron_jobs WHERE id = ?1)",
                params![req.job_id],
                |row| row.get(0),
            )
            .map_err(db_err)?;
        if !exists {
            return Err(TelegramError::NotFound(format!(
                "Cron job '{}' not found",
                req.job_id
            )));
        }

        let id = new_id("crun");
        let run_at = req.run_at.unwrap_or_else(now_ms);
        tx.execute(
            r#"
            INSERT INTO cron_history (id, job_id, run_at, status, output_preview, error, duration_ms)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                id,
                req.job_id,
                run_at,
                req.status,
                req.output_preview,
                req.error,
                req.duration_ms.max(0),
            ],
        )
        .map_err(db_err)?;

        tx.execute(
            "UPDATE cron_jobs SET last_run_at = ?1, next_run_at = COALESCE(?2, next_run_at) WHERE id = ?3",
            params![run_at, req.next_run_at, req.job_id],
        )
        .map_err(db_err)?;

        let rec = CronHistoryRecord {
            id,
            job_id: req.job_id,
            run_at,
            status: req.status,
            output_preview: req.output_preview,
            error: req.error,
            duration_ms: req.duration_ms.max(0),
        };
        tx.commit().map_err(db_err)?;
        Ok(rec)
    }

    pub fn list_cron_history(&self, job_id: &str, limit: usize) -> Result<Vec<CronHistoryRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, job_id, run_at, status, output_preview, error, duration_ms
                FROM cron_history
                WHERE job_id = ?1
                ORDER BY run_at DESC
                LIMIT ?2
                "#,
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![job_id, limit as i64], |row| {
                Ok(CronHistoryRecord {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    run_at: row.get(2)?,
                    status: row.get(3)?,
                    output_preview: row.get(4)?,
                    error: row.get(5)?,
                    duration_ms: row.get(6)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    fn get_cron_job_conn(conn: &Connection, job_id: &str) -> Result<Option<CronJobRecord>> {
        conn.query_row(
            r#"
            SELECT id, name, schedule, timezone, target_chat_id, prompt, model,
                   skills_json, enabled, deliver_to, origin_chat_id,
                   next_run_at, last_run_at, created_at
            FROM cron_jobs WHERE id = ?1
            "#,
            params![job_id],
            Self::row_to_cron_job,
        )
        .optional()
        .map_err(db_err)
    }

    fn row_to_cron_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<CronJobRecord> {
        let enabled_int: i64 = row.get(8)?;
        Ok(CronJobRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            schedule: row.get(2)?,
            timezone: row.get(3)?,
            target_chat_id: row.get(4)?,
            prompt: row.get(5)?,
            model: row.get(6)?,
            skills_json: row.get(7)?,
            enabled: enabled_int != 0,
            deliver_to: row.get(9)?,
            origin_chat_id: row.get(10)?,
            next_run_at: row.get(11)?,
            last_run_at: row.get(12)?,
            created_at: row.get(13)?,
        })
    }

    // =========================================================================
    // Approvals Queue
    // =========================================================================

    pub fn enqueue_approval(&self, req: CreateApprovalRequest) -> Result<ApprovalRecord> {
        let id = req
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| new_id("appr"));
        let created_at = now_ms();
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO approvals (
                id, session_id, command, risk_level, pattern_name,
                status, decided_by, decided_at, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', NULL, NULL, ?6)
            "#,
            params![
                id,
                req.session_id,
                req.command,
                req.risk_level,
                req.pattern_name,
                created_at
            ],
        )
        .map_err(db_err)?;

        Ok(ApprovalRecord {
            id,
            session_id: req.session_id,
            command: req.command,
            risk_level: req.risk_level,
            pattern_name: req.pattern_name,
            status: ApprovalStatus::Pending,
            decided_by: None,
            decided_at: None,
            created_at,
        })
    }

    pub fn get_approval(&self, approval_id: &str) -> Result<Option<ApprovalRecord>> {
        let conn = self.conn.lock();
        conn.query_row(
            r#"
            SELECT id, session_id, command, risk_level, pattern_name,
                   status, decided_by, decided_at, created_at
            FROM approvals WHERE id = ?1
            "#,
            params![approval_id],
            Self::row_to_approval,
        )
        .optional()
        .map_err(db_err)
    }

    pub fn list_approvals(
        &self,
        session_id: Option<&str>,
        pending_only: bool,
    ) -> Result<Vec<ApprovalRecord>> {
        let conn = self.conn.lock();
        let mut sql = String::from(
            "SELECT id, session_id, command, risk_level, pattern_name, status, decided_by, decided_at, created_at FROM approvals WHERE 1=1",
        );
        let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();
        if let Some(sid) = session_id {
            sql.push_str(" AND session_id = ?");
            params_vec.push(sid.to_string().into());
        }
        if pending_only {
            sql.push_str(" AND status = 'pending'");
        }
        sql.push_str(" ORDER BY created_at ASC");

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(params_vec), Self::row_to_approval)
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn decide_approval(
        &self,
        approval_id: &str,
        status: ApprovalStatus,
        decided_by: Option<String>,
    ) -> Result<ApprovalRecord> {
        let decided_at = now_ms();
        let conn = self.conn.lock();
        let affected = conn
            .execute(
                "UPDATE approvals SET status = ?1, decided_by = ?2, decided_at = ?3 WHERE id = ?4",
                params![status.as_str(), decided_by, decided_at, approval_id],
            )
            .map_err(db_err)?;
        if affected == 0 {
            return Err(TelegramError::NotFound(format!(
                "Approval '{approval_id}' not found"
            )));
        }
        conn.query_row(
            r#"
            SELECT id, session_id, command, risk_level, pattern_name,
                   status, decided_by, decided_at, created_at
            FROM approvals WHERE id = ?1
            "#,
            params![approval_id],
            Self::row_to_approval,
        )
        .map_err(db_err)
    }

    fn row_to_approval(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalRecord> {
        let status_raw: String = row.get(5)?;
        Ok(ApprovalRecord {
            id: row.get(0)?,
            session_id: row.get(1)?,
            command: row.get(2)?,
            risk_level: row.get(3)?,
            pattern_name: row.get(4)?,
            status: ApprovalStatus::parse(&status_raw),
            decided_by: row.get(6)?,
            decided_at: row.get(7)?,
            created_at: row.get(8)?,
        })
    }

    // =========================================================================
    // File Rollback Checkpoints
    // =========================================================================

    /// Saves a pre-edit file checkpoint for `(session_id, turn_index, file_path)`.
    /// Preserves the earliest pre-turn snapshot if the same file is edited multiple times in a turn.
    pub fn save_checkpoint(
        &self,
        session_id: &str,
        turn_index: i64,
        file_path: &str,
        original_content: Option<&str>,
    ) -> Result<CheckpointRecord> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;

        let existing = tx
            .query_row(
                r#"
                SELECT id, session_id, turn_index, file_path, original_content, created_at
                FROM checkpoints
                WHERE session_id = ?1 AND turn_index = ?2 AND file_path = ?3
                "#,
                params![session_id, turn_index, file_path],
                Self::row_to_checkpoint,
            )
            .optional()
            .map_err(db_err)?;

        if let Some(rec) = existing {
            tx.commit().map_err(db_err)?;
            return Ok(rec);
        }

        let id = new_id("chk");
        let created_at = now_ms();
        tx.execute(
            r#"
            INSERT INTO checkpoints (id, session_id, turn_index, file_path, original_content, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![id, session_id, turn_index, file_path, original_content, created_at],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;

        Ok(CheckpointRecord {
            id,
            session_id: session_id.to_string(),
            turn_index,
            file_path: file_path.to_string(),
            original_content: original_content.map(ToString::to_string),
            created_at,
        })
    }

    pub fn list_checkpoints(
        &self,
        session_id: &str,
        turn_index: Option<i64>,
    ) -> Result<Vec<CheckpointRecord>> {
        let conn = self.conn.lock();
        if let Some(turn) = turn_index {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, session_id, turn_index, file_path, original_content, created_at
                    FROM checkpoints
                    WHERE session_id = ?1 AND turn_index = ?2
                    ORDER BY created_at DESC
                    "#,
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![session_id, turn], Self::row_to_checkpoint)
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        } else {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, session_id, turn_index, file_path, original_content, created_at
                    FROM checkpoints
                    WHERE session_id = ?1
                    ORDER BY turn_index DESC, created_at DESC
                    "#,
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![session_id], Self::row_to_checkpoint)
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        }
    }

    pub fn latest_checkpoint_turn(&self, session_id: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT MAX(turn_index) FROM checkpoints WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(db_err)
    }

    pub fn delete_checkpoints_for_turn(&self, session_id: &str, turn_index: i64) -> Result<usize> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM checkpoints WHERE session_id = ?1 AND turn_index = ?2",
            params![session_id, turn_index],
        )
        .map_err(db_err)
    }

    fn row_to_checkpoint(row: &rusqlite::Row<'_>) -> rusqlite::Result<CheckpointRecord> {
        Ok(CheckpointRecord {
            id: row.get(0)?,
            session_id: row.get(1)?,
            turn_index: row.get(2)?,
            file_path: row.get(3)?,
            original_content: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    // =========================================================================
    // Kanban Boards & Cards
    // =========================================================================

    pub fn create_kanban_board(
        &self,
        name: &str,
        description: &str,
        columns: Option<Vec<String>>,
    ) -> Result<KanbanBoardRecord> {
        if name.trim().is_empty() {
            return Err(TelegramError::Validation("Board name cannot be empty".into()));
        }
        let cols = columns.unwrap_or_else(|| {
            vec![
                "Backlog".into(),
                "In Progress".into(),
                "Review".into(),
                "Done".into(),
            ]
        });
        let id = new_id("board");
        let now = now_ms();
        let columns_json = serde_json::to_string(&cols)?;

        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO kanban_boards (id, name, description, columns_json, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![id, name, description, columns_json, now, now],
        )
        .map_err(db_err)?;

        Ok(KanbanBoardRecord {
            id,
            name: name.to_string(),
            description: description.to_string(),
            columns: cols,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn get_kanban_board(&self, board_id: &str) -> Result<Option<KanbanBoardRecord>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, name, description, columns_json, created_at, updated_at FROM kanban_boards WHERE id = ?1",
            params![board_id],
            Self::row_to_kanban_board,
        )
        .optional()
        .map_err(db_err)
    }

    pub fn list_kanban_boards(&self) -> Result<Vec<KanbanBoardRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, name, description, columns_json, created_at, updated_at FROM kanban_boards ORDER BY created_at ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], Self::row_to_kanban_board)
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn create_kanban_card(&self, req: CreateKanbanCardRequest) -> Result<KanbanCardRecord> {
        if req.title.trim().is_empty() {
            return Err(TelegramError::Validation("Card title cannot be empty".into()));
        }
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;

        let board = tx
            .query_row(
                "SELECT id, name, description, columns_json, created_at, updated_at FROM kanban_boards WHERE id = ?1",
                params![req.board_id],
                Self::row_to_kanban_board,
            )
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| {
                TelegramError::NotFound(format!("Kanban board '{}' not found", req.board_id))
            })?;

        let column_name = req
            .column_name
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| {
                board
                    .columns
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Backlog".into())
            });

        let position: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(position), -1) + 1 FROM kanban_cards WHERE board_id = ?1 AND column_name = ?2",
                params![req.board_id, column_name],
                |row| row.get(0),
            )
            .map_err(db_err)?;

        let id = req
            .id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| new_id("card"));
        let now = now_ms();
        let description = req.description.unwrap_or_default();
        let priority = req.priority.unwrap_or_else(|| "medium".into());
        let labels = req.labels.unwrap_or_default();
        let labels_json = serde_json::to_string(&labels)?;

        tx.execute(
            r#"
            INSERT INTO kanban_cards (
                id, board_id, column_name, title, description, priority, assignee,
                labels_json, comments_json, checklist_json, position, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '[]', '[]', ?9, ?10, ?11)
            "#,
            params![
                id,
                req.board_id,
                column_name,
                req.title,
                description,
                priority,
                req.assignee,
                labels_json,
                position,
                now,
                now,
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "UPDATE kanban_boards SET updated_at = ?1 WHERE id = ?2",
            params![now, req.board_id],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;

        Ok(KanbanCardRecord {
            id,
            board_id: req.board_id,
            column_name,
            title: req.title,
            description,
            priority,
            assignee: req.assignee,
            labels,
            comments: Vec::new(),
            checklist: Vec::new(),
            position,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn get_kanban_card(&self, card_id: &str) -> Result<Option<KanbanCardRecord>> {
        let conn = self.conn.lock();
        Self::get_kanban_card_conn(&conn, card_id)
    }

    pub fn update_kanban_card(
        &self,
        card_id: &str,
        patch: KanbanCardPatch,
    ) -> Result<KanbanCardRecord> {
        let mut conn = self.conn.lock();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_err)?;
        let mut card = Self::get_kanban_card_conn(&tx, card_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Kanban card '{card_id}' not found")))?;

        if let Some(title) = patch.title {
            card.title = title;
        }
        if let Some(description) = patch.description {
            card.description = description;
        }
        if let Some(priority) = patch.priority {
            card.priority = priority;
        }
        if let Some(assignee) = patch.assignee {
            card.assignee = assignee;
        }
        if let Some(labels) = patch.labels {
            card.labels = labels;
        }
        if let Some(comments) = patch.comments {
            card.comments = comments;
        }
        if let Some(checklist) = patch.checklist {
            card.checklist = checklist;
        }
        if let Some(col) = patch.column_name {
            card.column_name = col;
        }
        if let Some(pos) = patch.position {
            card.position = pos;
        }
        card.updated_at = now_ms();

        let labels_json = serde_json::to_string(&card.labels)?;
        let comments_json = serde_json::to_string(&card.comments)?;
        let checklist_json = serde_json::to_string(&card.checklist)?;

        tx.execute(
            r#"
            UPDATE kanban_cards SET
                column_name = ?1, title = ?2, description = ?3, priority = ?4,
                assignee = ?5, labels_json = ?6, comments_json = ?7,
                checklist_json = ?8, position = ?9, updated_at = ?10
            WHERE id = ?11
            "#,
            params![
                card.column_name,
                card.title,
                card.description,
                card.priority,
                card.assignee,
                labels_json,
                comments_json,
                checklist_json,
                card.position,
                card.updated_at,
                card_id,
            ],
        )
        .map_err(db_err)?;
        tx.execute(
            "UPDATE kanban_boards SET updated_at = ?1 WHERE id = ?2",
            params![card.updated_at, card.board_id],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(card)
    }

    pub fn move_kanban_card(
        &self,
        card_id: &str,
        target_column: &str,
        position: Option<i64>,
    ) -> Result<KanbanCardRecord> {
        let next_pos = match position {
            Some(p) => p,
            None => {
                let card = self
                    .get_kanban_card(card_id)?
                    .ok_or_else(|| TelegramError::NotFound(format!("Kanban card '{card_id}' not found")))?;
                let conn = self.conn.lock();
                conn.query_row(
                    "SELECT COALESCE(MAX(position), -1) + 1 FROM kanban_cards WHERE board_id = ?1 AND column_name = ?2",
                    params![card.board_id, target_column],
                    |row| row.get(0),
                )
                .map_err(db_err)?
            }
        };
        self.update_kanban_card(
            card_id,
            KanbanCardPatch {
                column_name: Some(target_column.to_string()),
                position: Some(next_pos),
                ..Default::default()
            },
        )
    }

    pub fn delete_kanban_card(&self, card_id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let affected = conn
            .execute("DELETE FROM kanban_cards WHERE id = ?1", params![card_id])
            .map_err(db_err)?;
        Ok(affected > 0)
    }

    pub fn add_kanban_comment(
        &self,
        card_id: &str,
        author: &str,
        body: &str,
    ) -> Result<KanbanCardRecord> {
        let mut card = self
            .get_kanban_card(card_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Kanban card '{card_id}' not found")))?;
        card.comments.push(KanbanComment {
            id: new_id("kcmt"),
            author: author.to_string(),
            body: body.to_string(),
            created_at: now_ms(),
        });
        self.update_kanban_card(
            card_id,
            KanbanCardPatch {
                comments: Some(card.comments),
                ..Default::default()
            },
        )
    }

    pub fn add_kanban_checklist_item(
        &self,
        card_id: &str,
        text: &str,
    ) -> Result<KanbanCardRecord> {
        let mut card = self
            .get_kanban_card(card_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Kanban card '{card_id}' not found")))?;
        card.checklist.push(KanbanChecklistItem {
            id: new_id("kchk"),
            text: text.to_string(),
            checked: false,
        });
        self.update_kanban_card(
            card_id,
            KanbanCardPatch {
                checklist: Some(card.checklist),
                ..Default::default()
            },
        )
    }

    pub fn toggle_kanban_checklist_item(
        &self,
        card_id: &str,
        item_id: &str,
    ) -> Result<KanbanCardRecord> {
        let mut card = self
            .get_kanban_card(card_id)?
            .ok_or_else(|| TelegramError::NotFound(format!("Kanban card '{card_id}' not found")))?;
        let Some(item) = card.checklist.iter_mut().find(|i| i.id == item_id) else {
            return Err(TelegramError::NotFound(format!(
                "Checklist item '{item_id}' not found on card '{card_id}'"
            )));
        };
        item.checked = !item.checked;
        self.update_kanban_card(
            card_id,
            KanbanCardPatch {
                checklist: Some(card.checklist),
                ..Default::default()
            },
        )
    }

    pub fn list_kanban_cards(
        &self,
        board_id: &str,
        column_name: Option<&str>,
    ) -> Result<Vec<KanbanCardRecord>> {
        let conn = self.conn.lock();
        if let Some(col) = column_name {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, board_id, column_name, title, description, priority, assignee,
                           labels_json, comments_json, checklist_json, position, created_at, updated_at
                    FROM kanban_cards
                    WHERE board_id = ?1 AND column_name = ?2
                    ORDER BY position ASC, created_at ASC
                    "#,
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![board_id, col], Self::row_to_kanban_card)
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        } else {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, board_id, column_name, title, description, priority, assignee,
                           labels_json, comments_json, checklist_json, position, created_at, updated_at
                    FROM kanban_cards
                    WHERE board_id = ?1
                    ORDER BY column_name ASC, position ASC, created_at ASC
                    "#,
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![board_id], Self::row_to_kanban_card)
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
        }
    }

    pub fn search_kanban_cards(
        &self,
        query: &str,
        board_id: Option<&str>,
    ) -> Result<Vec<KanbanCardRecord>> {
        let escaped = escape_like(query.trim());
        let pattern = format!("%{escaped}%");
        let conn = self.conn.lock();
        let mut sql = String::from(
            r#"
            SELECT id, board_id, column_name, title, description, priority, assignee,
                   labels_json, comments_json, checklist_json, position, created_at, updated_at
            FROM kanban_cards
            WHERE (LOWER(title) LIKE LOWER(?) ESCAPE '\'
                   OR LOWER(description) LIKE LOWER(?) ESCAPE '\'
                   OR LOWER(labels_json) LIKE LOWER(?) ESCAPE '\')
            "#,
        );
        let mut params_vec: Vec<rusqlite::types::Value> = vec![
            pattern.clone().into(),
            pattern.clone().into(),
            pattern.into(),
        ];
        if let Some(bid) = board_id {
            sql.push_str(" AND board_id = ?");
            params_vec.push(bid.to_string().into());
        }
        sql.push_str(" ORDER BY updated_at DESC LIMIT 100");

        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(params_vec), Self::row_to_kanban_card)
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    fn get_kanban_card_conn(conn: &Connection, card_id: &str) -> Result<Option<KanbanCardRecord>> {
        conn.query_row(
            r#"
            SELECT id, board_id, column_name, title, description, priority, assignee,
                   labels_json, comments_json, checklist_json, position, created_at, updated_at
            FROM kanban_cards WHERE id = ?1
            "#,
            params![card_id],
            Self::row_to_kanban_card,
        )
        .optional()
        .map_err(db_err)
    }

    fn row_to_kanban_board(row: &rusqlite::Row<'_>) -> rusqlite::Result<KanbanBoardRecord> {
        let cols_raw: String = row.get(3)?;
        let columns: Vec<String> = serde_json::from_str(&cols_raw).unwrap_or_default();
        Ok(KanbanBoardRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            columns,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    }

    fn row_to_kanban_card(row: &rusqlite::Row<'_>) -> rusqlite::Result<KanbanCardRecord> {
        let labels_raw: String = row.get(7)?;
        let comments_raw: String = row.get(8)?;
        let checklist_raw: String = row.get(9)?;
        Ok(KanbanCardRecord {
            id: row.get(0)?,
            board_id: row.get(1)?,
            column_name: row.get(2)?,
            title: row.get(3)?,
            description: row.get(4)?,
            priority: row.get(5)?,
            assignee: row.get(6)?,
            labels: serde_json::from_str(&labels_raw).unwrap_or_default(),
            comments: serde_json::from_str(&comments_raw).unwrap_or_default(),
            checklist: serde_json::from_str(&checklist_raw).unwrap_or_default(),
            position: row.get(10)?,
            created_at: row.get(11)?,
            updated_at: row.get(12)?,
        })
    }

    // =========================================================================
    // Key-Value State
    // =========================================================================

    pub fn kv_get(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT value FROM kv_state WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
            |row| row.get(0),
        )
        .optional()
        .map_err(db_err)
    }

    pub fn kv_set(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"
            INSERT INTO kv_state (namespace, key, value, updated_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(namespace, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at
            "#,
            params![namespace, key, value, now_ms()],
        )
        .map_err(db_err)?;
        Ok(())
    }

    pub fn kv_delete(&self, namespace: &str, key: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let affected = conn
            .execute(
                "DELETE FROM kv_state WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
            )
            .map_err(db_err)?;
        Ok(affected > 0)
    }

    pub fn kv_list(&self, namespace: &str) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT key, value FROM kv_state WHERE namespace = ?1 ORDER BY key ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![namespace], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    pub fn kv_map(&self, namespace: &str) -> Result<HashMap<String, String>> {
        Ok(self.kv_list(namespace)?.into_iter().collect())
    }
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(
            ch as u32,
            0x4E00..=0x9FFF
                | 0x3400..=0x4DBF
                | 0x20000..=0x2A6DF
                | 0x3040..=0x309F
                | 0x30A0..=0x30FF
                | 0xAC00..=0xD7AF
                | 0x3000..=0x303F
                | 0xFF00..=0xFFEF
        )
    })
}

fn sanitize_fts5_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            let escaped = t.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Extracts a safe UTF-8 char-bounded snippet around the first case-insensitive match
/// and wraps matched occurrences with `**...**`.
pub(crate) fn build_highlighted_snippet(content: &str, query: &str, max_chars: usize) -> String {
    let trimmed_q = query.trim();
    if content.is_empty() || trimmed_q.is_empty() {
        return truncate_chars(content, max_chars);
    }

    let chars: Vec<char> = content.chars().collect();
    let lower_content = content.to_lowercase();
    let lower_query = trimmed_q.to_lowercase();

    let match_char_idx = lower_content.find(&lower_query).map(|byte_idx| {
        lower_content[..byte_idx].chars().count()
    });

    let q_char_len = trimmed_q.chars().count();
    let center = match_char_idx.unwrap_or(0);
    let half = max_chars / 2;
    let start = center.saturating_sub(half);
    let end = (start + max_chars).min(chars.len());
    let start = if end == chars.len() {
        end.saturating_sub(max_chars)
    } else {
        start
    };

    let mut excerpt: String = chars[start..end].iter().collect();
    if start > 0 {
        excerpt = format!("...{excerpt}");
    }
    if end < chars.len() {
        excerpt.push_str("...");
    }

    let lower_excerpt = excerpt.to_lowercase();
    if lower_excerpt.len() == excerpt.len() && lower_query.len() == trimmed_q.len() {
        if let Some(pos) = lower_excerpt.find(&lower_query) {
            let end_byte = pos + trimmed_q.len();
            if excerpt.is_char_boundary(pos) && excerpt.is_char_boundary(end_byte) && q_char_len > 0 {
                let mut highlighted = String::with_capacity(excerpt.len() + 4);
                highlighted.push_str(&excerpt[..pos]);
                highlighted.push_str("**");
                highlighted.push_str(&excerpt[pos..end_byte]);
                highlighted.push_str("**");
                highlighted.push_str(&excerpt[end_byte..]);
                return highlighted;
            }
        }
    }

    excerpt
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            break;
        }
        out.push(ch);
    }
    out
}
