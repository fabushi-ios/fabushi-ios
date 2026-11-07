use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};
use mahayana_host_runtime::gateway_server::GatewayCommandReport;

static TEMP_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = TEMP_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fabushi-gateway-command-telemetry-{}-{suffix}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn gateway_command_facade_preserves_frozen_success_and_error_shape() {
    let root = temp_root();
    let service =
        HostTelemetryService::open(root.join("gateway-events.jsonl")).expect("telemetry service");
    service.start().expect("telemetry start");

    let success = GatewayCommandReport {
        method: "sendPrompt".into(),
        duration_ms: 17,
        request_id: Some("req-success".into()),
        traceparent: Some("00-0123456789abcdef0123456789abcdef-0123456789abcdef-01".into()),
        status: 200,
        error: None,
        reason: None,
        error_class: None,
        errno: None,
    };
    service
        .logs
        .report_gateway_command_timing(&success)
        .expect("gateway timing");

    let failure = GatewayCommandReport {
        method: "runner.startRoutedProvider".into(),
        duration_ms: 41,
        request_id: Some("req-failure".into()),
        traceparent: Some("00-fedcba9876543210fedcba9876543210-fedcba9876543210-01".into()),
        status: 503,
        error: Some("provider unavailable".into()),
        reason: Some("upstream".into()),
        error_class: Some("connection".into()),
        errno: Some("ECONNREFUSED".into()),
    };
    service
        .logs
        .report_gateway_command_error(&failure)
        .expect("gateway error");

    let records = fs::read_to_string(service.records_path())
        .expect("telemetry jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);

    assert_eq!(records[0].event, "sand.gateway_command_timing");
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(records[0].payload["metadata"]["method"], "sendPrompt");
    assert_eq!(records[0].payload["metadata"]["duration_ms"], "17");
    assert_eq!(records[0].payload["metadata"]["request_id"], "req-success");
    assert_eq!(
        records[0].payload["metadata"]["trace_id"],
        "0123456789abcdef0123456789abcdef"
    );
    assert_eq!(
        records[0].payload["metadata"]["span_id"],
        "0123456789abcdef"
    );
    assert!(records[0].payload["metadata"]["status"].is_null());
    assert!(records[0].payload["metadata"]["traceparent"].is_null());
    assert!(records[0].payload["metadata"]["error"].is_null());

    assert_eq!(records[1].event, "sand.gateway_command_error");
    assert_eq!(records[1].payload["level"], "error");
    assert_eq!(
        records[1].payload["metadata"]["method"],
        "runner.startRoutedProvider"
    );
    assert_eq!(records[1].payload["metadata"]["duration_ms"], "41");
    assert_eq!(records[1].payload["metadata"]["request_id"], "req-failure");
    assert_eq!(records[1].payload["metadata"]["reason"], "upstream");
    assert_eq!(records[1].payload["metadata"]["error_class"], "connection");
    assert_eq!(records[1].payload["metadata"]["errno"], "ECONNREFUSED");
    assert_eq!(
        records[1].payload["metadata"]["trace_id"],
        "fedcba9876543210fedcba9876543210"
    );
    assert_eq!(
        records[1].payload["metadata"]["span_id"],
        "fedcba9876543210"
    );
    assert!(records[1].payload["metadata"]["status"].is_null());
    assert!(records[1].payload["metadata"]["traceparent"].is_null());
    assert!(records[1].payload["metadata"]["error"].is_null());

    service.dispose().expect("telemetry dispose");
    fs::remove_dir_all(root).ok();
}
