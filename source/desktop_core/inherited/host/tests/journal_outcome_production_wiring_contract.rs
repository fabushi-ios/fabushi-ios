#[test]
fn shipping_transcript_mirror_reports_journal_outcomes_through_unique_host_owner() {
    const HOST: &str = include_str!("../app/src/main.rs");
    const RUNNER_COMPOSITION: &str = include_str!("../src/host_runner_composition.rs");
    const MIRROR: &str = include_str!("../src/transcript_mirror/transcript_mirror.rs");
    const OWNER: &str = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    const COORDINATOR: &str = include_str!("../../node-agent-coordinator/src/main.rs");
    const ELECTRON: &str = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");

    assert!(HOST.contains("host_runner_composition"));
    assert!(HOST.contains(".compose_production_checkpoint_sink("));
    assert!(HOST.contains("let report = JournalOutcomeReport {"));
    assert!(HOST.contains("journal_logs.report_journal_outcome(&report)"));
    assert!(
        RUNNER_COMPOSITION.contains("ProductionTranscriptMirrorProvider::with_reporter(")
    );
    assert!(RUNNER_COMPOSITION.contains("Arc::clone(&report_outcome)"));
    assert!(MIRROR.contains("report_outcome"));
    assert!(OWNER.contains("pub fn report_journal_outcome(&self, report: &JournalOutcomeReport)"));
    assert!(OWNER.contains("self.report_projection(&journal_outcome_telemetry(report))"));
    assert!(!COORDINATOR.contains("sand.journal.outcome"));
    assert!(!ELECTRON.contains("sand.journal.outcome"));
}
