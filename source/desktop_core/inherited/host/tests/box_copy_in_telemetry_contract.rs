use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::box_store_sync::box_copy_in::{
    CopyInOutcome, CopyInResult, build_copy_in_result_telemetry,
    build_copy_in_started_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::box_copy_in_telemetry::{
    BOX_COPY_IN_EVENT, box_copy_in_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-box-copy-in-telemetry-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn frozen_copy_in_mapper_fixes_event_and_preserves_producer_level_metadata() {
    let projection = box_copy_in_telemetry(
        "warn",
        BTreeMap::from([
            ("outcome".into(), "stuck".into()),
            ("reason".into(), "moving-data-in-exceeded-threshold".into()),
            ("duration_ms".into(), "300000".into()),
        ]),
    );
    assert_eq!(projection.level, Some("warn"));
    assert_eq!(projection.event, Some(BOX_COPY_IN_EVENT));
    assert_eq!(projection.metadata["outcome"], "stuck");
    assert_eq!(
        projection.metadata["reason"],
        "moving-data-in-exceeded-threshold"
    );
    assert_eq!(projection.metadata["duration_ms"], "300000");
}

#[test]
fn real_copy_in_result_shaping_preserves_frozen_failure_and_optional_field_semantics() {
    let failed = CopyInResult {
        outcome: CopyInOutcome::Failed,
        reason: "download threw: network reset".into(),
        manifest_entries: 9,
        store_db_entries: 2,
        restored_store_db_entries: None,
        files: 4,
        bytes: 8192,
        verified: 3,
        failures: vec![
            "network reset https://private.example/object token=secret".into(),
        ],
        hydrate_source: None,
    };
    let event = build_copy_in_result_telemetry(&failed, 1250);
    assert_eq!(event.level, "error");
    assert_eq!(event.metadata["outcome"], "failed");
    assert_eq!(event.metadata["reason"], "download-error");
    assert_eq!(event.metadata["error_class"], "network");
    assert_eq!(
        event.metadata["error_summary"],
        "network reset <url> <redacted>"
    );
    assert_eq!(event.metadata["manifest_entries"], "9");
    assert_eq!(event.metadata["store_db_entries"], "2");
    assert_eq!(event.metadata["files"], "4");
    assert_eq!(event.metadata["bytes"], "8192");
    assert_eq!(event.metadata["verified"], "3");
    assert_eq!(event.metadata["failures"], "1");
    assert_eq!(event.metadata["duration_ms"], "1250");
    assert!(!event.metadata.contains_key("hydrate_source"));
    assert!(!event.metadata.contains_key("restored_store_db_entries"));
    assert!(!event.metadata.contains_key("store_backend"));

    let hydrated = CopyInResult {
        outcome: CopyInOutcome::Hydrated,
        reason: "store hydrated from legacy (v2 migration)".into(),
        manifest_entries: 6,
        store_db_entries: 2,
        restored_store_db_entries: Some(2),
        files: 6,
        bytes: 4096,
        verified: 6,
        failures: vec![],
        hydrate_source: Some("legacy".into()),
    };
    let event = build_copy_in_result_telemetry(&hydrated, 88);
    assert_eq!(event.level, "info");
    assert_eq!(event.metadata["outcome"], "hydrated");
    assert_eq!(event.metadata["hydrate_source"], "legacy");
    assert_eq!(event.metadata["restored_store_db_entries"], "2");
    assert_eq!(event.metadata["error_class"], "");
    assert_eq!(event.metadata["error_summary"], "");

    let started = build_copy_in_started_telemetry();
    assert_eq!(started.level, "info");
    assert_eq!(
        started.metadata,
        BTreeMap::from([
            ("duration_ms".into(), "0".into()),
            ("outcome".into(), "started".into()),
            ("reason".into(), "hydrate-started".into()),
        ])
    );
}

#[test]
fn typed_host_facade_writes_copy_in_jsonl_with_identity_and_failure_semantics() {
    let root = temp_root();
    let service = HostTelemetryService::open_with_identity_tags(
        root.join("events.jsonl"),
        BTreeMap::from([
            ("store_backend".into(), "sand-box-store-v2".into()),
            ("box_store_id".into(), "store-42".into()),
        ]),
    )
    .expect("telemetry");

    let failed = CopyInResult {
        outcome: CopyInOutcome::Failed,
        reason: "lock-held".into(),
        manifest_entries: 0,
        store_db_entries: 0,
        restored_store_db_entries: None,
        files: 0,
        bytes: 0,
        verified: 0,
        failures: vec![],
        hydrate_source: None,
    };
    let event = build_copy_in_result_telemetry(&failed, 17);
    service
        .logs
        .report_box_copy_in(event.level, event.metadata)
        .expect("typed copy-in report");

    let text = fs::read_to_string(service.records_path()).expect("jsonl");
    let records = text
        .lines()
        .map(|line| {
            serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record")
        })
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, BOX_COPY_IN_EVENT);
    assert_eq!(record.payload["level"], "error");
    assert_eq!(record.payload["metadata"]["outcome"], "failed");
    assert_eq!(record.payload["metadata"]["reason"], "error");
    assert_eq!(record.payload["metadata"]["error_class"], "lock-held");
    assert_eq!(record.payload["metadata"]["duration_ms"], "17");
    assert_eq!(
        record.payload["metadata"]["store_backend"],
        "sand-box-store-v2"
    );
    assert_eq!(record.payload["metadata"]["box_store_id"], "store-42");
    assert!(record.payload["metadata"].get("hydrate_source").is_none());
    assert!(record.payload["metadata"].get("restored_store_db_entries").is_none());

    let _ = fs::remove_dir_all(root);
}
