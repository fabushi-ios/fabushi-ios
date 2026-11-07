use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::automation_fire_telemetry::{
    AUTOMATION_LATE_FIRE_THRESHOLD_MS, AUTOMATION_RUN_EVENT, AutomationRunReport,
    automation_run_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};

fn root() -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-automation-run-telemetry-{}-{n}",
        std::process::id()
    ))
}

fn report(lateness_ms: Option<f64>) -> AutomationRunReport {
    AutomationRunReport {
        conversation_id: "agent-automation".into(),
        automation_id: "stable-automation".into(),
        trigger: "schedule".into(),
        outcome: "ok".into(),
        is_group: false,
        duration_ms: 25.4,
        lateness_ms,
        scheduled_for_ms: Some(1_000.0),
        sent_message_count: Some(2),
        event_batch_size: Some(3),
    }
}

#[test]
fn frozen_mapper_pins_level_event_and_complete_metadata() {
    let on_time = automation_run_telemetry(&report(Some(
        AUTOMATION_LATE_FIRE_THRESHOLD_MS - 1.0,
    )));
    assert_eq!(on_time.event, Some(AUTOMATION_RUN_EVENT));
    assert_eq!(on_time.level, Some("info"));
    assert_eq!(on_time.metadata["late"], "false");

    let late = automation_run_telemetry(&report(Some(AUTOMATION_LATE_FIRE_THRESHOLD_MS)));
    assert_eq!(late.level, Some("warn"));
    assert_eq!(late.metadata["conversation_id"], "agent-automation");
    assert_eq!(late.metadata["automation_id"], "stable-automation");
    assert_eq!(late.metadata["trigger"], "schedule");
    assert_eq!(late.metadata["outcome"], "ok");
    assert_eq!(late.metadata["is_group"], "false");
    assert_eq!(late.metadata["duration_ms"], "25.4");
    assert_eq!(late.metadata["lateness_ms"], "300000");
    assert_eq!(late.metadata["scheduled_for_ms"], "1000");
    assert_eq!(late.metadata["late"], "true");
    assert_eq!(late.metadata["sent_message_count"], "2");
    assert_eq!(late.metadata["event_batch_size"], "3");
}

#[test]
fn typed_host_facade_writes_frozen_jsonl() {
    let root = root();
    let service = HostTelemetryService::open(root.join("telemetry.jsonl"))
        .expect("telemetry service");
    service
        .logs
        .report_automation_run(&report(Some(AUTOMATION_LATE_FIRE_THRESHOLD_MS)))
        .expect("automation run telemetry");

    let text = fs::read_to_string(service.records_path()).expect("jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(text.lines().next().expect("record")).expect("record json");
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, AUTOMATION_RUN_EVENT);
    assert_eq!(record.payload["level"], "warn");
    assert_eq!(record.payload["metadata"]["late"], "true");
    assert_eq!(record.payload["metadata"]["sent_message_count"], "2");
    assert_eq!(record.payload["metadata"]["event_batch_size"], "3");

    let _ = fs::remove_dir_all(root);
}
