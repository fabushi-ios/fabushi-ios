use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::sand_pending_wake_store::{
    DurablePendingWakeMarker, PendingWakeKind,
};
use mahayana_host_runtime::sand_activity::{ActivityUpdate, AgentActivity};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-run-lifecycle-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn shipping_runtime_projects_runner_activity_into_agent_roster_rows() {
    let runtime = ProductionTranscriptRuntime::new(None);
    runtime.begin_provider_run("agent-a");
    runtime.track_runner_activity_update(
        "agent-a",
        &ActivityUpdate::ToolCall {
            id: "call-1".into(),
            name: "mcpToolCall".into(),
            status: "pending".into(),
            args: Some(r#"{"providerIdentifier":"calendar"}"#.into()),
            summary: None,
        },
        1_000,
    );

    let mut rows = serde_json::json!([{"id":"agent-a"},{"id":"agent-b"}]);
    runtime.decorate_agent_summaries(&mut rows);
    assert_eq!(rows[0]["isRunning"], true);
    assert_eq!(rows[0]["isRunningTurn"], true);
    assert_eq!(rows[0]["currentActivity"]["kind"], "tool");
    assert_eq!(rows[0]["currentActivity"]["tool"], "CallMcpTool");
    assert_eq!(rows[0]["currentActivity"]["detail"], "calendar");
    assert_eq!(rows[1]["isRunning"], false);

    runtime.track_runner_activity_update(
        "agent-a",
        &ActivityUpdate::TextDelta { text: "hello".into() },
        1_100,
    );
    let mut held = serde_json::json!([{"id":"agent-a"}]);
    runtime.decorate_agent_summaries(&mut held);
    assert_eq!(held[0]["currentActivity"]["tool"], "CallMcpTool");

    runtime.track_runner_activity_update(
        "agent-a",
        &ActivityUpdate::TurnEnded,
        4_000,
    );
    runtime.end_provider_run("agent-a");
    let mut ended = serde_json::json!([{"id":"agent-a"}]);
    runtime.decorate_agent_summaries(&mut ended);
    assert_eq!(ended[0]["isRunning"], false);
    assert!(ended[0]["currentActivity"].is_null());
}

#[test]
fn run_lifecycle_composing_and_retrying_follow_frozen_updates() {
    let runtime = ProductionTranscriptRuntime::new(None);
    runtime.begin_provider_run("agent-a");
    runtime.track_runner_activity_update(
        "agent-a",
        &ActivityUpdate::ToolCall {
            id: "send".into(),
            name: "SendMessage".into(),
            status: "pending".into(),
            args: None,
            summary: None,
        },
        10,
    );
    let mut composing = serde_json::json!([{"id":"agent-a"}]);
    runtime.decorate_agent_summaries(&mut composing);
    assert_eq!(composing[0]["isComposingMessage"], true);

    runtime.track_runner_activity_update("agent-a", &ActivityUpdate::Retrying, 20);
    let mut retrying = serde_json::json!([{"id":"agent-a"}]);
    runtime.decorate_agent_summaries(&mut retrying);
    assert_eq!(retrying[0]["isRetrying"], true);

    runtime.track_runner_activity_update(
        "agent-a",
        &ActivityUpdate::SendMessage,
        30,
    );
    let mut sent = serde_json::json!([{"id":"agent-a"}]);
    runtime.decorate_agent_summaries(&mut sent);
    assert_eq!(sent[0]["isComposingMessage"], false);
    assert_eq!(sent[0]["isRetrying"], false);
    runtime.end_provider_run("agent-a");
}

#[test]
fn durable_subagent_parent_projects_running_without_claiming_a_parent_turn() {
    let root = temp_root("durable-subagent-parent");
    fs::create_dir_all(&root).expect("root");
    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    let store = runtime.pending_wake_store().expect("pending wake store");
    assert!(store.mark_pending(DurablePendingWakeMarker {
        agent_id: "agent-parent".into(),
        kind: PendingWakeKind::Subagent,
        work_id: "subagent-1".into(),
        marked_at_ms: 100.0,
        quiet_origin: None,
        title: Some("Research".into()),
        subagent_type: Some("cursor-agent".into()),
        interrupted_by_recreate: false,
        delivery_operation_id: None,
        address_token: None,
        failure_reason: None,
        session_key: None,
    }));

    let mut rows = serde_json::json!([
        {"id":"agent-parent"},
        {"id":"agent-idle"}
    ]);
    runtime.decorate_agent_summaries(&mut rows);
    assert_eq!(rows[0]["isRunning"], true);
    assert_eq!(rows[0]["isRunningTurn"], false);
    assert_eq!(rows[0]["isComposingMessage"], false);
    assert_eq!(rows[0]["isRetrying"], false);
    assert!(rows[0]["currentActivity"].is_null());
    assert!(rows[0]["activeRemoteMemberId"].is_null());
    assert_eq!(rows[1]["isRunning"], false);

    assert!(store.clear_one("agent-parent", PendingWakeKind::Subagent, "subagent-1"));
    let mut cleared = serde_json::json!([{"id":"agent-parent"}]);
    runtime.decorate_agent_summaries(&mut cleared);
    assert_eq!(cleared[0]["isRunning"], false);
    assert_eq!(cleared[0]["isRunningTurn"], false);

    let _ = fs::remove_dir_all(root);
}


#[test]
fn live_runner_subagent_parent_projects_running_without_durable_wake() {
    let runtime = ProductionTranscriptRuntime::new(None);
    assert!(runtime.live_running_agent_ids().is_empty());
    assert!(!runtime.has_live_subagent("agent-parent"));

    runtime.begin_live_subagent("agent-parent");
    assert!(runtime.has_live_subagent("agent-parent"));
    assert_eq!(runtime.live_running_agent_ids(), vec!["agent-parent".to_string()]);

    let mut rows = serde_json::json!([{"id":"agent-parent"},{"id":"agent-idle"}]);
    runtime.decorate_agent_summaries(&mut rows);
    assert_eq!(rows[0]["isRunning"], true);
    assert_eq!(rows[0]["isRunningTurn"], false);
    assert_eq!(rows[0]["isComposingMessage"], false);
    assert_eq!(rows[0]["isRetrying"], false);
    assert!(rows[0]["currentActivity"].is_null());
    assert_eq!(rows[1]["isRunning"], false);

    runtime.end_live_subagent("agent-parent");
    assert!(!runtime.has_live_subagent("agent-parent"));
    assert!(runtime.live_running_agent_ids().is_empty());
}
