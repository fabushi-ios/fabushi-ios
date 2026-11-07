use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};
use mahayana_host_runtime::extensions::telemetry::invariant_violation_telemetry::{
    HOST_INVARIANT_VIOLATION_EVENT, invariant_violation_telemetry,
};
use mahayana_host_runtime::host_invariant::{
    SAND_INVARIANT_VIOLATION_NAME, SandInvariantReport,
};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-row82-invariant-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn frozen_invariant_mapper_fixes_error_event_and_name_only_metadata() {
    let projection = invariant_violation_telemetry(&SandInvariantReport {
        name: SAND_INVARIANT_VIOLATION_NAME.into(),
        frame: Some("at hidden/site.ts:1:2".into()),
    });
    assert_eq!(projection.level, Some("error"));
    assert_eq!(projection.event, Some(HOST_INVARIANT_VIOLATION_EVENT));
    assert_eq!(projection.metadata.len(), 1);
    assert_eq!(
        projection.metadata["name"],
        SAND_INVARIANT_VIOLATION_NAME
    );
    assert!(!projection.metadata.contains_key("frame"));
}

#[test]
fn typed_host_facade_writes_invariant_jsonl_without_frame() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("events.jsonl")).expect("telemetry");
    service
        .logs
        .report_invariant_violation(&SandInvariantReport {
            name: SAND_INVARIANT_VIOLATION_NAME.into(),
            frame: Some("at production/site.rs:7:9".into()),
        })
        .expect("invariant telemetry");

    let records = fs::read_to_string(service.records_path())
        .expect("jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, HOST_INVARIANT_VIOLATION_EVENT);
    assert_eq!(records[0].payload["level"], "error");
    assert_eq!(
        records[0].payload["metadata"]["name"],
        SAND_INVARIANT_VIOLATION_NAME
    );
    assert!(records[0].payload["metadata"].get("frame").is_none());

    let _ = fs::remove_dir_all(root);
}
