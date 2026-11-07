use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::agent_open_telemetry::{
    AGENT_OPEN_EVENT, AgentOpenReport, agent_open_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-row79-agent-open-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn frozen_agent_open_mapper_fixes_info_event_and_complete_metadata() {
    let projection = agent_open_telemetry(&AgentOpenReport {
        conversation_id: "agent-row79".into(),
        duration_ms: 17,
        entry_count: 23,
        was_active: false,
    });
    assert_eq!(projection.level, Some("info"));
    assert_eq!(projection.event, Some(AGENT_OPEN_EVENT));
    assert_eq!(projection.metadata.len(), 4);
    assert_eq!(projection.metadata["conversation_id"], "agent-row79");
    assert_eq!(projection.metadata["duration_ms"], "17");
    assert_eq!(projection.metadata["entry_count"], "23");
    assert_eq!(projection.metadata["was_active"], "false");
}

#[test]
fn typed_host_facade_writes_agent_open_jsonl() {
    let root = temp_root();
    let service = HostTelemetryService::open(root.join("events.jsonl")).expect("telemetry");
    service
        .logs
        .report_agent_open(&AgentOpenReport {
            conversation_id: "agent-row79".into(),
            duration_ms: 9,
            entry_count: 4,
            was_active: true,
        })
        .expect("agent open");

    let records = fs::read_to_string(service.records_path())
        .expect("jsonl")
        .lines()
        .map(|line| serde_json::from_str::<PersistedHostTelemetryRecord>(line).expect("record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].channel, "structured_log");
    assert_eq!(records[0].event, AGENT_OPEN_EVENT);
    assert_eq!(records[0].payload["level"], "info");
    assert_eq!(records[0].payload["metadata"]["conversation_id"], "agent-row79");
    assert_eq!(records[0].payload["metadata"]["duration_ms"], "9");
    assert_eq!(records[0].payload["metadata"]["entry_count"], "4");
    assert_eq!(records[0].payload["metadata"]["was_active"], "true");

    let _ = fs::remove_dir_all(root);
}
