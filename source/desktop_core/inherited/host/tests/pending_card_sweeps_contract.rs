use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-pending-card-sweeps-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn nested_status(
    entries: &[serde_json::Value],
    id: &str,
    nested: &str,
) -> Option<String> {
    entries
        .iter()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(id))
        .and_then(|entry| entry.get("message"))
        .and_then(|message| message.get(nested))
        .and_then(|value| value.get("status"))
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

#[test]
fn production_pending_card_sweeps_match_frozen_grok_filters_and_persist_status() {
    let root = temp_root("shipping");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let created = workers
        .materialize_new_session(None, "user", None)
        .expect("materialize");
    let agent_id = created.id;

    workers
        .append_agent_transcript_entries(
            &agent_id,
            &[
                serde_json::json!({
                    "id":"auto-a",
                    "kind":"send-message",
                    "timestampMs":10,
                    "message":{
                        "type":"auto-review-approval",
                        "approval":{"requestId":"req-auto-a","status":"pending","rule":"one"}
                    }
                }),
                serde_json::json!({
                    "id":"auto-b",
                    "kind":"send-message",
                    "timestampMs":20,
                    "message":{
                        "type":"auto-review-approval",
                        "approval":{"requestId":"req-auto-b","status":"pending"}
                    }
                }),
                serde_json::json!({
                    "id":"auto-complete",
                    "kind":"send-message",
                    "message":{
                        "type":"auto-review-approval",
                        "approval":{"requestId":"req-auto-complete","status":"approved"}
                    }
                }),
                serde_json::json!({
                    "id":"local-old",
                    "kind":"send-message",
                    "timestampMs":100,
                    "message":{
                        "type":"local-tool-permission",
                        "ask":{"requestId":"req-local-old","status":"pending","tool":"shell"}
                    }
                }),
                serde_json::json!({
                    "id":"local-new",
                    "kind":"send-message",
                    "timestampMs":300,
                    "message":{
                        "type":"local-tool-permission",
                        "ask":{"requestId":"req-local-new","status":"pending"}
                    }
                }),
            ],
        )
        .expect("seed transcript");

    assert_eq!(
        workers
            .expire_pending_auto_review_approvals(
                &agent_id,
                Some("req-auto-a"),
            )
            .expect("targeted auto review"),
        vec!["req-auto-a".to_string()]
    );
    let entries = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read after targeted auto");
    assert_eq!(
        nested_status(&entries, "auto-a", "approval").as_deref(),
        Some("expired")
    );
    assert_eq!(
        nested_status(&entries, "auto-b", "approval").as_deref(),
        Some("pending")
    );
    assert_eq!(
        nested_status(&entries, "auto-complete", "approval").as_deref(),
        Some("approved")
    );

    assert_eq!(
        workers
            .expire_pending_auto_review_approvals(&agent_id, None)
            .expect("remaining auto review"),
        vec!["req-auto-b".to_string()]
    );

    assert_eq!(
        workers
            .expire_pending_local_tool_permission_asks(
                &agent_id,
                None,
                Some(200.0),
            )
            .expect("old local asks"),
        vec!["req-local-old".to_string()]
    );
    let entries = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("read after cutoff");
    assert_eq!(
        nested_status(&entries, "local-old", "ask").as_deref(),
        Some("expired")
    );
    assert_eq!(
        nested_status(&entries, "local-new", "ask").as_deref(),
        Some("pending")
    );

    assert_eq!(
        workers
            .expire_pending_local_tool_permission_asks(
                &agent_id,
                Some("req-local-new"),
                None,
            )
            .expect("targeted local ask"),
        vec!["req-local-new".to_string()]
    );
    let entries = workers
        .read_agent_transcript_entries(&agent_id)
        .expect("final transcript");
    assert_eq!(
        nested_status(&entries, "local-new", "ask").as_deref(),
        Some("expired")
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
