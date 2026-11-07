use std::path::Path;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::{
    agents::settings_file::{get_sand_settings_path, read_sand_settings_file, write_sand_settings_file, DEFAULT_HIDDEN_FROM_SIDEBAR, DEFAULT_NOTIFY_ON_AGENT_UPDATES},
    extensions::{
        box_store_sync::{box_store_diagnostics::{pin_box_store_diagnostics_reporter, report_box_store_diagnostic}, box_store_sync_error::SandBoxStoreSyncError},
        cloud_agents::cloud_agent_launch_error::SandCloudAgentLaunchError,
        session::conversation_blobs_path::conversation_blobs_path,
        transcript::{channel_delivery_unregistered_error::SandChannelDeliveryUnregisteredError, send_not_persisted_error::SandSendNotPersistedError},
    },
    notify_drain_gate::NotifyDrainGate,
    storage::{
        agent_paths::{assert_valid_sand_agent_id, resolve_sand_agent_dir},
        sqlite_busy::{
            is_sqlite_busy_error, is_sqlite_cant_open_error, is_sqlite_corrupt_error,
            is_sqlite_io_error, retry_sqlite_busy_with_delay, RetrySqliteBusyOptions,
            SQLITE_BUSY, SQLITE_CANTOPEN, SQLITE_CORRUPT, SQLITE_IOERR,
        },
        sqlite_recovery::{
            copy_salvageable_sqlite_rows, open_sqlite_for_salvage,
            quarantine_corrupt_sqlite_db, remove_sqlite_sidecars,
        },
        store_db::{
            bump_db_write_generation, checkpoint_sand_agent_db,
            delete_sand_agent_db_write_generation, get_sand_agent_db_write_generation,
            has_live_sand_agent_db_handle, live_db_handle_count, register_live_db_handle,
            release_live_db_handle, wal_frames_fully_folded, DB_BUSY_TIMEOUT_MS,
            SQLITE_DB_SIDECAR_SUFFIXES,
        },
    },
};
use serde_json::{Map, Value};
use uuid::Uuid;

#[test]
fn notify_drain_gate_matches_notify_floor_and_safety_poll_semantics() {
    use std::cell::Cell;
    let now = Cell::new(1_000_u64);
    let mut gate = NotifyDrainGate::new(|| now.get(), || true, || true);
    assert!(gate.should_drain(false));
    gate.record_poll();
    assert!(!gate.should_drain(false));
    gate.record_notify();
    now.set(4_999);
    assert!(!gate.should_drain(false));
    now.set(5_000);
    assert!(gate.should_drain(false));
}

#[test]
fn agent_paths_reject_traversal_and_whitespace() {
    assert!(assert_valid_sand_agent_id("agent-1").is_ok());
    for bad in ["../x", " x", "x ", "a/b", "."] { assert!(assert_valid_sand_agent_id(bad).is_err()); }
    let dir = resolve_sand_agent_dir("agent-1", Some(Path::new("/tmp/home"))).unwrap();
    assert_eq!(dir.file_name().unwrap(), "agent-1");
}

#[test]
fn settings_file_uses_defaults_preserves_unknown_fields_and_atomic_replace() {
    let root = std::env::temp_dir().join(format!("fabushi-settings-{}", Uuid::new_v4()));
    let path = get_sand_settings_path(&root);
    let defaults = read_sand_settings_file(&path);
    assert_eq!(defaults.notify_on_agent_updates, DEFAULT_NOTIFY_ON_AGENT_UPDATES);
    assert_eq!(defaults.hidden_from_sidebar, DEFAULT_HIDDEN_FROM_SIDEBAR);
    let mut first = Map::new(); first.insert("unknown".into(), Value::String("keep".into())); first.insert("hiddenFromSidebar".into(), Value::Bool(true));
    write_sand_settings_file(&path, &first).unwrap();
    let mut update = Map::new(); update.insert("notifyOnAgentUpdates".into(), Value::Bool(false));
    write_sand_settings_file(&path, &update).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"unknown\": \"keep\""));
    let settings = read_sand_settings_file(&path);
    assert!(settings.hidden_from_sidebar);
    assert!(!settings.notify_on_agent_updates);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn session_and_error_contracts_match_reference_messages() {
    assert_eq!(conversation_blobs_path("/tmp/agent/store.db"), Path::new("/tmp/agent/conversation-blobs.db"));
    assert_eq!(SandChannelDeliveryUnregisteredError.to_string(), "No channel delivery mechanism is registered.");
    assert!(SandSendNotPersistedError.to_string().contains("client retry is not swallowed"));
    assert_eq!(SandBoxStoreSyncError::new("sync failed").to_string(), "sync failed");
    assert_eq!(SandCloudAgentLaunchError::new("launch failed").to_string(), "launch failed");
}

#[test]
fn box_store_diagnostics_reporter_can_be_pinned_and_cleared() {
    let seen = Arc::new(Mutex::new(0_u32));
    let sink = Arc::clone(&seen);
    pin_box_store_diagnostics_reporter(Some(Arc::new(move |_| *sink.lock().unwrap() += 1)));
    report_box_store_diagnostic(&Map::new());
    pin_box_store_diagnostics_reporter(None);
    assert_eq!(*seen.lock().unwrap(), 1);
}


#[test]
fn sqlite_busy_classification_and_retry_match_reference() {
    fn sqlite_failure(code: i32, message: &str) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some(message.to_string()),
        )
    }

    assert!(is_sqlite_busy_error(&sqlite_failure(
        SQLITE_BUSY,
        "database is locked"
    )));
    assert!(is_sqlite_io_error(&sqlite_failure(
        SQLITE_IOERR | (3 << 8),
        "short read"
    )));
    assert!(is_sqlite_cant_open_error(&sqlite_failure(
        SQLITE_CANTOPEN,
        "unable to open database file"
    )));
    assert!(is_sqlite_corrupt_error(&sqlite_failure(
        SQLITE_CORRUPT,
        "database disk image is malformed"
    )));

    let mut attempts = 0_u32;
    let mut delays = Vec::new();
    let value = retry_sqlite_busy_with_delay(
        || {
            attempts += 1;
            if attempts < 3 {
                Err(sqlite_failure(SQLITE_BUSY, "database is locked"))
            } else {
                Ok("ready")
            }
        },
        RetrySqliteBusyOptions {
            attempts: 5,
            base_delay_ms: 10,
        },
        |delay_ms| delays.push(delay_ms),
    )
    .expect("busy retry should eventually succeed");
    assert_eq!(value, "ready");
    assert_eq!(attempts, 3);
    assert_eq!(delays, vec![10, 20]);
}

#[test]
fn sqlite_recovery_quarantines_salvages_rows_and_store_db_tracks_handles() {
    assert_eq!(DB_BUSY_TIMEOUT_MS, 5_000);
    assert_eq!(SQLITE_DB_SIDECAR_SUFFIXES, ["-wal", "-shm", "-journal"]);

    let root = std::env::temp_dir().join(format!("fabushi-sqlite-storage-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create storage root");
    let db_path = root.join("agent.sqlite");
    {
        let db = rusqlite::Connection::open(&db_path).expect("create sqlite db");
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO items(value) VALUES ('a'), ('b');",
        )
        .expect("seed sqlite db");
    }

    let salvage = open_sqlite_for_salvage(&db_path, Some(DB_BUSY_TIMEOUT_MS))
        .expect("salvage should open a valid database");
    let destination = rusqlite::Connection::open_in_memory().expect("open destination");
    destination
        .execute_batch("CREATE TABLE items (id INTEGER PRIMARY KEY, value TEXT NOT NULL)")
        .expect("create destination table");
    let copied = copy_salvageable_sqlite_rows(
        &salvage,
        "SELECT id, value FROM items ORDER BY id",
        |row| {
            destination.execute(
                "INSERT INTO items(id, value) VALUES (?1, ?2)",
                rusqlite::params![row.get::<_, i64>(0)?, row.get::<_, String>(1)?],
            )?;
            Ok(())
        },
    );
    assert_eq!(copied, 2);
    drop(salvage);
    assert!(checkpoint_sand_agent_db(&db_path, DB_BUSY_TIMEOUT_MS));

    assert_eq!(get_sand_agent_db_write_generation(&db_path), 0);
    bump_db_write_generation(&db_path);
    bump_db_write_generation(&db_path);
    assert_eq!(get_sand_agent_db_write_generation(&db_path), 2);
    delete_sand_agent_db_write_generation(&db_path);
    assert_eq!(get_sand_agent_db_write_generation(&db_path), 0);

    assert_eq!(live_db_handle_count(&db_path), 0);
    register_live_db_handle(&db_path);
    register_live_db_handle(&db_path);
    assert!(has_live_sand_agent_db_handle(&db_path));
    assert_eq!(live_db_handle_count(&db_path), 2);
    release_live_db_handle(&db_path);
    release_live_db_handle(&db_path);
    assert!(!has_live_sand_agent_db_handle(&db_path));
    assert_eq!(wal_frames_fully_folded(3, 3), Some(true));
    assert_eq!(wal_frames_fully_folded(4, 3), Some(false));
    assert_eq!(wal_frames_fully_folded(-1, 0), None);

    let corrupt = root.join("corrupt.sqlite");
    let quarantine = root.join("quarantine.sqlite");
    std::fs::write(&corrupt, b"corrupt").expect("write corrupt source");
    std::fs::write(format!("{}-wal", corrupt.display()), b"wal")
        .expect("write source wal");
    let result = quarantine_corrupt_sqlite_db(
        &corrupt,
        Some(&quarantine),
        false,
        2,
        1,
    );
    assert_eq!(result.quarantine_path.as_deref(), Some(quarantine.as_path()));
    assert!(!result.copied);
    assert!(quarantine.exists());
    assert!(!corrupt.exists());

    std::fs::write(format!("{}-wal", db_path.display()), b"wal")
        .expect("write temporary wal");
    std::fs::write(format!("{}-shm", db_path.display()), b"shm")
        .expect("write temporary shm");
    remove_sqlite_sidecars(&db_path);
    assert!(!Path::new(&format!("{}-wal", db_path.display())).exists());
    assert!(!Path::new(&format!("{}-shm", db_path.display())).exists());

    drop(destination);
    let _ = std::fs::remove_dir_all(root);
}
