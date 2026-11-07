use mahayana_host_runtime::sand_activity::{
    ActivityTransition, ActivityUpdate, AgentActivity, GroupMemberActivityTracker,
    NAMED_ACTIVITY_MAX_HOLD_MS, NamedActivityHoldState, derive_activity_from_update,
    derive_tool_call_activity, extract_shell_edit_target, resolve_named_activity_hold,
};

#[test]
fn tool_activity_projection_matches_frozen_named_tools_and_details() {
    assert_eq!(
        derive_tool_call_activity(
            "call-1",
            "webFetchToolCall",
            Some(r#"{"url":"https://docs.example.com/a/b"}"#),
            None,
        ),
        AgentActivity::Tool {
            tool: "WebFetch".into(),
            detail: Some("docs.example.com".into()),
            target: None,
            call_id: "call-1".into(),
        }
    );
    assert_eq!(
        extract_shell_edit_target("printf hi > 'src/output.txt'"),
        Some("output.txt".into())
    );
    assert_eq!(
        extract_shell_edit_target("cat input | tee -a logs/build.log"),
        Some("build.log".into())
    );
    assert_eq!(
        derive_activity_from_update(&ActivityUpdate::ToolCall {
            id: "send".into(),
            name: "SendMessage".into(),
            status: "pending".into(),
            args: None,
            summary: None,
        }),
        ActivityTransition::Keep
    );
}

#[test]
fn named_activity_hold_keeps_tool_visible_across_short_thinking_delta() {
    let pending = ActivityUpdate::ToolCall {
        id: "search-1".into(),
        name: "webSearchToolCall".into(),
        status: "pending".into(),
        args: Some(r#"{"searchTerm":"rust sqlite"}"#.into()),
        summary: None,
    };
    let (transition, state) = resolve_named_activity_hold(
        &pending,
        &NamedActivityHoldState::default(),
        1_000,
        NAMED_ACTIVITY_MAX_HOLD_MS,
    );
    assert!(matches!(transition, ActivityTransition::Set(AgentActivity::Tool { .. })));

    let (transition, state) = resolve_named_activity_hold(
        &ActivityUpdate::ThinkingDelta,
        &state,
        1_100,
        NAMED_ACTIVITY_MAX_HOLD_MS,
    );
    assert_eq!(transition, ActivityTransition::Keep);

    let (transition, _) = resolve_named_activity_hold(
        &ActivityUpdate::ThinkingDelta,
        &state,
        1_000 + NAMED_ACTIVITY_MAX_HOLD_MS,
        NAMED_ACTIVITY_MAX_HOLD_MS,
    );
    assert_eq!(transition, ActivityTransition::Set(AgentActivity::Thinking));

    let (transition, reset) = resolve_named_activity_hold(
        &ActivityUpdate::TurnEnded,
        &state,
        4_000,
        NAMED_ACTIVITY_MAX_HOLD_MS,
    );
    assert_eq!(transition, ActivityTransition::Clear);
    assert_eq!(reset, NamedActivityHoldState::default());
}

#[test]
fn group_member_tracker_hides_thinking_after_streaming_starts() {
    let mut tracker = GroupMemberActivityTracker::default();
    assert_eq!(
        tracker.update(&ActivityUpdate::ThinkingDelta),
        ActivityTransition::Set(AgentActivity::Thinking)
    );
    assert_eq!(
        tracker.update(&ActivityUpdate::TextDelta { text: "x".into() }),
        ActivityTransition::Clear
    );
    assert_eq!(
        tracker.update(&ActivityUpdate::ThinkingDelta),
        ActivityTransition::Keep
    );
}
