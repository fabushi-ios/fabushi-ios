use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

use mahayana_host_runtime::extensions::box_store_sync::box_copy_in::{
    BOX_COPY_IN_TELEMETRY_EVENT, COPY_IN_STATUS_THROTTLE_MS, CopyInOutcome,
    CopyInProgress, CopyInRuntimeOptions, CopyInTraceStatus, build_copy_in_watchdog_event,
    resolve_copy_in_download_owner_values, run_box_copy_in_with_sources,
    write_copy_in_status_throttled,
};
use mahayana_host_runtime::extensions::box_store_sync::box_object_store::{
    BoxObjectStore, LocalFsObjectStore,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest_format::{
    BOX_STORE_MANIFEST_REL_PATH, parse_box_store_manifest,
};

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "fabushi-box-copy-in-parity-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn file_entry(bytes: &[u8]) -> (String, serde_json::Value) {
    let sha = format!("{:x}", Sha256::digest(bytes));
    (
        sha.clone(),
        serde_json::json!({
            "kind": "file",
            "sha": sha,
            "size": bytes.len(),
            "mode": 0o600,
        }),
    )
}

fn write_manifest(
    store: &dyn BoxObjectStore,
    fully_hydrated: Option<bool>,
    entries: serde_json::Map<String, serde_json::Value>,
) {
    let mut value = serde_json::json!({
        "version": 2,
        "updatedAtMs": 1,
        "writerWindowId": "copy-in-parity-contract",
        "entries": entries,
    });
    if let Some(fully_hydrated) = fully_hydrated {
        value["fullyHydrated"] = serde_json::Value::Bool(fully_hydrated);
    }
    store
        .put(
            BOX_STORE_MANIFEST_REL_PATH,
            &serde_json::to_vec(&value).expect("serialize manifest"),
        )
        .expect("write manifest");
}

#[test]
fn legacy_source_migrates_when_primary_is_unseeded_and_persists_handoff_state() {
    let root = temp_root("legacy-migration");
    let primary = LocalFsObjectStore::new(root.join("primary"));
    let legacy = LocalFsObjectStore::new(root.join("legacy"));
    let target = root.join("target");
    let marker = root.join(".box-store-legacy-hydration-complete");

    let payload = b"legacy-agent-store\n";
    let (sha, entry) = file_entry(payload);
    legacy
        .put(&format!("blobs/{sha}"), payload)
        .expect("write legacy blob");
    let mut entries = serde_json::Map::new();
    entries.insert(
        "home/box/sand-data/agents/agent-a/store.db".into(),
        entry,
    );
    write_manifest(&legacy, Some(true), entries);

    fs::write(&marker, b"stale-complete\n").expect("seed stale marker");
    let result = run_box_copy_in_with_sources(
        &primary,
        Some(&legacy),
        &target,
        Some(&marker),
        CopyInRuntimeOptions::default(),
    );

    assert_eq!(result.outcome, CopyInOutcome::Hydrated);
    assert_eq!(result.hydrate_source.as_deref(), Some("legacy"));
    assert_eq!(result.store_db_entries, 1);
    assert_eq!(result.restored_store_db_entries, Some(1));
    assert_eq!(
        fs::read(target.join("home/box/sand-data/agents/agent-a/store.db"))
            .expect("read restored store.db"),
        payload
    );

    let primary_manifest_bytes = primary
        .get(BOX_STORE_MANIFEST_REL_PATH)
        .expect("read primary manifest")
        .expect("primary incomplete manifest");
    let primary_value: serde_json::Value =
        serde_json::from_slice(&primary_manifest_bytes).expect("parse primary manifest json");
    let primary_manifest =
        parse_box_store_manifest(&primary_value).expect("parse primary manifest schema");
    assert_eq!(primary_manifest.fully_hydrated, Some(false));
    assert!(primary_manifest.entries.is_empty());
    assert_eq!(fs::read_to_string(&marker).expect("read handoff marker"), "complete\n");

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn incomplete_primary_requires_legacy_store_db_identity_coverage() {
    let root = temp_root("coverage");
    let primary = LocalFsObjectStore::new(root.join("primary"));
    let legacy = LocalFsObjectStore::new(root.join("legacy"));
    let target = root.join("target");
    let marker = root.join(".box-store-legacy-hydration-complete");

    let mut primary_entries = serde_json::Map::new();
    primary_entries.insert(
        "home/box/sand-data/agents/agent-a/store.db".into(),
        file_entry(b"a").1,
    );
    write_manifest(&primary, Some(false), primary_entries);

    let mut legacy_entries = serde_json::Map::new();
    legacy_entries.insert(
        "home/box/sand-data/agents/agent-a/store.db".into(),
        file_entry(b"a").1,
    );
    legacy_entries.insert(
        "home/box/sand-data/agents/agent-b/store.db".into(),
        file_entry(b"b").1,
    );
    write_manifest(&legacy, Some(true), legacy_entries);

    let result = run_box_copy_in_with_sources(
        &primary,
        Some(&legacy),
        &target,
        Some(&marker),
        CopyInRuntimeOptions::default(),
    );
    assert_eq!(result.outcome, CopyInOutcome::Failed);
    assert_eq!(result.hydrate_source.as_deref(), Some("legacy"));
    assert!(result.reason.contains("primary V2 store.db coverage is below"));
    assert!(!marker.exists());

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn incomplete_primary_fails_closed_when_legacy_has_no_store_db_identity() {
    let root = temp_root("no-legacy-store-db");
    let primary = LocalFsObjectStore::new(root.join("primary"));
    let legacy = LocalFsObjectStore::new(root.join("legacy"));
    let target = root.join("target");
    let marker = root.join(".box-store-legacy-hydration-complete");

    let mut primary_entries = serde_json::Map::new();
    primary_entries.insert(
        "home/box/sand-data/agents/agent-a/store.db".into(),
        file_entry(b"a").1,
    );
    write_manifest(&primary, Some(false), primary_entries);

    let mut legacy_entries = serde_json::Map::new();
    legacy_entries.insert("workspace/readme.txt".into(), file_entry(b"legacy").1);
    write_manifest(&legacy, Some(true), legacy_entries);

    let result = run_box_copy_in_with_sources(
        &primary,
        Some(&legacy),
        &target,
        Some(&marker),
        CopyInRuntimeOptions::default(),
    );
    assert_eq!(result.outcome, CopyInOutcome::Failed);
    assert_eq!(result.hydrate_source.as_deref(), Some("legacy"));
    assert!(result.reason.contains("no store.db identities to validate V2"));
    assert!(!marker.exists());

    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn status_throttle_watchdog_trace_and_root_owner_match_frozen_copy_in_contract() {
    let root = temp_root("runtime-contract");
    let status_path = root.join("copy-in-status.json");
    let mut last_write_ms = None;

    assert!(
        write_copy_in_status_throttled(
            &status_path,
            &serde_json::json!({"phase":"copying","restored":1}),
            1_000,
            &mut last_write_ms,
            false,
        )
        .expect("first status write")
    );
    assert_eq!(last_write_ms, Some(1_000));
    assert!(
        !write_copy_in_status_throttled(
            &status_path,
            &serde_json::json!({"phase":"copying","restored":2}),
            1_000 + COPY_IN_STATUS_THROTTLE_MS - 1,
            &mut last_write_ms,
            false,
        )
        .expect("throttled status write")
    );
    assert!(
        write_copy_in_status_throttled(
            &status_path,
            &serde_json::json!({"phase":"copying","restored":3}),
            1_000 + COPY_IN_STATUS_THROTTLE_MS,
            &mut last_write_ms,
            false,
        )
        .expect("boundary status write")
    );

    let trace = CopyInTraceStatus {
        copy_stage: Some("symlink".into()),
        file_entries: Some(3),
        symlink_entries: Some(2),
        symlinks_started: Some(2),
        symlinks_completed: Some(1),
        symlinks_in_flight: Some(1),
        active_symlink_steps: BTreeMap::from([("restore".into(), 1)]),
    };
    let stuck = build_copy_in_watchdog_event(
        false,
        CopyInProgress {
            total: 5,
            files: 3,
            bytes: 99,
        },
        300_000,
        300_000,
        Some(&trace),
    );
    assert_eq!(stuck.level, "warn");
    assert_eq!(stuck.metadata.get("outcome").map(String::as_str), Some("stuck"));
    assert_eq!(stuck.metadata.get("stage").map(String::as_str), Some("symlink"));
    assert_eq!(
        stuck.metadata.get("active_symlink_steps").map(String::as_str),
        Some("restore:1")
    );

    let slow = build_copy_in_watchdog_event(
        true,
        CopyInProgress {
            total: 5,
            files: 4,
            bytes: 120,
        },
        600_000,
        300_000,
        Some(&trace),
    );
    assert_eq!(slow.level, "info");
    assert_eq!(slow.metadata.get("outcome").map(String::as_str), Some("slow"));

    assert_eq!(resolve_copy_in_download_owner_values(0, 501, 20), Some((501, 20)));
    assert_eq!(resolve_copy_in_download_owner_values(0, 0, 0), None);
    assert_eq!(resolve_copy_in_download_owner_values(501, 501, 20), None);
    assert_eq!(BOX_COPY_IN_TELEMETRY_EVENT, "sand.box_copy_in");

    fs::remove_dir_all(root).expect("cleanup");
}
