use mahayana_host_runtime::runner::background_work::{
    SHELL_REWATCH_MAX_WAIT_MS, SHELL_REWATCH_MISSING_FILE_GIVE_UP,
};
use mahayana_host_runtime::runner::sand_prompt_markers::SAND_HIDDEN_PROMPT_MARKER;
use mahayana_host_runtime::runner::shell_terminal_watch::{
    ConfirmedUserTurnWatermark, MaterializedTurn, MaterializedTurnKind,
    MaterializedUserMessage, RecentTerminalUserMessage, ShellTerminalPollRead,
    ShellTerminalPollState, ShellWatchStatus, TerminalReadResult, WatermarkResult,
    collect_prepend_user_messages, find_confirmed_user_turn_watermark,
    is_group_turn_prompt_text, poll_shell_terminal_file, read_shell_terminal_snapshot,
    turn_refs_equal,
};

#[test]
fn terminal_snapshot_decodes_text_bytes_not_found_and_failures() {
    let text = read_shell_terminal_snapshot(TerminalReadResult::SuccessText("hello".into()))
        .expect("text");
    assert!(text.exists);
    assert_eq!(text.content, "hello");

    let bytes = read_shell_terminal_snapshot(TerminalReadResult::SuccessData(
        b"hello\xFF".to_vec(),
    ))
    .expect("bytes");
    assert!(bytes.exists);
    assert!(bytes.content.starts_with("hello"));

    let missing =
        read_shell_terminal_snapshot(TerminalReadResult::FileNotFound).expect("missing");
    assert!(!missing.exists);
    assert_eq!(missing.content, "");

    assert_eq!(
        read_shell_terminal_snapshot(TerminalReadResult::Failure("permission".into()))
            .unwrap_err()
            .to_string(),
        "terminal file read failed (permission)"
    );
}

#[test]
fn polling_settles_success_stream_error_missing_permission_and_timeout() {
    let mut state = ShellTerminalPollState::new(1_000);
    let success = state
        .observe_snapshot(
            2_000,
            "/term/a.txt",
            &mahayana_host_runtime::runner::shell_terminal_watch::TerminalFileSnapshot {
                exists: true,
                content: "output\n---\nexit_code: 0\n---".into(),
            },
        )
        .expect("success settlement");
    assert_eq!(success.status, ShellWatchStatus::Success);
    assert_eq!(success.detail, None);

    let mut stream = ShellTerminalPollState::new(0);
    let error = stream
        .observe_snapshot(
            1,
            "/term/b.txt",
            &mahayana_host_runtime::runner::shell_terminal_watch::TerminalFileSnapshot {
                exists: true,
                content: "output\n---\nerror: stream broke\nended_at: 1\n---".into(),
            },
        )
        .expect("stream error");
    assert_eq!(error.status, ShellWatchStatus::Error);
    assert!(error.detail.unwrap().contains("output stream failed"));

    let mut missing = ShellTerminalPollState::new(0);
    for index in 0..SHELL_REWATCH_MISSING_FILE_GIVE_UP - 1 {
        assert!(
            missing
                .observe_snapshot(
                    index as u64,
                    "/term/missing.txt",
                    &mahayana_host_runtime::runner::shell_terminal_watch::TerminalFileSnapshot {
                        exists: false,
                        content: String::new(),
                    },
                )
                .is_none()
        );
    }
    let gone = missing
        .observe_snapshot(
            9,
            "/term/missing.txt",
            &mahayana_host_runtime::runner::shell_terminal_watch::TerminalFileSnapshot {
                exists: false,
                content: String::new(),
            },
        )
        .expect("give up");
    assert!(gone.detail.unwrap().contains("no longer exists"));

    let denied = missing.permission_denied();
    assert!(denied.detail.unwrap().contains("no longer allowed"));

    let timed = ShellTerminalPollState::new(100)
        .timeout(100 + SHELL_REWATCH_MAX_WAIT_MS)
        .expect("timeout");
    assert!(timed.detail.unwrap().contains("300 minutes"));
}

#[test]
fn watermark_skips_hidden_and_group_turns_and_reuses_valid_cache() {
    assert!(is_group_turn_prompt_text("[Group chat: \"team\"]"));
    assert!(is_group_turn_prompt_text(&format!(
        "{SAND_HIDDEN_PROMPT_MARKER}[Group chat: \"team\"]"
    )));
    assert!(turn_refs_equal(&[1, 2], &[1, 2]));
    assert!(!turn_refs_equal(&[1, 2], &[1, 3]));

    let turns = vec![
        MaterializedTurn {
            turn_ref: vec![1],
            kind: MaterializedTurnKind::Agent {
                user_message: Some(MaterializedUserMessage {
                    text: "confirmed".into(),
                    message_id: "m1".into(),
                    rich_text: None,
                }),
            },
        },
        MaterializedTurn {
            turn_ref: vec![2],
            kind: MaterializedTurnKind::Agent {
                user_message: Some(MaterializedUserMessage {
                    text: format!("{SAND_HIDDEN_PROMPT_MARKER}hidden"),
                    message_id: "hidden".into(),
                    rich_text: None,
                }),
            },
        },
        MaterializedTurn {
            turn_ref: vec![3],
            kind: MaterializedTurnKind::Agent {
                user_message: Some(MaterializedUserMessage {
                    text: "[Group chat: \"team\"] hello".into(),
                    message_id: String::new(),
                    rich_text: None,
                }),
            },
        },
    ];
    let first = find_confirmed_user_turn_watermark(&turns, None);
    assert_eq!(first.result.last_user_message_id.as_deref(), Some("m1"));
    assert!(first.result.has_user_turn);
    let cache = first.cache.expect("cache");
    assert_eq!(cache.turn_count, 3);
    assert_eq!(cache.boundary_ref, vec![3]);

    let reused = find_confirmed_user_turn_watermark(&turns, Some(&cache));
    assert_eq!(reused.result.last_user_message_id.as_deref(), Some("m1"));

    let unreadable = find_confirmed_user_turn_watermark(
        &[MaterializedTurn {
            turn_ref: vec![9],
            kind: MaterializedTurnKind::Unreadable,
        }],
        None,
    );
    assert!(unreadable.result.has_user_turn);
    assert!(unreadable.cache.is_none());
}

#[test]
fn prepend_projection_selects_only_queued_messages_and_preserves_rich_text() {
    let recent = vec![
        RecentTerminalUserMessage {
            id: "m1".into(),
            text: "confirmed".into(),
            rich_text: None,
        },
        RecentTerminalUserMessage {
            id: "m2".into(),
            text: "queued".into(),
            rich_text: Some("{\"doc\":1}".into()),
        },
        RecentTerminalUserMessage {
            id: "m3".into(),
            text: "current".into(),
            rich_text: None,
        },
    ];
    let selected = collect_prepend_user_messages(
        &recent,
        Some("m3"),
        &WatermarkResult {
            last_user_message_id: Some("m1".into()),
            has_user_turn: true,
        },
    );
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].message_id, "m2");
    assert_eq!(selected[0].text, "[m2]\nqueued");
    assert_eq!(selected[0].rich_text.as_deref(), Some("{\"doc\":1}"));

    let none = collect_prepend_user_messages(
        &recent,
        None,
        &WatermarkResult {
            last_user_message_id: Some("m1".into()),
            has_user_turn: true,
        },
    );
    assert!(none.is_empty());
}

#[test]
fn invalidated_watermark_boundary_forces_rescan() {
    let cache = ConfirmedUserTurnWatermark {
        turn_count: 1,
        boundary_ref: vec![1],
        last_user_message_id: Some("old".into()),
        has_user_turn: true,
    };
    let turns = vec![MaterializedTurn {
        turn_ref: vec![2],
        kind: MaterializedTurnKind::Agent {
            user_message: Some(MaterializedUserMessage {
                text: "new".into(),
                message_id: "new-id".into(),
                rich_text: None,
            }),
        },
    }];
    let resolved = find_confirmed_user_turn_watermark(&turns, Some(&cache));
    assert_eq!(resolved.result.last_user_message_id.as_deref(), Some("new-id"));
}


#[test]
fn production_poll_loop_covers_terminal_success_exit_stream_missing_permission_timeout_and_transient_retry() {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    let run = |reads: Vec<ShellTerminalPollRead>| {
        let queue = RefCell::new(VecDeque::from(reads));
        let now = Cell::new(0u64);
        poll_shell_terminal_file(
            0,
            1,
            || queue.borrow_mut().pop_front().expect("poll read"),
            || now.get(),
            |delay| now.set(now.get().saturating_add(delay)),
            || false,
        )
        .expect("settlement")
    };

    let success = run(vec![ShellTerminalPollRead::Snapshot {
        output_path: "/term/success.txt".into(),
        result: TerminalReadResult::SuccessText(
            "ok\n---\nexit_code: 0\n---".into(),
        ),
    }]);
    assert_eq!(success.status, ShellWatchStatus::Success);

    let nonzero = run(vec![ShellTerminalPollRead::Snapshot {
        output_path: "/term/nonzero.txt".into(),
        result: TerminalReadResult::SuccessText(
            "bad\n---\nexit_code: 7\n---".into(),
        ),
    }]);
    assert_eq!(nonzero.status, ShellWatchStatus::Error);
    assert_eq!(nonzero.detail.as_deref(), Some("exit_code=7"));

    let stream = run(vec![ShellTerminalPollRead::Snapshot {
        output_path: "/term/stream.txt".into(),
        result: TerminalReadResult::SuccessText(
            "partial\n---\nerror: pipe broke\nended_at: 1\n---".into(),
        ),
    }]);
    assert!(stream
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("output stream failed")));

    let missing = run(
        (0..SHELL_REWATCH_MISSING_FILE_GIVE_UP)
            .map(|_| ShellTerminalPollRead::Snapshot {
                output_path: "/term/missing.txt".into(),
                result: TerminalReadResult::FileNotFound,
            })
            .collect(),
    );
    assert!(missing
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("no longer exists")));

    let denied = run(vec![ShellTerminalPollRead::PermissionDenied {
        output_path: Some("/term/denied.txt".into()),
    }]);
    assert!(denied
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("no longer allowed")));
    assert_eq!(denied.output_path.as_deref(), Some("/term/denied.txt"));

    let transient_then_success = run(vec![
        ShellTerminalPollRead::TransientFailure("box reconnecting".into()),
        ShellTerminalPollRead::Snapshot {
            output_path: "/term/recovered.txt".into(),
            result: TerminalReadResult::SuccessText(
                "ok\n---\nexit_code: 0\n---".into(),
            ),
        },
    ]);
    assert_eq!(transient_then_success.status, ShellWatchStatus::Success);

    let now = Cell::new(0u64);
    let timeout = poll_shell_terminal_file(
        0,
        SHELL_REWATCH_MAX_WAIT_MS,
        || ShellTerminalPollRead::TransientFailure("still running".into()),
        || now.get(),
        |delay| now.set(now.get().saturating_add(delay)),
        || false,
    )
    .expect("timeout");
    assert!(timeout
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("300 minutes")));

    assert!(poll_shell_terminal_file(
        0,
        1,
        || ShellTerminalPollRead::TransientFailure("unused".into()),
        || 0,
        |_| {},
        || true,
    )
    .is_none());
}
