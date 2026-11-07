const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const TRANSCRIPT_MANAGER: &str = include_str!("../src/extensions/transcript/transcript_manager.rs");
const PENDING_WAKE_REARM: &str = include_str!("../src/extensions/transcript/pending_wake_rearm.rs");

#[test]
fn shipping_host_rearms_durable_cloud_agent_and_lost_subagent_wakes_after_gateway_start() {
    assert!(TRANSCRIPT_MANAGER.contains("PendingWakeRearm::new("));
    assert!(TRANSCRIPT_MANAGER.contains("pub fn rearm_pending_wakes(&self)"));
    assert!(TRANSCRIPT_MANAGER.contains("owner.rearm_pending_wakes();"));
    assert!(PENDING_WAKE_REARM.contains("for marker in store.list_pending()"));
    assert!(PENDING_WAKE_REARM.contains("self.rearm_pending_wake(marker, now, None);"));
    assert!(SHIPPING_HOST.contains(".bind_pending_wake_runtime(Arc::new(ProductionPendingWakeRuntime"));
    assert!(SHIPPING_HOST.contains("transcript_manager.rearm_pending_wakes()"));
    assert!(SHIPPING_HOST.contains("background_shell_watches: Arc<RunnerBackgroundShellWatches>"));
    assert!(SHIPPING_HOST.contains("is_cloud_watch_armed(agent_id, work_id)"));
    assert!(SHIPPING_HOST.contains("CloudAgentWatchOptions::new(quiet_origin, false)"));
    assert!(SHIPPING_HOST.contains("completion_revivals: Arc<CompletionRevivals>"));
    assert!(SHIPPING_HOST.contains("handle_background_subagent_completion(SubagentCompletion"));
}

#[test]
fn startup_rearm_routes_recreate_interrupted_shell_through_completion_revivals() {
    assert!(!SHIPPING_HOST.contains("production shell pending-wake rearm is not wired yet"));
    assert!(SHIPPING_HOST.contains("watch_background_shell("));
    assert!(SHIPPING_HOST.contains("BackgroundShellWatchOptions::new("));
    assert!(!SHIPPING_HOST.contains("production recreate-interrupted shell notice is not wired yet"));
    assert!(SHIPPING_HOST.contains("handle_background_shell_completion(ShellCompletion"));
    assert!(SHIPPING_HOST.contains("shell_id: marker.work_id.clone()"));
    assert!(SHIPPING_HOST.contains("quiet_origin: marker.quiet_origin.clone()"));
    assert!(!SHIPPING_HOST.contains("production lost-subagent pending-wake revival is not wired yet"));
}


#[test]
fn shipping_generated_subagent_dispatch_persists_durable_pending_wake_before_provider_run() {
    let dispatch = SHIPPING_HOST
        .find("if let Ok(Some(pending)) = runner.begin_generated_subagent_with_lifecycle(")
        .expect("shipping generated subagent dispatch must expose its pending wake");
    let provider_run = SHIPPING_HOST
        .find("runner.run_routed_provider_with_projected_turn_input(")
        .expect("shipping generated subagent must execute through the Runner");
    assert!(dispatch < provider_run);
    assert!(SHIPPING_HOST[dispatch..provider_run].contains("&generated_lifecycle_revision,"));
    assert!(SHIPPING_HOST.contains("kind: PendingWakeKind::Subagent"));
    assert!(SHIPPING_HOST.contains("work_id: pending.work_id.clone()"));
    assert!(SHIPPING_HOST.contains("marked_at_ms: pending.marked_at_ms"));
    assert!(SHIPPING_HOST.contains("subagent_type: Some(pending.subagent_type.clone())"));
    assert!(SHIPPING_HOST.contains("marked_at_ms: completion.marked_at_ms as f64"));
    assert!(SHIPPING_HOST.contains("pending_wake_marker: Some(pending_wake_marker)"));
    assert!(SHIPPING_HOST.contains("pending_subagent_wake_persist_failed"));
    assert!(SHIPPING_HOST.contains(
        "publish_async_tasks_changed(\n                            &worker_events,\n                            worker_transcript_runtime.as_ref(),\n                            worker_generated_agent_runtime.as_ref(),\n                            worker_cloud_agent_watches.as_ref(),\n                            background_shell_watches.as_ref(),\n                            parent_agent_id,"
    ));
    assert!(SHIPPING_HOST.contains("handle_background_subagent_completion(SubagentCompletion"));
}
