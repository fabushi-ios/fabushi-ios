use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::automations::automation_status_reminder::{
    AUTOMATION_STATUS_PROMPT_MARKER, create_automation_status_reminder,
    render_automation_cleared_status_reminder,
};
use mahayana_host_runtime::automations::automation_store::FileAutomationStore;
use serde_json::json;

fn root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-automation-status-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn provider_projects_live_definition_runs_and_hides_the_firing_run() {
    let root = root("provider");
    let store = FileAutomationStore::with_user_time_zone_resolver(
        root.join("automations"),
        Arc::new(|| Some("UTC".to_string())),
    );
    let record = store
        .upsert(
            &AutomationSpec {
                name: "Morning Check".into(),
                prompt: "check inbox".into(),
                trigger: json!({"type":"cron","schedule":"0 9 * * 1-5"}),
                is_enabled: None,
            },
            1_700_000_000_000.0,
        )
        .unwrap()
        .unwrap();
    store
        .begin_run(
            &record.id,
            "schedule",
            1_700_000_060_000.0,
            None,
            Some("run-live"),
            None,
        )
        .unwrap()
        .unwrap();

    let live = create_automation_status_reminder(&store, None).unwrap();
    assert!(live.contains(AUTOMATION_STATUS_PROMPT_MARKER));
    assert!(live.contains("Morning Check (folder morning-check)"));
    assert!(live.contains("running now (started "));

    let firing = create_automation_status_reminder(&store, Some(&record.id)).unwrap();
    assert!(firing.contains("Morning Check (folder morning-check): never run"));
    assert!(!firing.contains("running now"));

    store
        .finish_run(
            &record.id,
            "run-live",
            "ok",
            1_700_000_120_000.0,
            None,
        )
        .unwrap()
        .unwrap();
    let settled = create_automation_status_reminder(&store, Some(&record.id)).unwrap();
    assert!(settled.contains("(succeeded)"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn provider_is_empty_for_no_definitions_and_cleared_snapshot_is_explicit() {
    let root = root("empty");
    let store = FileAutomationStore::new(root.join("automations"));
    assert!(create_automation_status_reminder(&store, None).is_none());

    let cleared = render_automation_cleared_status_reminder();
    assert!(cleared.contains(AUTOMATION_STATUS_PROMPT_MARKER));
    assert!(cleared.contains("No current routines."));

    let _ = fs::remove_dir_all(root);
}
