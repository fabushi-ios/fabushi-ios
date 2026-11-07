use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use mahayana_host_runtime::extensions::box_store_sync::box_store_vacuum_worker::{
    BoxStoreVacuumJob, run_box_store_vacuum_job, spawn_box_store_vacuum_job,
};
use mahayana_host_runtime::extensions::box_store_sync::sqlite_snapshot::{
    LOCKED_DB_COPY_ATTEMPTS, SqliteSnapshotOperation, SqliteSnapshotPathStage,
    copy_locked_sqlite_db, copy_locked_sqlite_db_with_reader, sqlite_vacuum_into,
};
use rusqlite::Connection;
use uuid::Uuid;

fn scratch(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "fabushi-{name}-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&path).expect("create scratch dir");
    path
}

fn seed_db(path: &std::path::Path) {
    let db = Connection::open(path).expect("open seed db");
    db.execute_batch(
        "CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);
         INSERT INTO notes(body) VALUES ('hello');",
    )
    .expect("seed sqlite");
}

fn assert_snapshot(path: &std::path::Path) {
    let db = Connection::open(path).expect("open snapshot");
    let value: String = db
        .query_row("SELECT body FROM notes WHERE id = 1", [], |row| row.get(0))
        .expect("read snapshot row");
    assert_eq!(value, "hello");
    let check: String = db
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .expect("quick check");
    assert_eq!(check, "ok");
}

#[test]
fn vacuum_into_creates_verified_sqlite_snapshot() {
    let root = scratch("sqlite-vacuum");
    let source = root.join("source.sqlite");
    let dest = root.join("dest.sqlite");
    seed_db(&source);
    sqlite_vacuum_into(&source, &dest, 5_000).expect("vacuum into");
    assert_snapshot(&dest);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn locked_copy_preserves_database_and_cleans_staged_sidecars() {
    let root = scratch("locked-copy");
    let source = root.join("source.sqlite");
    let dest = root.join("dest.sqlite");
    seed_db(&source);

    let mut failure = None;
    assert!(copy_locked_sqlite_db(&source, &dest, |value| failure = Some(value)));
    assert!(failure.is_none());
    assert_snapshot(&dest);
    for suffix in ["-wal", "-shm", "-journal"] {
        assert!(!std::path::PathBuf::from(format!("{}{}", dest.display(), suffix)).exists());
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn locked_copy_retries_source_changes_and_reports_final_failure() {
    let root = scratch("locked-copy-source-change");
    let source = root.join("source.sqlite");
    let dest = root.join("dest.sqlite");
    fs::write(&source, b"placeholder").expect("write source");
    let reads = AtomicUsize::new(0);
    let mut final_failure = None;

    let copied = copy_locked_sqlite_db_with_reader(
        &source,
        &dest,
        |_| {
            let call = reads.fetch_add(1, Ordering::SeqCst);
            if call % 2 == 0 {
                Ok(b"first".to_vec())
            } else {
                Ok(b"changed".to_vec())
            }
        },
        |failure| final_failure = Some(failure),
    );

    assert!(!copied);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        LOCKED_DB_COPY_ATTEMPTS * 2
    );
    let failure = final_failure.expect("final failure");
    assert_eq!(
        failure.operation,
        SqliteSnapshotOperation::VerifySourceStability
    );
    assert_eq!(failure.path_stage, SqliteSnapshotPathStage::SourceMain);
    assert_eq!(failure.cause, "source_changed");
    assert!(!dest.exists());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn vacuum_worker_returns_structured_success_and_failure() {
    let root = scratch("vacuum-worker");
    let source = root.join("source.sqlite");
    seed_db(&source);

    let dest = root.join("worker.sqlite");
    let success = spawn_box_store_vacuum_job(BoxStoreVacuumJob {
        src_path: source.clone(),
        dest_path: dest.clone(),
        busy_timeout_ms: 5_000,
    })
    .expect("spawn worker")
    .join()
    .expect("join worker");
    assert!(success.ok);
    assert!(success.message.is_none());
    assert_snapshot(&dest);

    let failed = run_box_store_vacuum_job(&BoxStoreVacuumJob {
        src_path: root.join("missing.sqlite"),
        dest_path: root.join("missing-out.sqlite"),
        busy_timeout_ms: 5_000,
    });
    assert!(!failed.ok);
    assert!(failed.message.as_deref().is_some_and(|message| !message.is_empty()));

    let _ = fs::remove_dir_all(root);
}
