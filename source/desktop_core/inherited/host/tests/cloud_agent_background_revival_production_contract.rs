const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");

#[test]
fn shipping_cloud_agent_tool_arms_the_shared_background_watcher() {
    assert!(
        SHIPPING_HOST.contains("watch: Some(cloud_agent_watch)"),
        "production CloudAgent must not ship with watch: None"
    );
    assert!(SHIPPING_HOST.contains("RunnerCloudAgentWatches::new("));
    assert!(SHIPPING_HOST.contains("watch_cloud_agent("));
    assert!(SHIPPING_HOST.contains("CloudAgentWatchOptions::new("));
}

#[test]
fn cloud_agent_watch_persists_and_revives_through_the_frozen_runtime_owners() {
    assert!(SHIPPING_HOST.contains("store.mark_pending(DurablePendingWakeMarker"));
    assert!(SHIPPING_HOST.contains("kind: PendingWakeKind::CloudAgent"));
    assert!(SHIPPING_HOST.contains("CompletionRevivals::new("));
    assert!(SHIPPING_HOST.contains("handle_background_subagent_completion(SubagentCompletion"));
    assert!(SHIPPING_HOST.contains("subagent_type: \"cursor-agent\".into()"));
    assert!(SHIPPING_HOST.contains("run_local_background_revival_turn("));
    assert!(SHIPPING_HOST.contains("\"requestSource\": source"));
    assert!(SHIPPING_HOST.contains("\"hidden\": true"));
}

#[test]
fn background_revival_clears_durable_wakes_and_emits_async_task_projection() {
    assert!(SHIPPING_HOST.contains("store.clear_if_matches(&marker)"));
    assert!(SHIPPING_HOST.contains("clear_pending_wake_by_key"));
    assert!(SHIPPING_HOST.contains("emit_async_tasks_for_agent(agent_id)"));
    assert!(SHIPPING_HOST.contains("\"channel\": \"async-tasks\""));
}
