use mahayana_host_runtime::groups::group_chat::{
    GroupDescription, GroupMember, GroupMessage, GroupSpeaker,
};
use mahayana_host_runtime::groups::xuser::{
    REMOTE_MEMBER_TURN_TIMEOUT_MS, REMOTE_TURN_MESSAGE_LIMIT, REMOTE_TURN_TEXT_MAX_LENGTH,
    XuserSpeakerKind, XuserTurnMessage, build_remote_member_turn_prompts,
    build_shared_room_guardrail_prompt, clamp_guest_name, from_xuser_turn_messages,
    resolve_shared_room_box_tools_enabled, to_xuser_turn_messages,
};

fn member(id: &str, name: &str) -> GroupMember {
    GroupMember {
        id: id.into(),
        name: name.into(),
        description: String::new(),
    }
}

#[test]
fn shared_room_flags_and_guardrail_match_the_frozen_contract() {
    assert_eq!(REMOTE_MEMBER_TURN_TIMEOUT_MS, 600_000);
    assert_eq!(REMOTE_TURN_MESSAGE_LIMIT, 24);
    assert_eq!(REMOTE_TURN_TEXT_MAX_LENGTH, 8_000);
    assert!(resolve_shared_room_box_tools_enabled(None));
    assert!(resolve_shared_room_box_tools_enabled(Some("")));
    assert!(!resolve_shared_room_box_tools_enabled(Some("0")));
    assert!(!resolve_shared_room_box_tools_enabled(Some("FALSE")));
    assert!(resolve_shared_room_box_tools_enabled(Some("no")));

    let foreign = build_shared_room_guardrail_prompt("Alice", true);
    assert!(foreign.starts_with("\nSHARED ROOM"));
    assert!(foreign.contains("hosted by Alice, a DIFFERENT person"));
    assert!(foreign.contains("Only SendMessage plain text is delivered to the room."));
    assert!(foreign.contains("Never reveal private chats, memory, files, credentials, tokens, or connector data."));

    let local = build_shared_room_guardrail_prompt("ignored", false);
    assert!(local.contains("shared with people other than your user"));
    assert!(!local.contains("DIFFERENT person"));
}

#[test]
fn xuser_message_projection_clamps_and_preserves_self_identity() {
    let mut history = (0..25)
        .map(|index| GroupMessage {
            speaker: GroupSpeaker::User { name: None },
            content: format!("old-{index}"),
        })
        .collect::<Vec<_>>();
    history.push(GroupMessage {
        speaker: GroupSpeaker::User {
            name: Some("  Human\r\nName  ".into()),
        },
        content: "  hello  ".into(),
    });
    history.push(GroupMessage {
        speaker: GroupSpeaker::Member {
            id: "self".into(),
            name: "  Agent\nName  ".into(),
        },
        content: "  answer  ".into(),
    });

    let projected = to_xuser_turn_messages(&history, "self");
    assert_eq!(projected.len(), REMOTE_TURN_MESSAGE_LIMIT);
    assert_eq!(projected[projected.len() - 2].speaker_kind, XuserSpeakerKind::Human);
    assert_eq!(projected[projected.len() - 2].speaker_name, "Human Name");
    assert_eq!(projected[projected.len() - 2].text, "hello");
    assert_eq!(projected.last().unwrap().speaker_kind, XuserSpeakerKind::Agent);
    assert_eq!(projected.last().unwrap().speaker_name, "Agent Name");
    assert!(projected.last().unwrap().is_self);

    let restored = from_xuser_turn_messages(
        &[
            XuserTurnMessage {
                speaker_kind: XuserSpeakerKind::Human,
                speaker_name: " Human ".into(),
                text: " body ".into(),
                is_self: false,
            },
            XuserTurnMessage {
                speaker_kind: XuserSpeakerKind::Agent,
                speaker_name: "Peer\nName".into(),
                text: " reply ".into(),
                is_self: false,
            },
            XuserTurnMessage {
                speaker_kind: XuserSpeakerKind::Agent,
                speaker_name: "Self".into(),
                text: " mine ".into(),
                is_self: true,
            },
        ],
        "self-id",
    );
    assert_eq!(
        restored[0],
        GroupMessage {
            speaker: GroupSpeaker::User {
                name: Some("Human".into())
            },
            content: "body".into(),
        }
    );
    assert_eq!(
        restored[1],
        GroupMessage {
            speaker: GroupSpeaker::Member {
                id: "peer:Peer\nName".into(),
                name: "Peer Name".into(),
            },
            content: "reply".into(),
        }
    );
    assert_eq!(
        restored[2],
        GroupMessage {
            speaker: GroupSpeaker::Member {
                id: "self-id".into(),
                name: "Self".into(),
            },
            content: "mine".into(),
        }
    );
}

#[test]
fn remote_member_prompts_reuse_group_contract_and_guest_name_rules() {
    let current = member("a", "Alice");
    let peers = vec![member("b", "Bob")];
    let group = GroupDescription {
        name: "Shared".into(),
        description: "Cross-user room".into(),
    };
    let prompts = build_remote_member_turn_prompts(
        &current,
        &group,
        &peers,
        "Remote Owner",
        &[XuserTurnMessage {
            speaker_kind: XuserSpeakerKind::Human,
            speaker_name: "Guest".into(),
            text: "@Alice hi".into(),
            is_self: false,
        }],
    );
    assert!(prompts.system_prompt.contains("You are Alice"));
    assert!(prompts.system_prompt.contains("hosted by Remote Owner, a DIFFERENT person"));
    assert!(prompts.prompt.starts_with("[Group chat: \"Shared\" - with Bob]"));
    assert!(prompts.prompt.contains("Guest (user): @Alice hi"));
    assert_eq!(clamp_guest_name(" \n "), "Someone");
    assert_eq!(clamp_guest_name(" Ada\r\nLovelace "), "Ada Lovelace");
}
