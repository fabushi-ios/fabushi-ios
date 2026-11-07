use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::box_store_sync::files::SAND_FILES_MAX_BYTES;
use mahayana_host_runtime::extensions::box_store_sync::object_store_port::BoxStoreCanonicalWriteConflictError;
use mahayana_host_runtime::extensions::session::session_diagnostics::{
    SessionDiagnostic, pin_session_diagnostics_reporter, report_session_diagnostic,
};
use mahayana_host_runtime::extensions::telemetry::auto_review_approval_telemetry::{
    AutoReviewApprovalReport, auto_review_approval_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::disk_pressure_telemetry::{
    DiskPressureReport, disk_pressure_telemetry, telemetry_level,
};
use mahayana_host_runtime::extensions::telemetry::host_diagnostic_telemetry::{
    HostDiagnostic, host_diagnostic_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::host_event_bus_telemetry::{
    HOST_EVENT_BUS_EVENT, HostEventBusReport, host_event_bus_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::search_index_health_telemetry::{
    SearchIndexHealthReport, search_index_health_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::send_trace_sampler::{
    NOT_RECORD, create_send_trace_sampler,
};
use mahayana_host_runtime::extensions::transcript::automation_snapshot::{
    AutomationAction, diff_automation_action, snapshot_automations,
};
use mahayana_host_runtime::automations::automation::{AutomationRecord, AutomationRun};
use mahayana_host_runtime::extensions::transcript::sand_automation_spend_guard::{
    SPEND_GUARD_IDLE_TTL_MS, SPEND_GUARD_PAUSE_DELAY_MS, SPEND_GUARD_SNOOZE_MS,
    SpendGuardAnswer, SpendGuardDecision, SpendGuardEvaluation,
    build_spend_guard_nudge_widget, build_spend_guard_paused_widget,
    count_automation_runs_since, evaluate_automation_spend_guard,
    interpret_spend_guard_answer, is_spend_guard_card, render_spend_guard_answer_ack,
    render_spend_guard_nudge_reminder,
};
use mahayana_host_runtime::extensions::transcript::sand_automation_failure::{
    is_background_automation_trigger, normalize_automation_error_kind,
    should_notify_automation_failure,
};
use serde_json::json;

#[test]
fn frozen_grok_small_extension_modules_preserve_behavior() {
    assert_eq!(SAND_FILES_MAX_BYTES, 16 * 1024 * 1024);

    let event_bus = host_event_bus_telemetry(&HostEventBusReport {
        kind: "subscriber_failed".into(),
        topic: Some("turn.updated".into()),
        error_class: "io".into(),
    });
    assert_eq!(event_bus.level, Some("error"));
    assert_eq!(event_bus.event, Some(HOST_EVENT_BUS_EVENT));
    assert_eq!(event_bus.metadata["kind"], "subscriber_failed");
    assert_eq!(event_bus.metadata["topic"], "turn.updated");
    assert_eq!(event_bus.metadata["error_class"], "io");

    let degraded = host_diagnostic_telemetry(&HostDiagnostic {
        kind: "send_ledger_degraded".into(),
        stage: Some("persist".into()),
        agent_id: Some("agent-7".into()),
        reason: None,
        error_class: Some("sqlite".into()),
    });
    assert_eq!(degraded.level, Some("error"));
    assert_eq!(degraded.metadata["stage"], "persist");
    assert_eq!(degraded.metadata["agent_id"], "agent-7");
    assert!(!degraded.metadata.contains_key("reason"));

    let warning = host_diagnostic_telemetry(&HostDiagnostic {
        kind: "other".into(),
        ..HostDiagnostic::default()
    });
    assert_eq!(warning.level, Some("warn"));

    let search = search_index_health_telemetry(&SearchIndexHealthReport {
        kind: "job_retry".into(),
        stage: Some("flush".into()),
        error_class: None,
        count: Some(3),
    });
    assert_eq!(search.level, Some("warn"));
    assert_eq!(search.event, Some("sand.search_index.health"));
    assert_eq!(search.metadata["count"], "3");

    let approval = auto_review_approval_telemetry(&AutoReviewApprovalReport {
        event_type: "settled".into(),
        conversation_id: "conversation-1".into(),
        approval_id: "approval-2".into(),
        surface: "desktop".into(),
        status: "approved".into(),
        age_ms: 12.6,
        ttl_ms: Some(-4.0),
        cause: Some("user".into()),
    });
    assert_eq!(approval.level, None);
    assert_eq!(approval.event, Some("sand.auto_review.approval"));
    assert_eq!(approval.metadata["age_ms"], "13");
    assert_eq!(approval.metadata["ttl_ms"], "0");
    assert_eq!(approval.metadata["cause"], "user");

    assert_eq!(telemetry_level("hard"), "error");
    assert_eq!(telemetry_level("soft"), "warn");
    assert_eq!(telemetry_level("future"), "info");
    let disk = disk_pressure_telemetry(&DiskPressureReport {
        level: "soft".into(),
        volume: "/".into(),
        trigger: "periodic".into(),
        total_bytes: 100.0,
        available_bytes: 12.0,
        used_percent: 88.55,
    });
    assert_eq!(disk.level, Some("warn"));
    assert_eq!(disk.event, None);
    assert_eq!(disk.metadata["total_bytes"], "100");
    assert_eq!(disk.metadata["available_bytes"], "12");
    assert_eq!(disk.metadata["used_percent"], "88.5");

    let sampler = create_send_trace_sampler();
    assert_eq!(sampler.should_sample().decision, NOT_RECORD);
    assert_eq!(sampler.to_string(), "ParentBased{root=AlwaysOffSampler}");

    let conflict = BoxStoreCanonicalWriteConflictError::new(
        "agents/a.json",
        Some("conflicts/a.json".into()),
        Some("etag-1".into()),
        Some("remote".into()),
    );
    assert_eq!(conflict.key, "agents/a.json");
    assert_eq!(conflict.base_etag.as_deref(), Some("etag-1"));
    assert_eq!(conflict.baseline_source.as_deref(), Some("remote"));
    assert_eq!(
        conflict.to_string(),
        "agent-store write for agents/a.json lost a concurrent-write race; content preserved at conflicts/a.json"
    );
    let canonical = BoxStoreCanonicalWriteConflictError::new(
        "agents/b.json",
        None,
        None,
        None,
    );
    assert_eq!(
        canonical.to_string(),
        "canonical write for agents/b.json lost a concurrent-write race"
    );

    assert!(is_background_automation_trigger("schedule"));
    assert!(is_background_automation_trigger("event"));
    assert!(!is_background_automation_trigger("manual"));
    assert_eq!(normalize_automation_error_kind(None), "unknown");
    assert_eq!(
        normalize_automation_error_kind(Some(
            "HTTP 503 (request 123) 550e8400-e29b-41d4-a716-446655440000 0xdeadBEEF Connection_Reset"
        )),
        "http connection reset"
    );
    for occurrence in [0, 1, 2, 4, 8, 16] {
        assert!(should_notify_automation_failure(occurrence));
    }
    for occurrence in [3, 5, 6, 7, 9] {
        assert!(!should_notify_automation_failure(occurrence));
    }
}

#[test]
fn session_diagnostics_reporter_can_be_pinned_replaced_and_cleared() {
    let seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let sink = Arc::clone(&seen);
    pin_session_diagnostics_reporter(Some(Arc::new(move |report| {
        sink.lock()
            .expect("session diagnostic sink")
            .push((report.family.clone(), report.kind.clone()));
    })));

    report_session_diagnostic(&SessionDiagnostic {
        family: "session".into(),
        kind: "recovered".into(),
        metadata: BTreeMap::from([("generation".into(), json!(3))]),
    });
    assert_eq!(
        seen.lock().expect("session diagnostic values").as_slice(),
        &[("session".into(), "recovered".into())]
    );

    pin_session_diagnostics_reporter(None);
    report_session_diagnostic(&SessionDiagnostic {
        family: "session".into(),
        kind: "ignored".into(),
        metadata: BTreeMap::new(),
    });
    assert_eq!(seen.lock().expect("cleared diagnostic values").len(), 1);
}


#[test]
fn automation_snapshot_matches_frozen_grok_diff_semantics() {
    let record = AutomationRecord {
        id: "morning-check".into(),
        name: "Morning Check".into(),
        prompt: "Check the inbox".into(),
        trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
        is_enabled: true,
        created_at: 1_000.0,
        last_run_at: None,
        raised_notices: Vec::new(),
        schedule: "0 9 * * *".into(),
        trigger_description: "Scheduled: 0 9 * * *".into(),
        next_run_at: Some(2_000.0),
        runs: Vec::new(),
        file_path: std::path::PathBuf::from("/tmp/automation.json"),
    };
    let snapshots = snapshot_automations(std::slice::from_ref(&record));
    let before = snapshots.get("morning-check").expect("snapshot");
    assert_eq!(before.id, "morning-check");
    assert_eq!(before.trigger_type, "cron");
    assert_eq!(before.recorded_run_count, 0);

    let mut updated = before.clone();
    updated.prompt = "Check mail and calendar".into();
    assert_eq!(
        diff_automation_action(before, &updated),
        Some(AutomationAction::Updated)
    );

    let mut disabled = before.clone();
    disabled.is_enabled = false;
    assert_eq!(
        diff_automation_action(before, &disabled),
        Some(AutomationAction::Disabled)
    );

    let mut enabled = disabled.clone();
    enabled.is_enabled = true;
    assert_eq!(
        diff_automation_action(&disabled, &enabled),
        Some(AutomationAction::Enabled)
    );

    let mut run_count_only = before.clone();
    run_count_only.recorded_run_count = 9;
    assert_eq!(diff_automation_action(before, &run_count_only), None);

}


#[test]
fn automation_spend_guard_matches_frozen_grok_decisions_and_copy() {
    let now = 1_800_000_000_000.0;
    let inactive = SpendGuardEvaluation {
        now_ms: now,
        last_viewed_at_ms: now - SPEND_GUARD_IDLE_TTL_MS - 1.0,
        unread_count: 15,
        fires_since_viewed_count: 0,
        nudged_at_ms: None,
        snoozed_until_ms: None,
        opted_out: false,
    };
    assert_eq!(
        evaluate_automation_spend_guard(inactive),
        SpendGuardDecision::Nudge
    );
    assert_eq!(
        evaluate_automation_spend_guard(SpendGuardEvaluation {
            last_viewed_at_ms: now - SPEND_GUARD_IDLE_TTL_MS - 2.0,
            unread_count: 0,
            fires_since_viewed_count: 0,
            nudged_at_ms: Some(now - SPEND_GUARD_PAUSE_DELAY_MS - 1.0),
            ..inactive
        }),
        SpendGuardDecision::Pause
    );
    assert_eq!(
        evaluate_automation_spend_guard(SpendGuardEvaluation {
            unread_count: 0,
            fires_since_viewed_count: 0,
            nudged_at_ms: Some(now - 1000.0),
            ..inactive
        }),
        SpendGuardDecision::AwaitingAck
    );
    assert_eq!(
        evaluate_automation_spend_guard(SpendGuardEvaluation {
            snoozed_until_ms: Some(now + SPEND_GUARD_SNOOZE_MS),
            unread_count: 0,
            fires_since_viewed_count: 0,
            ..inactive
        }),
        SpendGuardDecision::Snoozed
    );
    assert_eq!(
        evaluate_automation_spend_guard(SpendGuardEvaluation {
            opted_out: true,
            ..inactive
        }),
        SpendGuardDecision::OptedOut
    );
    assert_eq!(
        evaluate_automation_spend_guard(SpendGuardEvaluation {
            last_viewed_at_ms: now - 1000.0,
            unread_count: 100,
            ..inactive
        }),
        SpendGuardDecision::UserActive
    );

    assert_eq!(
        interpret_spend_guard_answer("spend-guard:never-ask"),
        Some(SpendGuardAnswer::OptOut)
    );
    assert_eq!(interpret_spend_guard_answer("unknown"), None);

    let nudge = build_spend_guard_nudge_widget();
    assert_eq!(
        nudge.prompt,
        "You've been away for a bit — keep my routines running?"
    );
    assert!(is_spend_guard_card(&nudge, SpendGuardAnswer::Keep.value()));
    assert!(!is_spend_guard_card(&nudge, "spend-guard:resume"));
    let paused = build_spend_guard_paused_widget();
    assert!(is_spend_guard_card(
        &paused,
        SpendGuardAnswer::Resume.value()
    ));
    assert!(render_spend_guard_answer_ack(SpendGuardAnswer::Pause)
        .contains("pause every one of your routines"));

    let reminder = render_spend_guard_nudge_reminder(inactive, Some("UTC"));
    assert!(reminder.contains("15 of your messages are unread"));
    assert!(reminder.contains("pause ALL of this agent's routines"));

    let record = AutomationRecord {
        id: "daily".into(),
        name: "Daily".into(),
        prompt: "Do it".into(),
        trigger: json!({"type":"cron","schedule":"@daily"}),
        is_enabled: true,
        created_at: 100.0,
        last_run_at: Some(300.0),
        raised_notices: Vec::new(),
        schedule: "@daily".into(),
        trigger_description: "Scheduled: @daily".into(),
        next_run_at: None,
        runs: vec![
            AutomationRun {
                id: "r1".into(),
                trigger: "schedule".into(),
                started_at: 200.0,
                finished_at: Some(210.0),
                status: "ok".into(),
                detail: None,
                event: None,
                coalesced_run_ids: None,
            },
            AutomationRun {
                id: "r2".into(),
                trigger: "manual".into(),
                started_at: 400.0,
                finished_at: Some(410.0),
                status: "ok".into(),
                detail: None,
                event: None,
                coalesced_run_ids: None,
            },
        ],
        file_path: std::path::PathBuf::from("/tmp/automation.json"),
    };
    assert_eq!(count_automation_runs_since(&[record], 250.0), 1);
}
