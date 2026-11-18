use crossbeam_queue::ArrayQueue;
use gpui_telegram_core::error::{Result, TelegramError};
use parking_lot::{Condvar, Mutex, MutexGuard};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_READER_CAPACITY: usize = 16;
const MAX_EPHEMERAL_READERS: usize = 64;

/// Thread-safe SQLite connection pool configured for WAL persistence with 1 exclusive writer
/// and N concurrent readers.
#[derive(Clone)]
pub struct SqlitePool {
    path: Option<PathBuf>,
    connect_path: PathBuf,
    writer: Arc<Mutex<Connection>>,
    readers: Arc<ArrayQueue<Connection>>,
    reader_capacity: usize,
    ephemeral_active: Arc<AtomicUsize>,
    reader_wakeup: Arc<(Mutex<()>, Condvar)>,
    // Declared LAST so writer and reader Connections close before TempDir is removed on Drop.
    _ephemeral_dir: Option<Arc<tempfile::TempDir>>,
}

impl std::fmt::Debug for SqlitePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqlitePool")
            .field("path", &self.path)
            .field("connect_path", &self.connect_path)
            .field("reader_capacity", &self.reader_capacity)
            .field("available_readers", &self.readers.len())
            .field("ephemeral_active", &self.ephemeral_active.load(Ordering::Relaxed))
            .finish()
    }
}

impl SqlitePool {
    /// Opens or creates a SQLite database file at `path` and applies WAL pragmas.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_capacity(path, DEFAULT_READER_CAPACITY)
    }

    /// Opens or creates a SQLite database file with custom reader pool capacity.
    pub fn open_with_capacity(path: impl AsRef<Path>, reader_capacity: usize) -> Result<Self> {
        let path_ref = path.as_ref();
        if path_ref == Path::new(":memory:") {
            return Self::open_in_memory_with_capacity(reader_capacity);
        }

        let path_str = path_ref.to_string_lossy();
        if path_ref.as_os_str().is_empty() || path_str.trim().is_empty() {
            return Err(TelegramError::Database(
                "Database file path cannot be empty".to_string(),
            ));
        }

        let path_buf = path_ref.to_path_buf();
        if path_buf.is_dir() {
            return Err(TelegramError::Database(format!(
                "Database path '{}' is a directory",
                path_buf.display()
            )));
        }

        if let Some(parent) = path_buf.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    TelegramError::Database(format!(
                        "Failed to create database directory '{}': {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }

        Self::open_wal_pool(Some(path_buf.clone()), path_buf, None, reader_capacity)
    }

    /// Opens an ephemeral WAL-backed SQLite database instance with concurrent multi-reader support.
    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with_capacity(DEFAULT_READER_CAPACITY)
    }

    /// Opens an ephemeral WAL-backed SQLite database instance with custom reader capacity.
    pub fn open_in_memory_with_capacity(reader_capacity: usize) -> Result<Self> {
        let tmp_dir = tempfile::Builder::new()
            .prefix("gpui_tg_mem_")
            .tempdir()
            .map_err(|e| {
                TelegramError::Database(format!(
                    "Failed to create ephemeral WAL temp directory: {e}"
                ))
            })?;
        let db_path = tmp_dir.path().join("inmem.sqlite");
        Self::open_wal_pool(None, db_path, Some(Arc::new(tmp_dir)), reader_capacity)
    }

    fn open_wal_pool(
        public_path: Option<PathBuf>,
        connect_path: PathBuf,
        ephemeral_dir: Option<Arc<tempfile::TempDir>>,
        reader_capacity: usize,
    ) -> Result<Self> {
        // Open exclusive writer connection first so WAL journal mode is initialized cleanly.
        let writer_conn = Connection::open(&connect_path).map_err(|e| {
            TelegramError::Database(format!(
                "Failed to open SQLite writer database '{}': {}",
                connect_path.display(),
                e
            ))
        })?;
        Self::apply_writer_pragmas(&writer_conn)?;

        let cap = reader_capacity.max(1);
        let queue = ArrayQueue::new(cap);

        for _ in 0..cap {
            let r_conn = Self::open_readonly_connection(&connect_path)?;
            let _ = queue.push(r_conn);
        }

        Ok(Self {
            path: public_path,
            connect_path,
            writer: Arc::new(Mutex::new(writer_conn)),
            readers: Arc::new(queue),
            reader_capacity: cap,
            ephemeral_active: Arc::new(AtomicUsize::new(0)),
            reader_wakeup: Arc::new((Mutex::new(()), Condvar::new())),
            _ephemeral_dir: ephemeral_dir,
        })
    }

    fn open_readonly_connection(path: &Path) -> Result<Connection> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| {
            TelegramError::Database(format!(
                "Failed to open SQLite read-only reader '{}': {}",
                path.display(),
                e
            ))
        })?;
        Self::apply_reader_pragmas(&conn)?;
        Ok(conn)
    }

    /// Applies recommended high-performance SQLite pragmas for writer connections.
    pub fn apply_writer_pragmas(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA busy_timeout = 5000;
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA temp_store = MEMORY;
            PRAGMA cache_size = -64000;
            "#,
        )
        .map_err(|e| TelegramError::Database(format!("Failed to apply writer pragmas: {}", e)))?;

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| TelegramError::Database(format!("SQLite integrity check failed: {}", e)))?;
        if !integrity.eq_ignore_ascii_case("ok") {
            return Err(TelegramError::Database(format!(
                "SQLite integrity check failed: {}",
                integrity
            )));
        }

        Ok(())
    }

    /// Applies read-only pragmas to guarantee readers never mutate state or escalate locks.
    pub fn apply_reader_pragmas(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA query_only = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA foreign_keys = ON;
            PRAGMA cache_size = -64000;
            "#,
        )
        .map_err(|e| TelegramError::Database(format!("Failed to apply reader pragmas: {}", e)))?;

        Ok(())
    }

    /// Backward-compatible alias for applying writer pragmas.
    pub fn apply_pragmas(conn: &Connection) -> Result<()> {
        Self::apply_writer_pragmas(conn)
    }

    /// Creates an ephemeral read-only reader connection when the reader queue is saturated.
    fn create_reader_connection(&self) -> Result<Connection> {
        Self::open_readonly_connection(&self.connect_path)
    }

    fn try_acquire_ephemeral_slot(&self) -> bool {
        let mut cur = self.ephemeral_active.load(Ordering::Acquire);
        loop {
            if cur >= MAX_EPHEMERAL_READERS {
                return false;
            }
            match self.ephemeral_active.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Acquires a lock on the exclusive writer connection.
    pub fn acquire(&self) -> MutexGuard<'_, Connection> {
        self.writer.lock()
    }

    /// Executes a read query against the pooled concurrent readers without blocking or being blocked by writers.
    pub fn with_reader<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> Result<R>,
    {
        let (conn, should_return) = if let Some(c) = self.readers.pop() {
            (c, true)
        } else if self.try_acquire_ephemeral_slot() {
            match self.create_reader_connection() {
                Ok(ephemeral) => (ephemeral, false),
                Err(e) => {
                    self.ephemeral_active.fetch_sub(1, Ordering::Release);
                    self.reader_wakeup.1.notify_one();
                    return Err(e);
                }
            }
        } else {
            let deadline = Instant::now() + Duration::from_millis(5000);
            let mut acquired = None;
            let (lock, cvar) = &*self.reader_wakeup;
            let mut guard = lock.lock();
            loop {
                if let Some(c) = self.readers.pop() {
                    acquired = Some((c, true));
                    break;
                }
                if self.try_acquire_ephemeral_slot() {
                    drop(guard);
                    match self.create_reader_connection() {
                        Ok(ephemeral) => {
                            acquired = Some((ephemeral, false));
                            break;
                        }
                        Err(e) => {
                            self.ephemeral_active.fetch_sub(1, Ordering::Release);
                            cvar.notify_one();
                            return Err(e);
                        }
                    }
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                cvar.wait_until(&mut guard, deadline);
            }
            match acquired {
                Some(pair) => pair,
                None => {
                    return Err(TelegramError::Database(
                        "Timed out waiting for SQLite reader connection".to_string(),
                    ));
                }
            }
        };

        struct ReaderGuard<'a> {
            conn: Option<Connection>,
            pool: &'a SqlitePool,
            should_return: bool,
        }

        impl<'a> Drop for ReaderGuard<'a> {
            fn drop(&mut self) {
                if self.should_return {
                    if let Some(c) = self.conn.take() {
                        if !c.is_autocommit() {
                            let _ = c.execute_batch("ROLLBACK;");
                        }
                        let clean = c.is_autocommit()
                            && c.execute_batch("PRAGMA query_only = ON;").is_ok();
                        if clean {
                            let _ = self.pool.readers.push(c);
                        } else if let Ok(fresh) = self.pool.create_reader_connection() {
                            let _ = self.pool.readers.push(fresh);
                        }
                    }
                } else {
                    drop(self.conn.take());
                    self.pool.ephemeral_active.fetch_sub(1, Ordering::Release);
                }
                self.pool.reader_wakeup.1.notify_one();
            }
        }

        let guard = ReaderGuard {
            conn: Some(conn),
            pool: self,
            should_return,
        };

        f(guard.conn.as_ref().unwrap())
    }

    /// Executes a closure with exclusive access to the writer connection.
    pub fn with_writer<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R>,
    {
        let mut guard = self.writer.lock();
        f(&mut guard)
    }

    /// Executes a closure inside an atomic `BEGIN IMMEDIATE` SQLite transaction on the writer connection.
    /// Automatically commits if the closure returns `Ok`, or rolls back on `Err`.
    pub fn with_transaction<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Transaction<'_>) -> Result<R>,
    {
        let mut guard = self.writer.lock();
        let tx = guard
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| TelegramError::Database(format!("Failed to begin transaction: {}", e)))?;

        match f(&tx) {
            Ok(result) => {
                tx.commit().map_err(|e| {
                    TelegramError::Database(format!("Failed to commit transaction: {}", e))
                })?;
                Ok(result)
            }
            Err(e) => {
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    /// Backward-compatible alias routing to `with_writer`.
    pub fn with_conn<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R>,
    {
        self.with_writer(f)
    }

    /// Returns the database path, if file-backed.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Total reader connection capacity in the pool.
    pub fn reader_capacity(&self) -> usize {
        self.reader_capacity
    }

    /// Current available reader connections in the pool.
    pub fn available_readers(&self) -> usize {
        self.readers.len()
    }
}
