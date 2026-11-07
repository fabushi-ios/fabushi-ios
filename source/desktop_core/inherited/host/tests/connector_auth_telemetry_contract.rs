use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::connector_auth_telemetry::{
    CONNECTOR_AUTH_EVENT, ConnectorAuthReport, connector_auth_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    ConnectorAuthGatewayError, HostTelemetryService, PersistedHostTelemetryRecord,
    REPORT_CONNECTOR_AUTH_GATEWAY_METHOD, dispatch_connector_auth_gateway,
};
use mahayana_host_runtime::extensions::telemetry::sand_error_tags::SandErrorValue;

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("time").as_nanos();
    let root = std::env::temp_dir().join(format!(
        "fabushi-connector-auth-telemetry-{}-{nonce}", std::process::id()
    ));
    fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn frozen_mapper_preserves_level_bounding_surface_and_error_tags() {
    let ok = connector_auth_telemetry(
        &ConnectorAuthReport {
            phase: "flow_started".into(),
            outcome: "ok".into(),
            server_name: Some("Google Drive".into()),
            server_id: Some("srv_123".into()),
            reauth: Some(true),
            error: None,
        },
        "host",
    );
    assert_eq!(ok.level, Some("info"));
    assert_eq!(ok.event, Some(CONNECTOR_AUTH_EVENT));
    assert_eq!(ok.metadata["phase"], "flow_started");
    assert_eq!(ok.metadata["connector"], "googledrive");
    assert_eq!(ok.metadata["outcome"], "ok");
    assert_eq!(ok.metadata["surface"], "host");
    assert_eq!(ok.metadata["server_id"], "srv_123");
    assert_eq!(ok.metadata["reauth"], "true");
    assert_eq!(ok.metadata.len(), 6);

    let failed = connector_auth_telemetry(
        &ConnectorAuthReport {
            phase: "callback_received".into(),
            outcome: "failed".into(),
            server_name: Some("Private Connector!".into()),
            server_id: Some("not a branded id".into()),
            reauth: None,
            error: Some(
                SandErrorValue::new("SAND-E0205")
                    .with_string("reason", "completion_timeout"),
            ),
        },
        "host",
    );
    assert_eq!(failed.level, Some("warn"));
    assert_eq!(failed.event, Some(CONNECTOR_AUTH_EVENT));
    assert_eq!(failed.metadata["connector"], "other");
    assert_eq!(failed.metadata["surface"], "host");
    assert!(!failed.metadata.contains_key("server_id"));
    assert!(!failed.metadata.contains_key("reauth"));
    assert_eq!(failed.metadata["error_code"], "SAND-E0205");
    assert_eq!(failed.metadata["error_domain"], "auth");
    assert_eq!(failed.metadata["error_retryable"], "true");
    assert_eq!(failed.metadata["reason"], "completion_timeout");

    let timeout = connector_auth_telemetry(
        &ConnectorAuthReport {
            phase: "token_stored".into(),
            outcome: "timeout".into(),
            server_name: None,
            server_id: None,
            reauth: None,
            error: Some(SandErrorValue::new("SAND-E0206")),
        },
        "host",
    );
    assert_eq!(timeout.level, Some("warn"));
    assert_eq!(timeout.metadata["connector"], "unknown");

    let max_server_id = "a".repeat(128);
    let cancelled = connector_auth_telemetry(
        &ConnectorAuthReport {
            phase: "token_stored".into(),
            outcome: "cancelled".into(),
            server_name: Some("GitHub".into()),
            server_id: Some(max_server_id.clone()),
            reauth: None,
            error: None,
        },
        "host",
    );
    assert_eq!(cancelled.level, Some("info"));
    assert_eq!(cancelled.metadata["server_id"], max_server_id);

    let too_long_server_id = connector_auth_telemetry(
        &ConnectorAuthReport {
            phase: "flow_started".into(),
            outcome: "ok".into(),
            server_name: Some("GitHub".into()),
            server_id: Some("a".repeat(129)),
            reauth: None,
            error: None,
        },
        "host",
    );
    assert!(!too_long_server_id.metadata.contains_key("server_id"));
}

#[test]
fn shipping_gateway_routes_typed_connector_auth_reports_into_host_jsonl() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("events.jsonl")).expect("telemetry");

    for report in [
        serde_json::json!({
            "phase": "flow_started", "outcome": "ok", "serverName": "GitHub",
            "serverId": "server_42", "reauth": true
        }),
        serde_json::json!({
            "phase": "callback_received", "outcome": "failed", "serverName": "Private Connector!",
            "serverId": "bad id", "error": {"code": "SAND-E0205", "reason": "completion_timeout"}
        }),
    ] {
        let result = dispatch_connector_auth_gateway(
            &service.logs, REPORT_CONNECTOR_AUTH_GATEWAY_METHOD, &report,
        )
        .expect("method owned")
        .expect("report emitted");
        assert_eq!(result["reported"], true);
    }

    let text = fs::read_to_string(service.records_path()).expect("jsonl");
    let records = text.lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2, "one typed producer report must yield one Host record");
    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, CONNECTOR_AUTH_EVENT);
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(records[0].payload["metadata"]["phase"], "flow_started");
    assert_eq!(records[0].payload["metadata"]["connector"], "github");
    assert_eq!(records[0].payload["metadata"]["outcome"], "ok");
    assert_eq!(records[0].payload["metadata"]["surface"], "host");
    assert_eq!(records[0].payload["metadata"]["server_id"], "server_42");
    assert_eq!(records[0].payload["metadata"]["reauth"], "true");
    assert_eq!(records[1].event, CONNECTOR_AUTH_EVENT);
    assert_eq!(records[1].payload["level"], "warn");
    assert_eq!(records[1].payload["metadata"]["connector"], "other");
    assert_eq!(records[1].payload["metadata"]["surface"], "host");
    assert!(records[1].payload["metadata"].get("server_id").is_none());
    assert_eq!(records[1].payload["metadata"]["error_code"], "SAND-E0205");
    assert_eq!(records[1].payload["metadata"]["error_domain"], "auth");
    assert_eq!(records[1].payload["metadata"]["error_retryable"], "true");
    assert_eq!(records[1].payload["metadata"]["reason"], "completion_timeout");

    assert!(dispatch_connector_auth_gateway(&service.logs, "unrelated", &serde_json::json!({})).is_none());
    let malformed = dispatch_connector_auth_gateway(
        &service.logs,
        REPORT_CONNECTOR_AUTH_GATEWAY_METHOD,
        &serde_json::json!({"phase":"flow_started","outcome":"ok","reauth":"yes"}),
    )
    .expect("method owned")
    .expect_err("malformed report rejected");
    assert!(matches!(malformed, ConnectorAuthGatewayError::BadRequest(_)));
    assert_eq!(
        fs::read_to_string(service.records_path()).expect("jsonl").lines().count(), 2,
        "rejected payload must not emit a structured record"
    );
    let _ = fs::remove_dir_all(root);
}
