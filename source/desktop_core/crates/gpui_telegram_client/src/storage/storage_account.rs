//! Target Symbol: StorageAccount
//! Telegram Desktop 1:1 Submodule: `StorageAccount`
//! Subsystem: `storage`, Feature ID: `TCG-N34`
//! Disposition: `rust_domain_or_server_1to1`

use std::path::{Path, PathBuf};
use parking_lot::Mutex;
use rusqlite::{params, Connection};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MessageDraft {
    pub peer_id: i64,
    pub topic_root_id: i32,
    pub reply_to_id: Option<i32>,
    pub text_content: String,
    pub cursor_position: i32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileLocation {
    pub media_key: String,
    pub file_path: String,
    pub file_size: i64,
    pub modified_time: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StickerSetRecord {
    pub set_id: u64,
    pub access_hash: u64,
    pub title: String,
    pub short_name: String,
    pub count: i32,
    pub installed_order: i32,
}

pub struct StorageAccount {
    pub data_name: String,
    pub base_path: PathBuf,
    conn: Mutex<Connection>,
    local_key: Option<[u8; 32]>,
}

impl StorageAccount {
    pub fn open_in_memory(data_name: impl Into<String>) -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let account = Self {
            data_name: data_name.into(),
            base_path: PathBuf::from("."),
            conn: Mutex::new(conn),
            local_key: None,
        };
        account.init_tables()?;
        Ok(account)
    }

    pub fn open(data_name: impl Into<String>, base_path: impl AsRef<Path>) -> Result<Self, String> {
        let base = base_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
        let name = data_name.into();
        let db_path = base.join(format!("{}.db", name));

        let conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        // WAL mode & high performance pragmas
        let _ = conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        );

        let account = Self {
            data_name: name,
            base_path: base,
            conn: Mutex::new(conn),
            local_key: None,
        };
        account.init_tables()?;
        Ok(account)
    }

    pub fn set_local_key(&mut self, key: [u8; 32]) {
        self.local_key = Some(key);
    }

    pub fn local_key(&self) -> Option<&[u8; 32]> {
        self.local_key.as_ref()
    }

    fn init_tables(&self) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS file_locations (
                media_key TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                modified_time INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS message_drafts (
                peer_id INTEGER NOT NULL,
                topic_root_id INTEGER NOT NULL DEFAULT 0,
                reply_to_id INTEGER,
                text_content TEXT NOT NULL,
                cursor_position INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (peer_id, topic_root_id)
            );

            CREATE TABLE IF NOT EXISTS sticker_sets (
                set_id INTEGER PRIMARY KEY,
                access_hash INTEGER NOT NULL,
                title TEXT NOT NULL,
                short_name TEXT NOT NULL,
                count INTEGER NOT NULL,
                installed_order INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS recent_hashtags (
                tag TEXT PRIMARY KEY,
                use_count INTEGER NOT NULL DEFAULT 1,
                last_used INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_values (
                key TEXT PRIMARY KEY,
                val BLOB NOT NULL
            );",
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn write_file_location(&self, location: &FileLocation) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO file_locations (media_key, file_path, file_size, modified_time)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                location.media_key,
                location.file_path,
                location.file_size,
                location.modified_time
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn read_file_location(&self, media_key: &str) -> Result<Option<FileLocation>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT file_path, file_size, modified_time FROM file_locations WHERE media_key = ?1",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt.query(params![media_key]).map_err(|e| e.to_string())?;
        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
            Ok(Some(FileLocation {
                media_key: media_key.to_string(),
                file_path: row.get(0).map_err(|e| e.to_string())?,
                file_size: row.get(1).map_err(|e| e.to_string())?,
                modified_time: row.get(2).map_err(|e| e.to_string())?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn write_draft(&self, draft: &MessageDraft) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO message_drafts (peer_id, topic_root_id, reply_to_id, text_content, cursor_position, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                draft.peer_id,
                draft.topic_root_id,
                draft.reply_to_id,
                draft.text_content,
                draft.cursor_position,
                draft.updated_at
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn read_draft(&self, peer_id: i64, topic_root_id: i32) -> Result<Option<MessageDraft>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT reply_to_id, text_content, cursor_position, updated_at FROM message_drafts
                 WHERE peer_id = ?1 AND topic_root_id = ?2",
            )
            .map_err(|e| e.to_string())?;

        let mut rows = stmt
            .query(params![peer_id, topic_root_id])
            .map_err(|e| e.to_string())?;

        if let Some(row) = rows.next().map_err(|e| e.to_string())? {
            Ok(Some(MessageDraft {
                peer_id,
                topic_root_id,
                reply_to_id: row.get(0).map_err(|e| e.to_string())?,
                text_content: row.get(1).map_err(|e| e.to_string())?,
                cursor_position: row.get(2).map_err(|e| e.to_string())?,
                updated_at: row.get(3).map_err(|e| e.to_string())?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn clear_draft(&self, peer_id: i64, topic_root_id: i32) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "DELETE FROM message_drafts WHERE peer_id = ?1 AND topic_root_id = ?2",
            params![peer_id, topic_root_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn write_sticker_set(&self, set: &StickerSetRecord) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO sticker_sets (set_id, access_hash, title, short_name, count, installed_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                set.set_id as i64,
                set.access_hash as i64,
                set.title,
                set.short_name,
                set.count,
                set.installed_order
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn read_installed_sticker_sets(&self) -> Result<Vec<StickerSetRecord>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT set_id, access_hash, title, short_name, count, installed_order FROM sticker_sets ORDER BY installed_order ASC")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let set_id: i64 = row.get(0)?;
                let access_hash: i64 = row.get(1)?;
                Ok(StickerSetRecord {
                    set_id: set_id as u64,
                    access_hash: access_hash as u64,
                    title: row.get(2)?,
                    short_name: row.get(3)?,
                    count: row.get(4)?,
                    installed_order: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut result = Vec::new();
        for r in rows {
            result.push(r.map_err(|e| e.to_string())?);
        }
        Ok(result)
    }

    pub fn increment_recent_hashtag(&self, tag: &str) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO recent_hashtags (tag, use_count, last_used)
             VALUES (?1, 1, ?2)
             ON CONFLICT(tag) DO UPDATE SET use_count = use_count + 1, last_used = ?2",
            params![tag, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn read_recent_hashtags(&self, limit: usize) -> Result<Vec<String>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT tag FROM recent_hashtags ORDER BY use_count DESC, last_used DESC LIMIT ?1")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![limit as i64], |row| row.get(0))
            .map_err(|e| e.to_string())?;

        let mut list = Vec::new();
        for r in rows {
            list.push(r.map_err(|e| e.to_string())?);
        }
        Ok(list)
    }
}
