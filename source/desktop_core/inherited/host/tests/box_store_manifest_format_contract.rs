use mahayana_host_runtime::extensions::box_store_sync::box_store_manifest_format::{
    BOX_STORE_LEGACY_MANIFEST_VERSION, BOX_STORE_MANIFEST_VERSION,
    BoxStoreManifestEntry, box_store_manifest_entries_equal, is_symlink_manifest_value,
    parse_box_store_manifest, parse_manifest_entry,
};

#[test]
fn parses_legacy_and_v2_manifest_entries_with_frozen_validation() {
    let legacy = serde_json::json!({
        "version": BOX_STORE_LEGACY_MANIFEST_VERSION,
        "updatedAtMs": 10,
        "entries": {
            "a.txt": { "sha": "abc", "size": 3 }
        }
    });
    let parsed = parse_box_store_manifest(&legacy).expect("legacy manifest");
    assert_eq!(
        parsed.entries["a.txt"],
        BoxStoreManifestEntry::LegacyFile {
            sha: "abc".into(),
            size: 3,
        }
    );

    let current = serde_json::json!({
        "version": BOX_STORE_MANIFEST_VERSION,
        "updatedAtMs": 20,
        "writerWindowId": "window-a",
        "fullyHydrated": true,
        "entries": {
            "a.txt": { "kind": "file", "sha": "def", "size": 4, "mode": 420 },
            "link": { "kind": "symlink", "target": "a.txt" }
        }
    });
    let parsed = parse_box_store_manifest(&current).expect("v2 manifest");
    assert_eq!(parsed.writer_window_id.as_deref(), Some("window-a"));
    assert_eq!(parsed.fully_hydrated, Some(true));
    assert!(matches!(
        parsed.entries["link"],
        BoxStoreManifestEntry::Symlink { .. }
    ));
}

#[test]
fn rejects_invalid_modes_legacy_kind_and_bad_header_types() {
    assert!(parse_manifest_entry(&serde_json::json!({
        "kind": "file", "sha": "x", "size": 1, "mode": 512
    }))
    .is_none());
    assert!(parse_box_store_manifest(&serde_json::json!({
        "version": 1,
        "updatedAtMs": 1,
        "entries": {
            "x": { "kind": "file", "sha": "x", "size": 1, "mode": 420 }
        }
    }))
    .is_none());
    assert!(parse_box_store_manifest(&serde_json::json!({
        "version": 2,
        "updatedAtMs": -1,
        "entries": {}
    }))
    .is_none());
}

#[test]
fn manifest_entry_equality_distinguishes_legacy_file_and_v2_file() {
    let legacy = BoxStoreManifestEntry::LegacyFile {
        sha: "same".into(),
        size: 3,
    };
    let legacy_same = legacy.clone();
    let file = BoxStoreManifestEntry::File {
        sha: "same".into(),
        size: 3,
        mode: 0,
    };
    assert!(box_store_manifest_entries_equal(Some(&legacy), Some(&legacy_same)));
    assert!(!box_store_manifest_entries_equal(Some(&legacy), Some(&file)));
    assert!(box_store_manifest_entries_equal(None, None));
    assert!(!box_store_manifest_entries_equal(Some(&legacy), None));
}

#[test]
fn symlink_value_requires_non_empty_target() {
    assert!(is_symlink_manifest_value(&serde_json::json!({
        "kind": "symlink",
        "target": "target"
    })));
    assert!(!is_symlink_manifest_value(&serde_json::json!({
        "kind": "symlink",
        "target": ""
    })));
}
