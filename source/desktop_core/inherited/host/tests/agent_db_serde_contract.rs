use mahayana_host_runtime::extensions::session::agent_db_serde::{
    AwaitingUserResponse, EpisodeTurn, MemoryPromptSnapshot, RequestRecord, SandProfile,
    SpendGuardState, UnreadState, is_valid_transcript_entry, parse_awaiting_state,
    parse_memory_prompt_snapshot, parse_pending_episode_turns, parse_profile,
    parse_request_records, parse_transcript_entry, parse_unread_state,
    resolve_spend_guard_state, serialize_spend_guard_state,
};
use serde_json::json;

#[test]
fn transcript_entry_validation_matches_frozen_grok_contract() {
    let valid_text = json!({
        "id": "entry-1",
        "kind": "send-message",
        "message": {"type": "text", "content": "hello"}
    });
    assert!(is_valid_transcript_entry(&valid_text));
    assert_eq!(
        parse_transcript_entry(&valid_text.to_string()),
        Some(valid_text)
    );

    assert!(parse_transcript_entry(
        r#"{"id":"entry-2","kind":"send-message","message":{"type":"text"}}"#
    )
    .is_none());
    assert!(parse_transcript_entry(
        r#"{"id":"entry-3","kind":"error","message":{"type":"binary"}}"#
    )
    .is_none());
    assert!(parse_transcript_entry(
        r#"{"id":"entry-4","kind":"event","event":{"type":"handoff"}}"#
    )
    .is_some());
    assert!(parse_transcript_entry(
        r#"{"id":"entry-5","kind":"user-attachment","file_path":"/tmp/a"}"#
    )
    .is_some());
}

#[test]
fn profile_and_unread_state_are_sanitized_like_grok() {
    assert_eq!(
        parse_profile(Some(
            r#"{"description":"worker","avatarPath":"/tmp/avatar.png"}"#
        )),
        SandProfile {
            description: "worker".into(),
            avatar_path: Some("/tmp/avatar.png".into()),
        }
    );
    assert_eq!(
        parse_profile(Some(r#"{"description":7,"avatarPath":""}"#)),
        SandProfile::default()
    );

    assert_eq!(
        parse_unread_state(Some(
            r#"{"lastActivityAt":11,"lastViewedAt":"bad","isManuallyUnread":true,"unreadCount":3.9}"#
        )),
        UnreadState {
            last_activity_at: 11.0,
            last_viewed_at: 0.0,
            is_manually_unread: true,
            unread_count: 3.0,
        }
    );
    assert_eq!(parse_unread_state(Some("not-json")), UnreadState::default());
}

#[test]
fn spend_guard_preserves_legacy_fallback_and_sparse_serialization() {
    assert_eq!(
        resolve_spend_guard_state(None, Some("42")),
        SpendGuardState {
            nudged_at_ms: Some(42.0),
            ..SpendGuardState::default()
        }
    );
    let state = resolve_spend_guard_state(
        Some(
            r#"{"nudgedAtMs":12,"snoozedUntilMs":-4,"optedOut":true,"cardEntryIds":["card","",4],"pausedAutomationIds":["auto"]}"#,
        ),
        Some("99"),
    );
    assert_eq!(
        state,
        SpendGuardState {
            nudged_at_ms: Some(12.0),
            snoozed_until_ms: None,
            opted_out: true,
            card_entry_ids: vec!["card".into()],
            paused_automation_ids: vec!["auto".into()],
        }
    );
    assert!(serialize_spend_guard_state(&SpendGuardState::default()).is_none());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &serialize_spend_guard_state(&state).expect("serialized state")
        )
        .expect("valid json"),
        json!({
            "nudgedAtMs": 12.0,
            "snoozedUntilMs": null,
            "optedOut": true,
            "cardEntryIds": ["card"],
            "pausedAutomationIds": ["auto"]
        })
    );
}

#[test]
fn awaiting_requests_memory_and_episode_parsers_filter_invalid_shapes() {
    assert_eq!(
        parse_awaiting_state(Some(r#"{"tabId":"tab-1","reason":7,"since":"bad"}"#)),
        Some(AwaitingUserResponse {
            tab_id: "tab-1".into(),
            reason: String::new(),
            since: 0.0,
        })
    );
    assert!(parse_awaiting_state(Some(r#"{"tabId":""}"#)).is_none());

    assert_eq!(
        parse_request_records(Some(
            r#"[{"id":" request-1 ","at":9,"prompt":"hello","source":"turn"},{"id":"request-2","at":-1,"source":"unknown"},{"id":"   ","at":3},7]"#,
        )),
        vec![
            RequestRecord {
                id: "request-1".into(),
                at: 9.0,
                prompt: Some("hello".into()),
                source: Some("turn".into()),
            },
            RequestRecord {
                id: "request-2".into(),
                at: 0.0,
                prompt: None,
                source: None,
            },
        ]
    );

    assert_eq!(
        parse_memory_prompt_snapshot(Some(
            r#"{"render":"memory","compactionEpoch":3}"#
        )),
        Some(MemoryPromptSnapshot {
            render: "memory".into(),
            compaction_epoch: 3.0,
        })
    );
    assert!(parse_memory_prompt_snapshot(Some(
        r#"{"render":"memory","compactionEpoch":"3"}"#
    ))
    .is_none());

    assert_eq!(
        parse_pending_episode_turns(Some(
            r#"[{"ts":5,"user":"u","agent":"a"},{"ts":"bad","user":"","agent":"reply"},{"ts":9,"user":"","agent":""}]"#,
        )),
        vec![
            EpisodeTurn {
                ts: 5.0,
                user: "u".into(),
                agent: "a".into(),
            },
            EpisodeTurn {
                ts: 0.0,
                user: String::new(),
                agent: "reply".into(),
            },
        ]
    );
}
