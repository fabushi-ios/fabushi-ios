use std::sync::{Arc, Mutex};

use mahayana_host_runtime::host_invariant::{
    SAND_INVARIANT_VIOLATION_NAME, SandInvariantReport, install_invariant_reporter,
    invariant_failure,
};

#[test]
fn host_invariant_producer_reports_before_panicking() {
    let observed = Arc::new(Mutex::new(Vec::<SandInvariantReport>::new()));
    let sink = Arc::clone(&observed);
    let _installed = install_invariant_reporter(Arc::new(move |report| {
        sink.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(report.clone());
    }));

    let outcome = std::panic::catch_unwind(|| invariant_failure());
    assert!(outcome.is_err(), "frozen invariant still terminates the failing path");

    let reports = observed
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].name, SAND_INVARIANT_VIOLATION_NAME);
    assert!(
        reports[0]
            .frame
            .as_deref()
            .is_some_and(|frame| frame.contains("invariant_violation_production_wiring_contract.rs")),
        "producer keeps the application frame even though the structured facade intentionally drops it"
    );
}

#[test]
fn shipping_host_installs_shared_invariant_producer_and_keeps_unique_structured_owner() {
    let app = include_str!("../app/src/main.rs");
    assert!(app.contains(
        "let _invariant_reporter = install_invariant_reporter(Arc::new(move |report|"
    ));
    assert!(app.contains("invariant_logs.report_invariant_violation(report)"));
    assert_eq!(
        app.matches("_ => invariant_failure(),").count(),
        2,
        "the two shipping impossible gateway branches must use the shared invariant producer"
    );

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains(
        "pub fn report_invariant_violation(&self, report: &SandInvariantReport)"
    ));
    assert!(owner.contains("self.report_projection(&invariant_violation_telemetry(report))"));

    let mapper = include_str!("../src/extensions/telemetry/invariant_violation_telemetry.rs");
    assert_eq!(mapper.matches("sand.host.invariant_violation").count(), 1);
    assert!(mapper.contains("level: Some(\"error\")"));
    assert!(mapper.contains("(\"name\".into(), report.name.clone())"));
    assert!(!mapper.contains("report.frame"));

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(!coordinator.contains("reportInvariantViolation"));
    assert!(!coordinator.contains("report_invariant_violation"));
    assert!(!coordinator.contains("sand.host.invariant_violation"));

    let electron =
        include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(!electron.contains("reportInvariantViolation"));
    assert!(!electron.contains("sand.host.invariant_violation"));

    let daemon = include_str!("../../local-exec-daemon/invariant-violation-log.ts");
    assert!(daemon.contains("sand.local_exec_daemon.invariant_violation"));
    assert!(!daemon.contains("sand.host.invariant_violation"));
}
