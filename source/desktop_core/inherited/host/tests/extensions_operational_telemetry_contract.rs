use mahayana_host_runtime::extensions::telemetry::automation_fire_telemetry::{
    AUTOMATION_LATE_FIRE_THRESHOLD_MS, AutomationFireDroppedReport, AutomationRunReport,
    automation_fire_dropped_telemetry, automation_run_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::conversation_gc_telemetry::{
    ConversationGcReport, capped_count, conversation_gc_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::local_exec_telemetry::{
    LocalExecFailedReport, LocalExecProviderReport, LocalExecRefusalCause, LocalExecRefusedReport,
    local_exec_failed_telemetry, local_exec_provider_telemetry, local_exec_refused_telemetry,
    refusal_error,
};
use mahayana_host_runtime::extensions::telemetry::queue_telemetry_mappers::{
    AckObligationReport, PendingWakeReport, QueueAcceptedReport, QueueDequeuedReport,
    QueueWatchdogReport, SendDispatchReport, ack_obligation_telemetry,
    pending_wake_telemetry, queue_accepted_telemetry, queue_dequeued_telemetry,
    queue_watchdog_telemetry, send_dispatch_telemetry,
};

#[test]
fn conversation_gc_and_automation_fire_match_frozen_grok_rules() {
    assert_eq!(capped_count(-3.0), "0");
    assert_eq!(capped_count(4.6), "5");
    assert_eq!(capped_count(9_007_199_254_740_992.0), "9007199254740991");

    let skipped = conversation_gc_telemetry(&ConversationGcReport::Skipped {
        trigger: "startup".into(),
        agent_id: "agent-1".into(),
        skip_reason: "unresolved-refs".into(),
        unresolved_proto_refs: Some(2.4),
    });
    assert_eq!(skipped.level, Some("warn"));
    assert_eq!(skipped.event, Some("sand.conversation.gc"));
    assert_eq!(skipped.metadata["unresolved_proto_refs"], "2");

    let collected = conversation_gc_telemetry(&ConversationGcReport::Collected {
        trigger: "pressure".into(),
        agent_id: "agent-1".into(),
        still_over_cap: false,
        deleted_rows: 3.0,
        deleted_bytes: 1024.0,
        live_rows: 9.0,
        live_bytes: 2048.0,
        vacuumed: true,
    });
    assert_eq!(collected.level, Some("info"));
    assert_eq!(collected.metadata["vacuumed"], "true");

    let late = automation_run_telemetry(&AutomationRunReport {
        conversation_id: "conversation-1".into(),
        automation_id: "automation-1".into(),
        trigger: "schedule".into(),
        outcome: "ok".into(),
        is_group: false,
        duration_ms: 12.5,
        lateness_ms: Some(AUTOMATION_LATE_FIRE_THRESHOLD_MS),
        scheduled_for_ms: Some(42.0),
        sent_message_count: Some(1),
        event_batch_size: None,
    });
    assert_eq!(late.level, Some("warn"));
    assert_eq!(late.metadata["late"], "true");
    assert_eq!(late.metadata["duration_ms"], "12.5");

    let dropped = automation_fire_dropped_telemetry(&AutomationFireDroppedReport {
        conversation_id: "conversation-1".into(),
        trigger: "event".into(),
        reason: "definition_missing".into(),
        scheduled_for_ms: None,
        lateness_ms: Some(7.0),
        error_type: Some("state".into()),
        error_code: Some("missing".into()),
        run_uuid: Some("run-1".into()),
        fire_age_ms: Some(8.0),
        has_definition_revision: Some(false),
        box_uptime_ms: Some(9.0),
    });
    assert_eq!(dropped.level, Some("warn"));
    assert_eq!(dropped.event, Some("sand.automation.fire_dropped"));
    assert_eq!(dropped.metadata["has_definition_revision"], "false");
}

#[test]
fn local_exec_telemetry_preserves_refusal_and_provider_contracts() {
    let error = refusal_error(LocalExecRefusalCause::NoProviders);
    assert_eq!(error.code, "SAND-E0111");
    assert_eq!(error.domain, "transport");
    assert!(error.retryable);

    let refused = local_exec_refused_telemetry(&LocalExecRefusedReport {
        cause: LocalExecRefusalCause::StaleHeartbeat,
        site: "shell".into(),
        conversation_id: "conversation-3".into(),
        provider_count: 2,
        live_provider_count: 0,
        ever_registered: true,
        empty_for_ms: Some(10.6),
    });
    assert_eq!(refused.level, Some("warn"));
    assert_eq!(refused.event, Some("sand.local_exec.refused"));
    assert_eq!(refused.metadata["cause"], "stale_heartbeat");
    assert_eq!(refused.metadata["empty_for_ms"], "11");
    assert_eq!(refused.metadata["error_code"], "SAND-E0112");

    let failed = local_exec_failed_telemetry(&LocalExecFailedReport {
        error_class: "spawn".into(),
        errno: Some("ENOENT".into()),
        site: "shell".into(),
        conversation_id: "conversation-3".into(),
    });
    assert_eq!(failed.event, Some("sand.local_exec.exec_failed"));
    assert_eq!(failed.metadata["surface"], "external");

    let hello = local_exec_provider_telemetry(&LocalExecProviderReport::Hello {
        provider_id: "provider-1".into(),
        provider_count: 1,
        hello_delay_ms: 2.6,
        computer_id_present: true,
        rehello: false,
        supervised: Some(true),
        variant: Some("desktop".into()),
    });
    assert_eq!(hello.level, Some("info"));
    assert_eq!(hello.metadata["phase"], "hello");
    assert_eq!(hello.metadata["hello_delay_ms"], "3");
    assert_eq!(hello.metadata["supervised"], "true");

    let detached = local_exec_provider_telemetry(&LocalExecProviderReport::Detached {
        provider_id: "provider-1".into(),
        provider_count: 0,
        age_ms: 8.4,
        had_hello: true,
        has_heartbeat: false,
        was_live: true,
        emptied: true,
    });
    assert_eq!(detached.metadata["phase"], "detached");
    assert_eq!(detached.metadata["age_ms"], "8");
    assert_eq!(detached.metadata["emptied"], "true");
}

#[test]
fn queue_telemetry_mappers_keep_rounding_levels_and_optional_fields() {
    let dispatch = send_dispatch_telemetry(&SendDispatchReport {
        conversation_id: "conversation-1".into(),
        dispatch_ms: Some(4.6),
        host_dispatch_ms: 8.4,
        skew: "none".into(),
        skew_reason: None,
        skew_bucket: Some("zero".into()),
        is_fork: false,
        model_id: Some("gpt".into()),
        trace_id: None,
        span_id: None,
    });
    assert_eq!(dispatch.event, Some("sand.send_dispatch"));
    assert_eq!(dispatch.metadata["send_dispatch_ms"], "5");
    assert_eq!(dispatch.metadata["send_dispatch_host_ms"], "8");

    let accepted = queue_accepted_telemetry(&QueueAcceptedReport {
        conversation_id: "conversation-1".into(),
        lane: "user".into(),
        source: "composer".into(),
        position: 2,
        depth_user: 2,
        depth_agent: 1,
        depth_background: 0,
        has_active: true,
    });
    assert_eq!(accepted.event, Some("sand.queue.accepted"));
    assert_eq!(accepted.metadata["has_active"], "true");

    let dequeued = queue_dequeued_telemetry(&QueueDequeuedReport {
        conversation_id: "conversation-1".into(),
        lane: "user".into(),
        source: "composer".into(),
        queue_wait_ms: 10.6,
        accepted_to_run_ms: Some(12.4),
        jumped_background: 2,
        depth_user: 0,
        depth_agent: 0,
        depth_background: 1,
    });
    assert_eq!(dequeued.metadata["queue_wait_ms"], "11");
    assert_eq!(dequeued.metadata["accepted_to_run_ms"], "12");
    assert_eq!(dequeued.metadata["jumped_background"], "2");

    let watchdog = queue_watchdog_telemetry(&QueueWatchdogReport {
        conversation_id: "conversation-1".into(),
        stage: "late_settle".into(),
        active_lane: Some("user".into()),
        active_source: None,
        active_runtime_ms: 99.6,
        waiting_user_age_ms: None,
        interrupted: Some(false),
    });
    assert_eq!(watchdog.level, Some("info"));
    assert_eq!(watchdog.metadata["active_runtime_ms"], "100");

    let lost = ack_obligation_telemetry(&AckObligationReport {
        conversation_id: "conversation-1".into(),
        outcome: "lost".into(),
        age_ms: Some(5.5),
        coalesced_count: Some(2),
        redrive_attempts: Some(1),
        time_to_first_visible_ack_ms: None,
        interrupt_to_replacement_ack_ms: None,
        reason: Some("disconnect".into()),
    });
    assert_eq!(lost.level, Some("warn"));
    assert_eq!(lost.event, Some("sand.ack.obligation"));

    let wake = pending_wake_telemetry(&PendingWakeReport {
        conversation_id: "conversation-1".into(),
        outcome: "pruned".into(),
        kind: Some("timer".into()),
        work_id: Some("work-1".into()),
        age_ms: Some(6.4),
        reason: Some("expired".into()),
        is_quiet_origin: Some(true),
    });
    assert_eq!(wake.level, Some("warn"));
    assert_eq!(wake.event, Some("sand.pending_wake"));
    assert_eq!(wake.metadata["quiet_origin"], "true");
}
