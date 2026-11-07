#[test]
fn shipping_box_copy_in_uses_real_host_producer_canonical_mapper_and_unique_owner() {
    let producer = include_str!("../src/extensions/box_store_sync/box_copy_in.rs");
    assert!(producer.contains("pub fn execute_production_box_copy_in_from_env("));
    assert!(producer.contains("HostTelemetryService::open_with_identity_tags("));
    assert!(producer.contains(r#"telemetry_identity_tags.insert("store_backend".into(), store_backend.into())"#));
    assert!(producer.contains("build_copy_in_started_telemetry()"));
    assert!(producer.contains("build_copy_in_result_telemetry(result, duration_ms)"));
    assert!(producer.contains("telemetry.report_box_copy_in(event.level, event.metadata)"));
    assert!(producer.contains("build_copy_in_watchdog_event("));
    assert!(
        !producer.contains("fn telemetry_projection("),
        "copy-in producer must not own a second structured-log projection"
    );
    assert!(
        !producer.contains("report_projection(&telemetry_projection"),
        "copy-in producer must route through the typed Host facade"
    );

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_box_copy_in("));
    assert!(owner.contains("self.report_projection(&box_copy_in_telemetry(level, metadata))"));

    let mapper = include_str!("../src/extensions/telemetry/box_copy_in_telemetry.rs");
    assert_eq!(
        mapper.matches("sand.box_copy_in").count(),
        1,
        "canonical mapper must be the only fixed event literal in its owner"
    );
    assert!(mapper.contains("event: Some(BOX_COPY_IN_EVENT)"));

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(!coordinator.contains("reportBoxCopyIn"));
    assert!(!coordinator.contains("sand.box_copy_in"));

    let electron = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(!electron.contains("reportBoxCopyIn"));
    assert!(!electron.contains("sand.box_copy_in"));
}

#[test]
fn shipping_copy_in_preserves_frozen_started_watchdog_terminal_and_failure_level_decisions() {
    let producer = include_str!("../src/extensions/box_store_sync/box_copy_in.rs");
    for needle in [
        r#"level: "info""#,
        r#"("outcome".into(), "started".into())"#,
        r#"("reason".into(), "hydrate-started".into())"#,
        r#"level: if progressed_since_last_tick { "info" } else { "warn" }"#,
        r#""moving-data-in-slow-but-advancing""#,
        r#""moving-data-in-exceeded-threshold""#,
        r#"if result.outcome == CopyInOutcome::Failed"#,
        r#""error""#,
        r#""info""#,
        r#""manifest_entries".into()"#,
        r#""store_db_entries".into()"#,
        r#""error_class".into()"#,
        r#""error_summary".into()"#,
        r#""duration_ms".into()"#,
    ] {
        assert!(producer.contains(needle), "missing frozen copy-in producer semantic: {needle}");
    }
    assert!(
        !producer.contains(r#"("hydrate_source".into(), result.hydrate_source.clone().unwrap_or_default())"#),
        "undefined frozen hydrate_source must be omitted rather than serialized as an empty string"
    );
    assert!(
        !producer.contains(r#"("store_backend".into(), store_backend.to_string())"#),
        "store_backend belongs to structured-log identity tags, not each copy-in event payload"
    );
}
