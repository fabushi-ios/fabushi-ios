use mahayana_host_runtime::extensions::plugin_state::blob_store::{
    BlobStoreError, PluginBlobStore, MAX_PLUGIN_BLOB_BYTES_PER_ENTRY,
};
use mahayana_host_runtime::extensions::plugin_state::state_store::{
    PluginStateKeyedStore, StateStoreError, MAX_PLUGIN_STATE_BULK_DELETE_ENTRIES,
    MAX_PLUGIN_STATE_VALUE_BYTES,
};
use mahayana_host_runtime::extensions::plugin_state::runtime_health::{
    DisplayGroupPick, RuntimeHealthRecordEnvelope, RuntimeHealthStore,
};
use serde_json::json;

#[test]
fn test_ocrm_plugin_state_blob_store_bounds() {
    let mut store = PluginBlobStore::new();
    let plugin = "plugin-media";

    // Normal blob put and get
    let data = b"sample-binary-blob".to_vec();
    assert!(store.put_blob(plugin, "ns", "blob-1", data.clone(), Some(5000), 1000).is_ok());
    assert_eq!(store.get_blob(plugin, "ns", "blob-1", 2000).unwrap(), data.as_slice());

    // Expired blob get returns NotFound
    assert_eq!(store.get_blob(plugin, "ns", "blob-1", 7000), Err(BlobStoreError::NotFound));

    // Delete expired blobs
    assert_eq!(store.delete_expired_blobs(plugin, 7000), 1);

    // Entry too large check (limit is 100 MiB)
    let huge_data = vec![0u8; MAX_PLUGIN_BLOB_BYTES_PER_ENTRY + 1];
    assert_eq!(
        store.put_blob(plugin, "ns", "huge", huge_data, None, 1000),
        Err(BlobStoreError::EntryTooLarge {
            size: MAX_PLUGIN_BLOB_BYTES_PER_ENTRY + 1,
            limit: MAX_PLUGIN_BLOB_BYTES_PER_ENTRY,
        })
    );
}

#[test]
fn test_ocrm_plugin_state_keyed_store_and_journal() {
    let mut store = PluginStateKeyedStore::new();
    let plugin = "plugin-auth";

    // Set and get JSON state
    let val = json!({"user": "alice", "roles": ["admin"]});
    let seq1 = store.set_state(plugin, "config", "prefs", val.clone(), 1000).expect("set state");
    assert_eq!(seq1, 1);

    let retrieved = store.get_state(plugin, "config", "prefs").expect("retrieved");
    assert_eq!(retrieved, &val);

    // Value too large check (limit 1 MiB)
    let huge_val = json!({"data": "x".repeat(MAX_PLUGIN_STATE_VALUE_BYTES + 10)});
    let err = store.set_state(plugin, "config", "huge", huge_val, 1000).expect_err("should reject large value");
    assert!(matches!(err, StateStoreError::ValueTooLarge { .. }));

    // Bulk delete within limit
    let entries = vec![("config".to_string(), "prefs".to_string())];
    assert_eq!(store.bulk_delete(plugin, &entries).unwrap(), 1);

    // Bulk delete exceeding limit (> 512)
    let mut too_many = Vec::new();
    for i in 0..MAX_PLUGIN_STATE_BULK_DELETE_ENTRIES + 1 {
        too_many.push(("ns".to_string(), format!("key-{}", i)));
    }
    assert_eq!(
        store.bulk_delete(plugin, &too_many),
        Err(StateStoreError::BulkDeleteLimitExceeded {
            count: MAX_PLUGIN_STATE_BULK_DELETE_ENTRIES + 1,
            limit: MAX_PLUGIN_STATE_BULK_DELETE_ENTRIES,
        })
    );
}

#[test]
fn test_ocrm_plugin_state_runtime_health() {
    let mut store = RuntimeHealthStore::new();

    let rec1 = RuntimeHealthRecordEnvelope {
        process_token: "proc-tok-1".to_string(),
        pid: 1001,
        display_key: "db_connection".to_string(),
        failed_at_ms: 1000,
        error_reason: "Connection reset by peer".to_string(),
    };

    let rec2 = RuntimeHealthRecordEnvelope {
        process_token: "proc-tok-1".to_string(),
        pid: 1001,
        display_key: "db_connection".to_string(),
        failed_at_ms: 2000,
        error_reason: "Connection refused on retry".to_string(),
    };

    store.record_health(rec1);
    store.record_health(rec2);

    // Pick Earliest display record
    let earliest = store.get_deduplicated_records(DisplayGroupPick::Earliest);
    assert_eq!(earliest.len(), 1);
    assert_eq!(earliest[0].failed_at_ms, 1000);
    assert_eq!(earliest[0].error_reason, "Connection reset by peer");

    // Pick Latest display record
    let latest = store.get_deduplicated_records(DisplayGroupPick::Latest);
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].failed_at_ms, 2000);
    assert_eq!(latest[0].error_reason, "Connection refused on retry");

    // Sweep stale processes
    store.sweep_stale_processes(&["active-token-only"]);
    assert!(store.get_deduplicated_records(DisplayGroupPick::Latest).is_empty());
}
