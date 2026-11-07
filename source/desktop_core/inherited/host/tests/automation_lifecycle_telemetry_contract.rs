use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::automation_lifecycle_telemetry::{
    AUTOMATION_LIFECYCLE_EVENT, automation_lifecycle_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::{
    HostTelemetryService, PersistedHostTelemetryRecord,
};
use mahayana_host_runtime::extensions::transcript::automation_runtime::{
    AutomationLifecycleAction, AutomationLifecycleEvent, AutomationLifecycleSource,
};

fn root() -> std::path::PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).expect("time").as_nanos();
    let root = std::env::temp_dir().join(format!("automation-lifecycle-row74-{}-{n}", std::process::id()));
    fs::create_dir_all(&root).expect("root");
    root
}

fn report() -> AutomationLifecycleEvent {
    AutomationLifecycleEvent {
        agent_id: "agent-74".into(),
        action: AutomationLifecycleAction::Created,
        automation_id: "local-74".into(),
        stable_automation_id: "stable-74".into(),
        automation_name: "Daily".into(),
        trigger_type: "cron".into(),
        created_at: 1_000.0,
        age_ms: 2_500.0,
        recorded_run_count: 3,
        scheduled_fires_next_7_days: Some(7),
        fires_on_weekend: Some(true),
        fires_overnight: Some(false),
        source: AutomationLifecycleSource::AutomationsUi,
    }
}

#[test]
fn frozen_mapper_fixes_info_event_and_complete_metadata() {
    let projection = automation_lifecycle_telemetry(&report());
    assert_eq!(projection.level, Some("info"));
    assert_eq!(projection.event, Some(AUTOMATION_LIFECYCLE_EVENT));
    assert_eq!(projection.metadata["conversation_id"], "agent-74");
    assert_eq!(projection.metadata["automation_id"], "stable-74");
    assert_eq!(projection.metadata["action"], "created");
    assert_eq!(projection.metadata["source"], "automations_ui");
    assert_eq!(projection.metadata["trigger_type"], "cron");
    assert_eq!(projection.metadata["scheduled_fires_next_7_days"], "7");
    assert_eq!(projection.metadata["fires_on_weekend"], "true");
    assert_eq!(projection.metadata["fires_overnight"], "false");
    assert_eq!(projection.metadata["age_ms"], "2500");
    assert_eq!(projection.metadata["recorded_run_count"], "3");
    assert_eq!(projection.metadata.len(), 10);
}

#[test]
fn typed_host_facade_writes_jsonl_and_omits_noncron_schedule_fields() {
    let root = root();
    let service = HostTelemetryService::open_with_identity_tags(
        root.join("events.jsonl"),
        BTreeMap::from([("box_store_id".into(), "store-74".into())]),
    ).expect("telemetry");
    let mut value = report();
    value.action = AutomationLifecycleAction::Deleted;
    value.source = AutomationLifecycleSource::Agent;
    value.trigger_type = "slack".into();
    value.scheduled_fires_next_7_days = None;
    value.fires_on_weekend = None;
    value.fires_overnight = None;
    service.logs.report_automation_lifecycle(&value).expect("report");

    let raw = fs::read_to_string(service.records_path()).expect("jsonl");
    let record: PersistedHostTelemetryRecord =
        serde_json::from_str(raw.lines().next().expect("record")).expect("json");
    assert_eq!(record.channel, "structured_log");
    assert_eq!(record.event, AUTOMATION_LIFECYCLE_EVENT);
    assert_eq!(record.payload["level"], "info");
    assert_eq!(record.payload["metadata"]["automation_id"], "stable-74");
    assert_eq!(record.payload["metadata"]["action"], "deleted");
    assert_eq!(record.payload["metadata"]["source"], "agent");
    assert_eq!(record.payload["metadata"]["box_store_id"], "store-74");
    assert!(record.payload["metadata"].get("scheduled_fires_next_7_days").is_none());
    assert!(record.payload["metadata"].get("fires_on_weekend").is_none());
    assert!(record.payload["metadata"].get("fires_overnight").is_none());
    assert!(record.payload["metadata"].get("automation_name").is_none());
    let _ = fs::remove_dir_all(root);
}
