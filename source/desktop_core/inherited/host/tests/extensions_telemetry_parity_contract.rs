use mahayana_host_runtime::extensions::telemetry::automation_shadow_prune_telemetry::{
    AutomationShadowPruneReport, automation_shadow_prune_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::box_log_ship_telemetry::{
    BoxLogShipReport, box_log_ship_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::experiments_diagnostic_telemetry::{
    ExperimentsDiagnostic, experiments_diagnostic_telemetry, level_for,
};
use mahayana_host_runtime::extensions::telemetry::host_extension_diagnostic_telemetry::{
    HostExtensionDiagnostic, host_extension_diagnostic_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::revival_telemetry_mappers::{
    ShellRevivalReport, SubagentRevivalReport, shell_revival_telemetry,
    subagent_revival_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::session_diagnostic_telemetry::{
    SessionDiagnosticFamily, SessionTelemetryDiagnostic, session_diagnostic_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::turn_empty_delivery_telemetry::{
    TurnEmptyDeliveryReport, turn_empty_delivery_telemetry,
};

#[test]
fn additional_frozen_grok_telemetry_mappings_preserve_levels_events_and_metadata() {
    let prune = automation_shadow_prune_telemetry(&AutomationShadowPruneReport {
        outcome: "failed".into(),
        conversation_id: "conversation-1".into(),
        automation_id: "automation-2".into(),
        local_definition_state: "loaded".into(),
        local_definition_count: 2,
        desired_count: 1,
        remote_shadow_count: 3,
        box_uptime_ms: Some(5000),
    });
    assert_eq!(prune.level, Some("warn"));
    assert_eq!(prune.event, Some("sand.automation.shadow_prune"));
    assert_eq!(prune.metadata["box_uptime_ms"], "5000");

    let empty = turn_empty_delivery_telemetry(&TurnEmptyDeliveryReport {
        conversation_id: "conversation-1".into(),
        request_id: Some("request-9".into()),
        source: "runner".into(),
        request_source: Some("user".into()),
        reply_nudge_attempts: Some(2),
        redrive_attempts: None,
        tool_call_count: 3,
        stream_output_produced: false,
        duration_ms: 12.6,
        ack_outstanding: true,
    });
    assert_eq!(empty.level, Some("warn"));
    assert_eq!(empty.event, Some("sand.turn.empty_delivery"));
    assert_eq!(empty.metadata["duration_ms"], "13");
    assert_eq!(empty.metadata["stream_output_produced"], "false");
    assert!(!empty.metadata.contains_key("redrive_attempts"));

    let progress = box_log_ship_telemetry(&BoxLogShipReport::Progress {
        bytes_written: 100,
        bytes_delivered: 80,
        pending_window_count: 2,
        oldest_pending_window_age_ms: 7,
    });
    assert_eq!(progress.level, Some("info"));
    assert_eq!(progress.metadata["kind"], "progress");
    assert_eq!(progress.metadata["bytes_delivered"], "80");
    let failed = box_log_ship_telemetry(&BoxLogShipReport::SaveFailed {
        error_class: "io".into(),
        failure_count: 4,
    });
    assert_eq!(failed.level, Some("warn"));
    assert_eq!(failed.metadata["kind"], "save_failed");

    let subagent = subagent_revival_telemetry(&SubagentRevivalReport {
        parent_agent_id: "parent-1".into(),
        outcome: "dropped".into(),
        completion_count: 1,
        subagent_type: Some("research".into()),
        subagent_agent_id: Some("sub-2".into()),
        reason: Some("transport".into()),
        sent_message_count: Some(0),
        is_quiet_origin: Some(true),
    });
    assert_eq!(subagent.level, Some("warn"));
    assert_eq!(subagent.event, Some("sand.subagent.revival"));
    assert_eq!(subagent.metadata["quiet_origin"], "true");
    let deleted = subagent_revival_telemetry(&SubagentRevivalReport {
        reason: Some("agent_deleted".into()),
        ..SubagentRevivalReport {
            parent_agent_id: "parent-1".into(),
            outcome: "dropped".into(),
            completion_count: 0,
            subagent_type: None,
            subagent_agent_id: None,
            reason: None,
            sent_message_count: None,
            is_quiet_origin: None,
        }
    });
    assert_eq!(deleted.level, Some("info"));

    let shell = shell_revival_telemetry(&ShellRevivalReport {
        conversation_id: "conversation-3".into(),
        outcome: "dropped".into(),
        completion_count: 2,
        sent_message_count: Some(1),
        is_quiet_origin: Some(false),
        reason: Some("agent_gone".into()),
    });
    assert_eq!(shell.level, Some("info"));
    assert_eq!(shell.event, Some("sand.shell.revival"));

    let config = ExperimentsDiagnostic {
        kind: "config_not_applied".into(),
        reason: Some("identity_unhydrated".into()),
        ..ExperimentsDiagnostic::default()
    };
    assert_eq!(level_for(&config), "info");
    let malformed = ExperimentsDiagnostic {
        kind: "bootstrap_config_unparseable".into(),
        ..ExperimentsDiagnostic::default()
    };
    assert_eq!(level_for(&malformed), "warn");
    let projected = experiments_diagnostic_telemetry(&malformed);
    assert_eq!(projected.event, Some("sand.experiments.diagnostic"));

    let attachment = host_extension_diagnostic_telemetry(&HostExtensionDiagnostic {
        extension: "attachments".into(),
        kind: Some("read_miss".into()),
        has_active: Some(false),
        ..HostExtensionDiagnostic::default()
    })
    .expect("attachment diagnostic");
    assert_eq!(attachment.level, Some("warn"));
    assert_eq!(attachment.event, Some("sand.attachment.read_miss"));
    assert_eq!(attachment.metadata["has_active"], "false");
    assert!(
        host_extension_diagnostic_telemetry(&HostExtensionDiagnostic {
            extension: "unknown".into(),
            ..HostExtensionDiagnostic::default()
        })
        .is_none()
    );

    let store = session_diagnostic_telemetry(&SessionTelemetryDiagnostic {
        family: SessionDiagnosticFamily::StoreDb,
        kind: "wal_unavailable".into(),
        agent_id: Some("agent-4".into()),
        error_class: Some("sqlite".into()),
        outcome: Some("salvaged".into()),
        quarantine: Some("db.bad".into()),
        salvaged_kv: Some(7),
        salvaged_blobs: Some(2),
        salvaged_transcript: None,
    });
    assert_eq!(store.level, Some("warn"));
    assert_eq!(store.event, Some("sand.session.store_db"));
    assert_eq!(store.metadata["salvaged_kv"], "7");
    let summary = session_diagnostic_telemetry(&SessionTelemetryDiagnostic {
        family: SessionDiagnosticFamily::SummaryBuild,
        kind: "failed".into(),
        agent_id: None,
        error_class: None,
        outcome: Some("ignored-for-non-store".into()),
        quarantine: None,
        salvaged_kv: None,
        salvaged_blobs: None,
        salvaged_transcript: None,
    });
    assert_eq!(summary.level, Some("error"));
    assert_eq!(summary.event, Some("sand.session.summary_build"));
    assert!(!summary.metadata.contains_key("outcome"));
}
