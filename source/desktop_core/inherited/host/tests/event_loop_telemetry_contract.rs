use std::sync::{Arc, mpsc};
use std::time::Duration;

use mahayana_host_runtime::extensions::telemetry::event_loop_telemetry::{
    EventLoopTelemetryConfig, EventLoopTelemetryRuntime, EventLoopTrigger, EventLoopWindowReport,
    HEARTBEAT_EVERY_N_WINDOWS, PRESSURE_P95_MS, WindowEmitArgs, event_loop_window_telemetry,
    resolve_window_emit,
};

#[test]
fn pressure_wins_and_default_heartbeat_matches_frozen_windows() {
    assert_eq!(
        resolve_window_emit(WindowEmitArgs {
            window_index: 1,
            p95_ms: PRESSURE_P95_MS,
            pressure_p95_ms: None,
            heartbeat_every_n_windows: None,
        }),
        Some(EventLoopTrigger::Pressure)
    );
    assert_eq!(
        resolve_window_emit(WindowEmitArgs {
            window_index: HEARTBEAT_EVERY_N_WINDOWS,
            p95_ms: PRESSURE_P95_MS - 0.1,
            pressure_p95_ms: None,
            heartbeat_every_n_windows: None,
        }),
        Some(EventLoopTrigger::Heartbeat)
    );
    assert_eq!(
        resolve_window_emit(WindowEmitArgs {
            window_index: 4,
            p95_ms: 1.0,
            pressure_p95_ms: None,
            heartbeat_every_n_windows: None,
        }),
        None
    );
}

#[test]
fn custom_threshold_and_disabled_heartbeat_are_fail_safe() {
    assert_eq!(
        resolve_window_emit(WindowEmitArgs {
            window_index: 6,
            p95_ms: 9.0,
            pressure_p95_ms: Some(10.0),
            heartbeat_every_n_windows: Some(3),
        }),
        Some(EventLoopTrigger::Heartbeat)
    );
    assert_eq!(
        resolve_window_emit(WindowEmitArgs {
            window_index: 10,
            p95_ms: 9.0,
            pressure_p95_ms: Some(10.0),
            heartbeat_every_n_windows: Some(0),
        }),
        None
    );
}

#[test]
fn telemetry_projection_preserves_levels_rounding_and_utilization_precision() {
    let pressure = event_loop_window_telemetry(EventLoopWindowReport {
        trigger: EventLoopTrigger::Pressure,
        p50_ms: 1.4,
        p95_ms: 50.6,
        max_ms: 101.5,
        utilization: 0.45678,
        window_ms: 60_123,
    });
    assert_eq!(pressure.level, Some("warn"));
    assert_eq!(pressure.event, Some("sand.host.event_loop"));
    assert_eq!(pressure.metadata.get("trigger").map(String::as_str), Some("pressure"));
    assert_eq!(pressure.metadata.get("p50_ms").map(String::as_str), Some("1"));
    assert_eq!(pressure.metadata.get("p95_ms").map(String::as_str), Some("51"));
    assert_eq!(pressure.metadata.get("max_ms").map(String::as_str), Some("102"));
    assert_eq!(pressure.metadata.get("utilization").map(String::as_str), Some("0.457"));
    assert_eq!(pressure.metadata.get("window_ms").map(String::as_str), Some("60123"));

    let heartbeat = event_loop_window_telemetry(EventLoopWindowReport {
        trigger: EventLoopTrigger::Heartbeat,
        p50_ms: 0.0,
        p95_ms: 0.0,
        max_ms: 0.0,
        utilization: 0.0,
        window_ms: 60_000,
    });
    assert_eq!(heartbeat.level, Some("info"));
}


#[test]
fn production_sampler_emits_window_metrics_and_disposes() {
    let (tx, rx) = mpsc::channel();
    let runtime = EventLoopTelemetryRuntime::start_with_config(
        EventLoopTelemetryConfig {
            resolution_ms: 5,
            window_ms: 40,
            pressure_p95_ms: f64::MAX,
            heartbeat_every_n_windows: 1,
        },
        Arc::new(move |report| {
            let _ = tx.send(report);
        }),
    );

    let report = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("production sampler should emit the first heartbeat window");
    assert_eq!(report.trigger, EventLoopTrigger::Heartbeat);
    assert!(report.window_ms >= 40);
    assert!(report.p50_ms >= 0.0);
    assert!(report.p95_ms >= report.p50_ms);
    assert!(report.max_ms >= report.p95_ms);
    assert!((0.0..=1.0).contains(&report.utilization));

    drop(runtime);
}
