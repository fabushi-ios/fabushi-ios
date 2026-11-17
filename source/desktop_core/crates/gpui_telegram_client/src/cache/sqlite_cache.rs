use gpui_telegram_core::error::{Result, TelegramError};
use gpui_telegram_core::types::{ChatId, MessageId};
use gpui_telegram_proto::contracts::{ChatDetailDto, ChatFolderDto, DraftDto};
use gpui_telegram_proto::message::{MessageItem, ReactionCount};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CachedMediaEntry {
    pub file_id: String,
    pub sha256: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub local_path: PathBuf,
}

pub(crate) struct SqliteClientCache {
    pub(crate) conn: Mutex<Connection>,
    base_dir: PathBuf,
    media_dir: PathBuf,
}

impl std::fmt::Debug for SqliteClientCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteClientCache")
            .field("media_dir", &self.media_dir)
            .finish()
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(&mut out, "{:02x}", b);
    }
    out
}

pub(crate) fn db_err(err: rusqlite::Error) -> TelegramError {
    TelegramError::Database(err.to_string())
}

const CLIENT_SCHEMA_SQL: &str = "
    PRAGMA journal_mode=WAL;
    PRAGMA synchronous=NORMAL;
    CREATE TABLE IF NOT EXISTS messages (
        chat_id INTEGER NOT NULL,
        msg_id INTEGER NOT NULL,
        date_ms INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        PRIMARY KEY (chat_id, msg_id)
    );
    CREATE INDEX IF NOT EXISTS idx_messages_chat_date ON messages(chat_id, date_ms, msg_id);
    CREATE TABLE IF NOT EXISTS chats (
        chat_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS drafts (
        chat_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS read_states (
        chat_id INTEGER PRIMARY KEY,
        read_inbox_max INTEGER NOT NULL DEFAULT 0,
        read_outbox_max INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS media_cas (
        file_id TEXT PRIMARY KEY,
        sha256 TEXT NOT NULL,
        mime_type TEXT,
        size_bytes INTEGER NOT NULL,
        local_path TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS message_reactions (
        chat_id INTEGER NOT NULL,
        msg_id INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        PRIMARY KEY (chat_id, msg_id)
    );
    CREATE TABLE IF NOT EXISTS cache_meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS outbox_messages (
        client_nonce INTEGER PRIMARY KEY,
        chat_id INTEGER NOT NULL,
        payload_json TEXT NOT NULL,
        text_preview TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL,
        retry_count INTEGER NOT NULL DEFAULT 0,
        next_retry_at_ms INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        server_message_id INTEGER,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_outbox_status_retry
        ON outbox_messages(status, next_retry_at_ms, client_nonce);
    CREATE TABLE IF NOT EXISTS file_transfers (
        transfer_id TEXT PRIMARY KEY,
        upload_id TEXT,
        direction TEXT NOT NULL DEFAULT 'upload',
        file_path TEXT NOT NULL,
        filename TEXT NOT NULL,
        mime_type TEXT NOT NULL,
        total_size INTEGER NOT NULL,
        part_size INTEGER NOT NULL,
        total_parts INTEGER NOT NULL,
        file_sha256 TEXT NOT NULL,
        chunk_hashes_json TEXT NOT NULL,
        uploaded_parts_json TEXT NOT NULL,
        status TEXT NOT NULL,
        server_file_id TEXT,
        last_error TEXT,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_file_transfers_status
        ON file_transfers(status, updated_at_ms);
    CREATE TABLE IF NOT EXISTS folders (
        folder_id INTEGER PRIMARY KEY,
        payload_json TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS chat_ttls (
        chat_id INTEGER PRIMARY KEY,
        ttl_seconds INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_identity (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        x25519_sec BLOB NOT NULL,
        x25519_pub BLOB NOT NULL,
        ed25519_sec BLOB NOT NULL,
        ed25519_pub BLOB NOT NULL,
        next_spk_id INTEGER NOT NULL,
        next_otpk_id INTEGER NOT NULL,
        created_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_signed_prekeys (
        key_id INTEGER PRIMARY KEY,
        secret BLOB NOT NULL,
        public BLOB NOT NULL,
        signature BLOB NOT NULL,
        created_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_one_time_prekeys (
        key_id INTEGER PRIMARY KEY,
        secret BLOB NOT NULL,
        public BLOB NOT NULL,
        consumed INTEGER NOT NULL DEFAULT 0,
        created_at_ms INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS e2ee_sessions (
        peer_user_id INTEGER NOT NULL,
        peer_device_id TEXT NOT NULL,
        state_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        PRIMARY KEY (peer_user_id, peer_device_id)
    );
    CREATE TABLE IF NOT EXISTS e2ee_sender_keys (
        chat_id INTEGER NOT NULL,
        sender_user_id INTEGER NOT NULL,
        sender_device_id TEXT NOT NULL,
        state_json TEXT NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        PRIMARY KEY (chat_id, sender_user_id, sender_device_id)
    );
    CREATE TABLE IF NOT EXISTS e2ee_distributed_keys (
        chat_id INTEGER NOT NULL,
        chain_id INTEGER NOT NULL,
        peer_user_id INTEGER NOT NULL,
        peer_device_id TEXT NOT NULL,
        distributed_at_ms INTEGER NOT NULL,
        PRIMARY KEY (chat_id, chain_id, peer_user_id, peer_device_id)
    );
";

impl SqliteClientCache {
    pub(crate) fn open(base_dir: &Path) -> Result<Self> {
        let media_dir = base_dir.join("media");
        std::fs::create_dir_all(&media_dir)?;
        let db_path = base_dir.join("client_cache.db");
        let conn = Connection::open(&db_path).map_err(db_err)?;
        conn.execute_batch(CLIENT_SCHEMA_SQL).map_err(db_err)?;
        Ok(Self {
            conn: Mutex::new(conn),
            base_dir: base_dir.to_path_buf(),
            media_dir,
        })
    }

    pub(crate) fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(db_err)?;
        conn.execute_batch(CLIENT_SCHEMA_SQL).map_err(db_err)?;
        Ok(Self {
            conn: Mutex::new(conn),
            base_dir: PathBuf::new(),
            media_dir: PathBuf::new(),
        })
    }

    pub(crate) fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub(crate) fn load_owner_scope(&self) -> Option<(String, u64)> {
        let conn = self.conn.lock();
        let uid_str: Option<String> = conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'owner_user_id'",
                [],
                |r| r.get(0),
            )
            .ok();
        let uid = uid_str.and_then(|s| s.parse::<u64>().ok()).filter(|&id| id > 0)?;
        let srv: String = conn
            .query_row(
                "SELECT value FROM cache_meta WHERE key = 'owner_server_url'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default();
        Some((srv, uid))
    }

    pub(crate) fn save_owner_scope(&self, server_url: &str, user_id: u64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO cache_meta (key, value) VALUES ('owner_user_id', ?1)",
            params![user_id.to_string()],
        );
        let _ = conn.execute(
            "INSERT OR REPLACE INTO cache_meta (key, value) VALUES ('owner_server_url', ?1)",
            params![server_url],
        );
    }

    pub(crate) fn load_message_reactions(&self) -> Result<HashMap<(ChatId, MessageId), Vec<ReactionCount>>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, msg_id, payload_json FROM message_reactions")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (cid, mid, json) = row.map_err(db_err)?;
            if let Ok(reactions) = serde_json::from_str::<Vec<ReactionCount>>(&json) {
                if !reactions.is_empty() {
                    out.insert((ChatId(cid), MessageId(mid)), reactions);
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_message_reactions(
        &self,
        chat_id: ChatId,
        msg_id: MessageId,
        reactions: &[ReactionCount],
    ) {
        let conn = self.conn.lock();
        if reactions.is_empty() {
            let _ = conn.execute(
                "DELETE FROM message_reactions WHERE chat_id = ?1 AND msg_id = ?2",
                params![chat_id.0, msg_id.0],
            );
        } else if let Ok(json) = serde_json::to_string(reactions) {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO message_reactions (chat_id, msg_id, payload_json) VALUES (?1, ?2, ?3)",
                params![chat_id.0, msg_id.0, json],
            );
        }
    }

    pub(crate) fn clear_chat_reactions(&self, chat_id: ChatId) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM message_reactions WHERE chat_id = ?1",
            params![chat_id.0],
        );
    }

    pub(crate) fn clear_all(&self) {
        let conn = self.conn.lock();
        let _ = conn.execute_batch(
            "DELETE FROM messages;
             DELETE FROM message_reactions;
             DELETE FROM chats;
             DELETE FROM drafts;
             DELETE FROM read_states;
             DELETE FROM cache_meta;
             DELETE FROM outbox_messages;
             DELETE FROM file_transfers;
             DELETE FROM folders;
             DELETE FROM chat_ttls;
             DELETE FROM e2ee_identity;
             DELETE FROM e2ee_signed_prekeys;
             DELETE FROM e2ee_one_time_prekeys;
             DELETE FROM e2ee_sessions;
             DELETE FROM e2ee_sender_keys;
             DELETE FROM e2ee_distributed_keys;",
        );
    }

    pub(crate) fn max_outbox_nonce(&self) -> u64 {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT COALESCE(MAX(client_nonce), 0) FROM outbox_messages",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        .max(0) as u64
    }

    pub(crate) fn load_folders(&self) -> Result<Vec<ChatFolderDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM folders ORDER BY folder_id ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(folder) = serde_json::from_str::<ChatFolderDto>(&json) {
                out.push(folder);
            }
        }
        Ok(out)
    }

    pub(crate) fn replace_folders(&self, folders: &[ChatFolderDto]) {
        let conn = self.conn.lock();
        let _ = conn.execute("DELETE FROM folders", []);
        for f in folders {
            if let Ok(json) = serde_json::to_string(f) {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO folders (folder_id, payload_json) VALUES (?1, ?2)",
                    params![f.id, json],
                );
            }
        }
    }

    pub(crate) fn upsert_folder(&self, folder: &ChatFolderDto) {
        if let Ok(json) = serde_json::to_string(folder) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO folders (folder_id, payload_json) VALUES (?1, ?2)",
                params![folder.id, json],
            );
        }
    }

    pub(crate) fn delete_folder(&self, folder_id: i64) {
        let conn = self.conn.lock();
        let _ = conn.execute("DELETE FROM folders WHERE folder_id = ?1", params![folder_id]);
    }

    pub(crate) fn load_chat_ttls(&self) -> Result<HashMap<ChatId, u32>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, ttl_seconds FROM chat_ttls")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let (cid, ttl) = row.map_err(db_err)?;
            if ttl > 0 {
                out.insert(ChatId(cid), ttl as u32);
            }
        }
        Ok(out)
    }

    pub(crate) fn upsert_chat_ttl(&self, chat_id: ChatId, ttl_seconds: u32) {
        let conn = self.conn.lock();
        if ttl_seconds == 0 {
            let _ = conn.execute("DELETE FROM chat_ttls WHERE chat_id = ?1", params![chat_id.0]);
        } else {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO chat_ttls (chat_id, ttl_seconds) VALUES (?1, ?2)",
                params![chat_id.0, ttl_seconds as i64],
            );
        }
    }

    pub(crate) fn load_messages(&self) -> Result<HashMap<ChatId, Vec<MessageItem>>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT payload_json FROM messages ORDER BY date_ms ASC, msg_id ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db_err)?;
        let mut out: HashMap<ChatId, Vec<MessageItem>> = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(item) = serde_json::from_str::<MessageItem>(&json) {
                out.entry(item.chat_id).or_default().push(item);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_chats(&self) -> Result<HashMap<ChatId, ChatDetailDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT payload_json FROM chats").map_err(db_err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(chat) = serde_json::from_str::<ChatDetailDto>(&json) {
                out.insert(ChatId(chat.chat_id), chat);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_drafts(&self) -> Result<HashMap<ChatId, DraftDto>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT payload_json FROM drafts").map_err(db_err)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let json = row.map_err(db_err)?;
            if let Ok(draft) = serde_json::from_str::<DraftDto>(&json) {
                out.insert(ChatId(draft.chat_id), draft);
            }
        }
        Ok(out)
    }

    pub(crate) fn load_read_states(&self) -> Result<(HashMap<ChatId, MessageId>, HashMap<ChatId, MessageId>)> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT chat_id, read_inbox_max, read_outbox_max FROM read_states")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))
            .map_err(db_err)?;
        let (mut inbox, mut outbox) = (HashMap::new(), HashMap::new());
        for row in rows {
            let (cid, r_in, r_out) = row.map_err(db_err)?;
            if r_in > 0 {
                inbox.insert(ChatId(cid), MessageId(r_in));
            }
            if r_out > 0 {
                outbox.insert(ChatId(cid), MessageId(r_out));
            }
        }
        Ok((inbox, outbox))
    }

    pub(crate) fn load_media_index(&self) -> Result<HashMap<String, CachedMediaEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT file_id, sha256, mime_type, size_bytes, local_path FROM media_cas")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(CachedMediaEntry {
                    file_id: r.get(0)?,
                    sha256: r.get(1)?,
                    mime_type: r.get(2)?,
                    size_bytes: r.get::<_, i64>(3)?.max(0) as u64,
                    local_path: PathBuf::from(r.get::<_, String>(4)?),
                })
            })
            .map_err(db_err)?;
        let mut out = HashMap::new();
        for row in rows {
            let entry = row.map_err(db_err)?;
            out.insert(entry.file_id.clone(), entry);
        }
        Ok(out)
    }

    pub(crate) fn upsert_message(&self, msg: &MessageItem) {
        if let Ok(json) = serde_json::to_string(msg) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO messages (chat_id, msg_id, date_ms, payload_json) VALUES (?1, ?2, ?3, ?4)",
                params![msg.chat_id.0, msg.id.0, msg.date.timestamp_millis(), json],
            );
        }
    }

    pub(crate) fn delete_message(&self, chat_id: ChatId, msg_id: MessageId) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "DELETE FROM messages WHERE chat_id = ?1 AND msg_id = ?2",
            params![chat_id.0, msg_id.0],
        );
        let _ = conn.execute(
            "DELETE FROM message_reactions WHERE chat_id = ?1 AND msg_id = ?2",
            params![chat_id.0, msg_id.0],
        );
    }

    pub(crate) fn upsert_chat(&self, chat: &ChatDetailDto) {
        if let Ok(json) = serde_json::to_string(chat) {
            let conn = self.conn.lock();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO chats (chat_id, payload_json) VALUES (?1, ?2)",
                params![chat.chat_id, json],
            );
        }
    }

    pub(crate) fn set_draft(&self, chat_id: ChatId, draft: Option<&DraftDto>) {
        let conn = self.conn.lock();
        if let Some(d) = draft {
            if let Ok(json) = serde_json::to_string(d) {
                let _ = conn.execute(
                    "INSERT OR REPLACE INTO drafts (chat_id, payload_json) VALUES (?1, ?2)",
                    params![chat_id.0, json],
                );
            }
        } else {
            let _ = conn.execute("DELETE FROM drafts WHERE chat_id = ?1", params![chat_id.0]);
        }
    }

    pub(crate) fn upsert_read_state(&self, chat_id: ChatId, inbox: i64, outbox: i64) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT OR REPLACE INTO read_states (chat_id, read_inbox_max, read_outbox_max) VALUES (?1, ?2, ?3)",
            params![chat_id.0, inbox, outbox],
        );
    }

    pub(crate) fn write_media_cas(
        &self,
        file_id: &str,
        bytes: &[u8],
        mime_type: Option<&str>,
    ) -> Result<CachedMediaEntry> {
        let sha = sha256_hex(bytes);
        let final_path = self.media_dir.join(format!("{}.bin", sha));
        if !final_path.exists() {
            let tmp_path = self
                .media_dir
                .join(format!(".tmp-{}-{}", sha, std::process::id()));
            std::fs::write(&tmp_path, bytes)?;
            if let Err(err) = std::fs::rename(&tmp_path, &final_path) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(err.into());
            }
        }
        let entry = CachedMediaEntry {
            file_id: file_id.to_string(),
            sha256: sha.clone(),
            mime_type: mime_type.map(|m| m.to_string()),
            size_bytes: bytes.len() as u64,
            local_path: final_path.clone(),
        };
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO media_cas (file_id, sha256, mime_type, size_bytes, local_path) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.file_id,
                entry.sha256,
                entry.mime_type,
                entry.size_bytes as i64,
                final_path.to_string_lossy().as_ref()
            ],
        )
        .map_err(db_err)?;
        Ok(entry)
    }
}
