use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_db::read_persisted_agent_serde_snapshot;
use mahayana_host_runtime::extensions::session::agent_db_serde::AwaitingUserResponse;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::box_handoff_resume::{
    BOX_HANDOFF_DISMISSED_PROMPT, BOX_HANDOFF_RESUME_PROMPT, BOX_HANDOFF_RESUME_TITLE,
    BOX_HANDOFF_VIEWER_CLOSED_PROMPT, LISTENER_CONNECT_RESUME_TITLE, MCP_AUTH_RESUME_TITLE,
    box_handoff_resume_prompt, build_box_handoff_resume_send_args,
    format_mcp_account_display_name, settle_box_handoff_state, should_resume_hidden_handoff,
};
use mahayana_host_runtime::extensions::transcript::production_runtime::{
    ProductionTranscriptRuntime, classify_send_dispatch,
};
use mahayana_host_runtime::extensions::transcript::roster_emit::ProductionRosterEmit;
use mahayana_host_runtime::extensions::transcript::run_scheduler::RunLane;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-box-handoff-resume-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn frozen_handoff_prompts_and_hidden_send_contract_are_preserved() {
    assert_eq!(box_handoff_resume_prompt("button"), BOX_HANDOFF_RESUME_PROMPT);
    assert_eq!(
        box_handoff_resume_prompt("dismissed"),
        BOX_HANDOFF_DISMISSED_PROMPT
    );
    assert_eq!(
        box_handoff_resume_prompt("viewer-closed"),
        BOX_HANDOFF_VIEWER_CLOSED_PROMPT
    );

    let args = build_box_handoff_resume_send_args(
        "agent-a",
        "agent:agent-a:main",
        "dismissed",
        123,
    );
    assert_eq!(args["agentId"], "agent-a");
    assert_eq!(args["sessionKey"], "agent:agent-a:main");
    assert_eq!(args["prompt"], BOX_HANDOFF_DISMISSED_PROMPT);
    assert_eq!(args["appendUserMessage"], false);
    assert_eq!(args["awaitTurn"], false);
    assert_eq!(args["directAddressedAcceptance"], true);
    assert_eq!(args["requestSource"], "handoff-resume");
    assert_eq!(args["hidden"], true);
    assert_eq!(args["skipAckObligation"], true);
    assert_eq!(args["clientNonce"], "box-handoff-resume:agent-a:123");

    let (lane, source) = classify_send_dispatch(&args).expect("handoff dispatch");
    assert_eq!(lane, RunLane::Background);
    assert_eq!(source, "handoff-resume");

    let ack_redrive = serde_json::json!({
        "requestSource": "handoff-resume",
        "ackRedrive": true
    });
    let (lane, source) = classify_send_dispatch(&ack_redrive).expect("ack dispatch");
    assert_eq!(lane, RunLane::Background);
    assert_eq!(source, "ack-redrive");
}

#[test]
fn production_handoff_settlement_clears_awaiting_and_resolves_matching_box_entry() {
    let root = temp_root("production");
    let sessions = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize agent");

    sessions
        .set_agent_awaiting_user_response(
            &record.id,
            Some(&AwaitingUserResponse {
                tab_id: "tab-1".into(),
                reason: "box-help".into(),
                since: 9.0,
            }),
        )
        .expect("awaiting state");
    sessions
        .append_agent_transcript_entries(
            &record.id,
            &[
                serde_json::json!({
                    "id": "box-request-1",
                    "kind": "send-message",
                    "message": {"type":"text","content":"Please take over"},
                    "timestampMs": 10,
                    "boxRequestId": "request-1",
                    "boxInstruction": "Solve the captcha"
                }),
                serde_json::json!({
                    "id": "box-request-other",
                    "kind": "send-message",
                    "message": {"type":"text","content":"Other request"},
                    "timestampMs": 11,
                    "boxRequestId": "request-other"
                }),
            ],
        )
        .expect("box request entries");

    let settled = settle_box_handoff_state(
        &sessions,
        &record.id,
        "request-1",
        "completed",
    )
    .expect("settle handoff");
    assert!(settled.awaiting_cleared);
    assert_eq!(
        settled
            .resolved_entry
            .as_ref()
            .and_then(|entry| entry.get("boxResolution"))
            .and_then(serde_json::Value::as_str),
        Some("completed")
    );

    let snapshot = read_persisted_agent_serde_snapshot(&record.db_path, 500)
        .expect("snapshot");
    assert!(snapshot.awaiting_user_response.is_none());

    let entries = sessions
        .read_agent_transcript_entries(&record.id)
        .expect("transcript");
    let resolved = entries
        .iter()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some("box-request-1"))
        .expect("resolved entry");
    assert_eq!(resolved["boxResolution"], "completed");
    let untouched = entries
        .iter()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some("box-request-other"))
        .expect("other entry");
    assert!(untouched.get("boxResolution").is_none());

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_and_listener_resume_prompts_match_frozen_hidden_handoff_contract() {
    use mahayana_host_runtime::extensions::transcript::box_handoff_resume::{
        build_hidden_handoff_resume_send_args, listener_connect_resume_prompt,
        mcp_auth_resume_prompt,
    };

    let mcp = mcp_auth_resume_prompt("Calendar (work)");
    assert_eq!(
        mcp,
        "[The \"Calendar (work)\" MCP server finished authorizing — it's connected and its tools are available now. Your first action is a SendMessage telling the user it's connected, then pick up whatever you paused to authorize it. If there was nothing else to do, just confirm it's ready and ask what they'd like to do with it. Remember: nothing reaches the user unless it's inside a SendMessage.]"
    );

    let slack = listener_connect_resume_prompt("slack");
    assert!(slack.starts_with("[Slack is now connected"));
    assert!(slack.contains("/invite @Cursor"));
    let github = listener_connect_resume_prompt("github");
    assert!(github.starts_with("[GitHub is now connected"));
    assert!(!github.contains("/invite @Cursor"));

    let args = build_hidden_handoff_resume_send_args(
        "agent-a",
        "agent:agent-a:main",
        &mcp,
        "mcp-auth-resume",
        42,
    );
    assert_eq!(args["sessionKey"], "agent:agent-a:main");
    assert_eq!(args["appendUserMessage"], false);
    assert_eq!(args["awaitTurn"], false);
    assert_eq!(args["requestSource"], "handoff-resume");
    assert_eq!(args["hidden"], true);
    assert_eq!(args["skipAckObligation"], true);
    assert_eq!(args["clientNonce"], "mcp-auth-resume:agent-a:42");

    assert_eq!(BOX_HANDOFF_RESUME_TITLE, "Agent failed to resume after box handoff");
    assert_eq!(MCP_AUTH_RESUME_TITLE, "Agent failed to resume after MCP authentication");
    assert_eq!(LISTENER_CONNECT_RESUME_TITLE, "Agent failed to resume after listener connect");

    assert_eq!(
        format_mcp_account_display_name(
            "Calendar",
            "  work <prod> \"quoted\" \\ [team] \n primary "
        ),
        "Calendar (work prod quoted team primary)"
    );
    assert_eq!(format_mcp_account_display_name("Calendar", "default"), "Calendar");
    let bounded = format_mcp_account_display_name("Calendar", &"a".repeat(80));
    let rendered_label = bounded
        .strip_prefix("Calendar (")
        .and_then(|value| value.strip_suffix(')'))
        .expect("rendered account label");
    assert_eq!(rendered_label.encode_utf16().count(), 64);

    assert!(!should_resume_hidden_handoff(false, Some(false)));
    assert!(!should_resume_hidden_handoff(true, None));
    assert!(!should_resume_hidden_handoff(true, Some(true)));
    assert!(should_resume_hidden_handoff(true, Some(false)));
}

#[test]
fn awaiting_state_sink_preserves_tab_identity_conditional_clear_and_roster_projection() {
    use mahayana_host_runtime::extensions::transcript::box_handoff_resume::AwaitingStateSink;

    let root = temp_root("awaiting-sink");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize agent");
    let transcript = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let roster = ProductionRosterEmit::new(
        Arc::clone(&sessions),
        transcript,
        Arc::new(move |event| sink_events.lock().expect("events").push(event)),
    );
    let sink = AwaitingStateSink::new(sessions.as_ref(), &roster);

    assert!(sink.try_set_for_tab(
        &record.id,
        "tab-a",
        &AwaitingUserResponse {
            tab_id: "tab-a".into(),
            reason: "question".into(),
            since: 100.0,
        },
    ));
    let awaiting = sessions
        .get_agent_awaiting_user_response(&record.id)
        .expect("awaiting")
        .expect("state");
    assert_eq!(awaiting.tab_id, "tab-a");
    assert!(!sink.clear_for_tab(&record.id, "tab-b", None));
    assert!(sessions
        .get_agent_awaiting_user_response(&record.id)
        .expect("awaiting")
        .is_some());
    assert!(!sink.clear_for_tab(&record.id, "tab-a", Some(50.0)));
    assert!(sink.clear_for_tab(&record.id, "tab-a", Some(150.0)));
    assert!(sessions
        .get_agent_awaiting_user_response(&record.id)
        .expect("awaiting")
        .is_none());
    assert!(!events.lock().expect("events").is_empty());

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn shipping_host_wires_box_listener_mcp_resume_and_error_projection() {
    const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
    const TRANSCRIPT_MANAGER: &str = include_str!("../src/extensions/transcript/transcript_manager.rs");
    assert!(SHIPPING_HOST.contains("resume_with_hidden_handoff("));
    assert!(SHIPPING_HOST.contains("run_local_background_revival_turn_with_context("));
    assert!(SHIPPING_HOST.contains("should_resume_hidden_handoff(true, is_group)"));
    assert!(SHIPPING_HOST.contains("mint_ack_run_token(agent_id)"));
    assert!(SHIPPING_HOST.contains("ack_token: worker_ack_token.clone()"));
    assert!(SHIPPING_HOST.contains("\"handoff-resume\""));
    assert!(SHIPPING_HOST.contains("worker_is_handoff_resume"));
    assert!(SHIPPING_HOST.contains("subscribe_to_auth_completion"));
    assert!(SHIPPING_HOST.contains("resume_after_mcp_auth("));
    assert!(SHIPPING_HOST.contains(".hand_back_forever_box("));
    assert!(TRANSCRIPT_MANAGER.contains("settle_box_handoff_state_with_sink("));
    assert!(TRANSCRIPT_MANAGER.contains(".resolve_box_request_tracking(&end.request_id)"));
    assert!(SHIPPING_HOST.contains("source: \"resume\".into()"));
    assert!(SHIPPING_HOST.contains("BOX_HANDOFF_RESUME_TITLE"));
    assert!(SHIPPING_HOST.contains("LISTENER_CONNECT_RESUME_TITLE"));
    assert!(SHIPPING_HOST.contains("MCP_AUTH_RESUME_TITLE"));
    assert!(SHIPPING_HOST.contains("\"channel\": \"forever-box\""));

    let box_help = SHIPPING_HOST
        .split("fn request_box_help(")
        .nth(1)
        .expect("shipping request_box_help owner exists")
        .split("fn resolve_attachment_source(")
        .next()
        .expect("shipping request_box_help boundary exists");
    assert!(box_help.contains("append_agent_transcript_entries_if_session_admitted"));
    assert!(box_help.contains("self.session_key.as_deref()"));
    assert!(!box_help.contains("append_agent_transcript_entries(\n                    &self.agent_id"));
}
