use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::box_store_sync::box_object_store::{
    BoxObjectStore, LocalFsObjectStore,
};
use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest::BoxManifestMap;
use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest_format::BoxStoreManifestEntry;
use mahayana_host_runtime::extensions::box_store_sync::box_store_transfer::{
    BOX_STORE_SNAPSHOT_TMP_SUFFIX, BoxStoreTransfer, glob_matches_path,
};

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-box-store-transfer-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn transfer(marker: Option<PathBuf>, threshold: u64) -> BoxStoreTransfer {
    BoxStoreTransfer::new(
        64 * 1024 * 1024,
        4,
        threshold,
        marker,
        Arc::new(|_| {}),
    )
}

#[test]
fn frozen_transfer_owns_small_large_symlink_ignore_and_prune_semantics() {
    let root = temp_root("shipping");
    let workspace = root.join("workspace");
    let store_root = root.join("store");
    fs::create_dir_all(workspace.join("src")).expect("create source");
    fs::write(workspace.join("src/small.txt"), b"small").expect("write small");
    fs::write(workspace.join("src/large.bin"), vec![7u8; 4096]).expect("write large");
    fs::write(workspace.join("ignored.tmp"), b"ignore").expect("write ignore");
    fs::write(
        workspace.join(format!("leak{BOX_STORE_SNAPSHOT_TMP_SUFFIX}deadbeef")),
        b"temp",
    )
    .expect("write leaked temp");

    #[cfg(unix)]
    std::os::unix::fs::symlink("small.txt", workspace.join("src/link.txt"))
        .expect("create symlink");

    let store = LocalFsObjectStore::new(&store_root);
    let transfer = transfer(None, 1024);
    let mut manifest = BoxManifestMap::new();
    let summary = transfer
        .sync_tree_category(
            &store,
            &mut manifest,
            &workspace,
            "workspace",
            "workspace",
            &["workspace/ignored.tmp"],
            None,
            false,
            true,
        )
        .expect("first transfer");

    assert_eq!(summary.failures, 0);
    assert_eq!(summary.metadata_failures, 0);
    #[cfg(unix)]
    assert_eq!(summary.files_uploaded, 3);
    #[cfg(not(unix))]
    assert_eq!(summary.files_uploaded, 2);
    assert!(manifest.contains_key("workspace/src/small.txt"));
    assert!(manifest.contains_key("workspace/src/large.bin"));
    assert!(!manifest.contains_key("workspace/ignored.tmp"));

    #[cfg(unix)]
    assert!(matches!(
        manifest.get("workspace/src/link.txt"),
        Some(BoxStoreManifestEntry::Symlink { target }) if target == "small.txt"
    ));

    for rel in ["workspace/src/small.txt", "workspace/src/large.bin"] {
        let sha = match manifest.get(rel).expect("manifest file") {
            BoxStoreManifestEntry::File { sha, .. } => sha,
            other => panic!("expected file entry, got {other:?}"),
        };
        assert!(
            store
                .get(&format!("blobs/{sha}"))
                .expect("read blob")
                .is_some()
        );
    }

    let second = transfer
        .sync_tree_category(
            &store,
            &mut manifest,
            &workspace,
            "workspace",
            "workspace",
            &["workspace/ignored.tmp"],
            None,
            false,
            true,
        )
        .expect("second transfer");
    assert_eq!(second.files_uploaded, 0);
    assert!(second.skipped_unchanged >= 2);

    fs::remove_file(workspace.join("src/small.txt")).expect("remove small");
    let pruned = transfer
        .sync_tree_category(
            &store,
            &mut manifest,
            &workspace,
            "workspace",
            "workspace",
            &["workspace/ignored.tmp"],
            None,
            false,
            true,
        )
        .expect("prune transfer");
    assert!(pruned.removed >= 1);
    assert!(!manifest.contains_key("workspace/src/small.txt"));

    let removed = transfer.sweep_leaked_temps(&[workspace.clone()]);
    assert_eq!(removed, 1);
    assert!(
        !workspace
            .join(format!("leak{BOX_STORE_SNAPSHOT_TMP_SUFFIX}deadbeef"))
            .exists()
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn frozen_transfer_skips_hydration_marker_and_its_temp_variants() {
    let root = temp_root("hydration-marker");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("create workspace");
    let marker = workspace.join("box-store-hydration-handoff.json");
    fs::write(&marker, b"marker").expect("marker");
    fs::write(
        workspace.join("box-store-hydration-handoff.json.abc.tmp"),
        b"temp",
    )
    .expect("marker temp");
    fs::write(workspace.join("keep.txt"), b"keep").expect("keep");

    let store = LocalFsObjectStore::new(root.join("store"));
    let transfer = transfer(Some(marker), 1024);
    let mut manifest = BoxManifestMap::new();
    let summary = transfer
        .sync_tree_category(
            &store,
            &mut manifest,
            &workspace,
            "workspace",
            "workspace",
            &[],
            None,
            false,
            true,
        )
        .expect("transfer");

    assert_eq!(summary.files_uploaded, 1);
    assert!(manifest.contains_key("workspace/keep.txt"));
    assert!(!manifest.contains_key("workspace/box-store-hydration-handoff.json"));
    assert!(!manifest.contains_key("workspace/box-store-hydration-handoff.json.abc.tmp"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn frozen_glob_exclusion_covers_descendants_not_only_exact_directory() {
    assert!(glob_matches_path(
        "home/box/chrome-profile/*/Cache",
        "home/box/chrome-profile/Default/Cache/data/index"
    ));
    assert!(glob_matches_path(
        "home/**/store.db",
        "home/box/sand-data/agents/a/store.db"
    ));
    assert!(!glob_matches_path(
        "home/box/chrome-profile/*/Cache",
        "home/box/chrome-profile/Default/Cookies"
    ));
}

#[test]
fn shipping_production_has_one_transfer_owner_and_no_parallel_sync_runtime() {
    let production = include_str!("../src/extensions/box_store_sync/production.rs");
    assert!(production.contains("transfer: BoxStoreTransfer"));
    assert!(production.contains("self.transfer.sync_tree_category("));
    assert!(production.contains("transfer.sync_staged_files("));
    assert!(production.contains("poll_inner.transfer.sweep_leaked_temps(&roots)"));
    assert!(!production.contains("\nfn sync_tree_category("));
    assert!(!production.contains("\nfn sync_file("));
}
