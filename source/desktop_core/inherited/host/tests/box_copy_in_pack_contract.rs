use std::fs;
use std::path::PathBuf;

use mahayana_host_runtime::extensions::box_store_sync::box_copy_in::{
    BOX_COPY_IN_EXIT_HYDRATED, CopyInOutcome, build_copy_in_status_from_result,
    execute_box_copy_in_from_env_with_provider_dependencies, is_transient_copy_in_failure,
    run_local_box_copy_in, write_copy_in_status_atomic,
};
use mahayana_host_runtime::extensions::box_store_sync::box_object_store::{
    BoxObjectStore, BoxObjectStoreProviderDependencies, LocalFsObjectStore,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest_format::{
    BOX_STORE_MANIFEST_REL_PATH,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_pack::{
    BOX_STORE_PACK_INDEX_KEY, BOX_STORE_PACKS_PREFIX, PACK_INDEX_VERSION, PackEntry, PackIndex,
    PackMember, PackSource, build_pack_file, serialize_pack_index, sha256_hex,
};

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "fabushi-box-copy-in-pack-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

#[test]
fn local_copy_in_hydrates_bulk_small_files_from_pack_without_loose_blobs() {
    let root = temp_root("only");
    let store_root = root.join("store");
    let source_root = root.join("source");
    let target_root = root.join("target");
    fs::create_dir_all(&source_root).expect("create source");
    fs::create_dir_all(&target_root).expect("create target");
    let store = LocalFsObjectStore::new(&store_root);

    let mut sources = Vec::new();
    let mut manifest_entries = serde_json::Map::new();
    let mut expected = Vec::new();
    for index in 0..16_u64 {
        let rel_path = format!("workspace/bulk-{index}.txt");
        let bytes = format!("bulk-small-pack-member-{index}\n").into_bytes();
        let sha = sha256_hex(&bytes);
        let abs_path = source_root.join(format!("member-{index}"));
        fs::write(&abs_path, &bytes).expect("write source member");
        sources.push(PackSource {
            abs_path,
            sha: sha.clone(),
            size: bytes.len() as u64,
            vmtime: index + 1,
        });
        manifest_entries.insert(
            rel_path.clone(),
            serde_json::json!({
                "kind": "file",
                "sha": sha,
                "size": bytes.len(),
                "mode": 0o600,
            }),
        );
        expected.push((rel_path, bytes, sha));
    }

    let pack_path = root.join("built.pack");
    let built = build_pack_file(&pack_path, &sources, || false)
        .expect("build pack")
        .expect("pack not aborted");
    assert_eq!(built.members.len(), 16);
    let pack_bytes = fs::metadata(&pack_path).expect("pack metadata").len();
    let pack_id = sha256_hex(&fs::read(&pack_path).expect("read pack"));
    store
        .put_from_file(&format!("{BOX_STORE_PACKS_PREFIX}/{pack_id}"), &pack_path)
        .expect("store pack");
    let index = PackIndex {
        version: PACK_INDEX_VERSION,
        max_vmtime: 16,
        packs: vec![PackEntry {
            id: pack_id,
            bytes: pack_bytes,
            members: built.members,
        }],
    };
    store
        .put(
            BOX_STORE_PACK_INDEX_KEY,
            serialize_pack_index(&index).expect("serialize index").as_bytes(),
        )
        .expect("store pack index");
    store
        .put(
            BOX_STORE_MANIFEST_REL_PATH,
            serde_json::to_vec(&serde_json::json!({
                "version": 2,
                "updatedAtMs": 1,
                "writerWindowId": "test",
                "entries": manifest_entries,
            }))
            .expect("serialize manifest")
            .as_slice(),
        )
        .expect("store manifest");

    for (_, _, sha) in &expected {
        assert!(
            store
                .get(&format!("blobs/{sha}"))
                .expect("probe loose blob")
                .is_none(),
            "test must prove hydration does not rely on loose blobs"
        );
    }

    let result = run_local_box_copy_in(&store, &target_root);
    assert_eq!(result.outcome, CopyInOutcome::Hydrated);
    assert_eq!(result.manifest_entries, 16);
    assert_eq!(result.files, 16);
    assert_eq!(result.verified, 16);
    assert!(result.failures.is_empty());

    for (rel_path, bytes, _) in expected {
        let restored = target_root.join(&rel_path);
        assert_eq!(fs::read(&restored).expect("read restored file"), bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&restored).expect("restored metadata").permissions().mode() & 0o777,
                0o600
            );
        }
    }

    fs::remove_dir_all(root).expect("cleanup");
}


#[test]
fn missing_pack_object_falls_back_to_loose_blobs() {
    let root = temp_root("missing-pack-fallback");
    let store = LocalFsObjectStore::new(root.join("store"));
    let target_root = root.join("target");
    fs::create_dir_all(&target_root).expect("create target");

    let mut manifest_entries = serde_json::Map::new();
    let mut members = Vec::new();
    let mut expected = Vec::new();
    for index in 0..16_u64 {
        let rel_path = format!("workspace/fallback-{index}.txt");
        let bytes = format!("fallback-member-{index}\n").into_bytes();
        let sha = sha256_hex(&bytes);
        store
            .put(&format!("blobs/{sha}"), &bytes)
            .expect("store loose blob");
        manifest_entries.insert(
            rel_path.clone(),
            serde_json::json!({
                "kind": "file",
                "sha": sha,
                "size": bytes.len(),
                "mode": 0o600,
            }),
        );
        members.push(PackMember {
            sha: sha.clone(),
            size: bytes.len() as u64,
            offset: index,
            clen: 1,
            vmtime: index + 1,
        });
        expected.push((rel_path, bytes));
    }

    let index = PackIndex {
        version: PACK_INDEX_VERSION,
        max_vmtime: 16,
        packs: vec![PackEntry {
            id: "intentionally-missing-pack".into(),
            bytes: 16,
            members,
        }],
    };
    store
        .put(
            BOX_STORE_PACK_INDEX_KEY,
            serialize_pack_index(&index).expect("serialize index").as_bytes(),
        )
        .expect("store pack index");
    store
        .put(
            BOX_STORE_MANIFEST_REL_PATH,
            serde_json::to_vec(&serde_json::json!({
                "version": 2,
                "updatedAtMs": 1,
                "writerWindowId": "test",
                "entries": manifest_entries,
            }))
            .expect("serialize manifest")
            .as_slice(),
        )
        .expect("store manifest");

    assert!(
        store
            .get(&format!("{BOX_STORE_PACKS_PREFIX}/intentionally-missing-pack"))
            .expect("probe missing pack")
            .is_none()
    );

    let result = run_local_box_copy_in(&store, &target_root);
    assert_eq!(result.outcome, CopyInOutcome::Hydrated);
    assert_eq!(result.files, 16);
    assert_eq!(result.verified, 16);
    assert!(result.failures.is_empty());
    for (rel_path, bytes) in expected {
        assert_eq!(
            fs::read(target_root.join(rel_path)).expect("read fallback file"),
            bytes
        );
    }

    fs::remove_dir_all(root).expect("cleanup");
}


#[test]
fn shipping_copy_in_composes_through_canonical_object_store_provider() {
    let root = temp_root("provider-composition");
    let store_root = root.join("store-root");
    let target_root = root.join("target");
    let store_id = "agentStore:copy-in-provider";
    fs::create_dir_all(&target_root).expect("create target");
    let store = LocalFsObjectStore::new(store_root.join(store_id));

    let bytes = b"provider-composed-copy-in\n".to_vec();
    let sha = sha256_hex(&bytes);
    store
        .put(&format!("blobs/{sha}"), &bytes)
        .expect("write blob");
    store
        .put(
            BOX_STORE_MANIFEST_REL_PATH,
            serde_json::to_vec(&serde_json::json!({
                "version": 2,
                "updatedAtMs": 1,
                "writerWindowId": "provider-composition",
                "entries": {
                    "workspace/provider.txt": {
                        "kind": "file",
                        "sha": sha,
                        "size": bytes.len(),
                        "mode": 0o600
                    }
                }
            }))
            .expect("serialize manifest")
            .as_slice(),
        )
        .expect("write manifest");

    let environment = std::collections::BTreeMap::from([
        ("SAND_BOX_STORE_COPY_IN".to_string(), "1".to_string()),
        ("SAND_BOX_STORE_LOCAL_DIR".to_string(), store_root.to_string_lossy().into_owned()),
        ("SAND_BOX_STORE_ID".to_string(), store_id.to_string()),
    ]);
    let exit = execute_box_copy_in_from_env_with_provider_dependencies(
        &environment,
        &target_root,
        BoxObjectStoreProviderDependencies::default(),
    );
    assert_eq!(exit, BOX_COPY_IN_EXIT_HYDRATED);
    assert_eq!(
        fs::read(target_root.join("workspace/provider.txt")).expect("restored file"),
        bytes
    );

    fs::remove_dir_all(root).expect("cleanup");
}


#[test]
fn copy_in_status_is_atomic_and_transient_retry_classification_fails_closed_for_auth() {
    let root = temp_root("status");
    let status_path = root.join("copy-in-status.json");
    let transient = mahayana_host_runtime::extensions::box_store_sync::box_copy_in::CopyInResult {
        outcome: CopyInOutcome::Failed,
        reason: "network connection reset".into(),
        manifest_entries: 2,
        store_db_entries: 1,
        restored_store_db_entries: Some(0),
        files: 1,
        bytes: 5,
        verified: 1,
        failures: vec!["network connection reset".into()],
        hydrate_source: None,
    };
    assert!(is_transient_copy_in_failure(&transient));
    let auth = mahayana_host_runtime::extensions::box_store_sync::box_copy_in::CopyInResult {
        reason: "401 unauthorized".into(),
        failures: vec!["401 unauthorized".into()],
        ..transient.clone()
    };
    assert!(!is_transient_copy_in_failure(&auth));

    let status = build_copy_in_status_from_result(&transient);
    write_copy_in_status_atomic(&status_path, &status).expect("write atomic status");
    let parsed: serde_json::Value =
        serde_json::from_slice(&fs::read(&status_path).expect("read status")).expect("parse status");
    assert_eq!(parsed["phase"], "failed");
    assert_eq!(parsed["total"], 2);
    assert_eq!(parsed["storeDbEntries"], 1);
    assert!(!status_path.with_extension("tmp").exists());

    fs::remove_dir_all(root).expect("cleanup");
}
