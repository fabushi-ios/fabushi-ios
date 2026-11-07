use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::transcript::production_runtime::{
    ProductionSendError, ProductionTranscriptRuntime,
};
use mahayana_host_runtime::extensions::transcript::send_pipeline::PersistedSendContext;
use mahayana_host_runtime::runner::RecoveryUserMessage;

#[test]
fn routed_prompt_admission_owns_nonce_durability_and_recovery_identity() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-routed-admission-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    let lifecycle = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let lifecycle_observer = Arc::clone(&lifecycle);
    runtime.set_agent_run_lifecycle_observer(Some(Arc::new(move |event_type, request_id| {
        lifecycle_observer
            .lock()
            .expect("lifecycle")
            .push((event_type.to_string(), request_id.to_string()));
    })));
    let args = serde_json::json!({
        "agentId": "agent-a",
        "prompt": "hello",
        "clientNonce": "routed-nonce",
        "streamId": "stream-a"
    });
    let persists = AtomicUsize::new(0);

    let first = runtime
        .accept_routed_send(&args, |_: &serde_json::Value| {
            persists.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ProductionSendError>(PersistedSendContext {
                echo_entry_id: Some("user-message:1".into()),
                user_message_id: Some("user-message:1".into()),
                recent_user_messages: vec![RecoveryUserMessage {
                    id: "user-message:1".into(),
                    text: "hello".into(),
                    confirmed: None,
                }],
                ..PersistedSendContext::default()
            })
        })
        .expect("first routed admission");
    assert!(!first.duplicate);
    assert_eq!(first.context.user_message_id.as_deref(), Some("user-message:1"));
    assert_eq!(runtime.current_turn_epoch("agent-a"), 1);
    assert_eq!(runtime.in_flight_run_count("agent-a"), 1);
    assert!(!runtime.is_turn_dispatch_idle("agent-a"));
    assert!(runtime.has_routed_turn_lease("agent-a", "stream-a"));
    assert_eq!(
        *lifecycle.lock().expect("lifecycle"),
        vec![("started".to_string(), "stream-a".to_string())]
    );

    let duplicate = runtime
        .accept_routed_send(&args, |_: &serde_json::Value| {
            persists.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ProductionSendError>(PersistedSendContext {
                echo_entry_id: Some("user-message:1".into()),
                user_message_id: Some("user-message:1".into()),
                recent_user_messages: vec![RecoveryUserMessage {
                    id: "user-message:1".into(),
                    text: "hello".into(),
                    confirmed: None,
                }],
                ..PersistedSendContext::default()
            })
        })
        .expect("duplicate routed admission");
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.context.echo_entry_id.as_deref(), Some("user-message:1"));
    assert_eq!(runtime.current_turn_epoch("agent-a"), 1);
    assert_eq!(runtime.in_flight_run_count("agent-a"), 1);
    assert_eq!(persists.load(Ordering::SeqCst), 2);

    runtime
        .require_routed_turn_lease("agent-a", "stream-a")
        .expect("provider start lease");
    runtime
        .settle_routed_turn("agent-a", "stream-a", 10_000)
        .expect("terminal settlement")
        .expect("lease settlement");
    assert_eq!(runtime.in_flight_run_count("agent-a"), 0);
    assert!(runtime.is_turn_dispatch_idle("agent-a"));
    assert!(!runtime.has_routed_turn_lease("agent-a", "stream-a"));
    assert_eq!(
        *lifecycle.lock().expect("lifecycle"),
        vec![
            ("started".to_string(), "stream-a".to_string()),
            ("ended".to_string(), "stream-a".to_string()),
        ]
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn human_handoff_admission_binds_destination_to_the_host_owned_turn_lease() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-routed-human-handoff-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    let args = serde_json::json!({
        "agentId": "agent-a",
        "prompt": "continue this Human conversation",
        "clientNonce": "human-handoff-nonce",
        "streamId": "human-handoff-stream",
        "requestSource": "human-handoff",
        "humanHandoffConversationId": "human-conversation:abc"
    });
    runtime
        .accept_routed_send(&args, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext::default())
        })
        .expect("Human handoff admission");
    assert_eq!(
        runtime
            .routed_human_handoff_conversation_id("agent-a", "human-handoff-stream")
            .as_deref(),
        Some("human-conversation:abc")
    );
    runtime
        .settle_routed_turn("agent-a", "human-handoff-stream", 10_000)
        .expect("settlement")
        .expect("lease");
    assert_eq!(
        runtime.routed_human_handoff_conversation_id("agent-a", "human-handoff-stream"),
        None
    );

    let missing_destination = serde_json::json!({
        "agentId": "agent-a",
        "prompt": "missing destination",
        "clientNonce": "missing-destination",
        "streamId": "missing-destination-stream",
        "requestSource": "human-handoff"
    });
    assert!(matches!(
        runtime.accept_routed_send(&missing_destination, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext::default())
        }),
        Err(ProductionSendError::BadRequest(_))
    ));

    let forged_destination = serde_json::json!({
        "agentId": "agent-a",
        "prompt": "not a handoff",
        "clientNonce": "forged-destination",
        "streamId": "forged-destination-stream",
        "humanHandoffConversationId": "human-conversation:abc"
    });
    assert!(matches!(
        runtime.accept_routed_send(&forged_destination, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext::default())
        }),
        Err(ProductionSendError::BadRequest(_))
    ));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn routed_prompt_queue_lease_blocks_the_next_turn_until_runner_terminal() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    let root = std::env::temp_dir().join(format!(
        "fabushi-routed-admission-queue-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let first_args = serde_json::json!({
        "agentId": "agent-a",
        "prompt": "first",
        "clientNonce": "nonce-a",
        "streamId": "stream-a"
    });
    runtime
        .accept_routed_send(&first_args, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext {
                echo_entry_id: Some("user-message:1".into()),
                user_message_id: Some("user-message:1".into()),
                recent_user_messages: vec![],
                ..PersistedSendContext::default()
            })
        })
        .expect("first admission");

    let second_runtime = Arc::clone(&runtime);
    let (tx, rx) = mpsc::channel();
    let join = std::thread::spawn(move || {
        let args = serde_json::json!({
            "agentId": "agent-a",
            "prompt": "second",
            "clientNonce": "nonce-b",
            "streamId": "stream-b"
        });
        let result = second_runtime.accept_routed_send(&args, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext {
                echo_entry_id: Some("user-message:2".into()),
                user_message_id: Some("user-message:2".into()),
                recent_user_messages: vec![],
                ..PersistedSendContext::default()
            })
        });
        tx.send(result).expect("send second admission");
    });

    std::thread::sleep(Duration::from_millis(30));
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    assert_eq!(runtime.in_flight_run_count("agent-a"), 2);
    assert_eq!(runtime.queued_turn_count("agent-a"), 1);

    runtime
        .settle_routed_turn("agent-a", "stream-a", 20_000)
        .expect("first terminal")
        .expect("first lease");
    let second = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second admission released")
        .expect("second admission");
    assert!(!second.duplicate);
    assert!(runtime.has_routed_turn_lease("agent-a", "stream-b"));
    assert_eq!(runtime.in_flight_run_count("agent-a"), 1);

    runtime
        .settle_routed_turn("agent-a", "stream-b", 30_000)
        .expect("second terminal")
        .expect("second lease");
    assert_eq!(runtime.in_flight_run_count("agent-a"), 0);
    assert!(runtime.is_turn_dispatch_idle("agent-a"));
    join.join().expect("second admission thread");

    let _ = std::fs::remove_dir_all(root);
}


#[test]
fn group_member_routed_admission_projects_running_without_user_turn_badge() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-routed-group-member-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime = ProductionTranscriptRuntime::new(Some(&root));
    let args = serde_json::json!({
        "agentId": "member-a",
        "sessionKey": "agent:member-a:group:room-a",
        "prompt": "hidden group prompt",
        "clientNonce": "group-member-nonce",
        "streamId": "group-member-stream",
        "requestSource": "group-member",
        "groupMemberTurn": true,
    });
    runtime
        .accept_routed_send(&args, |_| {
            Ok::<_, ProductionSendError>(PersistedSendContext::default())
        })
        .expect("group member admission");

    let mut roster = serde_json::json!([{"id":"member-a"}]);
    runtime.decorate_agent_summaries(&mut roster);
    assert_eq!(roster[0]["isRunning"], true);
    assert_eq!(roster[0]["isRunningTurn"], false);
    assert_eq!(runtime.active_turn_source("member-a").as_deref(), Some("group-member"));

    runtime.begin_provider_run_with_kind("member-a", true);
    let mut provider_roster = serde_json::json!([{"id":"member-a"}]);
    runtime.decorate_agent_summaries(&mut provider_roster);
    assert_eq!(provider_roster[0]["isRunning"], true);
    assert_eq!(provider_roster[0]["isRunningTurn"], false);
    runtime.end_provider_run_with_kind("member-a", true);

    runtime
        .settle_routed_turn("member-a", "group-member-stream", 50_000)
        .expect("settlement")
        .expect("lease");
    assert_eq!(runtime.in_flight_run_count("member-a"), 0);
    let _ = std::fs::remove_dir_all(root);
}


#[test]
fn upgrade_resume_admission_uses_the_background_exclusive_lane() {
    use mahayana_host_runtime::extensions::transcript::production_runtime::classify_send_dispatch;
    use mahayana_host_runtime::extensions::transcript::run_scheduler::RunLane;

    let (lane, source) = classify_send_dispatch(&serde_json::json!({
        "requestSource": "upgrade-resume"
    }))
    .expect("upgrade resume dispatch");
    assert_eq!(lane, RunLane::Background);
    assert_eq!(source, "upgrade-resume");
}
