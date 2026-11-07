use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, McpDiscoveryFailedGatewayError, PersistedHostTelemetryRecord,
    REPORT_MCP_DISCOVERY_FAILED_GATEWAY_METHOD, dispatch_mcp_discovery_failed_gateway,
};
use mahayana_host_runtime::extensions::telemetry::mcp_discovery_telemetry::{
    MCP_DISCOVERY_FAILED_EVENT, McpDiscoveryFailedReport, mcp_discovery_failed_telemetry,
};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-mcp-discovery-telemetry-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn frozen_mapper_preserves_warn_event_elapsed_rounding_and_stale_flag() {
    let projection = mcp_discovery_failed_telemetry(&McpDiscoveryFailedReport {
        error_class: "TimeoutError".into(),
        elapsed_ms: 12.6,
        served_stale: true,
    });
    assert_eq!(projection.level, Some("warn"));
    assert_eq!(projection.event, Some(MCP_DISCOVERY_FAILED_EVENT));
    assert_eq!(projection.metadata.len(), 3);
    assert_eq!(projection.metadata["error_class"], "TimeoutError");
    assert_eq!(projection.metadata["elapsed_ms"], "13");
    assert_eq!(projection.metadata["served_stale"], "true");
}

#[test]
fn shipping_gateway_routes_failure_through_typed_host_owner_into_jsonl() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("events.jsonl")).expect("telemetry");

    for (error_class, elapsed_ms, served_stale) in [
        ("TimeoutError", 12.6, true),
        ("TypeError", 8.2, false),
    ] {
        let result = dispatch_mcp_discovery_failed_gateway(
            &service.logs,
            REPORT_MCP_DISCOVERY_FAILED_GATEWAY_METHOD,
            &serde_json::json!({
                "errorClass": error_class,
                "elapsedMs": elapsed_ms,
                "servedStale": served_stale
            }),
        )
        .expect("method owned")
        .expect("failure reported");
        assert_eq!(result["reported"], true);
    }

    let text = fs::read_to_string(service.records_path()).expect("jsonl");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2, "one producer report must yield one Host-owned record");
    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, MCP_DISCOVERY_FAILED_EVENT);
    assert_eq!(records[0].payload["level"], "warn");
    assert_eq!(records[0].payload["metadata"]["error_class"], "TimeoutError");
    assert_eq!(records[0].payload["metadata"]["elapsed_ms"], "13");
    assert_eq!(records[0].payload["metadata"]["served_stale"], "true");
    assert_eq!(records[1].event, MCP_DISCOVERY_FAILED_EVENT);
    assert_eq!(records[1].payload["metadata"]["error_class"], "TypeError");
    assert_eq!(records[1].payload["metadata"]["elapsed_ms"], "8");
    assert_eq!(records[1].payload["metadata"]["served_stale"], "false");

    assert!(
        dispatch_mcp_discovery_failed_gateway(&service.logs, "unrelated", &serde_json::json!({}))
            .is_none()
    );
    let malformed = dispatch_mcp_discovery_failed_gateway(
        &service.logs,
        REPORT_MCP_DISCOVERY_FAILED_GATEWAY_METHOD,
        &serde_json::json!({
            "errorClass": "",
            "elapsedMs": -1,
            "servedStale": "yes"
        }),
    )
    .expect("method owned")
    .expect_err("malformed report rejected");
    assert!(matches!(malformed, McpDiscoveryFailedGatewayError::BadRequest(_)));
    assert_eq!(
        fs::read_to_string(service.records_path()).expect("jsonl").lines().count(),
        2,
        "rejected payload must not emit a structured record"
    );

    let _ = fs::remove_dir_all(root);
}
