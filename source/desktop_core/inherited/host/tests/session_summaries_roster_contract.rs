use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::{
    SandAgentProfile, write_sand_profile_file,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::groups::group_store::{
    GROUP_CONFIG_VERSION, SandGroupConfig, write_sand_group_config,
};
use mahayana_host_runtime::groups::remote_room_store::{
    REMOTE_ROOM_CONFIG_VERSION, RemoteRoomMember, SandRemoteRoomConfig,
    write_sand_remote_room_config,
};
use mahayana_host_runtime::storage::store_db::{
    register_live_db_handle, release_live_db_handle,
};
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-session-roster-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn production_roster_projects_profile_transcript_unread_and_settings() {
    let root = temp_root("summary");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = workers
        .materialize_new_session(
            Some(&SandAgentProfile {
                name: " Agent ".into(),
                description: "desc".into(),
                title: "Title".into(),
                avatar_shape: "round".into(),
                avatar_color: "blue".into(),
            }),
            "dev",
            Some("research"),
        )
        .expect("materialize");
    workers
        .append_agent_transcript_entries(
            &record.id,
            &[json!({
                "id":"m1",
                "kind":"send-message",
                "message":{"type":"text","content":"hello **there**"}
            })],
        )
        .expect("append");
    workers.mark_agent_activity(&record.id, 100.0).expect("activity");
    workers.set_agent_unread(&record.id, true, 101.0).expect("unread");
    fs::write(
        root.join(&record.id).join("avatar.png"),
        [137, 80, 78, 71, 13, 10, 26, 10, 0],
    )
    .expect("avatar");

    let summary = workers
        .summarize_agent_by_id(&record.id, Some(&record.id))
        .expect("summary")
        .expect("present");
    assert_eq!(summary.id, record.id);
    assert_eq!(summary.name, "Agent");
    assert_eq!(summary.title, "Title");
    assert_eq!(summary.last_message_id.as_deref(), Some("m1"));
    assert_eq!(summary.last_message_preview.as_deref(), Some("hello there"));
    assert!(summary.has_unread);
    assert_eq!(summary.origin, "dev");
    assert_eq!(summary.purpose.as_deref(), Some("research"));
    assert!(summary.is_active);
    assert!(
        summary
            .avatar_data_url
            .as_deref()
            .is_some_and(|value| value.starts_with("data:image/png;base64,"))
    );
    assert!(
        summary
            .avatar_version
            .as_deref()
            .is_some_and(|value| value.len() == 16)
    );

    let listed = workers
        .list_agent_summaries(Some(&record.id))
        .expect("list summaries");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, record.id);
    assert_eq!(listed[0].avatar_data_url, summary.avatar_data_url);
    assert_eq!(listed[0].avatar_version, summary.avatar_version);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_roster_recovers_profile_only_and_quarantined_missing_databases() {
    let root = temp_root("missing-db");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);

    let profile_agent = root.join("profile-only");
    fs::create_dir_all(&profile_agent).expect("profile dir");
    write_sand_profile_file(
        profile_agent.join("profile.json"),
        &SandAgentProfile {
            name: "Recovered Profile".into(),
            description: "profile survives db loss".into(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        },
    )
    .expect("profile");
    assert!(!profile_agent.join("store.db").exists());

    let memory_agent = root.join("memory-only");
    let memory_dir = memory_agent.join("memory");
    fs::create_dir_all(&memory_dir).expect("memory dir");
    fs::write(
        memory_dir.join("profile.md"),
        "# About the user\n\n- (2026-09-24) Durable memory survives database loss\n",
    )
    .expect("memory profile");
    assert!(!memory_agent.join("store.db").exists());

        let quarantined_agent = root.join("quarantined-only");
    fs::create_dir_all(&quarantined_agent).expect("quarantine dir");
    fs::write(
        quarantined_agent.join("store.db.corrupt-20260924"),
        b"quarantined",
    )
    .expect("quarantined marker");
    assert!(!quarantined_agent.join("store.db").exists());

    let listed = workers.list_agent_summaries(None).expect("recovered roster");
    let profile_summary = listed
        .iter()
        .find(|summary| summary.id == "profile-only")
        .expect("profile agent summary");
    assert_eq!(profile_summary.name, "Recovered Profile");
    assert!(profile_agent.join("store.db").is_file());

    assert!(listed.iter().any(|summary| summary.id == "quarantined-only"));
    assert!(quarantined_agent.join("store.db").is_file());

    assert!(listed.iter().any(|summary| summary.id == "memory-only"));
    assert!(memory_agent.join("store.db").is_file());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_roster_does_not_reseed_missing_db_while_live_handle_is_registered() {
    let root = temp_root("live-handle");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let agent_dir = root.join("live-agent");
    fs::create_dir_all(&agent_dir).expect("agent dir");
    write_sand_profile_file(
        agent_dir.join("profile.json"),
        &SandAgentProfile {
            name: "Live Agent".into(),
            description: String::new(),
            title: String::new(),
            avatar_shape: String::new(),
            avatar_color: String::new(),
        },
    )
    .expect("profile");
    let db_path = agent_dir.join("store.db");
    register_live_db_handle(&db_path);

    let summary = workers
        .summarize_agent_by_id("live-agent", None)
        .expect("summary")
        .expect("live summary");
    assert_eq!(summary.name, "Live Agent");
    assert!(!db_path.exists());

    release_live_db_handle(&db_path);
    let summary = workers
        .summarize_agent_by_id("live-agent", None)
        .expect("summary after release")
        .expect("recovered summary");
    assert_eq!(summary.name, "Live Agent");
    assert!(db_path.is_file());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_roster_excludes_agents_while_delete_fence_is_active() {
    let root = temp_root("delete-fence");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = workers
        .materialize_new_session(None, "user", None)
        .expect("materialize");
    assert!(
        workers
            .summarize_agent_by_id(&record.id, None)
            .expect("summary")
            .is_some()
    );

    workers
        .begin_agent_delete(&record.id)
        .expect("publish deletion fence");
    assert!(
        workers
            .summarize_agent_by_id(&record.id, None)
            .expect("fenced summary")
            .is_none()
    );
    assert!(
        workers
            .list_agent_summaries(None)
            .expect("fenced roster")
            .iter()
            .all(|summary| summary.id != record.id)
    );

    workers
        .end_agent_delete(&record.id)
        .expect("clear deletion fence");
    assert!(
        workers
            .summarize_agent_by_id(&record.id, None)
            .expect("restored summary")
            .is_some()
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_roster_cache_tracks_generation_and_prunes_removed_agents() {
    let root = temp_root("extras-cache");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let first = workers
        .materialize_new_session(None, "user", None)
        .expect("first agent");
    let second = workers
        .materialize_new_session(None, "user", None)
        .expect("second agent");

    let _ = workers.list_agent_summaries(None).expect("prime roster cache");
    assert_eq!(workers.roster_extras_cache_entry_count(), 2);

    workers
        .mark_agent_activity(&first.id, 123.0)
        .expect("activity mutation");
    let refreshed = workers
        .summarize_agent_by_id(&first.id, None)
        .expect("refreshed summary")
        .expect("first summary");
    assert_eq!(refreshed.last_activity_at, 123.0);

    workers
        .begin_agent_delete(&second.id)
        .expect("publish deletion fence");
    assert_eq!(workers.roster_extras_cache_entry_count(), 1);
    workers
        .end_agent_delete(&second.id)
        .expect("clear deletion fence");
    fs::remove_dir_all(root.join(&second.id)).expect("remove second agent");
    let listed = workers.list_agent_summaries(None).expect("pruned roster");
    assert!(listed.iter().all(|summary| summary.id != second.id));
    assert_eq!(workers.roster_extras_cache_entry_count(), 1);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_roster_projects_group_and_remote_room_durable_owners() {
    let root = temp_root("group-room-summary");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = workers
        .materialize_new_session(None, "user", None)
        .expect("materialize group");
    let agent_dir = root.join(&record.id);

    write_sand_group_config(
        &agent_dir,
        &SandGroupConfig {
            version: GROUP_CONFIG_VERSION,
            member_ids: vec!["member-a".into(), "member-b".into()],
            remote_members: None,
            shared_room_id: Some("shared-room-1".into()),
        },
    )
    .expect("group config");
    write_sand_remote_room_config(
        &agent_dir,
        &SandRemoteRoomConfig {
            version: REMOTE_ROOM_CONFIG_VERSION,
            room_id: "remote-room-1".into(),
            host_auth_id: "host-auth".into(),
            host_name: "Room Host".into(),
            host_avatar_url: None,
            members: vec![RemoteRoomMember {
                kind: "human".into(),
                auth_id: "guest-auth".into(),
                agent_id: String::new(),
                display_name: "Guest".into(),
                avatar_url: None,
            }],
            is_revoked: None,
        },
    )
    .expect("remote room config");

    let summary = workers
        .summarize_agent_by_id(&record.id, None)
        .expect("summary")
        .expect("group summary");
    assert!(summary.is_group);
    assert_eq!(summary.member_ids, vec!["member-a", "member-b"]);
    let remote_room = summary.remote_room.expect("remote room summary");
    assert_eq!(remote_room.room_id, "remote-room-1");
    assert_eq!(remote_room.host_name, "Room Host");
    assert_eq!(remote_room.members.len(), 1);

    let serialized = serde_json::to_value(
        workers
            .summarize_agent_by_id(&record.id, None)
            .expect("serialized summary")
            .expect("serialized group summary"),
    )
    .expect("summary json");
    assert_eq!(serialized["isGroup"], true);
    assert_eq!(serialized["memberIds"], json!(["member-a", "member-b"]));
    assert_eq!(serialized["remoteRoom"]["roomId"], "remote-room-1");

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn production_roster_ignores_reserved_session_namespaces_without_recovery_side_effects() {
    let root = temp_root("reserved-session-namespace");
    let workers = ProductionSessionWorkers::with_agents_root(&root, 500);
    let record = workers
        .materialize_new_session(
            Some(&SandAgentProfile {
                name: "Visible Agent".into(),
                description: String::new(),
                title: String::new(),
                avatar_shape: String::new(),
                avatar_color: String::new(),
            }),
            "user",
            None,
        )
        .expect("materialize");

    let reserved = root.join(".conversations");
    fs::create_dir_all(reserved.join("human-conversation")).expect("reserved namespace");

    let listed = workers.list_agent_summaries(None).expect("list agents");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, record.id);
    assert!(!reserved.join("store.db").exists());
    assert!(workers
        .summarize_agent_by_id(".conversations", None)
        .expect("reserved summary")
        .is_none());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
