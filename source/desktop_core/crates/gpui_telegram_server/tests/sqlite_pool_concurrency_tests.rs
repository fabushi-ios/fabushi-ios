use gpui_telegram_core::error::TelegramError;
use gpui_telegram_core::types::UserId;
use gpui_telegram_server::db::schema::UserRecord;
use gpui_telegram_server::db::{ServerDatabase, SqlitePool};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn test_concurrent_readers_during_active_write_lock() {
    let tmp = tempfile::tempdir().expect("Failed to create tempdir");
    let db_path = tmp.path().join("concurrency_test.db");

    let pool = SqlitePool::open_with_capacity(&db_path, 16).expect("Failed to open SqlitePool");

    // Initialize test table on writer
    pool.with_writer(|conn| {
        conn.execute_batch(
            r#"
            CREATE TABLE test_counter (
                id INTEGER PRIMARY KEY,
                val INTEGER NOT NULL
            );
            INSERT INTO test_counter (id, val) VALUES (1, 100);
            "#,
        )
        .map_err(|e| TelegramError::Database(e.to_string()))
    })
    .expect("Failed to create test table");

    let writer_held = Arc::new(AtomicBool::new(false));
    let writer_finished = Arc::new(AtomicBool::new(false));
    let writer_held_clone = writer_held.clone();
    let writer_finished_clone = writer_finished.clone();
    let writer_pool = pool.clone();

    // Spawn a writer thread that holds exclusive transaction for 250ms
    let writer_thread = std::thread::spawn(move || {
        let res = writer_pool.with_transaction(|tx| {
            writer_held_clone.store(true, Ordering::SeqCst);
            // Sleep while holding active write transaction
            std::thread::sleep(Duration::from_millis(250));
            tx.execute("UPDATE test_counter SET val = 200 WHERE id = 1", [])
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            Ok(())
        });
        writer_finished_clone.store(true, Ordering::SeqCst);
        res
    });

    // Wait until writer actually holds the lock
    while !writer_held.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }

    // Spawn 16 concurrent readers while writer is actively holding the lock
    let reader_success_count = Arc::new(AtomicUsize::new(0));
    let reader_latencies = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut reader_handles = Vec::new();

    for _ in 0..16 {
        let reader_pool = pool.clone();
        let success_ctr = reader_success_count.clone();
        let latencies = reader_latencies.clone();
        let w_done = writer_finished.clone();

        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            let val_res = reader_pool.with_reader(|conn| {
                conn.query_row("SELECT val FROM test_counter WHERE id = 1", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|e| TelegramError::Database(e.to_string()))
            });
            let elapsed = start.elapsed();

            match val_res {
                Ok(v) => {
                    // While writer was sleeping, readers observe snapshot 100
                    if !w_done.load(Ordering::SeqCst) {
                        assert_eq!(v, 100);
                    }
                    success_ctr.fetch_add(1, Ordering::SeqCst);
                    latencies.lock().push(elapsed);
                }
                Err(e) => {
                    panic!("Reader failed with error: {e}");
                }
            }
        });
        reader_handles.push(handle);
    }

    for h in reader_handles {
        h.join().expect("Reader thread panicked");
    }

    writer_thread.join().expect("Writer thread panicked").expect("Writer failed");

    assert_eq!(reader_success_count.load(Ordering::SeqCst), 16);

    let lats = reader_latencies.lock();
    let avg_ms = lats.iter().map(|d| d.as_millis()).sum::<u128>() as f64 / lats.len() as f64;
    // Readers should not be starved by the 250ms write lock (average latency well below 100ms)
    assert!(avg_ms < 100.0, "Average reader latency was too high: {avg_ms}ms");

    // Post-commit: reader observes updated value 200
    let final_val = pool
        .with_reader(|conn| {
            conn.query_row("SELECT val FROM test_counter WHERE id = 1", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(final_val, 200);
}

#[test]
fn test_with_transaction_atomic_rollback() {
    let pool = SqlitePool::open_in_memory().expect("Failed to open in-memory pool");

    pool.with_writer(|conn| {
        conn.execute_batch("CREATE TABLE items (id INT PRIMARY KEY, name TEXT);")
            .map_err(|e| TelegramError::Database(e.to_string()))
    })
    .unwrap();

    let err_res: gpui_telegram_core::error::Result<()> = pool.with_transaction(|tx| {
        tx.execute("INSERT INTO items (id, name) VALUES (1, 'apple')", [])
            .map_err(|e| TelegramError::Database(e.to_string()))?;

        // Forced error inside transaction
        Err(TelegramError::Database("Forced failure to trigger rollback".into()))
    });

    assert!(err_res.is_err());

    // Verify row was rolled back and does not exist
    let count: i64 = pool
        .with_reader(|conn| {
            conn.query_row("SELECT COUNT(*) FROM items WHERE id = 1", [], |r| {
                r.get(0)
            })
            .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .unwrap();

    assert_eq!(count, 0);
}

#[test]
fn test_reader_pool_queue_resilience() {
    let pool = SqlitePool::open_in_memory_with_capacity(4).unwrap();

    pool.with_writer(|conn| {
        conn.execute_batch("CREATE TABLE stats (k TEXT PRIMARY KEY, v INT); INSERT INTO stats VALUES ('load', 1);")
            .map_err(|e| TelegramError::Database(e.to_string()))
    })
    .unwrap();

    let mut handles = Vec::new();
    let completed = Arc::new(AtomicUsize::new(0));

    // Spawn 32 concurrent readers against a pool of capacity 4 (burst load)
    for _ in 0..32 {
        let p = pool.clone();
        let c = completed.clone();
        handles.push(std::thread::spawn(move || {
            let v: i64 = p
                .with_reader(|conn| {
                    conn.query_row("SELECT v FROM stats WHERE k = 'load'", [], |r| r.get(0))
                        .map_err(|e| TelegramError::Database(e.to_string()))
                })
                .unwrap();
            assert_eq!(v, 1);
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(completed.load(Ordering::SeqCst), 32);
}

#[test]
fn test_wal_pragmas_verification() {
    let pool = SqlitePool::open_in_memory().unwrap();

    // Reader connection has query_only = 1
    let query_only: i64 = pool
        .with_reader(|conn| {
            conn.query_row("PRAGMA query_only;", [], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(query_only, 1);

    // Any write query attempted on reader connection must fail at SQLite engine level
    let write_res = pool.with_reader(|conn| {
        conn.execute("CREATE TABLE should_fail (id INT);", [])
            .map_err(|e| TelegramError::Database(e.to_string()))
    });

    assert!(write_res.is_err(), "Writer operation should have been rejected on query_only connection");
}

#[test]
fn test_in_memory_concurrent_readers_during_active_write_tx() {
    let pool = SqlitePool::open_in_memory_with_capacity(16).expect("open in-memory pool");
    pool.with_writer(|conn| {
        conn.execute_batch("CREATE TABLE kv (k TEXT PRIMARY KEY, v INT); INSERT INTO kv VALUES ('a', 10);")
            .map_err(|e| TelegramError::Database(e.to_string()))
    })
    .unwrap();

    let held = Arc::new(AtomicBool::new(false));
    let held_w = held.clone();
    let pool_w = pool.clone();

    let writer = std::thread::spawn(move || {
        pool_w
            .with_transaction(|tx| {
                held_w.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(250));
                tx.execute("UPDATE kv SET v = 20 WHERE k = 'a'", [])
                    .map_err(|e| TelegramError::Database(e.to_string()))?;
                Ok(())
            })
            .unwrap();
    });

    while !held.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }

    let ok_count = Arc::new(AtomicUsize::new(0));
    let mut readers = Vec::new();
    for _ in 0..20 {
        let p = pool.clone();
        let c = ok_count.clone();
        readers.push(std::thread::spawn(move || {
            let v: i64 = p
                .with_reader(|conn| {
                    conn.query_row("SELECT v FROM kv WHERE k = 'a'", [], |r| r.get(0))
                        .map_err(|e| TelegramError::Database(e.to_string()))
                })
                .expect("in-memory WAL reader should not fail with SQLITE_LOCKED");
            assert!(v == 10 || v == 20);
            c.fetch_add(1, Ordering::SeqCst);
        }));
    }

    for r in readers {
        r.join().unwrap();
    }
    writer.join().unwrap();
    assert_eq!(ok_count.load(Ordering::SeqCst), 20);
}

#[test]
fn test_sqlite_open_read_only_prevents_pragma_poisoning() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("ro_guard.db");
    let pool = SqlitePool::open_with_capacity(&db_path, 1).unwrap();

    pool.with_writer(|conn| {
        conn.execute_batch("CREATE TABLE ro_test (id INT PRIMARY KEY, val TEXT); INSERT INTO ro_test VALUES (1, 'init');")
            .map_err(|e| TelegramError::Database(e.to_string()))
    })
    .unwrap();

    // Attempting PRAGMA query_only = OFF + INSERT must fail due to SQLITE_OPEN_READ_ONLY
    let bypass_res = pool.with_reader(|conn| {
        conn.execute_batch("PRAGMA query_only = OFF; INSERT INTO ro_test VALUES (99, 'bypassed');")
            .map_err(|e| TelegramError::Database(e.to_string()))
    });
    assert!(bypass_res.is_err(), "SQLITE_OPEN_READ_ONLY must block writes even after PRAGMA query_only = OFF");

    // Subsequent reader on the same pooled slot must remain read-only and unpoisoned
    let subsequent_write = pool.with_reader(|conn| {
        conn.execute("INSERT INTO ro_test VALUES (100, 'poisoned');", [])
            .map_err(|e| TelegramError::Database(e.to_string()))
    });
    assert!(subsequent_write.is_err());

    let query_only: i64 = pool
        .with_reader(|conn| {
            conn.query_row("PRAGMA query_only;", [], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))
        })
        .unwrap();
    assert_eq!(query_only, 1);
}

#[test]
fn test_nested_with_reader_capacity_one_no_stall() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("nested_cap1.db");
    let pool = SqlitePool::open_with_capacity(&db_path, 1).unwrap();

    let start = Instant::now();
    let sum = pool
        .with_reader(|c1| {
            let a: i64 = c1
                .query_row("SELECT 20", [], |r| r.get(0))
                .map_err(|e| TelegramError::Database(e.to_string()))?;
            let b: i64 = pool.with_reader(|c2| {
                c2.query_row("SELECT 22", [], |r| r.get(0))
                    .map_err(|e| TelegramError::Database(e.to_string()))
            })?;
            Ok(a + b)
        })
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(sum, 42);
    assert!(
        elapsed < Duration::from_millis(200),
        "Nested with_reader on capacity=1 stalled: {elapsed:?}"
    );
}

#[test]
fn test_server_database_reads_unblocked_by_writer_mutex() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("server_db_ro.db");
    let db = Arc::new(ServerDatabase::open(&db_path).unwrap());

    let now_ms = chrono::Utc::now().timestamp_millis();
    db.create_user(&UserRecord {
        user_id: UserId(1001),
        username: Some("alice_ro".into()),
        phone: None,
        first_name: "Alice".into(),
        last_name: None,
        bio: None,
        password_hash: Some("hash".into()),
        is_bot: false,
        is_verified: false,
        is_support: false,
        is_scam: false,
        is_fake: false,
        profile_photo_file_id: None,
        account_generation: 1,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    })
    .unwrap();

    let held = Arc::new(AtomicBool::new(false));
    let held_w = held.clone();
    let db_w = db.clone();

    let writer = std::thread::spawn(move || {
        db_w.pool()
            .with_transaction(|_tx| {
                held_w.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(250));
                Ok(())
            })
            .unwrap();
    });

    while !held.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(5));
    }

    let start = Instant::now();
    let user = db.get_user(UserId(1001)).unwrap().unwrap();
    let elapsed = start.elapsed();
    assert_eq!(user.username.as_deref(), Some("alice_ro"));
    assert!(
        elapsed < Duration::from_millis(100),
        "ServerDatabase::get_user blocked on writer mutex ({elapsed:?})"
    );

    writer.join().unwrap();
}
