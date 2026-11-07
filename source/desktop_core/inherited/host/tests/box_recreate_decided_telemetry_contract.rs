use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::box_recreate_decided_telemetry::{
    BOX_RECREATE_DECIDED_EVENT, box_recreate_decided_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("time").as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-box-recreate-decided-telemetry-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn frozen_recreate_decided_mapper_fixes_info_event_and_preserves_metadata() {
    let metadata = BTreeMap::from([
        ("trigger".into(), "hibernation_auto_update".into()),
        ("mode".into(), "pod_recreate".into()),
        ("preserved".into(), "true".into()),
    ]);
    let projection = box_recreate_decided_telemetry(metadata.clone());
    assert_eq!(projection.level, Some("info"));
    assert_eq!(projection.event, Some(BOX_RECREATE_DECIDED_EVENT));
    assert_eq!(projection.metadata, metadata);
}

#[test]
fn typed_host_facade_writes_recreate_decided_jsonl_with_identity() {
    let root = temp_root();
    let service = HostTelemetryService::open_with_identity_tags(
        root.join("events.jsonl"),
        BTreeMap::from([("box_store_id".into(), "store-70".into())]),
    ).expect("telemetry");
    service.logs.report_box_recreate_decided(BTreeMap::from([
        ("trigger".into(), "auto_update".into()),
        ("mode".into(), "pod_recreate".into()),
        ("preserved".into(), "true".into()),
    ])).expect("typed recreate decision");
    let text = fs::read_to_string(service.records_path()).expect("jsonl");
    let records = text.lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, BOX_RECREATE_DECIDED_EVENT);
    assert_eq!(record.payload["level"], "info");
    assert_eq!(record.payload["metadata"]["trigger"], "auto_update");
    assert_eq!(record.payload["metadata"]["mode"], "pod_recreate");
    assert_eq!(record.payload["metadata"]["preserved"], "true");
    assert_eq!(record.payload["metadata"]["box_store_id"], "store-70");
    let _ = fs::remove_dir_all(root);
}
