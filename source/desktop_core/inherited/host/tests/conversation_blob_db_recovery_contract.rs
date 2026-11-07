use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mahayana_host_runtime::agent_isolation::{
    open_configured_conversation_blob_db, open_conversation_blob_db,
    open_conversation_blob_db_with_options, read_conversation_blob_migration_state,
    run_quick_check, set_conversation_blob_migration_state, ConversationBlobDbOptions,
    ConversationBlobMigrationState, ConversationBlobRecoveryOutcome,
};
use rusqlite::params;
use uuid::Uuid;

fn test_db_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "fabushi-{label}-{}.sqlite",
        Uuid::new_v4()
    ))
}

fn suffix(path: &Path, value: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), value))
}

fn cleanup(path: &Path) {
    let _ = fs::remove_file(path);
    for suffix_value in ["-wal", "-shm", "-journal"] {
        let _ = fs::remove_file(suffix(path, suffix_value));
    }
}

fn cleanup_recovery_family(path: &Path) {
    cleanup(path);
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let prefix = format!("{name}.corrupt-");
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let Some(entry_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if entry_name.starts_with(&prefix)
                || entry_name == format!("{name}.pending")
                || entry_name == format!("{name}.replacement")
            {
                cleanup(&entry.path());
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

#[test]
fn corrupt_source_is_quarantined_and_reset_to_healthy_recovery_db() {
    let path = test_db_path("conversation-recovery-corrupt");
    fs::write(&path, b"definitely-not-sqlite").expect("write corrupt source");

    let db = open_conversation_blob_db(&path, 500).expect("recover corrupt db");
    assert!(run_quick_check(&db).expect("quick check"));
    assert_eq!(
        read_conversation_blob_migration_state(&db).expect("migration state"),
        ConversationBlobMigrationState::RecoveryRebuilt
    );
    drop(db);

    let name = path.file_name().and_then(|name| name.to_str()).unwrap();
    let prefix = format!("{name}.corrupt-");
    let quarantines = fs::read_dir(path.parent().unwrap())
        .expect("read recovery dir")
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|entry_name| {
            entry_name.starts_with(&prefix)
                && !entry_name.ends_with(".pending")
                && !entry_name.ends_with(".intent")
                && !entry_name.ends_with(".replacement")
        })
        .collect::<Vec<_>>();
    assert_eq!(quarantines.len(), 1);

    cleanup_recovery_family(&path);
}

#[test]
fn pending_recovery_resumes_and_salvages_blob_rows() {
    let path = test_db_path("conversation-recovery-resume");
    let quarantine = suffix(&path, ".corrupt-0000000000001");
    let pending = suffix(&quarantine, ".pending");

    {
        let source =
            open_configured_conversation_blob_db(&quarantine, 500).expect("create quarantine");
        source
            .execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                params!["aa", b"salvage-me".as_slice()],
            )
            .expect("seed salvage row");
    }
    fs::write(&pending, b"").expect("write pending marker");

    let db = open_conversation_blob_db(&path, 500).expect("resume recovery");
    assert_eq!(
        db.query_row("SELECT data FROM blobs WHERE id = 'aa'", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .expect("salvaged row"),
        b"salvage-me".to_vec()
    );
    assert_eq!(
        read_conversation_blob_migration_state(&db).expect("migration state"),
        ConversationBlobMigrationState::RecoveryRebuilt
    );
    assert!(!pending.exists());
    assert!(quarantine.exists());
    drop(db);

    cleanup_recovery_family(&path);
}

#[test]
fn completed_replacement_is_installed_before_attempting_resalvage() {
    let path = test_db_path("conversation-recovery-replacement");
    let quarantine = suffix(&path, ".corrupt-0000000000002");
    let pending = suffix(&quarantine, ".pending");
    let replacement = suffix(&quarantine, ".replacement");

    {
        let db =
            open_configured_conversation_blob_db(&replacement, 500).expect("create replacement");
        db.execute(
            "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
            params!["bb", b"completed".as_slice()],
        )
        .expect("seed replacement");
        set_conversation_blob_migration_state(
            &db,
            ConversationBlobMigrationState::RecoveryRebuilt,
        )
        .expect("mark replacement complete");
    }
    fs::write(&pending, b"").expect("write pending marker");

    let db = open_conversation_blob_db(&path, 500).expect("install replacement");
    assert_eq!(
        db.query_row("SELECT data FROM blobs WHERE id = 'bb'", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .expect("installed row"),
        b"completed".to_vec()
    );
    assert!(!pending.exists());
    assert!(!replacement.exists());
    assert_eq!(
        read_conversation_blob_migration_state(&db).expect("migration state"),
        ConversationBlobMigrationState::RecoveryRebuilt
    );
    drop(db);

    cleanup_recovery_family(&path);
}

#[test]
fn recovery_callback_reports_salvage_and_callback_panics_do_not_break_install() {
    let path = test_db_path("conversation-recovery-callback");
    let quarantine = suffix(&path, ".corrupt-0000000000003");
    let pending = suffix(&quarantine, ".pending");

    {
        let source =
            open_configured_conversation_blob_db(&quarantine, 500).expect("create quarantine");
        source
            .execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                params!["cc", b"callback-row".as_slice()],
            )
            .expect("seed salvage row");
    }
    fs::write(&pending, b"").expect("write pending marker");

    let reports = Rc::new(RefCell::new(Vec::new()));
    let reports_for_callback = Rc::clone(&reports);
    let callback = move |info: &mahayana_host_runtime::agent_isolation::ConversationBlobRecoveryInfo| {
        reports_for_callback.borrow_mut().push(info.clone());
    };
    let options = ConversationBlobDbOptions {
        db_path: &path,
        busy_timeout_ms: 500,
        agent_id: "agent-recovery-test",
        log: None,
        on_recovery: Some(&callback),
    };
    let db = open_conversation_blob_db_with_options(&options).expect("recover with callback");
    assert_eq!(reports.borrow().len(), 1);
    assert_eq!(reports.borrow()[0].outcome, ConversationBlobRecoveryOutcome::Recovered);
    assert_eq!(reports.borrow()[0].salvaged_blobs, 1);
    drop(db);

    let path_two = test_db_path("conversation-recovery-callback-panic");
    let quarantine_two = suffix(&path_two, ".corrupt-0000000000004");
    let pending_two = suffix(&quarantine_two, ".pending");
    {
        let source =
            open_configured_conversation_blob_db(&quarantine_two, 500).expect("create quarantine");
        source
            .execute(
                "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
                params!["dd", b"panic-row".as_slice()],
            )
            .expect("seed salvage row");
    }
    fs::write(&pending_two, b"").expect("write pending marker");
    let panic_callback = |_info: &mahayana_host_runtime::agent_isolation::ConversationBlobRecoveryInfo| {
        panic!("callback failure must not invalidate recovery");
    };
    let panic_options = ConversationBlobDbOptions {
        db_path: &path_two,
        busy_timeout_ms: 500,
        agent_id: "agent-recovery-panic",
        log: None,
        on_recovery: Some(&panic_callback),
    };
    let panic_db =
        open_conversation_blob_db_with_options(&panic_options).expect("recovery survives callback panic");
    assert_eq!(
        panic_db
            .query_row("SELECT data FROM blobs WHERE id = 'dd'", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .expect("installed panic row"),
        b"panic-row".to_vec()
    );
    drop(panic_db);

    cleanup_recovery_family(&path);
    cleanup_recovery_family(&path_two);
}
