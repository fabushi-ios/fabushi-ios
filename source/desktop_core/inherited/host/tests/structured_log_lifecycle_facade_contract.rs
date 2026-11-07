use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::host_lifecycle_progress::{
    HostLifecycleError, HostLifecycleReport,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};

static TEMP_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = TEMP_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fabushi-structured-log-lifecycle-{}-{suffix}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn startup_and_lifecycle_facades_use_frozen_structured_log_mappers() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("host-events.jsonl"))
        .expect("telemetry service");
    service.start().expect("telemetry start");

    service
        .logs
        .report_host_startup(BTreeMap::from([
            ("auto_update".into(), "true".into()),
            ("duration_ms".into(), "42".into()),
            ("host_bundle_version".into(), "0.18-test".into()),
        ]))
        .expect("host startup structured log");
    service
        .logs
        .report_host_lifecycle(HostLifecycleReport::Failed {
            phase: "transcript_read".into(),
            duration_ms: 77,
            error: HostLifecycleError::Failed,
        })
        .expect("host lifecycle structured log");

    let text = fs::read_to_string(service.records_path()).expect("telemetry jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);

    let startup = &records[0];
    assert_eq!(startup.channel, "structured_log");
    assert_eq!(startup.event, "sand.host.startup");
    assert_eq!(startup.payload["level"], "info");
    assert_eq!(startup.payload["metadata"]["auto_update"], "true");
    assert_eq!(startup.payload["metadata"]["duration_ms"], "42");
    assert_eq!(
        startup.payload["metadata"]["host_bundle_version"],
        "0.18-test"
    );
    assert!(startup.payload["metadata"]["host_built_at_ms"].is_string());

    let lifecycle = &records[1];
    assert_eq!(lifecycle.channel, "structured_log");
    assert_eq!(lifecycle.event, "sand.host.lifecycle");
    assert_eq!(lifecycle.payload["level"], "error");
    assert_eq!(lifecycle.payload["metadata"]["outcome"], "failed");
    assert_eq!(lifecycle.payload["metadata"]["phase"], "transcript_read");
    assert_eq!(lifecycle.payload["metadata"]["duration_ms"], "77");
    assert_eq!(lifecycle.payload["metadata"]["error_code"], "SAND-E0303");
    assert_eq!(lifecycle.payload["metadata"]["error_domain"], "rebuild");

    service.dispose().expect("telemetry dispose");
    fs::remove_dir_all(root).ok();
}
