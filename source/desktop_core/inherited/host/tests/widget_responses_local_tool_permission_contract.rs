use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::local_tool_permission::local_tool_permission_resolution::StaleLocalToolPermissionCardSettlement;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::widget_responses::WidgetResponses;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-widget-responses-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn permission_entry(
    id: &str,
    request_id: &str,
    status: &str,
    timestamp_ms: u64,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "kind": "send-message",
        "timestampMs": timestamp_ms,
        "message": {
            "type": "local-tool-permission",
            "ask": {
                "requestId": request_id,
                "status": status,
                "action": "read-file",
                "target": "/tmp/a"
            }
        }
    })
}

#[test]
fn stale_permission_card_is_retired_durably_and_second_resolution_is_idempotent() {
    let root = temp_root("stale");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = workers
        .materialize_new_session(None, "user", None)
        .expect("session");
    workers
        .append_agent_transcript_entries(
            &record.id,
            &[permission_entry("entry-1", "request-1", "pending", 10)],
        )
        .expect("append");
    let widgets = WidgetResponses::new(Arc::clone(&workers));

    assert_eq!(
        widgets
            .settle_stale_local_tool_permission_card(&record.id, "entry-1", "request-1")
            .expect("retire"),
        StaleLocalToolPermissionCardSettlement::Retired
    );
    let entry = workers
        .read_agent_transcript_entries(&record.id)
        .expect("read")
        .into_iter()
        .find(|entry| entry["id"] == "entry-1")
        .expect("entry");
    assert_eq!(entry["message"]["ask"]["status"], "expired");

    assert_eq!(
        widgets
            .settle_stale_local_tool_permission_card(&record.id, "entry-1", "request-1")
            .expect("already settled"),
        StaleLocalToolPermissionCardSettlement::Settled
    );
    assert_eq!(
        widgets
            .settle_stale_local_tool_permission_card(&record.id, "missing", "request-1")
            .expect("missing"),
        StaleLocalToolPermissionCardSettlement::NotSettled
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_bound_widget_interaction_rejects_archived_owner_without_partial_update() {
    let root = temp_root("session-bound-archive");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = workers
        .materialize_new_session(None, "user", None)
        .expect("session");
    let current = workers
        .read_session_work_admission(&record.id)
        .expect("admission")
        .expect("main admission");
    workers
        .append_agent_transcript_entries(
            &record.id,
            &[serde_json::json!({
                "id": "bound-widget",
                "kind": "send-message",
                "sessionKey": current.session_key.clone(),
                "message": {
                    "type": "widget",
                    "widget": {"prompt": "Confirm", "options": []}
                }
            })],
        )
        .expect("widget");
    workers
        .transition_session_work_archive(
            &record.id,
            &current.session_key,
            &current.session_id,
            Some(&current.lifecycle_revision),
            true,
            Some("human-1"),
            Some("review"),
            100.0,
        )
        .expect("archive");
    let widgets = WidgetResponses::new(Arc::clone(&workers));
    let error = widgets
        .dismiss_widget("bound-widget", &record.id)
        .expect_err("archived session must reject bound widget update");
    assert!(error.contains("session is not admitted"));
    let entry = workers
        .read_agent_transcript_entries(&record.id)
        .expect("transcript")
        .into_iter()
        .find(|entry| entry["id"] == "bound-widget")
        .expect("bound widget");
    assert!(entry.get("widgetDismissed").is_none());
    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_bound_permission_sweep_rejects_archived_owner_without_expiry() {
    let root = temp_root("session-bound-sweep-archive");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = workers
        .materialize_new_session(None, "user", None)
        .expect("session");
    let current = workers
        .read_session_work_admission(&record.id)
        .expect("admission")
        .expect("main admission");
    let mut entry = permission_entry("bound-permission", "bound-request", "pending", 10);
    entry["sessionKey"] = serde_json::Value::String(current.session_key.clone());
    workers
        .append_agent_transcript_entries(&record.id, &[entry])
        .expect("permission");
    workers
        .transition_session_work_archive(
            &record.id,
            &current.session_key,
            &current.session_id,
            Some(&current.lifecycle_revision),
            true,
            Some("human-1"),
            Some("review"),
            100.0,
        )
        .expect("archive");
    let error = workers
        .expire_pending_local_tool_permission_asks(&record.id, Some("bound-request"), None)
        .expect_err("archived session must reject bound stale sweep");
    assert!(error.contains("session is not admitted"));
    let entry = workers
        .read_agent_transcript_entries(&record.id)
        .expect("transcript")
        .into_iter()
        .find(|entry| entry["id"] == "bound-permission")
        .expect("permission entry");
    assert_eq!(entry["message"]["ask"]["status"], "pending");
    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn boot_sweep_expires_only_cards_pending_before_cutoff_across_agents() {
    let root = temp_root("sweep");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let first = workers
        .materialize_new_session(None, "user", None)
        .expect("first");
    let second = workers
        .materialize_new_session(None, "user", None)
        .expect("second");
    workers
        .append_agent_transcript_entries(
            &first.id,
            &[permission_entry("old", "old-request", "pending", 10)],
        )
        .expect("old");
    workers
        .append_agent_transcript_entries(
            &second.id,
            &[permission_entry("new", "new-request", "pending", 100)],
        )
        .expect("new");

    let widgets = WidgetResponses::new(Arc::clone(&workers));
    assert_eq!(
        widgets.expire_all_pending_local_tool_permission_cards(Some(50)),
        1
    );

    let old = workers
        .read_agent_transcript_entries(&first.id)
        .expect("read first");
    let new = workers
        .read_agent_transcript_entries(&second.id)
        .expect("read second");
    assert_eq!(old[0]["message"]["ask"]["status"], "expired");
    assert_eq!(new[0]["message"]["ask"]["status"], "pending");

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unanswered_widget_prompts_are_classified_and_marked_skipped_durably() {
    let root = temp_root("unanswered-prompts");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = workers
        .materialize_new_session(None, "user", None)
        .expect("session");
    workers
        .append_agent_transcript_entries(
            &record.id,
            &[
                serde_json::json!({
                    "id":"widget-open",
                    "kind":"send-message",
                    "message":{
                        "type":"widget",
                        "widget":{
                            "prompt":"Choose a path",
                            "options":[{"label":"A"},{"label":"B"}]
                        }
                    }
                }),
                serde_json::json!({
                    "id":"widget-dismissed",
                    "kind":"send-message",
                    "widgetDismissed":true,
                    "message":{
                        "type":"widget",
                        "widget":{"prompt":"Dismissed question","options":[]}
                    }
                }),
                serde_json::json!({
                    "id":"widget-answered",
                    "kind":"send-message",
                    "respondedValue":"A",
                    "message":{
                        "type":"widget",
                        "widget":{"prompt":"Answered question","options":[]}
                    }
                }),
            ],
        )
        .expect("append widgets");

    let widgets = WidgetResponses::new(Arc::clone(&workers));
    let prompts = widgets
        .collect_unanswered_question_prompts(&record.id)
        .expect("collect");
    assert_eq!(prompts.skipped_question_prompts, vec!["Choose a path — A / B"]);
    assert_eq!(
        prompts.dismissed_question_prompts,
        vec!["Dismissed question"]
    );

    let entries = workers
        .read_agent_transcript_entries(&record.id)
        .expect("read");
    let open = entries
        .iter()
        .find(|entry| entry["id"] == "widget-open")
        .expect("open");
    let dismissed = entries
        .iter()
        .find(|entry| entry["id"] == "widget-dismissed")
        .expect("dismissed");
    let answered = entries
        .iter()
        .find(|entry| entry["id"] == "widget-answered")
        .expect("answered");
    assert_eq!(open["widgetSkipped"], true);
    assert_eq!(dismissed["widgetSkipped"], true);
    assert!(answered.get("widgetSkipped").is_none());

    let second = widgets
        .collect_unanswered_question_prompts(&record.id)
        .expect("collect again");
    assert!(second.skipped_question_prompts.is_empty());
    assert!(second.dismissed_question_prompts.is_empty());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
