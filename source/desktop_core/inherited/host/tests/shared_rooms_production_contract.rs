use std::fs;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::transcript_manager::TranscriptManager;
use mahayana_host_runtime::groups::group_store::{
    RemoteGroupMember, read_sand_group_config,
};
use mahayana_host_runtime::groups::remote_room_store::{
    RemoteRoomMember, read_sand_remote_room_config,
};
use serde_json::json;
use url::Url;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-shared-rooms-{label}-{}-{suffix}",
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
fn transcript_manager_owns_one_shared_rooms_service() {
    let root = temp_root("owner");
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, sessions);
    let first = manager.shared_rooms();
    let second = manager.shared_rooms();
    assert!(Arc::ptr_eq(&first, &second));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn hosted_room_materializes_filters_members_persists_notice_and_restamps_content() {
    let root = temp_root("hosted");
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let member = sessions
        .materialize_new_session(Some(&profile("Member")), "user", None)
        .expect("member");
    let group = sessions
        .materialize_new_session(Some(&profile("Nested group")), "user", None)
        .expect("nested group");
    mahayana_host_runtime::groups::group_store::write_sand_group_config(
        sessions.agents_root().join(&group.id),
        &mahayana_host_runtime::groups::group_store::SandGroupConfig {
            version: mahayana_host_runtime::groups::group_store::GROUP_CONFIG_VERSION,
            member_ids: vec![member.id.clone()],
            remote_members: None,
            shared_room_id: None,
        },
    )
    .expect("nested group marker");

    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let rooms = manager.shared_rooms();
    let remote = RemoteGroupMember {
        owner_auth_id: "remote-owner".into(),
        agent_id: "remote-agent".into(),
        name: "Remote".into(),
        avatar_data_url: Some("data:image/png;base64,AA==".into()),
    };
    let room_agent = rooms
        .ensure_hosted_shared_room(
            "room-hosted",
            "Shared group",
            &[member.id.clone(), group.id.clone(), "missing".into()],
            &[remote.clone()],
            true,
        )
        .expect("ensure hosted")
        .expect("created");

    let binding = read_sand_group_config(sessions.agents_root().join(&room_agent))
        .expect("group binding");
    assert_eq!(binding.shared_room_id.as_deref(), Some("room-hosted"));
    assert_eq!(binding.member_ids, vec![member.id.clone()]);
    assert_eq!(binding.remote_members, Some(vec![remote]));
    assert_eq!(
        rooms.get_shared_room_id_for_agent(&room_agent).as_deref(),
        Some("room-hosted")
    );

    assert!(rooms
        .append_shared_room_activity_notice("room-hosted", "Member is responding.")
        .expect("notice"));
    let entries = sessions
        .read_agent_transcript_entries(&room_agent)
        .expect("entries");
    assert!(entries.iter().any(|entry| {
        entry.get("kind").and_then(serde_json::Value::as_str) == Some("notice")
            && entry.get("text").and_then(serde_json::Value::as_str)
                == Some("Member is responding.")
    }));

    let room_admission = sessions
        .read_session_work_admission(&room_agent)
        .expect("room admission")
        .expect("room admission record");
    sessions
        .append_agent_transcript_entries(
            &room_agent,
            &[json!({
                "kind": "message",
                "id": "t1u",
                "role": "user",
                "content": "hello",
                "sessionKey": room_admission.session_key.clone(),
                "timestampMs": 1.0
            })],
        )
        .expect("append content");
    assert!(rooms
        .restamp_room_entry("room-hosted", "t1u", 42.0)
        .expect("restamp"));
    let stamped = sessions
        .read_agent_transcript_entries(&room_agent)
        .expect("stamped entries")
        .into_iter()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some("t1u"))
        .expect("stamped entry");
    assert_eq!(
        stamped.get("timestampMs").and_then(serde_json::Value::as_f64),
        Some(42.0)
    );

    sessions
        .transition_session_work_archive(
            &room_agent,
            &room_admission.session_key,
            &room_admission.session_id,
            Some(&room_admission.lifecycle_revision),
            true,
            Some("contract-test"),
            Some("restamp archive fence"),
            43.0,
        )
        .expect("archive room session");
    let archived_error = rooms
        .restamp_room_entry("room-hosted", "t1u", 44.0)
        .expect_err("archived bound restamp must reject");
    assert!(archived_error.contains("session is not admitted"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn mirror_room_materializes_remote_binding_filters_self_and_can_be_revoked() {
    let root = temp_root("mirror");
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let rooms = manager.shared_rooms();
    let members = vec![
        RemoteRoomMember {
            kind: "human".into(),
            auth_id: "self".into(),
            agent_id: String::new(),
            display_name: "Me".into(),
            avatar_url: None,
        },
        RemoteRoomMember {
            kind: "agent".into(),
            auth_id: "remote".into(),
            agent_id: "agent-r".into(),
            display_name: "Remote agent".into(),
            avatar_url: Some("avatar".into()),
        },
    ];

    let room_agent = rooms
        .ensure_mirror_room(
            "room-mirror",
            "Remote room",
            "host",
            "Host",
            Some("host-avatar"),
            &members,
            "self",
        )
        .expect("mirror");

    let binding = read_sand_remote_room_config(sessions.agents_root().join(&room_agent))
        .expect("remote binding");
    assert_eq!(binding.room_id, "room-mirror");
    assert_eq!(binding.host_auth_id, "host");
    assert_eq!(binding.members.len(), 1);
    assert_eq!(binding.members[0].auth_id, "remote");
    assert_eq!(binding.is_revoked, None);

    rooms
        .mark_mirror_room_revoked("room-mirror")
        .expect("revoke");
    let revoked = read_sand_remote_room_config(sessions.agents_root().join(&room_agent))
        .expect("revoked binding");
    assert_eq!(revoked.is_revoked, Some(true));

    let _ = fs::remove_dir_all(root);
}


#[test]
fn shared_room_ingress_materializes_inline_images_on_shipping_transcript_paths() {
    let root = temp_root("inline-images");
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let rooms = manager.shared_rooms();

    let hosted_agent = rooms
        .ensure_hosted_shared_room(
            "room-images-hosted",
            "Hosted images",
            &[],
            &[],
            true,
        )
        .expect("ensure hosted")
        .expect("hosted room agent");
    rooms
        .post_shared_room_guest_message(&json!({
            "roomId": "room-images-hosted",
            "text": "guest image",
            "authorAuthId": "remote-human",
            "authorName": "Remote human",
            "images": [{
                "base64": STANDARD.encode(b"hosted-image"),
                "mediaType": "image/png",
                "alt": "Hosted"
            }]
        }))
        .expect("post hosted guest image");

    let hosted_entry = sessions
        .read_agent_transcript_entries(&hosted_agent)
        .expect("hosted transcript")
        .into_iter()
        .find(|entry| entry.get("content").and_then(serde_json::Value::as_str) == Some("guest image"))
        .expect("hosted image entry");
    let hosted_url = hosted_entry["images"][0]["url"]
        .as_str()
        .expect("hosted image url");
    let hosted_path = Url::parse(hosted_url)
        .expect("hosted file url")
        .to_file_path()
        .expect("hosted file path");
    assert_eq!(fs::read(&hosted_path).expect("hosted image bytes"), b"hosted-image");
    assert_eq!(
        hosted_path
            .parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str()),
        Some("xuser-attachments")
    );
    assert_eq!(hosted_entry["images"][0]["alt"], "Hosted");

    let mirror_agent = rooms
        .ensure_mirror_room(
            "room-images-mirror",
            "Mirror images",
            "remote-host",
            "Remote host",
            None,
            &[],
            "self",
        )
        .expect("mirror room");
    assert!(rooms
        .append_mirror_room_entry(
            &json!({
                "roomId": "room-images-mirror",
                "entry": {
                    "kind": "agent-message",
                    "entryId": "remote-entry-image",
                    "agentOwnerAuthId": "remote-owner",
                    "agentId": "remote-agent",
                    "authorName": "Remote agent",
                    "text": "mirror image",
                    "timestampMs": 42.0,
                    "images": [{
                        "base64": STANDARD.encode(b"mirror-image"),
                        "mediaType": "image/webp",
                        "alt": "Mirror"
                    }]
                }
            }),
            "self",
        )
        .expect("append mirror image"));

    let mirror_entry = sessions
        .read_agent_transcript_entries(&mirror_agent)
        .expect("mirror transcript")
        .into_iter()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some("xu-remote-entry-image"))
        .expect("mirror image entry");
    let mirror_url = mirror_entry["message"]["images"][0]["url"]
        .as_str()
        .expect("mirror image url");
    let mirror_path = Url::parse(mirror_url)
        .expect("mirror file url")
        .to_file_path()
        .expect("mirror file path");
    assert_eq!(fs::read(&mirror_path).expect("mirror image bytes"), b"mirror-image");
    assert!(mirror_path.to_string_lossy().ends_with(".webp"));
    assert_eq!(mirror_entry["message"]["images"][0]["alt"], "Mirror");

    let _ = fs::remove_dir_all(root);
}
