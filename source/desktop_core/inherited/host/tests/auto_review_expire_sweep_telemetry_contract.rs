use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};

fn root() -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-auto-review-expire-sweep-{}-{n}",
        std::process::id()
    ))
}

#[test]
fn typed_host_facade_writes_frozen_warn_event_and_metadata() {
    let root = root();
    let service = HostTelemetryService::open(root.join("telemetry.jsonl"))
        .expect("telemetry service");
    service
        .logs
        .report_auto_review_expire_sweep_failed(
            "expire_pending_auto_review_approvals",
            "Error",
        )
        .expect("expire sweep telemetry");

    let text = fs::read_to_string(service.records_path()).expect("jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(text.lines().next().expect("record")).expect("record json");
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, "sand.auto_review.expire_sweep_failed");
    assert_eq!(record.payload["level"], "warn");
    assert_eq!(
        record.payload["metadata"]["stage"],
        "expire_pending_auto_review_approvals"
    );
    assert_eq!(record.payload["metadata"]["error_class"], "Error");

    let _ = fs::remove_dir_all(root);
}
