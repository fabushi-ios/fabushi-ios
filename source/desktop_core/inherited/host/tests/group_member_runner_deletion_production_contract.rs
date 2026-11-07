use std::fs;
use std::sync::Arc;

use mahayana_host_runtime::extensions::transcript::run_scheduler::RunLane;
use mahayana_host_runtime::extensions::transcript::runner_registry::TranscriptRunnerRegistry;
use mahayana_host_runtime::extensions::transcript::send_turn_dispatch::ProductionTurnDispatch;
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedProviderTaskRegistry;

#[test]
fn group_member_registry_is_independent_and_preemption_state_is_consumable() {
    let ordinary_tasks = Arc::new(RoutedProviderTaskRegistry::default());
    let registry = TranscriptRunnerRegistry::new(ordinary_tasks);

    let direct = registry
        .register_routed_provider("agent-a", "turn-1")
        .expect("direct runner");
    let group = registry
        .register_group_member("agent-a", "group-member-1")
        .expect("group member runner");

    assert_eq!(
        registry.active_group_member_stream_ids_for_agent("agent-a"),
        vec!["group-member-1".to_string()]
    );
    assert_eq!(
        registry.active_stream_ids_for_agent("agent-a"),
        vec!["group-member-1".to_string(), "turn-1".to_string()]
    );

    assert_eq!(registry.cancel_agent("agent-a", "direct cancelled"), 1);
    assert!(direct.is_cancelled());
    assert!(!group.is_cancelled());

    assert_eq!(
        registry.preempt_group_member_agent(
            "agent-a",
            "direct user message preempted group member turn",
        ),
        1
    );
    assert!(group.is_cancelled());
    assert!(registry.take_group_member_preempted("agent-a"));
    assert!(!registry.take_group_member_preempted("agent-a"));

    registry.finish_routed_provider("group-member-1");
    let delete_group = registry
        .register_group_member("agent-a", "group-member-delete")
        .expect("delete group runner");
    assert_eq!(
        registry.cancel_group_member_agent("agent-a", "agent deleted"),
        1
    );
    assert!(delete_group.is_cancelled());

    registry.finish_routed_provider("group-member-delete");
    let stale_marker = registry
        .register_group_member("agent-a", "group-member-preempt")
        .expect("preempt marker runner");
    assert_eq!(
        registry.preempt_group_member_agent("agent-a", "direct preempt"),
        1
    );
    assert!(stale_marker.is_cancelled());
    registry.clear_group_member_preempted("agent-a");
    assert!(!registry.take_group_member_preempted("agent-a"));

    registry.finish_routed_provider("turn-1");
    registry.finish_routed_provider("group-member-preempt");
    assert!(registry.active_stream_ids_for_agent("agent-a").is_empty());
}

#[test]
fn shipping_group_member_turn_preemption_and_deletion_use_the_independent_owner() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main = format!(
        "{}\n{}",
        fs::read_to_string(manifest_dir.join("app/src/main.rs")).expect("shipping main"),
        fs::read_to_string(manifest_dir.join("app/src/ocrm_handlers/group_turn.rs"))
            .unwrap_or_default()
    );

    assert!(main.contains(
        "runner_registry.register_group_member(&agent_id, &stream_id)"
    ));
    assert!(main.contains(
        "self.runner_registry.preempt_group_member_agent("
    ));
    assert!(main.contains(
        "deps.runner_registry.take_group_member_preempted(&member_id)"
    ));
    assert!(main.contains(
        "runner_registry.clear_group_member_preempted(agent_id)"
    ));
    assert!(main.contains(
        "runner_registry.cancel_group_member_agent(agent_id, \"agent deleted\")"
    ));
    assert!(main.contains(
        "runner_registry.cancel_agent(agent_id, \"agent deleted\")"
    ));
    assert!(main.contains(
        "runner_registry.active_stream_ids_for_agent(agent_id).is_empty()"
    ));
    assert!(main.contains(
        "transcript_runtime.is_turn_dispatch_idle(agent_id)"
    ));
}

#[test]
fn deletion_drain_tracks_pending_exclusive_turn_dispatch_until_all_lanes_settle() {
    let mut dispatch = ProductionTurnDispatch::with_watchdog(120_000, 30_000);
    let (first, _, first_started) = dispatch
        .enqueue_turn_with_start(
            "agent-a",
            Some("nonce-1"),
            1_000,
            1_000,
            RunLane::User,
            "turn",
            None,
        )
        .expect("first turn");
    let first_generation = first_started.expect("first starts").generation;

    let (second, _, second_started) = dispatch
        .enqueue_turn_with_start(
            "agent-a",
            Some("nonce-2"),
            1_001,
            1_001,
            RunLane::Background,
            "event",
            None,
        )
        .expect("second turn");
    assert!(second_started.is_none(), "second turn must remain pending");
    assert!(!dispatch.is_idle("agent-a"));
    assert_eq!(dispatch.queued_task_ids("agent-a"), vec![second.task_id.clone()]);

    let (_, next) = dispatch.settle_and_start_next(&first, first_generation, 1_002);
    let next = next.expect("pending turn starts after first settles");
    assert_eq!(next.task_id, second.task_id);
    assert!(!dispatch.is_idle("agent-a"));

    let second_generation = dispatch
        .active_generation_for(&second)
        .expect("second generation");
    let (_, after_second) =
        dispatch.settle_and_start_next(&second, second_generation, 1_003);
    assert!(after_second.is_none());
    assert!(dispatch.is_idle("agent-a"));
}
