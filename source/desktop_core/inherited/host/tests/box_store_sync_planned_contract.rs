use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use mahayana_host_runtime::extensions::box_store_sync::agent_store_sand_files::{
    AgentStoreMutableWriteEtags, PRESIGN_READ_BATCH_MAX, batch_read_paths,
    is_conditional_write_rejection, normalize_rel_path,
};
use mahayana_host_runtime::extensions::box_store_sync::box_copy_in::{
    BOX_COPY_IN_EXIT_FAILED, BOX_COPY_IN_EXIT_HYDRATED, BOX_COPY_IN_EXIT_NOOP,
    COPY_IN_HYDRATE_ATTEMPTS, CopyInMeteredOutcome, CopyInOutcome,
    classify_copy_in_metered_outcome, empty, execute_box_copy_in_from_env, outcome_to_exit_code,
    resolve_copy_in_attempts, run_local_box_copy_in,
};
use mahayana_host_runtime::extensions::box_store_sync::box_object_store::{
    BoxObjectStore, LocalFsObjectStore, is_under_prefix,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_download::{
    BoxStoreByteBudget, is_critical_rel_path, resolve_restore_destination,
    symlink_target_stays_within_root,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest::{
    BoxStoreManifestRevisionState, count_agent_dir_manifest_entries,
    count_store_db_manifest_entries, has_manifest_path_conflict, set_manifest_entry,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest_format::BoxStoreManifestEntry;
use mahayana_host_runtime::extensions::box_store_sync::box_store_pack_pipeline::{
    BoxStorePackCategory, resolve_local_path_for_rel_path, should_build_pack,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_sync::{
    BoxStoreCycleSummary, BoxStoreWriterLock, box_store_sync_cycle_telemetry,
    evaluate_box_store_flush,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_sync_service::{
    BOX_HOME_REL_PREFIX, build_box_home_category, is_better_cli_home_enabled,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_transfer::{
    CategoryTransferSummary, FileOutcome, TransferSummary, dedupe_nested_roots,
    glob_matches_path, tally_file_outcome,
};
use mahayana_host_runtime::extensions::box_store_sync::sand_box_store_files::{
    plan_sand_box_store_multipart_parts_from_bytes, should_use_multipart,
};
use mahayana_host_runtime::extensions::box_store_sync::store_db_bundle_capture::{
    AgentDbCaptureQueues, StoreDbCaptureFailurePhase, create_store_db_capture_trace,
    record_store_db_capture_failure,
};
use mahayana_host_runtime::extensions::box_store_sync::store_db_capture::{
    BoxStoreDbCaptureTelemetrySummary, StoreDbCaptureOutcome,
    aggregate_store_db_sweep_outcome, box_store_db_capture_telemetry,
    box_store_db_capture_telemetry_level,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-box-store-planned-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn agent_store_path_and_etag_contracts_are_fail_closed() {
    assert_eq!(normalize_rel_path(r"a\b/./c").unwrap(), "a/b/c");
    assert!(normalize_rel_path("../escape").is_err());
    assert!(normalize_rel_path("C:\\escape").is_err());
    assert!(normalize_rel_path("CON/file").is_err());
    assert!(is_conditional_write_rejection(409));
    assert!(is_conditional_write_rejection(412));
    assert!(!is_conditional_write_rejection(500));

    let batches =
        batch_read_paths((0..(PRESIGN_READ_BATCH_MAX + 1)).map(|n| format!("k/{n}")));
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0].len(), PRESIGN_READ_BATCH_MAX);

    let mut etags = AgentStoreMutableWriteEtags::default();
    etags.set("store", "manifest.json", "etag");
    assert_eq!(etags.get("store", "manifest.json"), Some("etag"));
    etags.delete("store", "manifest.json");
    assert_eq!(etags.get("store", "manifest.json"), None);
}

#[test]
fn copy_in_resolution_preserves_frozen_defaults_and_exit_semantics() {
    assert_eq!(resolve_copy_in_attempts(None), COPY_IN_HYDRATE_ATTEMPTS);
    assert_eq!(resolve_copy_in_attempts(Some("3")), 3);
    assert_eq!(outcome_to_exit_code(CopyInOutcome::Hydrated), 10);
    let noop = empty("missing manifest");
    assert_eq!(
        classify_copy_in_metered_outcome(&noop),
        CopyInMeteredOutcome::Empty
    );
}



#[test]
fn shipping_copy_in_bootstrap_restores_local_store_and_fails_closed_for_remote() {
    let root = temp_root("copy-in-shipping");
    let object_root = root.join("objects");
    let target_root = root.join("target");
    let store = LocalFsObjectStore::new(object_root.join("store-a"));
    let payload = b"durable-agent-store";
    let sha = format!("{:x}", Sha256::digest(payload));
    store.put(&format!("blobs/{sha}"), payload).expect("seed blob");
    let manifest = serde_json::json!({
        "version": 2, "updatedAtMs": 1, "writerWindowId": "test", "fullyHydrated": true,
        "entries": { "home/box/sand-data/agents/agent-a/store.db": {
            "kind": "file", "sha": sha, "size": payload.len(), "mode": 384
        }}
    });
    store.put("manifest.json", &serde_json::to_vec(&manifest).unwrap()).expect("seed manifest");
    let mut env = BTreeMap::from([
        ("SAND_BOX_STORE_COPY_IN".to_string(), "1".to_string()),
        ("SAND_BOX_STORE_ID".to_string(), "store-a".to_string()),
        ("SAND_BOX_STORE_LOCAL_DIR".to_string(), object_root.to_string_lossy().into_owned()),
    ]);
    assert_eq!(execute_box_copy_in_from_env(&env, &target_root), BOX_COPY_IN_EXIT_HYDRATED);
    assert_eq!(fs::read(target_root.join("home/box/sand-data/agents/agent-a/store.db")).unwrap(), payload);
    env.remove("SAND_BOX_STORE_LOCAL_DIR");
    assert_eq!(execute_box_copy_in_from_env(&env, &target_root), BOX_COPY_IN_EXIT_FAILED);
    env.insert("SAND_BOX_STORE_COPY_IN".into(), "0".into());
    assert_eq!(execute_box_copy_in_from_env(&env, &target_root), BOX_COPY_IN_EXIT_NOOP);
}

#[test]
fn local_copy_in_rejects_corrupt_blob_instead_of_claiming_hydration() {
    let root = temp_root("copy-in-corrupt");
    let store = LocalFsObjectStore::new(root.join("objects"));
    let expected = b"expected";
    let sha = format!("{:x}", Sha256::digest(expected));
    store.put(&format!("blobs/{sha}"), b"corrupt!").unwrap();
    let manifest = serde_json::json!({
        "version": 2, "updatedAtMs": 1, "fullyHydrated": true,
        "entries": { "home/box/data.txt": {
            "kind": "file", "sha": sha, "size": expected.len(), "mode": 384
        }}
    });
    store.put("manifest.json", &serde_json::to_vec(&manifest).unwrap()).unwrap();
    let result = run_local_box_copy_in(&store, &root.join("target"));
    assert_eq!(result.outcome, CopyInOutcome::Failed);
    assert_eq!(result.verified, 0);
    assert!(!result.failures.is_empty());
}

#[test]
fn local_object_store_is_scoped_and_round_trips_bytes() {
    let root = temp_root("object-store");
    let store = LocalFsObjectStore::new(&root);
    store.put("blobs/a", b"alpha").unwrap();
    store.put("blobs/b", b"beta").unwrap();
    assert_eq!(store.get("blobs/a").unwrap(), Some(b"alpha".to_vec()));
    assert_eq!(store.list("blobs").unwrap(), vec!["blobs/a", "blobs/b"]);
    assert!(store.put("../escape", b"x").is_err());
    assert!(is_under_prefix("blobs/a", "blobs"));
    store.delete("blobs/a").unwrap();
    assert_eq!(store.get("blobs/a").unwrap(), None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn restore_paths_and_symlinks_cannot_escape_target_root() {
    let root = temp_root("restore");
    assert_eq!(
        resolve_restore_destination(&root, "home/box/file").unwrap(),
        root.join("home/box/file")
    );
    assert!(resolve_restore_destination(&root, "../../etc/passwd").is_none());
    assert!(symlink_target_stays_within_root(
        &root,
        &root.join("a/b/link"),
        "../target"
    ));
    assert!(!symlink_target_stays_within_root(
        &root,
        &root.join("link"),
        "../../etc/passwd"
    ));
    assert!(is_critical_rel_path(
        "home/box/sand-data/agents/a/store.db"
    ));

    let budget = BoxStoreByteBudget::new(10);
    assert!(budget.try_acquire(6));
    assert!(!budget.try_acquire(5));
    budget.release(6);
    assert!(budget.try_acquire(10));
}

#[test]
fn manifest_counts_conflicts_and_revision_fencing_match_reference() {
    let mut manifest = BTreeMap::new();
    manifest.insert(
        "home/box/sand-data/agents/a/store.db".into(),
        BoxStoreManifestEntry::File {
            sha: "a".into(),
            size: 1,
            mode: 0o600,
        },
    );
    manifest.insert(
        "home/box/sand-data/agents/a/other".into(),
        BoxStoreManifestEntry::File {
            sha: "b".into(),
            size: 1,
            mode: 0o600,
        },
    );
    assert_eq!(count_store_db_manifest_entries(Some(&manifest)), 1);
    assert_eq!(count_agent_dir_manifest_entries(Some(&manifest)), 1);
    assert!(has_manifest_path_conflict(["a", "a/b"]));
    assert!(!has_manifest_path_conflict(["a/b", "a/c"]));

    let mut state = BoxStoreManifestRevisionState::default();
    let capture = state.capture(&manifest, None, false).unwrap();
    assert!(state.has_pending_write_for_revision());
    state.mark_persisted(&capture, None);
    state.release(&capture);
    assert!(state.capture(&manifest, None, false).is_none());
    state.mark_changed();
    assert!(state.capture(&manifest, None, false).is_some());

    set_manifest_entry(
        &mut manifest,
        "home/box/sand-data/agents/a",
        BoxStoreManifestEntry::Symlink {
            target: "other".into(),
        },
    );
    assert_eq!(manifest.len(), 1);
}

#[test]
fn pack_sync_transfer_and_multipart_decisions_are_deterministic() {
    let categories = vec![
        BoxStorePackCategory {
            abs_root: "/home".into(),
            rel_prefix: "home".into(),
            stage_only: false,
        },
        BoxStorePackCategory {
            abs_root: "/agents".into(),
            rel_prefix: "home/box/sand-data/agents".into(),
            stage_only: false,
        },
    ];
    assert_eq!(
        resolve_local_path_for_rel_path(
            &categories,
            "home/box/sand-data/agents/a/store.db"
        )
        .unwrap(),
        std::path::PathBuf::from("/agents/a/store.db")
    );
    assert!(should_build_pack(16, 1));
    assert!(glob_matches_path("home/**/store.db", "home/a/store.db"));
    assert_eq!(
        dedupe_nested_roots(["/home", "/home/box", "/workspace"]),
        vec![
            std::path::PathBuf::from("/home"),
            std::path::PathBuf::from("/workspace")
        ]
    );

    let mut transfer = TransferSummary::default();
    tally_file_outcome(&mut transfer, FileOutcome::Uploaded, 9);
    tally_file_outcome(&mut transfer, FileOutcome::MetadataError, 0);
    assert_eq!(transfer.files_uploaded, 1);
    assert_eq!(transfer.bytes_uploaded, 9);
    assert_eq!(transfer.failures, 1);
    assert_eq!(transfer.metadata_failures, 1);

    let bytes = vec![1_u8; 10];
    let plan = plan_sand_box_store_multipart_parts_from_bytes(&bytes, 4).unwrap();
    assert_eq!(plan.parts.len(), 3);
    assert_eq!(plan.parts[0].offset_bytes, 0);
    assert_eq!(plan.parts[2].size_bytes, 2);
    assert!(should_use_multipart(6, 5));
}

#[test]
fn service_flush_and_db_capture_classification_preserve_reference_priority() {
    let category = build_box_home_category("/tmp/home");
    assert_eq!(category.rel_prefix, BOX_HOME_REL_PREFIX);
    assert!(is_better_cli_home_enabled(Some("1"), Some("1")));
    assert!(!is_better_cli_home_enabled(Some("1"), Some("0")));

    let summary = BoxStoreCycleSummary {
        ok: true,
        reason: None,
        store_id: Some("s".into()),
        duration_ms: 1,
        categories: vec![CategoryTransferSummary {
            name: "store.db".into(),
            ..CategoryTransferSummary::default()
        }],
        manifest_entries: 2,
        store_db_entries: 1,
        store_db_complete: false,
        agent_dir_entries: 1,
        total_files_uploaded: 1,
        total_bytes_uploaded: 4,
        total_failures: 0,
        metadata_failures: 0,
    };
    assert_eq!(
        evaluate_box_store_flush(Some(&summary), "chrome-session").reason,
        "store-db-incomplete"
    );

    assert_eq!(
        aggregate_store_db_sweep_outcome(1, 1, 1, 1),
        StoreDbCaptureOutcome::Error
    );
    assert_eq!(
        aggregate_store_db_sweep_outcome(0, 1, 1, 1),
        StoreDbCaptureOutcome::Oversize
    );
    let mut trace = create_store_db_capture_trace();
    record_store_db_capture_failure(&mut trace, StoreDbCaptureFailurePhase::Capture);
    record_store_db_capture_failure(
        &mut trace,
        StoreDbCaptureFailurePhase::ManifestCommit,
    );
    record_store_db_capture_failure(
        &mut trace,
        StoreDbCaptureFailurePhase::BlobUpload,
    );
    assert_eq!(
        trace.failure_phase,
        Some(StoreDbCaptureFailurePhase::ManifestCommit)
    );
}


#[test]
fn box_store_cycle_telemetry_matches_frozen_metadata_and_skip_semantics() {
    let category = CategoryTransferSummary {
        name: "workspace".into(),
        files_uploaded: 2,
        bytes_uploaded: 9,
        excluded_files: 3,
        excluded_bytes: 11,
        pruned_dirs: 4,
        ..CategoryTransferSummary::default()
    };
    let summary = BoxStoreCycleSummary {
        ok: true,
        reason: None,
        store_id: Some("store-a".into()),
        duration_ms: 7,
        categories: vec![category],
        manifest_entries: 5,
        store_db_entries: 1,
        store_db_complete: true,
        agent_dir_entries: 1,
        total_files_uploaded: 2,
        total_bytes_uploaded: 9,
        total_failures: 0,
        metadata_failures: 0,
    };
    let telemetry = box_store_sync_cycle_telemetry(&summary).expect("success telemetry");
    assert_eq!(telemetry.level, "info");
    assert_eq!(telemetry.metadata.get("ok").map(String::as_str), Some("true"));
    assert_eq!(telemetry.metadata.get("duration_ms").map(String::as_str), Some("7"));
    assert_eq!(telemetry.metadata.get("files_uploaded").map(String::as_str), Some("2"));
    assert_eq!(telemetry.metadata.get("bytes_uploaded").map(String::as_str), Some("9"));
    assert_eq!(telemetry.metadata.get("manifest_entries").map(String::as_str), Some("5"));
    assert_eq!(telemetry.metadata.get("excluded_files").map(String::as_str), Some("3"));
    assert_eq!(telemetry.metadata.get("excluded_bytes").map(String::as_str), Some("11"));
    assert_eq!(telemetry.metadata.get("pruned_dirs").map(String::as_str), Some("4"));
    assert_eq!(telemetry.metadata.get("store_db_entries").map(String::as_str), Some("1"));
    assert_eq!(telemetry.metadata.get("store_id").map(String::as_str), Some("store-a"));
    assert_eq!(telemetry.metadata.get("failures").map(String::as_str), Some("0"));
    assert_eq!(telemetry.metadata.get("workspace_uploaded").map(String::as_str), Some("2"));
    assert_eq!(telemetry.metadata.get("workspace_bytes").map(String::as_str), Some("9"));

    let mut skipped = summary.clone();
    skipped.ok = false;
    skipped.reason = Some("in-flight".into());
    assert!(box_store_sync_cycle_telemetry(&skipped).is_none());

    skipped.reason = Some("locked".into());
    let telemetry = box_store_sync_cycle_telemetry(&skipped).expect("locked telemetry");
    assert_eq!(telemetry.level, "warn");
    assert_eq!(telemetry.metadata.get("reason").map(String::as_str), Some("locked"));
}

#[test]
fn box_store_writer_lock_serializes_owners_and_releases_on_drop() {
    let root = temp_root("writer-lock");
    fs::create_dir_all(&root).expect("create writer-lock test root");
    let root = fs::canonicalize(&root).expect("canonicalize writer-lock test root");
    let lock_path = root.join("box-store-sync.lock");

    let first = BoxStoreWriterLock::try_acquire(&lock_path, "window-a")
        .expect("first writer-lock acquire")
        .expect("first owner must acquire");
    assert_eq!(first.path(), lock_path.as_path());

    let second = BoxStoreWriterLock::try_acquire(&lock_path, "window-b")
        .expect("contending writer-lock acquire");
    assert!(second.is_none(), "second owner must observe the held writer lock");

    drop(first);
    let reacquired = BoxStoreWriterLock::try_acquire(&lock_path, "window-b")
        .expect("writer-lock reacquire")
        .expect("writer lock must be released when the owner drops");
    drop(reacquired);

    let _ = fs::remove_dir_all(root);
}


#[test]
fn store_db_capture_telemetry_matches_frozen_levels_and_fields() {
    assert_eq!(box_store_db_capture_telemetry_level(StoreDbCaptureOutcome::Error), "warn");
    assert_eq!(box_store_db_capture_telemetry_level(StoreDbCaptureOutcome::Skipped), "warn");
    assert_eq!(box_store_db_capture_telemetry_level(StoreDbCaptureOutcome::Oversize), "info");
    assert_eq!(box_store_db_capture_telemetry_level(StoreDbCaptureOutcome::Uploaded), "info");
    assert_eq!(box_store_db_capture_telemetry_level(StoreDbCaptureOutcome::Unchanged), "info");

    let telemetry = box_store_db_capture_telemetry(&BoxStoreDbCaptureTelemetrySummary {
        outcome: StoreDbCaptureOutcome::Error,
        trigger: "turn_end",
        failure_phase: Some(StoreDbCaptureFailurePhase::ManifestCommit),
        is_committed: false,
        agent_count: 1,
        files_scanned: 2,
        files_uploaded: 1,
        bytes: 4096,
        duration_ms: 23,
        queue_duration_ms: 5,
        capture_duration_ms: 7,
        blob_upload_duration_ms: 8,
        manifest_commit_duration_ms: 3,
        store_id: Some("store-a".into()),
    });
    assert_eq!(telemetry.level, "warn");
    assert_eq!(telemetry.metadata.get("outcome").map(String::as_str), Some("error"));
    assert_eq!(telemetry.metadata.get("trigger").map(String::as_str), Some("turn_end"));
    assert_eq!(telemetry.metadata.get("failure_phase").map(String::as_str), Some("manifest_commit"));
    assert_eq!(telemetry.metadata.get("committed").map(String::as_str), Some("false"));
    assert_eq!(telemetry.metadata.get("agent_count").map(String::as_str), Some("1"));
    assert_eq!(telemetry.metadata.get("files_scanned").map(String::as_str), Some("2"));
    assert_eq!(telemetry.metadata.get("files_uploaded").map(String::as_str), Some("1"));
    assert_eq!(telemetry.metadata.get("bytes").map(String::as_str), Some("4096"));
    assert_eq!(telemetry.metadata.get("duration_ms").map(String::as_str), Some("23"));
    assert_eq!(telemetry.metadata.get("queue_duration_ms").map(String::as_str), Some("5"));
    assert_eq!(telemetry.metadata.get("capture_duration_ms").map(String::as_str), Some("7"));
    assert_eq!(telemetry.metadata.get("blob_upload_duration_ms").map(String::as_str), Some("8"));
    assert_eq!(telemetry.metadata.get("manifest_commit_duration_ms").map(String::as_str), Some("3"));
    assert_eq!(telemetry.metadata.get("store_id").map(String::as_str), Some("store-a"));
}


#[test]
fn store_db_capture_queue_duration_only_measures_same_agent_serialization_wait() {
    let queues = Arc::new(AgentDbCaptureQueues::default());
    let first_queues = Arc::clone(&queues);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let first = thread::spawn(move || {
        first_queues.run_serialized_with_queue_duration("agent-a", |queue_duration_ms| {
            assert_eq!(
                queue_duration_ms, 0,
                "the first capture for an agent must not report ordinary lock acquisition as queue time"
            );
            started_tx.send(()).expect("signal first capture started");
            release_rx.recv().expect("release first capture");
        });
    });
    started_rx.recv().expect("first capture started");

    let second_queues = Arc::clone(&queues);
    let second = thread::spawn(move || {
        second_queues.run_serialized_with_queue_duration("agent-a", |queue_duration_ms| {
            queue_duration_ms
        })
    });

    thread::sleep(Duration::from_millis(25));
    release_tx.send(()).expect("release first capture");
    first.join().expect("first capture join");
    let queued_ms = second.join().expect("second capture join");
    assert!(
        queued_ms >= 20,
        "a capture blocked behind an in-flight capture for the same agent must report serialization wait, got {queued_ms}ms"
    );

    let unqueued = queues.run_serialized_with_queue_duration("agent-b", |queue_duration_ms| {
        queue_duration_ms
    });
    assert_eq!(
        unqueued, 0,
        "a different agent with no predecessor must retain frozen zero queue duration"
    );
}
