use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::box_request_entries::{
    ActiveBoxRequest, pending_box_request_from_entry, resolve_box_request_entry,
    track_box_request_entry,
};
use mahayana_host_runtime::extensions::transcript::send_pipeline::SendPipelineState;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-box-request-entries-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn frozen_tracking_only_accepts_unresolved_send_message_box_requests_and_supersedes_same_agent() {
    let entry = serde_json::json!({
        "id": "entry-2",
        "kind": "send-message",
        "message": {"type":"text","content":"help"},
        "boxRequestId": "request-2"
    });
    assert_eq!(
        pending_box_request_from_entry("agent-a", &entry),
        Some(ActiveBoxRequest {
            agent_id: "agent-a".into(),
            entry_id: "entry-2".into(),
            request_id: "request-2".into(),
        })
    );
    assert!(pending_box_request_from_entry(
        "agent-a",
        &serde_json::json!({
            "id":"entry-3",
            "kind":"send-message",
            "boxRequestId":"request-3",
            "boxResolution":"completed"
        })
    )
    .is_none());

    let prior = ActiveBoxRequest {
        agent_id: "agent-a".into(),
        entry_id: "entry-1".into(),
        request_id: "request-1".into(),
    };
    let decision = track_box_request_entry(Some(&prior), "agent-a", &entry);
    assert_eq!(decision.superseded_request_id.as_deref(), Some("request-1"));
    assert_eq!(
        decision.next.as_ref().map(|next| next.request_id.as_str()),
        Some("request-2")
    );

    let other_agent = track_box_request_entry(Some(&prior), "agent-b", &entry);
    assert_eq!(other_agent.superseded_request_id, None);
}

#[test]
fn production_resolution_updates_only_matching_unresolved_request() {
    let root = temp_root("production");
    let sessions = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize agent");
    sessions
        .append_agent_transcript_entries(
            &record.id,
            &[
                serde_json::json!({
                    "id":"entry-1",
                    "kind":"send-message",
                    "message":{"type":"text","content":"first"},
                    "boxRequestId":"request-1",
                    "timestampMs":1
                }),
                serde_json::json!({
                    "id":"entry-2",
                    "kind":"send-message",
                    "message":{"type":"text","content":"second"},
                    "boxRequestId":"request-2",
                    "timestampMs":2
                }),
            ],
        )
        .expect("entries");

    let updated = resolve_box_request_entry(
        &sessions,
        &record.id,
        "request-1",
        "dismissed",
    )
    .expect("resolve")
    .expect("updated entry");
    assert_eq!(updated["boxResolution"], "dismissed");

    assert!(resolve_box_request_entry(
        &sessions,
        &record.id,
        "missing-request",
        "completed",
    )
    .expect("missing")
    .is_none());

    let entries = sessions
        .read_agent_transcript_entries(&record.id)
        .expect("transcript");
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry["id"] == "entry-1")
            .and_then(|entry| entry.get("boxResolution"))
            .and_then(serde_json::Value::as_str),
        Some("dismissed")
    );
    assert!(
        entries
            .iter()
            .find(|entry| entry["id"] == "entry-2")
            .and_then(|entry| entry.get("boxResolution"))
            .is_none()
    );

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_bound_box_resolution_rejects_archived_owner_without_partial_update() {
    let root = temp_root("session-bound-archive");
    let sessions = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize agent");
    let session_key = format!("agent:{}:main", record.id);
    let current = sessions
        .read_session_work_admission(&record.id)
        .expect("admission")
        .expect("main admission");
    sessions
        .append_agent_transcript_entries(
            &record.id,
            &[serde_json::json!({
                "id": "entry-bound",
                "kind": "send-message",
                "message": {"type": "text", "content": "help"},
                "boxRequestId": "request-bound",
                "sessionKey": session_key,
            })],
        )
        .expect("entry");
    sessions
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
    let error = resolve_box_request_entry(&sessions, &record.id, "request-bound", "dismissed")
        .expect_err("archived session must reject session-bound resolution");
    assert!(error.contains("session is not admitted"));
    let entry = sessions
        .read_agent_transcript_entries(&record.id)
        .expect("transcript")
        .into_iter()
        .find(|entry| entry["id"] == "entry-bound")
        .expect("bound entry");
    assert!(entry.get("boxResolution").is_none());
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_send_pipeline_owns_active_box_request_supersede_and_resolution_identity() {
    let mut pipeline = SendPipelineState::default();
    let first = serde_json::json!({
        "id":"entry-1",
        "kind":"send-message",
        "message":{"type":"text","content":"first"},
        "boxRequestId":"request-1"
    });
    let first_decision = pipeline.track_box_request_entry("agent-a", &first);
    assert_eq!(first_decision.superseded_request_id, None);
    assert_eq!(
        pipeline.active_box_request().map(|active| active.request_id.as_str()),
        Some("request-1")
    );

    let second = serde_json::json!({
        "id":"entry-2",
        "kind":"send-message",
        "message":{"type":"text","content":"second"},
        "boxRequestId":"request-2"
    });
    let second_decision = pipeline.track_box_request_entry("agent-a", &second);
    assert_eq!(
        second_decision.superseded_request_id.as_deref(),
        Some("request-1")
    );
    assert_eq!(
        pipeline.active_box_request().map(|active| active.request_id.as_str()),
        Some("request-2")
    );
    assert!(!pipeline.resolve_box_request_tracking("request-1"));
    assert!(pipeline.resolve_box_request_tracking("request-2"));
    assert!(pipeline.active_box_request().is_none());

    pipeline.track_box_request_entry("agent-a", &first);
    let other_agent = serde_json::json!({
        "id":"entry-b",
        "kind":"send-message",
        "message":{"type":"text","content":"other"},
        "boxRequestId":"request-b"
    });
    let other_decision = pipeline.track_box_request_entry("agent-b", &other_agent);
    assert_eq!(other_decision.superseded_request_id, None);
    assert_eq!(
        pipeline.active_box_request().map(|active| {
            (active.agent_id.as_str(), active.request_id.as_str())
        }),
        Some(("agent-b", "request-b"))
    );
}
