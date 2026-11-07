use mahayana_host_runtime::extensions::transcript::send_pipeline::{
    PersistedSendContext, RecoverySend,
};
use mahayana_host_runtime::extensions::transcript::turn_runtime::{
    CLOSING_SEND_NUDGE_PROMPT, MAX_REPLY_NUDGES, QueuedTurnRecoveryCheck, REPLY_NUDGE_PROMPT,
    TurnTerminalKind, build_turn_empty_delivery_report, is_delivery_owed, project_turn_terminal,
    shape_closing_send_nudge_turn_input, shape_reply_nudge_turn_input,
    should_attempt_reply_nudge, should_supersede_stale_turn,
};
use mahayana_host_runtime::extensions::inference::provider_session::ProviderMessage;
use mahayana_host_runtime::runner::{
    RecoveryUserMessage, would_recover_via_prepend,
};

fn message(id: &str, text: &str, confirmed: Option<bool>) -> RecoveryUserMessage {
    RecoveryUserMessage {
        id: id.into(),
        text: text.into(),
        confirmed,
    }
}

#[test]
fn frozen_runner_prepend_eligibility_uses_order_and_confirmed_flag() {
    let recent = vec![
        message("m1", "one", Some(true)),
        message("m2", "two", None),
        message("m3", "three", None),
    ];
    assert!(would_recover_via_prepend(&recent, "m3", "m2"));
    assert!(would_recover_via_prepend(&recent, "m2", "m2"));
    assert!(!would_recover_via_prepend(&recent, "m2", "m3"));
    assert!(!would_recover_via_prepend(&recent, "m3", "m1"));
    assert!(!would_recover_via_prepend(&recent, "missing", "m2"));
}

#[test]
fn turn_runtime_applies_all_frozen_stale_recovery_guards() {
    let old = PersistedSendContext {
        echo_entry_id: Some("m2".into()),
        user_message_id: Some("m2".into()),
        recent_user_messages: vec![
            message("m1", "one", None),
            message("m2", "two", None),
        ],
        ..PersistedSendContext::default()
    };
    let latest = RecoverySend {
        epoch: 3,
        message_id: "m3".into(),
        recent_user_messages: vec![
            message("m1", "one", None),
            message("m2", "two", None),
            message("m3", "three", None),
        ],
    };
    let check = |break_epoch, is_fork, has_reply, attachments| {
        should_supersede_stale_turn(QueuedTurnRecoveryCheck {
            epoch: 2,
            current_epoch: 3,
            recovery_break_epoch: break_epoch,
            prompt: "two",
            context: &old,
            latest_recovery: Some(&latest),
            is_fork,
            has_reply_context: has_reply,
            attachment_count: attachments,
            image_count: 0,
            video_count: 0,
        })
    };
    assert!(check(0, false, false, 0));
    assert!(!check(2, false, false, 0));
    assert!(!check(0, true, false, 0));
    assert!(!check(0, false, true, 0));
    assert!(!check(0, false, false, 1));

    let confirmed_latest = RecoverySend {
        epoch: 3,
        message_id: "m3".into(),
        recent_user_messages: vec![
            message("m2", "two", Some(true)),
            message("m3", "three", None),
        ],
    };
    assert!(!should_supersede_stale_turn(QueuedTurnRecoveryCheck {
        epoch: 2,
        current_epoch: 3,
        recovery_break_epoch: 0,
        prompt: "two",
        context: &old,
        latest_recovery: Some(&confirmed_latest),
        is_fork: false,
        has_reply_context: false,
        attachment_count: 0,
        image_count: 0,
        video_count: 0,
    }));
}


#[test]
fn frozen_delivery_owed_treats_successful_reaction_as_user_visible_delivery() {
    assert!(is_delivery_owed(0, false));
    assert!(!is_delivery_owed(1, false));
    assert!(!is_delivery_owed(0, true));
    assert!(REPLY_NUDGE_PROMPT.contains("actually invoking the SendMessage tool"));
    assert!(REPLY_NUDGE_PROMPT.contains("ack ≠ delivery"));
}


#[test]
fn frozen_terminal_projection_keeps_delivery_and_reply_nudge_semantics_together() {
    let completed_silent = project_turn_terminal(false, false, true, 0, false);
    assert_eq!(completed_silent.kind, TurnTerminalKind::Completed);
    assert!(!completed_silent.delivered);
    assert!(completed_silent.reply_nudge_owed);

    let completed_sent = project_turn_terminal(false, false, true, 1, false);
    assert!(completed_sent.delivered);
    assert!(!completed_sent.reply_nudge_owed);

    let completed_reaction = project_turn_terminal(false, false, true, 0, true);
    assert!(completed_reaction.delivered);
    assert!(!completed_reaction.reply_nudge_owed);

    let waiting = project_turn_terminal(true, false, true, 0, false);
    assert_eq!(waiting.kind, TurnTerminalKind::WaitingUser);
    assert!(!waiting.reply_nudge_owed);

    let cancelled = project_turn_terminal(false, true, true, 0, false);
    assert_eq!(cancelled.kind, TurnTerminalKind::Cancelled);
    assert!(!cancelled.reply_nudge_owed);

    let failed = project_turn_terminal(false, false, false, 0, false);
    assert_eq!(failed.kind, TurnTerminalKind::Failed);
    assert!(!failed.reply_nudge_owed);
}


#[test]
fn frozen_reply_nudge_retries_are_epoch_fenced_and_bounded() {
    assert_eq!(MAX_REPLY_NUDGES, 3);
    assert!(should_attempt_reply_nudge(0, false, 0, 7, 7, false, false, true));
    assert!(should_attempt_reply_nudge(0, false, 2, 7, 7, false, false, true));
    assert!(!should_attempt_reply_nudge(0, false, 3, 7, 7, false, false, true));
    assert!(!should_attempt_reply_nudge(1, false, 0, 7, 7, false, false, true));
    assert!(!should_attempt_reply_nudge(0, true, 0, 7, 7, false, false, true));
    assert!(!should_attempt_reply_nudge(0, false, 0, 7, 8, false, false, true));
    assert!(!should_attempt_reply_nudge(0, false, 0, 7, 7, true, false, true));
    assert!(!should_attempt_reply_nudge(0, false, 0, 7, 7, false, true, true));
    assert!(!should_attempt_reply_nudge(0, false, 0, 7, 7, false, false, false));
}


#[test]
fn reply_nudge_checkpoint_input_is_hidden_from_the_original_user_identity() {
    let lifecycle = vec![ProviderMessage {
        role: "user".into(),
        content: "original constraint FABUSHI-MARKER-7421".into(),
    }];
    let provider = lifecycle.clone();
    let options = mahayana_host_runtime::runner::TurnRunOptions {
        inference_request_id: Some("request-1".into()),
        message_id: Some("message-1".into()),
        recent_message_text: Some("original".into()),
        recent_message_rich_text: Some("<p>original</p>".into()),
        recent_user_messages: Vec::new(),
        turn_started_at_ms: Some(41),
        is_fork: true,
        attachment_count: 2,
        image_count: 1,
        video_count: 1,
        has_reply_context: true,
    };
    let shaped = shape_reply_nudge_turn_input(&lifecycle, &provider, &options);
    let shaped_nudge = shaped.lifecycle_messages.last().unwrap().content.as_str();
    assert!(shaped_nudge.starts_with(REPLY_NUDGE_PROMPT));
    assert!(shaped_nudge.contains("--- BEGIN PENDING USER REQUEST ---"));
    assert!(shaped_nudge.contains("original constraint FABUSHI-MARKER-7421"));
    assert_eq!(shaped.provider_messages.last().unwrap().content, shaped_nudge);
    assert_eq!(shaped.options.inference_request_id.as_deref(), Some("request-1"));
    assert_eq!(shaped.options.message_id, None);
    assert_eq!(shaped.options.recent_message_text.as_deref(), Some(shaped_nudge));
    assert_eq!(shaped.options.recent_message_rich_text, None);
    assert!(shaped.options.recent_user_messages.is_empty());
    assert_eq!(shaped.options.turn_started_at_ms, Some(41));
    assert!(!shaped.options.is_fork);
    assert_eq!(shaped.options.attachment_count, 0);
    assert_eq!(shaped.options.image_count, 0);
    assert_eq!(shaped.options.video_count, 0);
    assert!(!shaped.options.has_reply_context);
}


#[test]
fn closing_send_nudge_uses_the_same_hidden_checkpoint_identity_rules() {
    let lifecycle = vec![ProviderMessage {
        role: "user".into(),
        content: "original closing constraint CLOSING-MARKER-5937".into(),
    }];
    let provider = lifecycle.clone();
    let options = mahayana_host_runtime::runner::TurnRunOptions {
        inference_request_id: Some("request-2".into()),
        message_id: Some("message-2".into()),
        recent_message_text: Some("original".into()),
        recent_message_rich_text: Some("<p>original</p>".into()),
        recent_user_messages: Vec::new(),
        turn_started_at_ms: Some(41),
        is_fork: true,
        attachment_count: 3,
        image_count: 1,
        video_count: 1,
        has_reply_context: true,
    };
    let shaped = shape_closing_send_nudge_turn_input(&lifecycle, &provider, &options);
    let shaped_nudge = shaped.lifecycle_messages.last().unwrap().content.as_str();
    assert!(shaped_nudge.starts_with(CLOSING_SEND_NUDGE_PROMPT));
    assert!(shaped_nudge.contains("--- BEGIN PENDING USER REQUEST ---"));
    assert!(shaped_nudge.contains("original closing constraint CLOSING-MARKER-5937"));
    assert_eq!(shaped.provider_messages.last().unwrap().content, shaped_nudge);
    assert_eq!(
        shaped.options.inference_request_id.as_deref(),
        Some("request-2")
    );
    assert_eq!(shaped.options.message_id, None);
    assert_eq!(
        shaped.options.recent_message_text.as_deref(),
        Some(shaped_nudge)
    );
    assert_eq!(shaped.options.recent_message_rich_text, None);
    assert_eq!(shaped.options.turn_started_at_ms, Some(41));
    assert!(!shaped.options.is_fork);
    assert_eq!(shaped.options.attachment_count, 0);
    assert_eq!(shaped.options.image_count, 0);
    assert_eq!(shaped.options.video_count, 0);
    assert!(!shaped.options.has_reply_context);
}


#[test]
fn ordinary_turn_empty_delivery_report_requires_a_settled_same_epoch_delivery_debt() {
    let report = build_turn_empty_delivery_report(
        "agent-1",
        Some("request-1"),
        Some("turn"),
        3,
        4,
        true,
        1250,
        true,
        0,
        false,
        false,
        false,
        true,
        8,
        8,
    )
    .expect("empty delivery report");
    assert_eq!(report.conversation_id, "agent-1");
    assert_eq!(report.request_id.as_deref(), Some("request-1"));
    assert_eq!(report.source, "turn");
    assert_eq!(report.request_source.as_deref(), Some("turn"));
    assert_eq!(report.reply_nudge_attempts, Some(3));
    assert_eq!(report.redrive_attempts, None);
    assert_eq!(report.tool_call_count, 4);
    assert!(report.stream_output_produced);
    assert_eq!(report.duration_ms, 1250.0);
    assert!(report.ack_outstanding);

    assert!(build_turn_empty_delivery_report(
        "agent-1", Some("request-1"), Some("turn"), 3, 4, false, 1, false,
        1, false, false, false, true, 8, 8,
    ).is_none());
    assert!(build_turn_empty_delivery_report(
        "agent-1", Some("request-1"), Some("turn"), 3, 4, false, 1, false,
        0, true, false, false, true, 8, 8,
    ).is_none());
    assert!(build_turn_empty_delivery_report(
        "agent-1", Some("request-1"), Some("turn"), 3, 4, false, 1, false,
        0, false, true, false, true, 8, 8,
    ).is_none());
    assert!(build_turn_empty_delivery_report(
        "agent-1", Some("request-1"), Some("turn"), 3, 4, false, 1, false,
        0, false, false, true, true, 8, 8,
    ).is_none());
    assert!(build_turn_empty_delivery_report(
        "agent-1", Some("request-1"), Some("turn"), 3, 4, false, 1, false,
        0, false, false, false, true, 8, 9,
    ).is_none());
}
