use std::path::PathBuf;

use mahayana_host_runtime::automations::automation::{AutomationRecord, AutomationRun};
use mahayana_host_runtime::extensions::transcript::automation_snapshot::{
    AutomationAction, AutomationSnapshot, diff_automation_action, snapshot_automations,
};
use serde_json::json;

fn record() -> AutomationRecord {
    AutomationRecord {
        id: "auto-1".into(),
        name: "Daily review".into(),
        prompt: "Review updates".into(),
        trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
        is_enabled: true,
        created_at: 100.0,
        last_run_at: Some(150.0),
        raised_notices: vec!["notice-a".into()],
        schedule: "0 9 * * *".into(),
        trigger_description: "Scheduled: 0 9 * * *".into(),
        next_run_at: Some(200.0),
        runs: vec![AutomationRun {
            id: "run-1".into(),
            trigger: "schedule".into(),
            started_at: 150.0,
            finished_at: Some(151.0),
            status: "ok".into(),
            detail: None,
            event: None,
            coalesced_run_ids: None,
        }],
        file_path: PathBuf::from("/tmp/automation.json"),
    }
}

#[test]
fn snapshot_projects_exact_frozen_fields() {
    let records = vec![record()];
    let snapshot = snapshot_automations(&records);
    let value = snapshot.get("auto-1").expect("snapshot");

    assert_eq!(value.id, "auto-1");
    assert_eq!(value.name, "Daily review");
    assert_eq!(value.prompt, "Review updates");
    assert_eq!(value.trigger_type, "cron");
    assert_eq!(value.schedule, "0 9 * * *");
    assert!(value.is_enabled);
    assert_eq!(value.created_at, 100.0);
    assert_eq!(value.recorded_run_count, 1);
    assert!(!value.trigger.is_empty());
}

#[test]
fn diff_matches_frozen_updated_then_enablement_precedence() {
    let base = AutomationSnapshot {
        id: "auto-1".into(),
        name: "Daily".into(),
        prompt: "Review".into(),
        trigger: "cron:0 9 * * *".into(),
        trigger_type: "cron".into(),
        schedule: "0 9 * * *".into(),
        is_enabled: true,
        created_at: 100.0,
        recorded_run_count: 1,
    };

    let mut changed = base.clone();
    changed.name = "Daily updated".into();
    changed.is_enabled = false;
    assert_eq!(
        diff_automation_action(&base, &changed),
        Some(AutomationAction::Updated)
    );

    let mut disabled = base.clone();
    disabled.is_enabled = false;
    assert_eq!(
        diff_automation_action(&base, &disabled),
        Some(AutomationAction::Disabled)
    );

    let mut enabled_before = base.clone();
    enabled_before.is_enabled = false;
    assert_eq!(
        diff_automation_action(&enabled_before, &base),
        Some(AutomationAction::Enabled)
    );
}

#[test]
fn diff_ignores_snapshot_metadata_not_used_by_frozen_contract() {
    let base = AutomationSnapshot {
        id: "auto-1".into(),
        name: "Daily".into(),
        prompt: "Review".into(),
        trigger: "cron:0 9 * * *".into(),
        trigger_type: "cron".into(),
        schedule: "0 9 * * *".into(),
        is_enabled: true,
        created_at: 100.0,
        recorded_run_count: 1,
    };
    let mut metadata_only = base.clone();
    metadata_only.schedule = "30 9 * * *".into();
    metadata_only.trigger_type = "event".into();
    metadata_only.created_at = 999.0;
    metadata_only.recorded_run_count = 25;

    assert_eq!(diff_automation_action(&base, &metadata_only), None);
}
