use mahayana_host_runtime::groups::group_chat::{
    GroupDescription, GroupMember, GroupMessage, GroupSpeaker, assert_members_are_not_groups,
    build_group_member_system_prompt, build_group_turn_prompt, format_group_history,
    is_group_turn_prompt_text, is_pass_content, is_potential_pass_prefix, is_same_member_set,
    member_mention_handles, messages_since_member_last_spoke, order_round_speakers,
    parse_group_mentions, resolve_responders,
};

fn member(id: &str, name: &str) -> GroupMember {
    GroupMember {
        id: id.into(),
        name: name.into(),
        description: String::new(),
    }
}

#[test]
fn frozen_group_helper_contracts_cover_mentions_ordering_and_pass_detection() {
    assert_eq!(order_round_speakers(&["a", "b", "c"], 1), vec!["b", "c", "a"]);
    assert_eq!(order_round_speakers(&["a", "b", "c"], -1), vec!["c", "a", "b"]);
    assert!(is_same_member_set(&["a".into(), "b".into()], &["b".into(), "a".into()]));
    assert!(!is_same_member_set(&["a".into()], &["a".into(), "a".into()]));
    assert_eq!(
        member_mention_handles("Alice Smith"),
        vec!["alice smith", "alicesmith", "alice"]
    );

    let members = vec![member("a", "Alice Smith"), member("b", "Bob")];
    assert_eq!(
        parse_group_mentions("hi @alice and @bob!", &members).member_ids,
        vec!["a", "b"]
    );
    assert!(parse_group_mentions("@everyone hello", &members).is_everyone);
    assert!(!parse_group_mentions("mail@aliceexample", &members).member_ids.contains(&"a".into()));

    assert!(is_pass_content("(pass)."));
    assert!(is_pass_content(" PASS "));
    assert!(is_potential_pass_prefix("(pa"));
    assert!(!is_potential_pass_prefix("party"));
    assert!(assert_members_are_not_groups(&["a".into()], |_| false).is_ok());
    let nested = assert_members_are_not_groups(&["g".into(), "g".into()], |id| id == "g")
        .expect_err("nested group");
    assert_eq!(nested.nested_group_ids, vec!["g"]);
}

#[test]
fn responder_and_prompt_projection_match_frozen_history_semantics() {
    let members = vec![member("a", "Alice"), member("b", "Bob")];
    let history = vec![
        GroupMessage {
            speaker: GroupSpeaker::Member { id: "a".into(), name: "Alice".into() },
            content: "old".into(),
        },
        GroupMessage {
            speaker: GroupSpeaker::User { name: None },
            content: "@Bob please".into(),
        },
        GroupMessage {
            speaker: GroupSpeaker::Member { id: "a".into(), name: "Alice".into() },
            content: "after user".into(),
        },
    ];
    assert_eq!(
        resolve_responders(&members, &history)
            .into_iter()
            .map(|member| member.id)
            .collect::<Vec<_>>(),
        vec!["b"]
    );
    assert_eq!(
        messages_since_member_last_spoke(&history, "a"),
        &history[3..]
    );
    let group = GroupDescription { name: "Team".into(), description: "Build".into() };
    let system = build_group_member_system_prompt(&members[0], &group, &members[1..], false);
    assert!(system.contains("You are Alice"));
    assert!(system.contains("Other participants in the room"));
    let turn = build_group_turn_prompt(&members[0], &group, &members[1..], &history);
    assert!(turn.starts_with("[Group chat: \"Team\" - with Bob]"));
    assert!(turn.contains("It's your turn, Alice"));
    assert!(is_group_turn_prompt_text(&turn));
    assert_eq!(
        format_group_history(&[], "a", 24),
        "(no messages yet)"
    );
}
