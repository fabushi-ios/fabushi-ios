use std::time::Duration;

use mahayana_host_runtime::{
    AttemptCheckpoint, AttemptProgress, RetryDecision, StreamAttemptPolicy,
    TerminalOutcome, TransientStreamError,
};
use mahayana_host_runtime::runner::{StreamFailureKind, StreamWatchdog, TurnSettlement};

#[test]
fn first_output_watchdog_expires_only_before_output() {
    let policy = StreamAttemptPolicy {
        first_output_timeout: Duration::from_millis(100),
        ..StreamAttemptPolicy::default()
    };
    let mut watchdog = StreamWatchdog::new(&policy);
    assert!(!watchdog.expired_at(Duration::from_millis(99)));
    assert!(watchdog.expired_at(Duration::from_millis(100)));
    watchdog.mark_output();
    assert!(!watchdog.expired_at(Duration::from_secs(10)));
}

#[test]
fn transient_failure_retries_before_any_visible_output() {
    let policy = StreamAttemptPolicy::default();
    let error = TransientStreamError::classify("connection reset", None, None);
    assert_eq!(error.kind, StreamFailureKind::Transport);
    assert!(matches!(
        policy.retry_decision(1, &AttemptProgress::default(), &error),
        RetryDecision::RetryAfter(_)
    ));
}

#[test]
fn partial_output_is_not_retried_without_checkpoint() {
    let policy = StreamAttemptPolicy::default();
    let error = TransientStreamError::classify("upstream timeout", None, None);
    let mut progress = AttemptProgress::default();
    progress.record_output(12);
    assert_eq!(policy.retry_decision(1, &progress, &error), RetryDecision::Fail);
}

#[test]
fn partial_output_can_resume_from_checkpoint() {
    let policy = StreamAttemptPolicy::default();
    let error = TransientStreamError::classify("upstream timeout", None, None);
    let mut progress = AttemptProgress::default();
    progress.record_output(12);
    progress.checkpoint = Some(AttemptCheckpoint::new("cursor-7", 12, 0));
    assert!(matches!(
        policy.retry_decision(1, &progress, &error),
        RetryDecision::ResumeAfter { .. }
    ));
}

#[test]
fn retry_after_is_bounded() {
    let policy = StreamAttemptPolicy::default();
    let error = TransientStreamError::classify("rate limit", Some(429), Some(120_000));
    match policy.retry_decision(1, &AttemptProgress::default(), &error) {
        RetryDecision::RetryAfter(delay) => assert_eq!(delay, Duration::from_secs(30)),
        other => panic!("unexpected decision: {other:?}"),
    }
}

#[test]
fn max_attempt_count_is_terminal() {
    let policy = StreamAttemptPolicy::default();
    let error = TransientStreamError::classify("503", Some(503), None);
    assert_eq!(
        policy.retry_decision(policy.max_attempts, &AttemptProgress::default(), &error),
        RetryDecision::Fail
    );
}

#[test]
fn authentication_and_protocol_errors_do_not_retry() {
    let policy = StreamAttemptPolicy::default();
    for error in [
        TransientStreamError::classify("unauthorized", Some(401), None),
        TransientStreamError::classify("malformed stream", None, None),
    ] {
        assert_eq!(
            policy.retry_decision(1, &AttemptProgress::default(), &error),
            RetryDecision::Fail
        );
    }
}

#[test]
fn terminal_settlement_is_exactly_once() {
    let mut settlement = TurnSettlement::default();
    settlement.settle(TerminalOutcome::Completed).unwrap();
    assert!(settlement.settle(TerminalOutcome::Cancelled).is_err());
    assert_eq!(settlement.outcome(), Some(&TerminalOutcome::Completed));
}


#[test]
fn turn_run_shell_enforces_owner_generation_and_pre_dispatch_supersede_rules() {
    use mahayana_host_runtime::{
        TerminalOutcome, TurnRunOptions, TurnRunShell, TurnRunShellError,
    };

    let mut shell = TurnRunShell::default();
    assert_eq!(
        shell.begin_run("   ", TurnRunOptions::default()),
        Err(TurnRunShellError::EmptyPrompt)
    );

    let first = shell
        .begin_run(
            "recover me",
            TurnRunOptions {
                inference_request_id: Some("request-1".into()),
                message_id: Some("t1u".into()),
                recent_message_text: Some("fallback should not matter".into()),
                recent_user_messages: vec![
                    mahayana_host_runtime::RecentUserMessage {
                        id: "t0u".into(),
                        text: "older".into(),
                    },
                    mahayana_host_runtime::RecentUserMessage {
                        id: "t1u".into(),
                        text: "recover me".into(),
                    },
                ],
                ..TurnRunOptions::default()
            },
        )
        .expect("first run");
    assert!(first.recovery_shaped);
    assert_eq!(shell.active_request_id(), Some("request-1"));

    assert!(
        !shell.interrupt("superseded without recovery", Some(false)),
        "a pre-dispatch run must not be interrupted by an unsafe supersede"
    );
    assert!(
        shell.interrupt("superseded with recovery", Some(true)),
        "a recovery-shaped run can be safely superseded before dispatch"
    );
    let cancelled = shell.finish_cancelled(&first.owner).expect("cancelled");
    assert_eq!(cancelled.outcome, TerminalOutcome::Cancelled);

    let fork = shell
        .begin_run(
            "forked",
            TurnRunOptions {
                inference_request_id: Some("request-fork".into()),
                message_id: Some("fork-u".into()),
                recent_user_messages: vec![mahayana_host_runtime::RecentUserMessage {
                    id: "fork-u".into(),
                    text: "forked".into(),
                }],
                is_fork: true,
                ..TurnRunOptions::default()
            },
        )
        .expect("fork run");
    assert!(!fork.recovery_shaped);
    let fork_done = shell.finish_completed(&fork.owner).expect("finish fork");

    let second = shell
        .begin_run(
            "new turn",
            TurnRunOptions {
                inference_request_id: Some("request-2".into()),
                ..TurnRunOptions::default()
            },
        )
        .expect("second run");
    assert!(fork_done.owner.generation > first.owner.generation);
    assert!(second.owner.generation > fork_done.owner.generation);
    assert_eq!(
        shell.mark_dispatched(&first.owner),
        Err(TurnRunShellError::StaleOwner)
    );
    assert_eq!(
        shell.finish_completed(&first.owner),
        Err(TurnRunShellError::StaleOwner)
    );
    shell.mark_dispatched(&second.owner).unwrap();
    let completed = shell.finish_completed(&second.owner).unwrap();
    assert_eq!(completed.outcome, TerminalOutcome::Completed);
    assert!(!shell.has_active_run());
}

#[test]
fn turn_run_shell_converts_checkpoint_boundary_to_waiting_user_or_quiesce() {
    use mahayana_host_runtime::{
        CheckpointBoundary, TerminalOutcome, TurnRunOptions, TurnRunShell,
    };

    let mut shell = TurnRunShell::default();
    let run = shell
        .begin_run(
            "choose",
            TurnRunOptions {
                inference_request_id: Some("request-user".into()),
                ..TurnRunOptions::default()
            },
        )
        .unwrap();
    shell.mark_dispatched(&run.owner).unwrap();
    shell
        .end_turn_awaiting_user(&run.owner, "permission selection")
        .unwrap();
    assert!(matches!(
        shell.checkpoint_boundary(&run.owner).unwrap(),
        CheckpointBoundary::Cancel(ref cancellation)
            if cancellation.intentional && cancellation.reason == "permission selection"
    ));
    let waiting = shell.finish_cancelled(&run.owner).unwrap();
    assert_eq!(waiting.outcome, TerminalOutcome::WaitingUser);

    let upgrade = shell
        .begin_run(
            "upgrade",
            TurnRunOptions {
                inference_request_id: Some("request-upgrade".into()),
                ..TurnRunOptions::default()
            },
        )
        .unwrap();
    shell.request_quiesce_for_upgrade();
    assert!(shell.is_quiescing_for_upgrade());
    assert!(matches!(
        shell.checkpoint_boundary(&upgrade.owner).unwrap(),
        CheckpointBoundary::Cancel(ref cancellation)
            if cancellation.reason.contains("forced host upgrade")
    ));
    let ended = shell.finish_cancelled(&upgrade.owner).unwrap();
    assert!(ended.quiesced_for_upgrade);
    shell.cancel_quiesce_for_upgrade();
    assert!(!shell.is_quiescing_for_upgrade());
}


#[test]
fn tool_call_identity_resolves_late_names_and_releases_completed_entries() {
    use mahayana_host_runtime::{ToolCallIdentity, ToolSurfaceUpdate, TOOL_CALL_IDENTITY_CAP};
    use serde_json::{Map, Value};

    let mut identity = ToolCallIdentity::default();
    let mut fields = Map::new();
    fields.insert("status".into(), Value::String("pending".into()));
    identity.stash_surface_unresolved_pending(
        "call-1",
        ToolSurfaceUpdate { fields },
    );
    let emitted = identity
        .record_model_tool_name("call-1", "mcp.search")
        .expect("held update");
    assert_eq!(emitted.fields["name"], "mcp.search");
    assert_eq!(
        identity.resolve_model_tool_name("toolCallStarted", "call-1", "Tool"),
        "mcp.search"
    );
    assert_eq!(
        identity.resolve_model_tool_name("toolCallCompleted", "call-1", "Tool"),
        "mcp.search"
    );
    assert_eq!(identity.resolve_model_tool_name("toolCallStarted", "call-1", "Tool"), "Tool");

    for index in 0..=TOOL_CALL_IDENTITY_CAP {
        identity.record_model_tool_name(format!("id-{index}"), format!("tool-{index}"));
    }
    assert_eq!(identity.name_count(), TOOL_CALL_IDENTITY_CAP);
    assert_eq!(identity.resolve_model_tool_name("toolCallStarted", "id-0", "fallback"), "fallback");
}

#[test]
fn turn_usage_clamps_bigint_equivalent_and_merges_without_overflow() {
    use mahayana_host_runtime::{
        MAX_SAFE_TOKEN_COUNT, TurnEndedUsage, TurnUsage, merge_turn_usage,
        to_safe_token_count, turn_usage_from_turn_ended,
    };

    assert_eq!(to_safe_token_count(-1), 0);
    assert_eq!(
        to_safe_token_count(i128::MAX),
        MAX_SAFE_TOKEN_COUNT
    );
    assert!(turn_usage_from_turn_ended(&TurnEndedUsage::default()).is_none());
    let usage = turn_usage_from_turn_ended(&TurnEndedUsage {
        input_tokens: Some(10),
        output_tokens: Some(5),
        cache_read_tokens: Some(-1),
        cache_write_tokens: Some(2),
        reasoning_tokens: Some(i128::MAX),
    })
    .unwrap();
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.cache_read_tokens, 0);
    assert_eq!(usage.reasoning_tokens, Some(MAX_SAFE_TOKEN_COUNT));

    let merged = merge_turn_usage(
        Some(TurnUsage {
            input_tokens: MAX_SAFE_TOKEN_COUNT,
            output_tokens: 1,
            cache_read_tokens: 2,
            cache_write_tokens: 3,
            reasoning_tokens: None,
        }),
        Some(TurnUsage {
            input_tokens: 99,
            output_tokens: 4,
            cache_read_tokens: 5,
            cache_write_tokens: 6,
            reasoning_tokens: Some(7),
        }),
    )
    .unwrap();
    assert_eq!(merged.input_tokens, MAX_SAFE_TOKEN_COUNT);
    assert_eq!(merged.output_tokens, 5);
    assert_eq!(merged.reasoning_tokens, Some(7));
}

#[test]
fn transient_stream_error_matches_frozen_transport_tokens() {
    use mahayana_host_runtime::{StreamFailureKind, TransientStreamError};

    for message in [
        "socket hang up",
        "premature close",
        "stream closed",
        "ECONNRESET",
        "EPIPE",
        "ENETUNREACH",
        "EAI_AGAIN",
        "the operation was aborted",
        "[UNAVAILABLE]",
    ] {
        let error = TransientStreamError::classify(message, None, None);
        assert!(
            error.retryable(),
            "frozen transient token should be retryable: {message}"
        );
        assert_eq!(error.kind, StreamFailureKind::Transport);
    }

    for message in ["ETIMEDOUT", "[DEADLINE_EXCEEDED]"] {
        let error = TransientStreamError::classify(message, None, None);
        assert!(error.retryable());
        assert_eq!(error.kind, StreamFailureKind::Timeout);
    }
}

#[test]
fn conversation_state_recovers_unconfirmed_user_messages_and_rejects_stale_model_resolution() {
    use mahayana_host_runtime::{
        HIDDEN_PROMPT_MARKER, RecentUserMessage, ResolvedModelTracker,
        build_unanswered_questions_note, sanitize_usage,
        select_unconfirmed_user_messages, should_use_self_summary,
    };

    let messages = vec![
        RecentUserMessage { id: "u1".into(), text: "first".into() },
        RecentUserMessage { id: "u2".into(), text: "   ".into() },
        RecentUserMessage { id: "u3".into(), text: "third".into() },
        RecentUserMessage { id: "u4".into(), text: "current".into() },
    ];
    let recovered = select_unconfirmed_user_messages(
        &messages,
        Some("u4"),
        Some("u1"),
        true,
    );
    assert_eq!(
        recovered.iter().map(|message| message.id.as_str()).collect::<Vec<_>>(),
        vec!["u3"]
    );
    assert!(select_unconfirmed_user_messages(&messages, Some("u4"), Some("missing"), true).is_empty());
    let first_turn = select_unconfirmed_user_messages(&messages, Some("u4"), None, false);
    assert_eq!(
        first_turn.iter().map(|message| message.id.as_str()).collect::<Vec<_>>(),
        vec!["u1", "u3"]
    );

    let note = build_unanswered_questions_note(
        &["old question".into()],
        &["private question".into()],
    );
    assert!(note.starts_with(HIDDEN_PROMPT_MARKER));
    assert!(note.contains("moved on"));
    assert!(note.contains("dismissed"));

    let usage = sanitize_usage(f64::NAN, 2.9, 0.0);
    assert_eq!(usage.prompt_tokens, 0);
    assert_eq!(usage.completion_tokens, 2);
    assert_eq!(usage.total_tokens, 2);

    let mut tracker = ResolvedModelTracker::default();
    let older = tracker.begin_request("requested");
    let newer = tracker.begin_request("requested");
    assert!(tracker.accept_resolution(&newer, "resolved-new"));
    assert!(!tracker.accept_resolution(&older, "resolved-old"));
    assert_eq!(tracker.resolved("requested"), Some("resolved-new"));

    assert!(should_use_self_summary("grok-4.5#account"));
    assert!(should_use_self_summary("cursor/vega-x"));
    assert!(!should_use_self_summary("gpt-4.1"));
}


#[test]
fn conversation_state_covers_await_stream_usage_and_summarization_contracts() {
    use mahayana_host_runtime::{
        CompletedAwaitOutcome, ContextWindowTracker, FullStreamSanitizer,
        StreamSanitizerItem, SUMMARIZATION_MAX_OUTPUT_TOKENS,
        SUMMARIZATION_MAX_PROMPT_CHARS, await_block_until_ms,
        classify_completed_await_outcome, summarization_policy,
    };
    use serde_json::json;

    assert_eq!(await_block_until_ms(Some(&json!(42.9))), 42.9);
    assert_eq!(await_block_until_ms(Some(&json!("17"))), 17.0);
    assert_eq!(await_block_until_ms(None), 0.0);
    assert!(await_block_until_ms(Some(&json!({ "bad": true }))).is_nan());

    let slept = json!({
        "result": {
            "result": {
                "case": "success",
                "value": {
                    "awaitResult": {
                        "case": "complete",
                        "value": { "taskId": "   " }
                    }
                }
            }
        }
    });
    assert_eq!(
        classify_completed_await_outcome(&slept),
        CompletedAwaitOutcome::SleptFull
    );

    let completed = json!({
        "result": {
            "result": {
                "case": "success",
                "value": {
                    "awaitResult": {
                        "case": "stillRunning",
                        "value": { "regexMatch": ["ready"] }
                    }
                }
            }
        }
    });
    assert_eq!(
        classify_completed_await_outcome(&completed),
        CompletedAwaitOutcome::CompletedEarly
    );

    let mut windows = ContextWindowTracker::default();
    let usage = windows.sanitize_extended_usage(
        &json!({
            "inputTokens": 10.8,
            "outputTokens": 3,
            "cacheReadTokens": -2,
            "cacheWriteTokens": 4,
            "maxTokens": 128000
        }),
        "model-a",
    );
    assert_eq!(usage.input_tokens, 10);
    assert_eq!(usage.output_tokens, 3);
    assert_eq!(usage.cache_read_tokens, 0);
    assert_eq!(usage.cache_write_tokens, 4);
    assert_eq!(usage.max_tokens, 128000);
    assert_eq!(windows.last_reported("model-a"), Some(128000));

    let mut stream = FullStreamSanitizer::default();
    assert_eq!(stream.accept_value("before"), StreamSanitizerItem::Emit("before"));
    assert_eq!(
        stream.accept_error("provider stream failed"),
        StreamSanitizerItem::DeferredError("provider stream failed".into())
    );
    assert_eq!(stream.accept_value("after"), StreamSanitizerItem::Suppressed);
    assert_eq!(stream.accept_error("second error"), StreamSanitizerItem::Suppressed);
    assert_eq!(stream.finish(), Err("provider stream failed".into()));

    let policy = summarization_policy(true);
    assert!(policy.enable_reduce_inputs_retry);
    assert_eq!(policy.max_prompt_chars, SUMMARIZATION_MAX_PROMPT_CHARS);
    assert_eq!(policy.max_output_tokens, SUMMARIZATION_MAX_OUTPUT_TOKENS);
    assert!(policy.preserve_latest_image);
}
