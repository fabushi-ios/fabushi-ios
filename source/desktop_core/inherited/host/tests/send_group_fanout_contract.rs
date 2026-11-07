const SHIPPING_HOST: &str = concat!(
    include_str!("../app/src/main.rs"),
    "\n",
    include_str!("../app/src/ocrm_handlers/group_turn.rs")
);

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::production_runtime::ProductionTranscriptRuntime;
use mahayana_host_runtime::extensions::transcript::send_group_fanout::{
    GROUP_MEMBER_DM_PREEMPTED_ERROR, GroupMemberTurnExecutor, LocalGroupFanoutDisposition,
    dispatch_local_group_send,
};
use mahayana_host_runtime::groups::group_chat::build_group_redrive_note;
use mahayana_host_runtime::groups::group_store::{
    GROUP_CONFIG_VERSION, SandGroupConfig, write_sand_group_config,
};
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-group-fanout-{label}-{}-{suffix}",
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
fn transcript_runtime_late_binds_the_canonical_shared_group_remote_executor() {
    let runtime = ProductionTranscriptRuntime::new(None);
    assert!(runtime.shared_group_remote_executor().is_none());

    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&calls);
    let executor: GroupMemberTurnExecutor = Arc::new(move |request| {
        observed
            .lock()
            .expect("calls")
            .push((request.member.id.clone(), request.prompt.clone()));
        Ok(vec!["remote reply".into()])
    });
    runtime.bind_shared_group_remote_executor(Some(executor));

    let bound = runtime
        .shared_group_remote_executor()
        .expect("late-bound remote executor");
    let result = bound(mahayana_host_runtime::extensions::transcript::group_chat_orchestrator::GroupMemberTurnRequest {
        member: mahayana_host_runtime::groups::group_chat::GroupMember {
            id: "remote-1".into(),
            name: "Remote".into(),
            description: String::new(),
        },
        system_prompt: "system".into(),
        prompt: "hello".into(),
        group: mahayana_host_runtime::groups::group_chat::GroupDescription {
            name: "Shared".into(),
            description: String::new(),
        },
        peers: Vec::new(),
        new_messages: Vec::new(),
        shared_room_id: Some("shared-room-1".into()),
    })
    .expect("remote turn");

    assert_eq!(result, vec!["remote reply".to_string()]);
    assert_eq!(
        calls.lock().expect("calls").as_slice(),
        &[("remote-1".to_string(), "hello".to_string())]
    );
}

#[test]
fn local_group_fanout_reads_room_history_executes_members_and_durably_posts_authored_messages() {
    let root = temp_root("local");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let alice = sessions
        .materialize_new_session(Some(&profile("Alice")), "user", None)
        .expect("Alice");
    let bob = sessions
        .materialize_new_session(Some(&profile("Bob")), "user", None)
        .expect("Bob");
    let room = sessions
        .materialize_new_session(Some(&profile("Team")), "user", None)
        .expect("room");

    write_sand_group_config(
        root.join(&room.id),
        &SandGroupConfig {
            version: GROUP_CONFIG_VERSION,
            member_ids: vec![alice.id.clone(), bob.id.clone()],
            remote_members: None,
            shared_room_id: None,
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
                "content":"Please answer as a team",
                "timestampMs":1,
            })],
        )
        .expect("user message");

    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&calls);
    let executor: GroupMemberTurnExecutor = Arc::new(move |request| {
        observed
            .lock()
            .expect("calls")
            .push((request.member.id.clone(), request.prompt.clone(), request.system_prompt.clone()));
        Ok(vec![format!("{} reply", request.member.name)])
    });
    let runtime = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let outcome = dispatch_local_group_send(
        Arc::clone(&sessions),
        Arc::clone(&runtime),
        &room.id,
        0,
        executor,
        None,
    )
    .expect("fanout");
    let LocalGroupFanoutDisposition::Completed {
        posted_messages,
        member_failures,
    } = outcome
    else {
        panic!("expected local completion");
    };
    assert!(posted_messages > 0);
    assert!(member_failures.is_empty());
    let calls = calls.lock().expect("calls");
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|(_, _, system)| system.contains("SendMessage")));

    let entries = sessions
        .read_agent_transcript_entries(&room.id)
        .expect("room transcript");
    let room_session_key = sessions
        .read_session_work_admission(&room.id)
        .expect("room admission")
        .expect("room admission record")
        .session_key;
    let authored = entries
        .iter()
        .filter(|entry| entry.get("kind").and_then(serde_json::Value::as_str) == Some("send-message"))
        .collect::<Vec<_>>();
    assert!(!authored.is_empty());
    assert!(authored.iter().all(|entry| {
        entry.get("author")
            .and_then(|author| author.get("id"))
            .and_then(serde_json::Value::as_str)
            .is_some()
            && entry.get("sessionKey").and_then(serde_json::Value::as_str)
                == Some(room_session_key.as_str())
    }));

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_or_shared_group_is_deferred_instead_of_fake_local_execution() {
    let root = temp_root("remote");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let room = sessions
        .materialize_new_session(Some(&profile("Shared")), "user", None)
        .expect("room");
    write_sand_group_config(
        root.join(&room.id),
        &SandGroupConfig {
            version: GROUP_CONFIG_VERSION,
            member_ids: vec![],
            remote_members: None,
            shared_room_id: Some("shared-1".into()),
        },
    )
    .expect("group config");
    let executor: GroupMemberTurnExecutor = Arc::new(|_| panic!("must not execute"));
    let outcome = dispatch_local_group_send(
        Arc::clone(&sessions),
        Arc::new(ProductionTranscriptRuntime::new(Some(&root))),
        &room.id,
        0,
        executor,
        None,
    )
    .expect("fanout");
    assert_eq!(
        outcome,
        LocalGroupFanoutDisposition::DeferredRemote {
            shared_room_id: Some("shared-1".into()),
            remote_member_count: 0,
        }
    );
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn shared_group_uses_remote_executor_without_creating_a_second_group_runtime() {
    use mahayana_host_runtime::groups::group_store::RemoteGroupMember;

    let root = temp_root("shared-remote");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let room = sessions
        .materialize_new_session(Some(&profile("Shared Team")), "user", None)
        .expect("room");
    write_sand_group_config(
        root.join(&room.id),
        &SandGroupConfig {
            version: GROUP_CONFIG_VERSION,
            member_ids: vec![],
            remote_members: Some(vec![RemoteGroupMember {
                owner_auth_id: "owner@example.com".into(),
                agent_id: "remote-agent".into(),
                name: "Remote Agent".into(),
                avatar_data_url: None,
            }]),
            shared_room_id: Some("shared-remote-1".into()),
        },
    )
    .expect("group config");
    sessions
        .append_agent_transcript_entries(
            &room.id,
            &[json!({
                "kind":"message",
                "id":"shared-user",
                "role":"user",
                "content":"Please answer from the remote room",
                "timestampMs":1,
            })],
        )
        .expect("user message");

    let local: GroupMemberTurnExecutor = Arc::new(|request| {
        panic!("local executor must not receive {}", request.member.id)
    });
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_remote = Arc::clone(&observed);
    let runtime = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    runtime.begin_provider_run(&room.id);
    let observed_runtime = Arc::clone(&runtime);
    let observed_room_id = room.id.clone();
    let remote: GroupMemberTurnExecutor = Arc::new(move |request| {
        let mut rows = json!([{"id": observed_room_id}]);
        observed_runtime.decorate_agent_summaries(&mut rows);
        assert_eq!(rows[0]["isRunning"], true);
        assert_eq!(
            rows[0]["activeRemoteMemberId"].as_str(),
            Some(request.member.id.as_str()),
        );
        observed_remote.lock().expect("remote calls").push((
            request.member.id.clone(),
            request.shared_room_id.clone(),
            request.group.name.clone(),
            request.peers.len(),
            request.new_messages.len(),
        ));
        Ok(vec!["remote reply".into()])
    });

    let outcome = dispatch_local_group_send(
        Arc::clone(&sessions),
        Arc::clone(&runtime),
        &room.id,
        0,
        local,
        Some(remote),
    )
    .expect("shared fanout");
    let LocalGroupFanoutDisposition::Completed {
        posted_messages,
        member_failures,
    } = outcome
    else {
        panic!("expected shared completion");
    };
    assert!(posted_messages > 0);
    assert!(member_failures.is_empty());
    let calls = observed.lock().expect("remote calls");
    assert!(!calls.is_empty());
    assert!(
        calls
            .iter()
            .all(|(member_id, room_id, group, _, _)| {
                member_id.starts_with("sand-remote:owner%40example.com/remote-agent")
                    && room_id.as_deref() == Some("shared-remote-1")
                    && group == "Shared Team"
            })
    );

    let mut settled_rows = json!([{"id": room.id.clone()}]);
    runtime.decorate_agent_summaries(&mut settled_rows);
    assert!(settled_rows[0]["activeRemoteMemberId"].is_null());
    runtime.end_provider_run(&room.id);

    let entries = sessions
        .read_agent_transcript_entries(&room.id)
        .expect("room transcript");
    assert!(entries.iter().any(|entry| {
        entry
            .get("author")
            .and_then(|author| author.get("id"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id.starts_with("sand-remote:"))
    }));

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_agent_posted_group_turn_routes_through_group_chat_glue_owner() {
    let start = SHIPPING_HOST
        .find("fn run_agent_posted_group_turn(")
        .expect("agent-posted group turn");
    let end = SHIPPING_HOST[start..]
        .find("fn run_local_group_member_turn(")
        .map(|offset| start + offset)
        .expect("group member turn boundary");
    let body = &SHIPPING_HOST[start..end];

    assert!(body.contains("dispatch_runtime.shared_group_remote_executor()"));
    assert!(body.contains("deps.transcript_manager.group_chat()"));
    assert!(body.contains("dispatch_group_chat.run_group_turn("));
    assert!(!body.contains("GroupChatGlue::new("));
    assert!(body.contains("remote_executor,"));
    assert!(!body.contains("dispatch_local_group_send("));
}

#[test]
fn shipping_group_fanout_keeps_cursor_on_the_canonical_host_runner_path() {
    assert!(SHIPPING_HOST.contains("self.transcript_manager.group_chat().run_group_turn("));
    assert!(!SHIPPING_HOST.contains("GroupChatGlue::new(Arc::clone(&self.session_workers))"));
    assert!(SHIPPING_HOST.contains("self.cross_user.remote_executor()"));
    assert!(SHIPPING_HOST.contains("run_local_group_member_turn(deps.clone(), provider, Some(&member_room_id), request)"));
    assert!(SHIPPING_HOST.contains("start_routed_provider_task("));
    assert!(!SHIPPING_HOST.contains("Cursor group member turns remain on the compatibility path"));
    assert!(!SHIPPING_HOST.contains("if provider == RoutedProvider::Cursor {\n            return Ok(None);"));
}

#[test]
fn shipping_shared_room_fanout_keeps_cursor_on_the_same_canonical_runner_path() {
    assert!(SHIPPING_HOST.contains("let run_shared_room_turn: SharedRoomTurnRunner"));
    assert!(SHIPPING_HOST.contains("let shared_room_group_chat = transcript_manager.group_chat();"));
    assert!(SHIPPING_HOST.contains("shared_room_group_chat.run_group_turn("));
    assert!(!SHIPPING_HOST.contains("GroupChatGlue::new(Arc::clone(&shared_room_sessions))"));
    assert!(SHIPPING_HOST.contains("Some(&member_room_id)"));
    assert!(!SHIPPING_HOST.contains("Cursor shared-room fanout remains on the compatibility path"));
}

#[test]
fn shipping_send_prompt_routes_mirror_and_hosted_room_entries_through_cross_user_publisher() {
    assert!(SHIPPING_HOST.contains("dispatch_mirror_or_group_send_if_supported("));
    assert!(SHIPPING_HOST.contains("read_sand_remote_room_config(&agent_dir)"));
    assert!(SHIPPING_HOST.contains("read_sand_group_config(&agent_dir)"));
    assert!(SHIPPING_HOST.contains("publish_room_entry_and_wait(&remote_room.room_id, &entry)"));
    assert!(SHIPPING_HOST.contains("publish_room_entry_and_wait(shared_room_id, &entry)"));
    assert!(SHIPPING_HOST.contains("Shared mirror rooms only support image attachments."));
    assert!(SHIPPING_HOST.contains("persisted_send_context"));
}


#[test]
fn dm_preempted_group_member_redrives_only_while_room_epoch_is_current() {
    let root = temp_root("dm-preempt");
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
            shared_room_id: None,
        },
    )
    .expect("group config");
    sessions
        .append_agent_transcript_entries(
            &room.id,
            &[json!({
                "kind":"message",
                "id":"dm-preempt-user",
                "role":"user",
                "content":"answer",
                "timestampMs":1,
            })],
        )
        .expect("user message");

    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = Arc::clone(&prompts);
    let executor: GroupMemberTurnExecutor = Arc::new(move |request| {
        observed.lock().expect("prompts").push(request.prompt);
        Err(GROUP_MEMBER_DM_PREEMPTED_ERROR.into())
    });
    let runtime = Arc::new(ProductionTranscriptRuntime::new(Some(&root)));
    let outcome = dispatch_local_group_send(
        Arc::clone(&sessions),
        Arc::clone(&runtime),
        &room.id,
        0,
        executor,
        None,
    )
    .expect("fanout");
    let prompts = prompts.lock().expect("prompts").clone();
    assert_eq!(prompts.len(), 3);
    assert!(!prompts[0].contains(build_group_redrive_note()));
    assert!(prompts[1].ends_with(build_group_redrive_note()));
    assert!(prompts[2].ends_with(build_group_redrive_note()));
    let LocalGroupFanoutDisposition::Completed { posted_messages, member_failures } = outcome else {
        panic!("expected completion");
    };
    assert_eq!(posted_messages, 0);
    assert!(member_failures.is_empty());

    let stale_calls = Arc::new(Mutex::new(0usize));
    let observed = Arc::clone(&stale_calls);
    let stale_executor: GroupMemberTurnExecutor = Arc::new(move |_| {
        *observed.lock().expect("stale calls") += 1;
        Err(GROUP_MEMBER_DM_PREEMPTED_ERROR.into())
    });
    let stale = dispatch_local_group_send(
        Arc::clone(&sessions),
        Arc::clone(&runtime),
        &room.id,
        99,
        stale_executor,
        None,
    )
    .expect("stale fanout");
    assert_eq!(*stale_calls.lock().expect("stale calls"), 0);
    let LocalGroupFanoutDisposition::Completed { posted_messages, member_failures } = stale else {
        panic!("expected stale completion");
    };
    assert_eq!(posted_messages, 0);
    assert!(member_failures.is_empty());

    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}
