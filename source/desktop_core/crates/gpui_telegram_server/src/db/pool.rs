use gpui_telegram_core::error::{Result, TelegramError};
use parking_lot::{Mutex, MutexGuard};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Thread-safe SQLite connection pool configured for WAL persistence.
#[derive(Clone)]
pub struct SqlitePool {
    path: Option<PathBuf>,
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for SqlitePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlitePool")
            .field("path", &self.path)
            .finish()
    }
}

impl SqlitePool {
    /// Opens or creates a SQLite database file at `path` and applies WAL pragmas.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        if path_ref == Path::new(":memory:") {
            return Self::open_in_memory();
        }

        let path_buf = path_ref.to_path_buf();
        if let Some(parent) = path_buf.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(TelegramError::from)?;
            }
        }

        let conn = Connection::open(&path_buf).map_err(|e| TelegramError::Database(e.to_string()))?;
        Self::apply_pragmas(&conn)?;

        Ok(Self {
            path: Some(path_buf),
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Opens an in-memory SQLite database instance with WAL/foreign-key pragmas.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| TelegramError::Database(e.to_string()))?;
        Self::apply_pragmas(&conn)?;

        Ok(Self {
            path: None,
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Applies recommended high-performance SQLite pragmas.
    pub fn apply_pragmas(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA temp_store = MEMORY;
            PRAGMA cache_size = -64000;
            "#,
        )
        .map_err(|e| TelegramError::Database(format!("Failed to apply pragmas: {}", e)))?;
        Ok(())
    }

    /// Acquires a lock on the primary database connection.
    pub fn acquire(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock()
    }

    /// Executes a closure with exclusive access to the underlying connection.
    pub fn with_conn<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R>,
    {
        let mut guard = self.conn.lock();
        f(&mut guard)
    }

    /// Executes a closure inside an atomic `BEGIN IMMEDIATE` SQLite transaction.
    /// Automatically commits if the closure returns `Ok`, or rolls back on `Err`.
    pub fn with_transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Transaction<'_>) -> Result<R>,
    {
        let mut guard = self.conn.lock();
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| TelegramError::Database(format!("Failed to begin transaction: {}", e)))?;

        let result = f(&tx)?;

        tx.commit()
            .map_err(|e| TelegramError::Database(format!("Failed to commit transaction: {}", e)))?;

        Ok(result)
    }

    /// Returns the database path, if file-backed.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}
