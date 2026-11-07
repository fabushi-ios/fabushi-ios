const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");

#[test]
fn shipping_host_binds_one_auto_review_controller_per_routed_runner_turn() {
    assert!(SHIPPING_HOST.contains(
        "auto_review.bind_runner_with_session(\n        &agent_id,\n        worker_session_admitted_key.as_deref(),\n        approvals_resolvable,\n    )"
    ));
    assert!(SHIPPING_HOST.contains("auto_review_service.unbind_runner("));
    assert!(SHIPPING_HOST.contains("spawn_error_auto_review.unbind_runner("));
}

#[test]
fn shipping_turn_cancellation_is_bound_to_the_canonical_auto_review_controller() {
    assert!(SHIPPING_HOST.contains(
        "auto_review_controller.set_cancellation_probe(Arc::new(move ||"
    ));
    assert!(SHIPPING_HOST.contains(
        "auto_review_cancellation.is_cancelled()"
    ));
}

#[test]
fn shipping_cloud_agent_tool_uses_live_auto_review_hook() {
    assert!(SHIPPING_HOST.contains("build_cloud_agent_auto_review_hook("));
    assert!(SHIPPING_HOST.contains("review: Some(cloud_agent_review)"));
    assert!(SHIPPING_HOST.contains("create_sand_backend_smart_mode_classifier_executor_with_cancellation("));
    assert!(SHIPPING_HOST.contains("review_sand_cloud_agent_action("));
    assert!(SHIPPING_HOST.contains("review_sand_cloud_agent_lifecycle_action("));
}

#[test]
fn background_turns_do_not_expose_resolvable_approval_cards() {
    assert!(SHIPPING_HOST.contains("Some(\"group-member\" | \"automation\")"));
    assert!(SHIPPING_HOST.contains("let approvals_resolvable = !matches!("));
}


#[test]
fn shipping_auto_review_display_recheck_uses_unique_host_structured_log_owner() {
    assert!(SHIPPING_HOST.contains(
        "set_display_recheck_failed_sink(Arc::new(move |agent_id|"
    ));
    assert!(SHIPPING_HOST.contains(
        ".report_auto_review_display_recheck_failed(agent_id)"
    ));

    const SERVICE: &str =
        include_str!("../src/extensions/auto_review/auto_review_service.rs");
    assert!(SERVICE.contains("Some(Arc::new(move |agent_id|"));
    assert!(SERVICE.contains("sink(agent_id);"));
    assert!(!SHIPPING_HOST.contains(
        "\"sand.auto_review.display_recheck_failed\""
    ));
}
