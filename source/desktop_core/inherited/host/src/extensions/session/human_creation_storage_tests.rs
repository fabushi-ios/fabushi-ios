use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{SandAgentDb, SandAgentDbOptions, encode_hex};

const WAIT: Duration = Duration::from_secs(20);

fn test_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("human-creation-storage-{label}-{}", Uuid::new_v4()))
}

fn metadata(title: &str, created_at: u64) -> Value {
    json!({
        "conversationKind": "human",
        "conversationId": "human-direct-storage-fixture",
        "participantIds": ["human-local", "human-peer"],
        "title": title,
        "createdAt": created_at,
        "lastActivityAt": created_at + 1,
        "ancillaryState": {"ownerTitle": title, "mustSurvive": true},
    })
}

fn open_observed_owner(path: &Path, busy_count: Arc<AtomicUsize>) -> SandAgentDb {
    let mut options = SandAgentDbOptions::default();
    options.recovery.busy_timeout_ms = 0;
    options.on_busy_error = Some(Arc::new(move |operation, message| {
        assert_eq!(operation, "initializeHumanMetadata");
        assert!(!message.is_empty());
        busy_count.fetch_add(1, Ordering::SeqCst);
    }));
    SandAgentDb::open_with_options_and_seed(path, options, false).unwrap()
}

fn open_existing_writer(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap()
}

struct InsertBusyPause {
    reached: mpsc::Sender<i32>,
    resume: mpsc::Receiver<()>,
}

thread_local! {
    // rusqlite's function-pointer callback runs on the connection's calling
    // thread. Only the dedicated contender thread installs this owned state;
    // no callback state is shared between threads, connections, or tests.
    static INSERT_BUSY_PAUSE: Cell<Option<InsertBusyPause>> = const { Cell::new(None) };
}

fn wait_for_writer_commit(attempt: i32) -> bool {
    INSERT_BUSY_PAUSE.try_with(|slot| {
        let Some(pause) = slot.take() else {
            return false;
        };
        // Fail closed on channel loss or timeout, without panicking across the
        // SQLite callback. Taking the state also bounds this to one retry.
        pause.reached.send(attempt).is_ok() && pause.resume.recv_timeout(WAIT).is_ok()
    }).unwrap_or(false)
}

struct ScopedInsertBusyHandler<'a> {
    owner: &'a SandAgentDb,
}

impl<'a> ScopedInsertBusyHandler<'a> {
    fn install(owner: &'a SandAgentDb, pause: InsertBusyPause) -> Self {
        let scope = Self { owner };
        INSERT_BUSY_PAUSE.with(|slot| assert!(slot.replace(Some(pause)).is_none()));
        owner.connection.lock().unwrap().as_mut().unwrap()
            .busy_handler(Some(wait_for_writer_commit)).unwrap();
        scope
    }
}

impl Drop for ScopedInsertBusyHandler<'_> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.owner.connection.lock() {
            if let Some(db) = guard.as_mut() {
                let _ = db.busy_handler(None::<fn(i32) -> bool>);
            }
        }
        let _ = INSERT_BUSY_PAUSE.try_with(|slot| slot.set(None));
    }
}

#[test]
fn distinct_handles_initialize_one_complete_metadata_winner_and_preserve_it_on_reopen() {
    let root = test_root("distinct-handles");
    let path = root.join("store.db");
    let owners = [
        Arc::new(SandAgentDb::open_human_conversation_store(&path, 1000).unwrap()),
        Arc::new(SandAgentDb::open_human_conversation_store(&path, 1000).unwrap()),
    ];
    let candidates = [metadata("First contender", 11), metadata("Second contender", 29)];
    for owner in &owners {
        assert_eq!(owner.read_kv("metadata").unwrap(), None);
        owner.connection.lock().unwrap().as_ref().unwrap()
            .pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    }
    // The empty schema is durable before the contenders write. Keep both
    // connections open and automatic checkpoints disabled so the winning
    // identity is committed through a live WAL when another creator opens.
    let checkpoint = owners[0].connection.lock().unwrap().as_ref().unwrap()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get::<_, i64>(0))
        .unwrap();
    assert_eq!(checkpoint, 0);
    let start = Arc::new(Barrier::new(3));
    let (finished, results) = mpsc::channel();
    let mut tasks = Vec::new();
    for (index, owner) in owners.iter().enumerate() {
        let owner = Arc::clone(owner);
        let candidate = candidates[index].clone();
        let start = Arc::clone(&start);
        let finished = finished.clone();
        tasks.push(thread::spawn(move || {
            start.wait();
            let changed = owner.initialize_human_metadata_if_empty(candidate).unwrap();
            finished.send((index, changed)).unwrap();
        }));
    }
    drop(finished);
    start.wait();
    let mut inserted = [false; 2];
    for _ in 0..2 {
        let (index, changed) = results.recv_timeout(WAIT).expect("both SQLite creators finish");
        inserted[index] = changed;
    }
    for task in tasks {
        task.join().expect("SQLite creator did not panic");
    }
    assert_eq!(usize::from(inserted[0]) + usize::from(inserted[1]), 1);
    let winner = &candidates[usize::from(!inserted[0])];
    let winner_raw = encode_hex(&serde_json::to_vec(winner).unwrap());
    for owner in &owners {
        assert_eq!(owner.read_kv("metadata").unwrap(), Some(winner_raw.clone()));
        assert_eq!(owner.read_metadata().unwrap(), *winner);
        assert!(!owner.initialize_human_metadata_if_empty(metadata("Retry title", 99)).unwrap());
        assert_eq!(owner.read_kv("metadata").unwrap(), Some(winner_raw.clone()));
    }
    assert!(fs::metadata(root.join("store.db-wal")).unwrap().len() > 32);
    assert!(fs::metadata(root.join("store.db-shm")).unwrap().len() > 0);
    let wal_reader = SandAgentDb::open_human_conversation_store(&path, 1000).unwrap();
    assert_eq!(wal_reader.read_kv("metadata").unwrap(), Some(winner_raw.clone()));
    assert_eq!(wal_reader.read_metadata().unwrap(), *winner);
    assert!(!wal_reader.initialize_human_metadata_if_empty(metadata("Live WAL retry", 99)).unwrap());
    assert_eq!(wal_reader.read_kv("metadata").unwrap(), Some(winner_raw.clone()));
    drop(wal_reader);
    drop(owners);
    let reopened = SandAgentDb::open_human_conversation_store(&path, 1000).unwrap();
    assert_eq!(reopened.read_kv("metadata").unwrap(), Some(winner_raw.clone()));
    assert!(!reopened.initialize_human_metadata_if_empty(metadata("Restart title", 100)).unwrap());
    assert_eq!(reopened.read_kv("metadata").unwrap(), Some(winner_raw));
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn held_metadata_transaction_preserves_first_commit_in_the_waiting_initializer() {
    let root = test_root("held-metadata");
    let path = root.join("store.db");
    let busy_count = Arc::new(AtomicUsize::new(0));
    let contender = open_observed_owner(&path, Arc::clone(&busy_count));
    let winner = metadata("Committed first writer", 41);
    let winner_raw = encode_hex(&serde_json::to_vec(&winner).unwrap());
    let mut writer = open_existing_writer(&path);
    let transaction = writer.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    assert_eq!(transaction.execute(
        "INSERT INTO kv (key, value) VALUES ('metadata', ?1)",
        params![winner_raw],
    ).unwrap(), 1);

    // The first writer owns SQLite's real write lock, and its identity remains
    // invisible to the distinct contender connection.
    assert_eq!(contender.read_kv("metadata").unwrap(), None);
    let (reached, waiting) = mpsc::channel();
    let (resume, committed) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    let task = thread::spawn(move || {
        let pause = ScopedInsertBusyHandler::install(
            &contender, InsertBusyPause { reached, resume: committed },
        );
        let initialized = contender.initialize_human_metadata_if_empty(metadata("Waiting contender", 50));
        drop(pause);
        let changed = initialized.unwrap();
        let persisted = contender.read_kv("metadata").unwrap();
        finished.send((changed, persisted)).unwrap();
    });

    // This observation comes from inside SQLite while INSERT is waiting for
    // the held write lock. Commit only then, and resume the SAME initializer.
    assert_eq!(waiting.recv_timeout(WAIT).expect("contender reaches SQLite's write-lock wait"), 0);
    assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
    transaction.commit().unwrap();
    resume.send(()).expect("release the waiting SQLite INSERT after the winner commits");
    let (changed, persisted) = result.recv_timeout(WAIT).expect("the same initializer finishes");
    task.join().expect("contending initializer did not panic");

    // A split SELECT-then-UPSERT would now overwrite the winner and return
    // true. The atomic statement must instead observe the committed identity.
    assert!(!changed);
    assert_eq!(busy_count.load(Ordering::SeqCst), 0, "SQLite resumed instead of returning busy");
    assert_eq!(persisted, Some(winner_raw.clone()));
    assert_eq!(writer.query_row(
        "SELECT value FROM kv WHERE key = 'metadata'", [], |row| row.get::<_, String>(0),
    ).unwrap(), winner_raw);
    assert_eq!(writer.query_row("SELECT COUNT(*) FROM kv", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    drop(writer);
    let reopened = SandAgentDb::open_conversation_store(&path, 1000).unwrap();
    assert_eq!(reopened.read_kv("metadata").unwrap(), Some(winner_raw));
    assert_eq!(reopened.read_metadata().unwrap(), winner);
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn held_durable_row_transactions_prevent_missing_metadata_adoption_after_commit() {
    let fixtures = [
        (
            "kv",
            "INSERT INTO kv (key, value) VALUES ('retained-state', 'must survive')",
            "SELECT value FROM kv WHERE key = 'retained-state'",
            "must survive",
            (1, 0, 0),
        ),
        (
            "transcript",
            r#"INSERT INTO transcript_entries (id, entry) VALUES ('retained-message', '{"kind":"message","content":"must survive"}')"#,
            "SELECT entry FROM transcript_entries WHERE id = 'retained-message'",
            r#"{"kind":"message","content":"must survive"}"#,
            (0, 1, 0),
        ),
        (
            "blob",
            "INSERT INTO blobs (id, data) VALUES ('retained-blob', X'000102FF')",
            "SELECT hex(data) FROM blobs WHERE id = 'retained-blob'",
            "000102FF",
            (0, 0, 1),
        ),
    ];
    for (label, insert, select, expected_value, expected_counts) in fixtures {
        let root = test_root(label);
        let path = root.join("store.db");
        let busy_count = Arc::new(AtomicUsize::new(0));
        let contender = open_observed_owner(&path, Arc::clone(&busy_count));
        let mut writer = open_existing_writer(&path);
        let transaction = writer.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        assert_eq!(transaction.execute(insert, []).unwrap(), 1);
        assert_eq!(contender.read_kv("metadata").unwrap(), None);
        assert!(!contender.initialize_human_metadata_if_empty(metadata("Blocked identity", 71)).unwrap());
        assert_eq!(busy_count.load(Ordering::SeqCst), 1, "{label} owns the SQLite write lock");
        transaction.commit().unwrap();

        // The retry must re-evaluate every durable table at its write boundary;
        // missing metadata does not authorize adopting the committed row.
        assert!(!contender.initialize_human_metadata_if_empty(metadata("Refused identity", 72)).unwrap());
        assert_eq!(busy_count.load(Ordering::SeqCst), 1);
        assert_eq!(contender.read_kv("metadata").unwrap(), None);
        assert_eq!(writer.query_row(select, [], |row| row.get::<_, String>(0)).unwrap(), expected_value);
        let counts = writer.query_row(
            "SELECT (SELECT COUNT(*) FROM kv),
                    (SELECT COUNT(*) FROM transcript_entries),
                    (SELECT COUNT(*) FROM blobs)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        ).unwrap();
        assert_eq!(counts, expected_counts);
        drop(writer);
        drop(contender);
        fs::remove_dir_all(root).unwrap();
    }
}
