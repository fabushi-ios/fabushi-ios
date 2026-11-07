use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::telemetry::analytics_service::{
    ANALYTICS_BUFFER_LIMIT, ANALYTICS_FLUSH_INTERVAL_MS, ANALYTICS_NORMAL_FLUSH_TIMEOUT_MS,
    AnalyticsClient, AutomationRunAnalyticsTelemetry, FROZEN_ANALYTICS_SERVICE_BLOB,
    MAX_DEFERRED_ANALYTICS_EVENTS, TelemetryService, product_analytics_event,
};
use serde_json::{Value, json};

#[derive(Default)]
struct RecordingTelemetry {
    methods: Mutex<Vec<&'static str>>,
}

impl TelemetryService for RecordingTelemetry {
    fn report(&self, method: &'static str, _report: &Value) {
        self.methods
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(method);
    }
}

#[derive(Default)]
struct RecordingAnalytics {
    events: Mutex<Vec<(String, Value)>>,
}

impl AnalyticsClient for RecordingAnalytics {
    fn track_event(&self, name: &str, properties: &Value) {
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((name.to_string(), properties.clone()));
    }
}

#[test]
fn frozen_telemetry_facade_forwards_every_declared_method() {
    assert_eq!(
        FROZEN_ANALYTICS_SERVICE_BLOB,
        "4ea2919cfcc54b1c549f856188538ae032c5a522"
    );
    let telemetry = Arc::new(RecordingTelemetry::default());
    let analytics = Arc::new(RecordingAnalytics::default());
    let facade = AutomationRunAnalyticsTelemetry::new(
        telemetry.clone(),
        analytics.clone(),
    );
    let report = json!({"value": 1});

    facade.start_turn(&report);
    facade.report_tool_call_error(&report);
    facade.report_tool_call_stalled(&report);
    facade.report_tool_call_started(&report);
    facade.report_agent_error(&report);
    facade.report_bot_block(&report);
    facade.report_daemon_ping(&report);
    facade.report_box_boot_stage(&report);
    facade.report_exec_daemon_restart(&report);
    facade.report_supervisor_restart(&report);
    facade.report_turn_interrupt(&report);
    facade.report_turn_await(&report);
    facade.report_turn_retry(&report);
    facade.report_user_message_received(&report);
    facade.report_closing_send_nudge(&report);
    facade.report_subagent_revival(&report);
    facade.report_shell_revival(&report);
    facade.report_computer_use_usage(&report);
    facade.report_ttft(&report);
    facade.report_send_dispatch(&report);
    facade.report_queue_accepted(&report);
    facade.report_queue_dequeued(&report);
    facade.report_queue_watchdog(&report);
    facade.report_ack_obligation(&report);
    facade.report_pending_wake(&report);
    facade.report_turn_usage(&report);
    facade.report_turn_empty_delivery(&report);
    facade.report_journal_outcome(&report);
    facade.report_auto_review_expire_sweep_failed(&report);
    facade.report_automation_lifecycle(&report);
    facade.report_automation_fire_dropped(&report);

    let methods = telemetry
        .methods
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(
        methods,
        vec![
            "startTurn",
            "reportToolCallError",
            "reportToolCallStalled",
            "reportToolCallStarted",
            "reportAgentError",
            "reportBotBlock",
            "reportDaemonPing",
            "reportBoxBootStage",
            "reportExecDaemonRestart",
            "reportSupervisorRestart",
            "reportTurnInterrupt",
            "reportTurnAwait",
            "reportTurnRetry",
            "reportUserMessageReceived",
            "reportClosingSendNudge",
            "reportSubagentRevival",
            "reportShellRevival",
            "reportComputerUseUsage",
            "reportTtft",
            "reportSendDispatch",
            "reportQueueAccepted",
            "reportQueueDequeued",
            "reportQueueWatchdog",
            "reportAckObligation",
            "reportPendingWake",
            "reportTurnUsage",
            "reportTurnEmptyDelivery",
            "reportJournalOutcome",
            "reportAutoReviewExpireSweepFailed",
            "reportAutomationLifecycle",
            "reportAutomationFireDropped",
        ]
    );
    assert!(
        analytics
            .events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty()
    );
}

#[test]
fn automation_run_wrapper_tracks_product_event_then_delegates() {
    let telemetry = Arc::new(RecordingTelemetry::default());
    let analytics = Arc::new(RecordingAnalytics::default());
    let facade = AutomationRunAnalyticsTelemetry::new(
        telemetry.clone(),
        analytics.clone(),
    );
    facade.report_automation_run(&json!({
        "conversationId": "agent-1",
        "automationId": "automation-2",
        "trigger": "schedule",
        "outcome": "ok",
        "isGroup": true,
        "sentMessageCount": 3,
        "ignored": {"nested": true}
    }));

    assert_eq!(
        telemetry
            .methods
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_slice(),
        ["reportAutomationRun"]
    );
    let events = analytics
        .events
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "sand.automation.run");
    assert_eq!(events[0].1["agent_id"], "agent-1");
    assert_eq!(events[0].1["automation_id"], "automation-2");
    assert_eq!(events[0].1["trigger"], "schedule");
    assert_eq!(events[0].1["outcome"], "ok");
    assert_eq!(events[0].1["is_group"], true);
    assert_eq!(events[0].1["sent_message_count"], 3);
    assert!(events[0].1.get("ignored").is_none());
}

#[test]
fn product_event_sanitization_keeps_only_frozen_scalar_shapes() {
    let event = product_analytics_event(
        "sand.test",
        &json!({
            "string": "value",
            "number": 4.5,
            "bool": true,
            "null": null,
            "array": [1, 2],
            "object": {"secret": "drop"}
        }),
    );
    assert_eq!(event.name, "sand.test");
    assert_eq!(event.properties["string"], "value");
    assert_eq!(event.properties["number"], 4.5);
    assert_eq!(event.properties["bool"], true);
    assert!(!event.properties.contains_key("null"));
    assert!(!event.properties.contains_key("array"));
    assert!(!event.properties.contains_key("object"));
}


#[test]
fn product_analytics_buffer_policy_matches_frozen_runtime() {
    assert_eq!(MAX_DEFERRED_ANALYTICS_EVENTS, 256);
    assert_eq!(ANALYTICS_BUFFER_LIMIT, 200);
    assert_eq!(ANALYTICS_FLUSH_INTERVAL_MS, 3_000);
    assert_eq!(ANALYTICS_NORMAL_FLUSH_TIMEOUT_MS, 2_500);
}
