use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::groups::group_store::{
    GROUP_CONFIG_VERSION, GROUP_MAX_MEMBERS, RemoteGroupMember, SandGroupConfig,
    get_sand_group_path, is_sand_group_dir, normalize_member_ids, normalize_remote_members,
    read_sand_group_config, write_sand_group_config,
};
use mahayana_host_runtime::groups::remote_room_store::{
    REMOTE_ROOM_CONFIG_VERSION, REMOTE_ROOM_MAX_MEMBERS, RemoteRoomMember,
    SandRemoteRoomConfig, get_sand_remote_room_path, is_sand_remote_room_dir,
    normalize_remote_room_members, read_sand_remote_room_config,
    write_sand_remote_room_config,
};
use serde_json::json;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-groups-store-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn group_store_matches_frozen_normalization_read_write_and_presence_contract() {
    let root = temp_root("group");
    let agent_dir = root.join("agent");

    let ids = normalize_member_ids(&json!([
        " a ", "a", "", 7, "b", "c", "d", "e", "f", "g"
    ]));
    assert_eq!(ids.len(), GROUP_MAX_MEMBERS);
    assert_eq!(ids, vec!["a", "b", "c", "d", "e", "f"]);

    let remote = normalize_remote_members(&json!([
        {"ownerAuthId":" owner ","agentId":" agent ","name":"  ","avatarDataUrl":"data:x"},
        {"ownerAuthId":"owner","agentId":"agent","name":"duplicate"},
        {"ownerAuthId":"","agentId":"x","name":"invalid"},
        {"ownerAuthId":"two","agentId":"a","name":" Two "}
    ]));
    assert_eq!(
        remote,
        vec![
            RemoteGroupMember {
                owner_auth_id: "owner".into(),
                agent_id: "agent".into(),
                name: "Agent".into(),
                avatar_data_url: Some("data:x".into()),
            },
            RemoteGroupMember {
                owner_auth_id: "two".into(),
                agent_id: "a".into(),
                name: "Two".into(),
                avatar_data_url: None,
            },
        ]
    );

    write_sand_group_config(
        &agent_dir,
        &SandGroupConfig {
            version: GROUP_CONFIG_VERSION,
            member_ids: vec![" a ".into(), "a".into(), "b".into()],
            remote_members: Some(remote.clone()),
            shared_room_id: Some("room-1".into()),
        },
    )
    .expect("write group");
    assert!(fs::read_to_string(get_sand_group_path(&agent_dir))
        .expect("group json")
        .ends_with('\n'));
    assert!(is_sand_group_dir(&agent_dir));
    let read = read_sand_group_config(&agent_dir).expect("group config");
    assert_eq!(read.member_ids, vec!["a", "b"]);
    assert_eq!(read.remote_members, Some(remote));
    assert_eq!(read.shared_room_id.as_deref(), Some("room-1"));

    fs::write(
        get_sand_group_path(&agent_dir),
        r#"{"version":"bad","memberIds":[],"remoteMembers":[],"sharedRoomId":""}"#,
    )
    .expect("empty config");
    assert!(!is_sand_group_dir(&agent_dir));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn remote_room_store_matches_frozen_defaults_member_cap_and_round_trip() {
    let root = temp_root("remote");
    let agent_dir = root.join("agent");
    let raw_members = (0..30)
        .map(|index| {
            if index == 0 {
                json!({"kind":"agent","authId":"auth-0","agentId":"agent-0","displayName":"","avatarUrl":"avatar"})
            } else {
                json!({"kind":"other","authId":format!("auth-{index}"),"agentId":7,"displayName":format!("Member {index}")})
            }
        })
        .collect::<Vec<_>>();
    let members = normalize_remote_room_members(&json!(raw_members));
    assert_eq!(members.len(), REMOTE_ROOM_MAX_MEMBERS);
    assert_eq!(
        members[0],
        RemoteRoomMember {
            kind: "agent".into(),
            auth_id: "auth-0".into(),
            agent_id: "agent-0".into(),
            display_name: "Someone".into(),
            avatar_url: Some("avatar".into()),
        }
    );
    assert_eq!(members[1].kind, "human");
    assert_eq!(members[1].agent_id, "");

    let config = SandRemoteRoomConfig {
        version: REMOTE_ROOM_CONFIG_VERSION,
        room_id: "room-1".into(),
        host_auth_id: "host-1".into(),
        host_name: "Host".into(),
        host_avatar_url: Some("avatar-host".into()),
        members: members[..2].to_vec(),
        is_revoked: Some(true),
    };
    write_sand_remote_room_config(&agent_dir, &config).expect("write room");
    assert!(fs::read_to_string(get_sand_remote_room_path(&agent_dir))
        .expect("room json")
        .ends_with('\n'));
    assert!(is_sand_remote_room_dir(&agent_dir));
    assert_eq!(read_sand_remote_room_config(&agent_dir), Some(config));

    fs::write(
        get_sand_remote_room_path(&agent_dir),
        r#"{"roomId":"","hostAuthId":"host"}"#,
    )
    .expect("invalid room");
    assert!(!is_sand_remote_room_dir(&agent_dir));

    fs::write(
        get_sand_remote_room_path(&agent_dir),
        r#"{"roomId":"room","hostAuthId":"host","members":[]}"#,
    )
    .expect("default room");
    let defaulted = read_sand_remote_room_config(&agent_dir).expect("defaulted room");
    assert_eq!(defaulted.host_name, "The host");
    assert_eq!(defaulted.version, REMOTE_ROOM_CONFIG_VERSION);
    assert_eq!(defaulted.is_revoked, None);

    let _ = fs::remove_dir_all(root);
}
