use std::collections::BTreeMap;

use mahayana_host_runtime::extensions::telemetry::host_lifecycle_progress::{
    HostLifecycleError, HostLifecycleReport,
};
use mahayana_host_runtime::extensions::telemetry::lifecycle_telemetry::{
    BOX_BOOT_STAGE_EVENT, BOX_IMAGE_CHECK_EVENT, DAEMON_PING_EVENT, HOST_LIFECYCLE_EVENT,
    HOST_STARTUP_EVENT, BoxImageCheckReport, BoxInfrastructureEvent, DaemonPingReport,
    box_image_check_telemetry, box_infrastructure_telemetry, daemon_ping_telemetry,
    host_lifecycle_telemetry, host_startup_telemetry,
};
use mahayana_host_runtime::extensions::telemetry::sand_error_tags::SandErrorValue;

#[test]
fn host_startup_and_lifecycle_match_frozen_event_levels_and_fields() {
    let startup = host_startup_telemetry(
        BTreeMap::from([
            ("build".into(), "release".into()),
            ("host_built_at_ms".into(), "caller-wins".into()),
        ]),
        "1700000000000",
    );
    assert_eq!(startup.level, Some("info"));
    assert_eq!(startup.event, Some(HOST_STARTUP_EVENT));
    assert_eq!(startup.metadata["build"], "release");
    assert_eq!(startup.metadata["host_built_at_ms"], "caller-wins");

    let plugin = host_lifecycle_telemetry(&HostLifecycleReport::Completed {
        phase: "plugin_graph".into(),
        plugin_count: Some(35),
        entry_count: Some(99),
        duration_ms: 41,
    });
    assert_eq!(plugin.event, Some(HOST_LIFECYCLE_EVENT));
    assert_eq!(plugin.level, Some("info"));
    assert_eq!(plugin.metadata["plugin_count"], "35");
    assert!(!plugin.metadata.contains_key("entry_count"));

    let transcript = host_lifecycle_telemetry(&HostLifecycleReport::Completed {
        phase: "transcript_read".into(),
        plugin_count: Some(35),
        entry_count: Some(99),
        duration_ms: 9,
    });
    assert_eq!(transcript.metadata["entry_count"], "99");
    assert!(!transcript.metadata.contains_key("plugin_count"));

    let stuck = host_lifecycle_telemetry(&HostLifecycleReport::Stuck {
        phase: "plugin_graph".into(),
        duration_ms: 300_000,
        error: HostLifecycleError::Stalled,
    });
    assert_eq!(stuck.level, Some("warn"));
    assert_eq!(stuck.metadata["error_code"], "SAND-E0302");
    assert_eq!(stuck.metadata["error_domain"], "rebuild");

    let failed = host_lifecycle_telemetry(&HostLifecycleReport::Failed {
        phase: "transcript_read".into(),
        duration_ms: 3,
        error: HostLifecycleError::Failed,
    });
    assert_eq!(failed.level, Some("error"));
    assert_eq!(failed.metadata["error_code"], "SAND-E0303");
}

#[test]
fn daemon_and_image_check_telemetry_preserve_frozen_fail_closed_semantics() {
    let ok = daemon_ping_telemetry(&DaemonPingReport {
        outcome: "ok".into(),
        attempts: 2,
        duration_ms: 10,
        unready_duration_ms: 7,
        readiness_state: "ready".into(),
        target: "box".into(),
        cause_summary: Some("must-not-emit".into()),
    });
    assert_eq!(ok.event, Some(DAEMON_PING_EVENT));
    assert_eq!(ok.level, Some("warn"));
    assert!(!ok.metadata.contains_key("cause"));

    let failed = daemon_ping_telemetry(&DaemonPingReport {
        outcome: "failed".into(),
        attempts: 3,
        duration_ms: 22,
        unready_duration_ms: 21,
        readiness_state: "unready".into(),
        target: "box".into(),
        cause_summary: Some("connect".into()),
    });
    assert_eq!(failed.level, Some("error"));
    assert_eq!(failed.metadata["cause"], "connect");

    let skipped = box_image_check_telemetry(&BoxImageCheckReport::Skipped {
        trigger: "startup".into(),
        duration_ms: 2,
        skip_reason: "disabled".into(),
    });
    assert_eq!(skipped.event, Some(BOX_IMAGE_CHECK_EVENT));
    assert_eq!(skipped.metadata["skip_reason"], "disabled");
    assert!(!skipped.metadata.contains_key("error_code"));

    let timeout = box_image_check_telemetry(&BoxImageCheckReport::Timeout {
        trigger: "startup".into(),
        duration_ms: 3000,
        error: SandErrorValue::new("SAND-E0304"),
    });
    assert_eq!(timeout.level, Some("warn"));
    assert_eq!(timeout.metadata["error_code"], "SAND-E0304");
}

#[test]
fn box_infrastructure_event_projection_matches_frozen_levels_and_optional_fields() {
    let ready = box_infrastructure_telemetry(&BoxInfrastructureEvent::BootStage {
        stage: "ready".into(),
        duration_ms: 25,
    });
    assert_eq!(ready.event, Some(BOX_BOOT_STAGE_EVENT));
    assert_eq!(ready.level, Some("warn"));

    let fallback = box_infrastructure_telemetry(&BoxInfrastructureEvent::HostBootFetch {
        outcome: "fallback".into(),
        reason: Some("cache-miss".into()),
        duration_ms: 12,
        swap_ms: None,
        from_version: Some("1".into()),
        to_version: Some("2".into()),
    });
    assert_eq!(fallback.level, Some("warn"));
    assert_eq!(fallback.metadata["reason"], "cache-miss");
    assert!(!fallback.metadata.contains_key("swap_ms"));

    let cookie = box_infrastructure_telemetry(&BoxInfrastructureEvent::CookiePersist {
        phase: "flush".into(),
        outcome: "failed".into(),
        seed_cookies: 4,
        injected: Some(3),
        missing_after: Some(1),
        attempts: Some(2),
    });
    assert_eq!(cookie.level, Some("error"));
    assert_eq!(cookie.metadata["seed_cookies"], "4");

    let crash = box_infrastructure_telemetry(&BoxInfrastructureEvent::ProcessCrash {
        binary: "box-exec-daemon".into(),
        signal: "SIGKILL".into(),
        count: 2,
    });
    assert_eq!(crash.level, Some("warn"));
    assert_eq!(crash.metadata["count"], "2");
}
