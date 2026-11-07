use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::group_chat_glue::{
    GroupChatGlue, GroupMemberPreview, GroupRoomEntryObserver, group_member_reaction_target,
    should_redrive_group_member_after_preemption,
};
use mahayana_host_runtime::extensions::transcript::production_runtime::{
    ProductionTranscriptRuntime, SharedGroupRoomEntryPublisher,
};
use mahayana_host_runtime::extensions::transcript::send_group_fanout::{
    GroupMemberTurnExecutor, LocalGroupFanoutDisposition,
};
use mahayana_host_runtime::groups::group_store::{
    GROUP_CONFIG_VERSION, SandGroupConfig, write_sand_group_config,
};
use serde_json::{Value, json};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-group-glue-owner-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn profile(name: &str) -> SandAgentProfile {
    SandAgentProfile {
        name: name.into(),
        description: String::new(),
        title: String::new(),
        avatar_shape: String::new(),
        avatar_color: String::new(),
    }
}

#[test]
fn group_preview_suppresses_pass_stream_and_has_append_update_remove_lifecycle() {
    let mut preview = GroupMemberPreview::new("room-a", "member-a", "Alice", "stream-a");
    assert_eq!(preview.on_text_delta("("), None);
    assert_eq!(preview.on_text_delta("(pas"), None);
    assert_eq!(preview.on_text_delta("(pass)"), None);

    let appended = preview.on_text_delta("Hel").expect("preview append");
    assert_eq!(appended["type"], "appended");
    assert_eq!(appended["agentId"], "room-a");
    assert_eq!(appended["entry"]["streaming"], true);
    assert_eq!(appended["entry"]["author"]["id"], "member-a");
    assert_eq!(appended["entry"]["message"]["content"], "Hel");

    let updated = preview.on_text_delta("Hello").expect("preview update");
    assert_eq!(updated["type"], "updated");
    assert_eq!(updated["entry"]["message"]["content"], "Hello");

    let removed = preview.finish().expect("preview cleanup");
    assert_eq!(removed["type"], "removed");
    assert_eq!(removed["agentId"], "room-a");
    assert_eq!(preview.finish(), None);
}

#[test]
fn reaction_and_preemption_settlement_preserve_group_delivery_semantics() {
    assert_eq!(
        group_member_reaction_target(Some("room-a"), "member-a"),
        "room-a"
    );
    assert_eq!(
        group_member_reaction_target(None, "member-a"),
        "member-a"
    );
    assert!(should_redrive_group_member_after_preemption(0, false));
    assert!(!should_redrive_group_member_after_preemption(0, true));
    assert!(!should_redrive_group_member_after_preemption(1, false));
}

#[test]
fn group_chat_glue_owns_final_persistence_observation_and_shared_publication() {
    let root = temp_root("shared");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let alice = sessions
        .materialize_new_session(Some(&profile("Alice")), "user", None)
        .expect("Alice");
    let room = sessions
        .materialize_new_session(Some(&profile("Team")), "user", None)
        .expect("room");
    write_sand_group_config(
        root.join(&room.id),
        &SandGroupConfig {
            version: GROUP_CONFIG_VERSION,
            member_ids: vec![alice.id.clone()],
            remote_members: None,
            shared_room_id: Some("shared-room-a".into()),
        },
    )
    .expect("group config");
    sessions
        .append_agent_transcript_entries(
            &room.id,
            &[json!({
                "kind":"message",
                "id":"t0u",
                "role":"user",
                "content":"hello team",
                "timestampMs":1,
            })],
        )
        .expect("user message");

    let runtime = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let published = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let published_sink = Arc::clone(&published);
    let publisher: SharedGroupRoomEntryPublisher = Arc::new(move |room_id, entry| {
        published_sink
            .lock()
            .expect("published")
            .push((room_id.to_string(), entry.clone()));
        Ok(())
    });
    runtime.bind_shared_group_room_entry_publisher(Some(publisher));

    let observed = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    let observed_sink = Arc::clone(&observed);
    let observer: GroupRoomEntryObserver = Arc::new(move |room_id, entry| {
        observed_sink
            .lock()
            .expect("observed")
            .push((room_id.to_string(), entry.clone()));
    });

    let executor: GroupMemberTurnExecutor =
        Arc::new(|request| Ok(vec![format!("{} says hello", request.member.name)]));
    let remote_executor: GroupMemberTurnExecutor = Arc::new(|_| Ok(Vec::new()));

    let glue = GroupChatGlue::new(Arc::clone(&sessions));
    let outcome = glue
        .run_group_turn(
            Arc::clone(&runtime),
            &room.id,
            runtime.current_turn_epoch(&room.id),
            executor,
            Some(remote_executor),
            Some(observer),
        )
        .expect("group turn");

    let LocalGroupFanoutDisposition::Completed {
        posted_messages,
        member_failures,
    } = outcome
    else {
        panic!("expected completed group turn");
    };
    assert!(posted_messages > 0);
    assert!(member_failures.is_empty());

    let observed = observed.lock().expect("observed");
    assert_eq!(observed.len(), posted_messages);
    assert!(observed.iter().all(|(room_id, entry)| {
        room_id == &room.id && entry["author"]["id"] == alice.id
    }));

    let published = published.lock().expect("published");
    assert_eq!(published.len(), posted_messages);
    assert!(published.iter().all(|(shared_room_id, entry)| {
        shared_room_id == "shared-room-a" && entry["author"]["id"] == alice.id
    }));

    let durable = sessions
        .read_agent_transcript_entries(&room.id)
        .expect("room transcript");
    let durable_member_messages = durable
        .iter()
        .filter(|entry| {
            entry.get("kind").and_then(Value::as_str) == Some("send-message")
                && entry
                    .get("author")
                    .and_then(|author| author.get("id"))
                    .and_then(Value::as_str)
                    == Some(alice.id.as_str())
        })
        .count();
    assert_eq!(durable_member_messages, posted_messages);

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
