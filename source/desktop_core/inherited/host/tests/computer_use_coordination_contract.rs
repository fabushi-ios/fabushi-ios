use mahayana_host_runtime::runner::TurnUsage;
use mahayana_host_runtime::runner::computer_use::{
    ComputerUseCoordination, ComputerUsePrewarmStage, PreparationState,
};

#[test]
fn single_desktop_control_lease_is_reentrant_exclusive_and_generation_fenced() {
    let mut coordination = ComputerUseCoordination::new(true);
    let lease_a = coordination.acquire_control_lease("a").expect("lease a");
    assert_eq!(lease_a.owner_agent_id(), "a");
    assert_eq!(lease_a.window_index(), 1);
    assert_eq!(coordination.acquire_control_lease("a"), Some(lease_a.clone()));
    assert!(coordination.owns_control_lease(&lease_a));
    assert!(coordination.acquire_control_lease("b").is_none());

    coordination.begin_preparation("a");
    assert_eq!(coordination.preparation_for("a"), Some(PreparationState::Pending));
    coordination.mark_preparation_ready("a");
    assert_eq!(coordination.preparation_for("a"), Some(PreparationState::Ready));

    assert!(coordination.release_control_lease(&lease_a));
    assert!(!coordination.owns_control_lease(&lease_a));
    assert_eq!(coordination.preparation_for("a"), None);

    let lease_b = coordination.acquire_control_lease("b").expect("lease b");
    assert!(lease_b.generation() > lease_a.generation());
    assert!(!coordination.release_control_lease(&lease_a));
    assert!(coordination.owns_control_lease(&lease_b));
}

#[test]
fn prewarm_failures_are_fail_soft_but_preserve_stage_and_error_class() {
    let mut coordination = ComputerUseCoordination::new(true);
    coordination.begin_preparation("worker");
    coordination.mark_preparation_failed(
        "worker",
        ComputerUsePrewarmStage::Browser,
        "RemoteBrowserPrepareError",
    );

    assert_eq!(
        coordination.preparation_for("worker"),
        Some(PreparationState::Failed)
    );
    let diagnostic = &coordination.diagnostics()[0];
    assert_eq!(diagnostic.kind, "computer_use_prewarm_skipped");
    assert_eq!(diagnostic.stage, ComputerUsePrewarmStage::Browser);
    assert_eq!(diagnostic.error_class, "RemoteBrowserPrepareError");
}

#[test]
fn action_audit_usage_and_model_projection_match_frozen_coordination() {
    let mut coordination = ComputerUseCoordination::new(true);
    coordination.record_audit_intent("screenshot");
    coordination.record_audit_intent("screenshot");
    coordination.record_audit_intent("click");
    coordination.record_audit_intent("unsupported");
    assert_eq!(coordination.audit_action_counts().get("screenshot"), Some(&2));
    assert_eq!(coordination.audit_action_counts().get("click"), Some(&1));
    assert_eq!(coordination.audit_action_counts().len(), 2);

    coordination.record_turn_ended(Some(TurnUsage {
        input_tokens: 10,
        output_tokens: 2,
        cache_read_tokens: 3,
        cache_write_tokens: 0,
        reasoning_tokens: Some(1),
    }));
    coordination.record_turn_ended(Some(TurnUsage {
        input_tokens: 5,
        output_tokens: 7,
        cache_read_tokens: 0,
        cache_write_tokens: 4,
        reasoning_tokens: None,
    }));
    coordination.record_model_id("model-a");
    let one = coordination.usage_snapshot();
    assert_eq!(one.model_id.as_deref(), Some("model-a"));
    assert_eq!(one.turn_ended_count, 2);
    let usage = one.usage.expect("usage");
    assert_eq!(usage.input_tokens, 15);
    assert_eq!(usage.output_tokens, 9);
    assert_eq!(usage.cache_read_tokens, 3);
    assert_eq!(usage.cache_write_tokens, 4);
    assert_eq!(usage.reasoning_tokens, Some(1));

    coordination.record_model_id("model-b");
    assert_eq!(coordination.usage_snapshot().model_id.as_deref(), Some("mixed"));
}

#[test]
fn navigation_probe_is_lazy_and_absent_without_auditor() {
    let mut disabled = ComputerUseCoordination::new(false);
    assert!(!disabled.get_or_create_navigation_probe());
    assert!(!disabled.has_navigation_probe());
    disabled.record_audit_intent("click");
    assert!(disabled.audit_action_counts().is_empty());

    let mut enabled = ComputerUseCoordination::new(true);
    assert!(!enabled.has_navigation_probe());
    assert!(enabled.get_or_create_navigation_probe());
    assert!(enabled.has_navigation_probe());
}

#[test]
fn navigation_probe_baseline_dedupes_urls_and_coalesces_trailing_requests() {
    let mut coordination = ComputerUseCoordination::new(true);
    coordination.capture_navigation_baseline(
        r#"[{"type":"page","id":"page-1","url":"https://example.com/start?x=1","title":"Start"}]"#,
    );
    assert!(coordination.has_navigation_probe());
    let first = coordination.request_navigation_probe().expect("first probe");
    let superseding = coordination.request_navigation_probe().expect("superseding probe");
    assert!(coordination.navigation_probe_wait_ms(first, 100).is_none());
    assert_eq!(coordination.navigation_probe_wait_ms(superseding, 100), Some(0));
    let records = coordination.complete_navigation_probe(
        superseding,
        r#"[{"type":"page","id":"page-1","url":"https://example.com/next?q=2","title":"Next"}]"#,
        "agent-1",
        Some("turn-1"),
        100,
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action["kind"], "browserNavigation");
    assert_eq!(records[0].action["url"], "https://example.com/next");
    assert_eq!(records[0].action["pageTitle"], "Next");
    let later = coordination.request_navigation_probe().expect("later probe");
    assert_eq!(coordination.navigation_probe_wait_ms(later, 101), Some(1999));
    assert!(coordination.complete_navigation_probe(
        later,
        r#"[{"type":"page","id":"page-1","url":"https://example.com/next","title":"Next"}]"#,
        "agent-1",
        Some("turn-1"),
        2100,
    ).is_empty());
}
