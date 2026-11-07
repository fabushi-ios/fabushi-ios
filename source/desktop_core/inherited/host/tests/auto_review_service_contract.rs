use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::auto_review::auto_review_service::{
    AutoReviewService, SAND_AUTO_REVIEW_STALE,
};
use mahayana_host_runtime::runner::sand_auto_review::{
    SandAutoReviewDecision, SandAutoReviewExpiryPolicy, SandAutoReviewRequest,
    SandAutoReviewRequestOutcome, SandAutoReviewResolution, SandAutoReviewSurface,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-auto-review-service-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn production_service_projects_pending_approval_into_durable_transcript_and_awaiting_state() {
    let root = temp_root("durable");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize session");
    let updates = Arc::new(Mutex::new(Vec::new()));
    let updates_sink = Arc::clone(&updates);
    let service = AutoReviewService::new(
        Arc::clone(&sessions),
        "host-generation",
        Arc::new(move |agent_id, update| {
            updates_sink
                .lock()
                .expect("updates")
                .push((agent_id.to_string(), update));
        }),
        Arc::new(|_| {}),
    );
    let session_key = format!("agent:{}:main", record.id);
    let controller = service.bind_runner_with_session(&record.id, Some(&session_key), true);

    let pending = match controller.request_approval(SandAutoReviewRequest {
        agent_id: None,
        surface: SandAutoReviewSurface::Mcp,
        fingerprint: "fingerprint".into(),
        reason: "sensitive request".into(),
        summary: "Call MCP tool".into(),
        command: None,
        proposed_rule: Some("allow this tool".into()),
        expiry_policy: Some(SandAutoReviewExpiryPolicy::Park),
    }) {
        SandAutoReviewRequestOutcome::Pending(pending) => pending,
        SandAutoReviewRequestOutcome::Immediate(_) => panic!("expected pending approval"),
    };
    let request_id = pending.approval.id.clone();
    assert_eq!(pending.approval.session_key.as_deref(), Some(session_key.as_str()));

    let entries = sessions
        .read_agent_transcript_entries(&record.id)
        .expect("transcript entries");
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str)
                == Some(request_id.as_str()))
            .and_then(|entry| entry.pointer("/message/approval/status"))
            .and_then(serde_json::Value::as_str),
        Some("pending")
    );
    assert_eq!(
        sessions
            .get_agent_awaiting_user_response(&record.id)
            .expect("awaiting state")
            .expect("pending awaiting")
            .tab_id,
        "auto-review"
    );
    assert_eq!(service.agent_ids_with_pending_approvals(), vec![record.id.clone()]);

    service
        .resolve_approval(
            &request_id,
            SandAutoReviewResolution::Approved,
            &record.id,
        )
        .expect("resolve approval");
    assert_eq!(
        pending.wait().expect("approval decision"),
        SandAutoReviewDecision::Approved
    );
    assert!(
        sessions
            .get_agent_awaiting_user_response(&record.id)
            .expect("awaiting state after resolution")
            .is_none()
    );
    let settled = sessions
        .read_agent_transcript_entries(&record.id)
        .expect("settled transcript");
    assert_eq!(
        settled
            .iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str)
                == Some(request_id.as_str()))
            .and_then(|entry| entry.pointer("/message/approval/status"))
            .and_then(serde_json::Value::as_str),
        Some("approved")
    );
    assert!(
        updates
            .lock()
            .expect("updates")
            .iter()
            .any(|(_, update)| update.get("type").and_then(serde_json::Value::as_str)
                == Some("auto-review-status"))
    );

    service
        .resolve_approval(
            &request_id,
            SandAutoReviewResolution::Approved,
            &record.id,
        )
        .expect("duplicate settlement is idempotent");

    service.stop();
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn stale_resolution_fails_closed_when_neither_live_nor_durable_card_exists() {
    let root = temp_root("stale");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize session");
    let service = AutoReviewService::new(
        Arc::clone(&sessions),
        "host-generation",
        Arc::new(|_, _| {}),
        Arc::new(|_| {}),
    );

    let error = service
        .resolve_approval("missing", SandAutoReviewResolution::Denied, &record.id)
        .expect_err("missing approval must fail closed");
    assert!(error.contains(SAND_AUTO_REVIEW_STALE));

    service.stop();
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
