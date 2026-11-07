use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::extensions::session::agent_db::{
    read_persisted_agent_serde_snapshot, read_persisted_latest_root_blob_id,
};
use mahayana_host_runtime::extensions::session::agent_db_serde::{
    AwaitingUserResponse, EpisodeTurn, SandProfile, SpendGuardState,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::transcript_mutation_events::subscribe_transcript_mutations;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-agent-db-mutation-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn production_session_owns_agent_db_state_mutations_and_transcript_lifecycle() {
    let root = temp_root("shipping");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let created = workers
        .materialize_new_session(
            Some(&SandAgentProfile {
                name: "Mutation Agent".into(),
                description: String::new(),
                title: String::new(),
                avatar_shape: String::new(),
                avatar_color: String::new(),
            }),
            "user",
            None,
        )
        .expect("materialize");
    let agent_id = created.id;
    let db_path = created.db_path;
    assert_eq!(workers.active_agent_db_owner_count(), 1);

    assert!(workers
        .set_agent_sand_profile(
            &agent_id,
            &SandProfile {
                description: "profile description".into(),
                avatar_path: Some("/tmp/avatar.png".into()),
            },
        )
        .expect("set profile"));
    assert!(workers.mark_agent_activity(&agent_id, 100.0).expect("activity"));
    assert!(workers.set_agent_unread(&agent_id, true, 200.0).expect("unread"));

    let state = read_persisted_agent_serde_snapshot(&db_path, 500).expect("state");
    assert_eq!(state.profile.description, "profile description");
    assert_eq!(state.unread_state.last_activity_at, 100.0);
    assert!(state.unread_state.is_manually_unread);
    assert_eq!(state.unread_state.unread_count, 1.0);

    assert!(workers.get_agent_introduction_pending(&agent_id).expect("intro pending"));
    assert!(workers
        .set_agent_introduction_pending(&agent_id, false)
        .expect("clear intro pending"));
    assert!(!workers
        .get_agent_introduction_pending(&agent_id)
        .expect("intro cleared"));

    let spend_guard = SpendGuardState {
        nudged_at_ms: Some(42.0),
        snoozed_until_ms: Some(84.0),
        opted_out: true,
        card_entry_ids: vec!["card-a".into()],
        paused_automation_ids: vec!["auto-a".into()],
    };
    assert!(workers
        .set_agent_automation_spend_guard_state(&agent_id, &spend_guard)
        .expect("set spend guard"));
    assert_eq!(
        workers
            .get_agent_automation_spend_guard_state(&agent_id)
            .expect("get spend guard"),
        spend_guard
    );

    assert!(!workers
        .add_agent_conversation_partner(&agent_id, &agent_id)
        .expect("reject self partner"));
    assert!(workers
        .add_agent_conversation_partner(&agent_id, " partner-b ")
        .expect("add partner"));
    assert!(!workers
        .add_agent_conversation_partner(&agent_id, "partner-b")
        .expect("dedupe partner"));
    assert_eq!(
        workers
            .get_agent_conversation_partner_ids(&agent_id)
            .expect("partners"),
        vec!["partner-b".to_string()]
    );

    assert!(!workers
        .mark_agent_viewed(&agent_id, 300.0, true)
        .expect("preserve manual unread"));
    assert!(workers
        .mark_agent_viewed(&agent_id, 300.0, false)
        .expect("viewed"));

    let awaiting = AwaitingUserResponse {
        tab_id: "tab-a".into(),
        reason: "approval".into(),
        since: 400.0,
    };
    assert!(workers
        .set_agent_awaiting_user_response(&agent_id, Some(&awaiting))
        .expect("set awaiting"));
    assert!(!workers
        .set_agent_awaiting_user_response_for_tab(
            &agent_id,
            "tab-b",
            None,
            None,
        )
        .expect("wrong tab"));
    assert!(workers
        .set_agent_awaiting_user_response_for_tab(
            &agent_id,
            "tab-a",
            None,
            Some(500.0),
        )
        .expect("clear awaiting"));

    assert!(workers
        .record_agent_request_id(
            &agent_id,
            " request-1 ",
            500.0,
            Some("first prompt"),
            Some("turn"),
        )
        .expect("request one"));
    assert!(!workers
        .record_agent_request_id(
            &agent_id,
            "request-1",
            501.0,
            Some("duplicate"),
            Some("turn"),
        )
        .expect("duplicate request"));
    assert!(workers
        .record_agent_request_id(
            &agent_id,
            "request-2",
            502.0,
            Some("second prompt"),
            Some("automation"),
        )
        .expect("request two"));

    assert!(workers
        .record_agent_episode_turn(
            &agent_id,
            &EpisodeTurn {
                ts: 600.0,
                user: "question".into(),
                agent: "answer".into(),
            },
        )
        .expect("episode"));
    assert!(workers
        .set_agent_memory_prompt_snapshot(
            &agent_id,
            &serde_json::json!({"render":"memory","compactionEpoch":2}),
        )
        .expect("memory snapshot"));

    assert!(workers
        .set_agent_profile_prompt_snapshot(
            &agent_id,
            &serde_json::json!({"render":"profile","version":3}),
        )
        .expect("profile prompt snapshot"));
    assert_eq!(
        workers
            .get_agent_profile_prompt_snapshot(&agent_id)
            .expect("read profile prompt snapshot"),
        Some(serde_json::json!({"render":"profile","version":3}))
    );

    let state = read_persisted_agent_serde_snapshot(&db_path, 500).expect("mutated state");
    assert_eq!(state.request_ids.len(), 2);
    assert_eq!(state.request_ids[0].id, "request-1");
    assert_eq!(state.request_ids[1].id, "request-2");
    assert_eq!(state.pending_episode_turns.len(), 1);
    assert_eq!(
        state.memory_prompt_snapshot.as_ref().map(|snapshot| snapshot.render.as_str()),
        Some("memory")
    );
    assert!(state.awaiting_user_response.is_none());
    assert!(!state.unread_state.is_manually_unread);
    assert_eq!(state.unread_state.unread_count, 0.0);

    let events = Arc::new(Mutex::new(Vec::new()));
    let listener_events = Arc::clone(&events);
    let subscription = subscribe_transcript_mutations(move |event| {
        listener_events
            .lock()
            .expect("events")
            .push(serde_json::Value::Object(event.clone()));
    });

    assert_eq!(
        workers
            .append_agent_transcript_entries(
                &agent_id,
                &[
                    serde_json::json!({
                        "id":"entry-1",
                        "kind":"message",
                        "role":"user",
                        "content":"hello",
                        "timestampMs":700
                    }),
                    serde_json::json!({
                        "id":"entry-2",
                        "kind":"notice",
                        "text":"notice"
                    }),
                ],
            )
            .expect("append transcript"),
        2
    );
    assert_eq!(
        workers
            .update_agent_transcript_entry(
                &agent_id,
                "entry-1",
                &serde_json::json!({
                    "id":"entry-1",
                    "kind":"message",
                    "role":"user",
                    "content":"edited",
                    "timestampMs":700
                }),
            )
            .expect("update transcript")
            .and_then(|entry| entry.get("content").cloned()),
        Some(serde_json::Value::String("edited".into()))
    );
    assert!(workers
        .delete_agent_transcript_entry(&agent_id, "entry-2")
        .expect("delete transcript"));
    assert_eq!(
        workers
            .read_agent_transcript_entries(&agent_id)
            .expect("read transcript")
            .len(),
        1
    );

    assert_eq!(
        workers
            .get_agent_newest_divider_anchor_timestamp_ms(&agent_id)
            .expect("newest divider anchor"),
        0.0
    );

    assert!(workers
        .clear_agent_transient_state(&agent_id)
        .expect("clear transient state"));
    let transient_cleared =
        read_persisted_agent_serde_snapshot(&db_path, 500).expect("transient cleared state");
    assert_eq!(transient_cleared.unread_state.unread_count, 0.0);
    assert_eq!(
        transient_cleared.spend_guard_state,
        SpendGuardState::default()
    );
    assert!(transient_cleared.awaiting_user_response.is_none());
    assert!(transient_cleared.request_ids.is_empty());
    assert!(transient_cleared.pending_episode_turns.is_empty());
    assert!(transient_cleared.memory_prompt_snapshot.is_none());
    assert!(
        workers
            .get_agent_profile_prompt_snapshot(&agent_id)
            .expect("profile snapshot cleared by transient reset")
            .is_none()
    );

    let store = workers.create_agent_blob_store(&agent_id).expect("blob store");
    futures::executor::block_on(store.set_blob(&(), &[0x55; 32], b"blob"))
        .expect("persist durable blob");

    assert!(workers
        .clear_agent_conversation(&agent_id)
        .expect("clear conversation"));
    assert!(
        workers
            .read_agent_transcript_entries(&agent_id)
            .expect("cleared transcript")
            .is_empty()
    );
    assert!(
        read_persisted_latest_root_blob_id(&db_path, 500)
            .expect("cleared root")
            .is_empty()
    );
    let state = read_persisted_agent_serde_snapshot(&db_path, 500).expect("cleared state");
    assert!(state.request_ids.is_empty());
    assert!(state.pending_episode_turns.is_empty());
    assert!(state.memory_prompt_snapshot.is_none());
    assert!(state.awaiting_user_response.is_none());
    assert_eq!(
        futures::executor::block_on(store.get_blob(&(), &[0x55; 32]))
            .expect("cleared durable blob"),
        None
    );

    let events = events.lock().expect("events");
    assert!(events.iter().any(|event| event["kind"] == "entries-upserted"));
    assert!(events.iter().any(|event| event["kind"] == "entry-deleted"));
    assert!(events.iter().any(|event| event["kind"] == "conversation-cleared"));
    drop(events);
    subscription.unsubscribe();

    workers.shutdown();
    assert_eq!(workers.active_agent_db_owner_count(), 0);
    let _ = fs::remove_dir_all(root);
}
