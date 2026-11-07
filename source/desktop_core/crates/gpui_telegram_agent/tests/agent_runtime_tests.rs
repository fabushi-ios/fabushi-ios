use chrono::{TimeZone, Utc};
use gpui_telegram_agent::{
    acp::{
        AcpRunner, AgentTurnInput, AgentTurnOutput, HostSubagentLineage, SubagentArtifactReceipt,
        SubagentLifecycleStatus, SubagentLineage,
    },
    tasks::{
        approval::{ApprovalDecision, ApprovalGate, SAND_AUTO_REVIEW_APPROVAL_TTL_MS},
        cron::{
            compile_cron_matcher, parse_every_interval_ms, CronExecutionRecord, CronRoutine,
        },
        fsm::{TaskFsm, TaskState},
    },
};
use mahayana_host_runtime::runner::background_work::RevivingBackgroundWorkRegistry;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[tokio::test]
async fn test_acp_runner_turn_execution_and_streaming() {
    let delta_called = Arc::new(AtomicBool::new(false));
    let delta_flag = delta_called.clone();

    let runner = AcpRunner::new("grok-3").with_text_delta_callback(Arc::new(
        move |_step, _delta| {
            delta_flag.store(true, Ordering::SeqCst);
        },
    ));

    let input = AgentTurnInput {
        session_id: "session-42".to_string(),
        prompt: "Analyze the Telegram agent architecture".to_string(),
        parent_turn_id: None,
        model_id: Some("grok-3".to_string()),
        thinking_budget_tokens: Some(32_000),
    };

    // Test static projection generation
    let static_config = runner.build_static_config(&input);
    assert_eq!(static_config.model_id, "grok-3");
    assert_eq!(static_config.conversation_id, "session-42");
    assert_eq!(static_config.agent_token_limit, 200_000);
    assert!(!static_config.is_subagent_runner);

    let projection = runner.build_static_projection(&input);
    assert_eq!(projection.agent_token_limit, 200_000);

    // Test stream execution
    let mut rx = runner
        .execute_turn_stream(input.clone())
        .await
        .expect("Streaming should initiate");
    let mut received = Vec::new();

    while let Some(out) = rx.recv().await {
        received.push(out);
    }

    assert_eq!(received.len(), 2);
    match &received[0] {
        AgentTurnOutput::Thinking { token, .. } => {
            assert!(token.contains("grok-3"));
            assert!(token.contains("session-42"));
        }
        _ => panic!("Expected first event to be Thinking"),
    }

    match &received[1] {
        AgentTurnOutput::FinalResponse { text } => {
            assert!(
                text.contains("Analyze the Telegram agent architecture"),
                "Final response should contain substance of user prompt"
            );
            assert!(text.contains("Autonomous Agent Response"));
        }
        _ => panic!("Expected second event to be FinalResponse"),
    }

    assert!(delta_called.load(Ordering::SeqCst));

    // Test synchronous helper
    let sync_outputs = AcpRunner::execute_turn(input)
        .await
        .expect("Sync turn execution should succeed");
    assert_eq!(sync_outputs.len(), 2);
}

#[test]
fn test_subagent_lineage_hierarchy_and_artifacts() {
    let mut lineage = SubagentLineage::new();

    // 1. Add Root Agent
    lineage.add_agent("root_agent".into(), None, "orchestrator".into());

    // 2. Add Child Subagent using HostSubagentLineage integration
    let host_lineage = HostSubagentLineage {
        parent_request_id: Some("root_agent".into()),
        root_parent_request_id: Some("root_agent".into()),
        parent_agent_tool_call_id: Some("call_tool_001".into()),
    };
    lineage.add_subagent_with_lineage(
        "worker_m4".into(),
        &host_lineage,
        "implementer".into(),
    );

    // 3. Add Grandchild Subagent
    lineage.add_agent(
        "sub_worker_1".into(),
        Some("worker_m4".into()),
        "qa".into(),
    );

    // Verify root finding
    assert_eq!(lineage.find_root_id("sub_worker_1"), "root_agent");
    assert_eq!(lineage.find_root_id("worker_m4"), "root_agent");
    assert_eq!(lineage.find_root_id("root_agent"), "root_agent");

    // Verify HostSubagentLineage roundtrip
    let retrieved_host_lineage = lineage
        .to_host_lineage("worker_m4")
        .expect("Should produce HostSubagentLineage");
    assert_eq!(
        retrieved_host_lineage.parent_request_id.as_deref(),
        Some("root_agent")
    );
    assert_eq!(
        retrieved_host_lineage.root_parent_request_id.as_deref(),
        Some("root_agent")
    );
    assert_eq!(
        retrieved_host_lineage.parent_agent_tool_call_id.as_deref(),
        Some("call_tool_001")
    );

    // Increment tool calls and attach artifact receipt
    lineage.increment_tool_calls("worker_m4");
    lineage.increment_tool_calls("worker_m4");
    assert_eq!(lineage.get_node("worker_m4").unwrap().tool_call_count, 2);

    let receipt = SubagentArtifactReceipt {
        receipt_id: "rcpt_001".into(),
        title: "Test Coverage Report".into(),
        path: Some("reports/coverage.html".into()),
        summary: "100% tests passed".into(),
        created_at_ms: Utc::now().timestamp_millis() as u64,
    };
    lineage.add_artifact("worker_m4", receipt.clone());
    assert_eq!(
        lineage.get_node("worker_m4").unwrap().artifacts,
        vec![receipt]
    );

    // Mark completion
    lineage.mark_completed("worker_m4", SubagentLifecycleStatus::Done);
    assert_eq!(
        lineage.get_node("worker_m4").unwrap().status,
        SubagentLifecycleStatus::Done
    );
    assert!(lineage.get_node("worker_m4").unwrap().completed_at_ms.is_some());

    // Build hierarchical execution tree
    let tree = lineage.build_execution_tree();
    assert_eq!(tree.len(), 1); // 1 root agent
    assert_eq!(tree[0].node.agent_id, "root_agent");
    assert_eq!(tree[0].children.len(), 1); // worker_m4
    assert_eq!(tree[0].children[0].node.agent_id, "worker_m4");
    assert_eq!(tree[0].children[0].children.len(), 1); // sub_worker_1
    assert_eq!(tree[0].children[0].children[0].node.agent_id, "sub_worker_1");
}

#[test]
fn test_task_fsm_7_state_transitions_and_validation() {
    let mut fsm = TaskFsm::new("task_abc".into());
    assert_eq!(fsm.state, TaskState::Pending);
    assert!(!fsm.is_terminal());
    assert!(!fsm.is_active());

    // Pending -> Running
    assert!(fsm.transition_to(TaskState::Running).is_ok());
    assert_eq!(fsm.state, TaskState::Running);
    assert!(fsm.is_active());
    assert!(!fsm.is_terminal());

    // Running -> WaitingAuth
    assert!(fsm.transition_to(TaskState::WaitingAuth).is_ok());
    assert_eq!(fsm.state, TaskState::WaitingAuth);
    assert!(fsm.is_active());

    // WaitingAuth -> Running (User Approved)
    assert!(fsm.transition_to(TaskState::Running).is_ok());
    assert_eq!(fsm.state, TaskState::Running);

    // Running -> WaitingConnection (Network Drop)
    assert!(fsm.transition_to(TaskState::WaitingConnection).is_ok());
    assert_eq!(fsm.state, TaskState::WaitingConnection);
    assert!(fsm.is_active());

    // WaitingConnection -> Running (Reconnected)
    assert!(fsm.transition_to(TaskState::Running).is_ok());
    assert_eq!(fsm.state, TaskState::Running);

    // Running -> Completed (Terminal)
    assert!(fsm.transition_to(TaskState::Completed).is_ok());
    assert_eq!(fsm.state, TaskState::Completed);
    assert!(fsm.is_terminal());
    assert!(!fsm.is_active());

    // Completed -> Running (Invalid: Terminal cannot transition)
    let invalid = fsm.transition_to(TaskState::Running);
    assert!(invalid.is_err());
    assert_eq!(fsm.state, TaskState::Completed);

    // Test cancellation transitions
    let mut fsm_cancel_pending = TaskFsm::new("task_pend".into());
    assert!(fsm_cancel_pending.transition_to(TaskState::Cancelled).is_ok());
    assert!(fsm_cancel_pending.is_terminal());

    let mut fsm_cancel_auth = TaskFsm::new("task_auth".into());
    fsm_auth_setup(&mut fsm_cancel_auth);
    assert_eq!(fsm_cancel_auth.state, TaskState::WaitingAuth);
    assert!(fsm_cancel_auth.transition_to(TaskState::Cancelled).is_ok());
    assert!(fsm_cancel_auth.is_terminal());

    let mut fsm_fail = TaskFsm::new("task_fail".into());
    assert!(fsm_fail.transition_to(TaskState::Running).is_ok());
    assert!(fsm_fail.transition_to(TaskState::Failed).is_ok());
    assert!(fsm_fail.is_terminal());

    // Abort helper
    let mut fsm_abort = TaskFsm::new("task_ab".into());
    assert!(fsm_abort.transition_to(TaskState::Running).is_ok());
    assert!(fsm_abort.abort("User cancelled from UI").is_ok());
    assert_eq!(fsm_abort.state, TaskState::Cancelled);
    assert_eq!(
        fsm_abort.abort_reason.as_deref(),
        Some("User cancelled from UI")
    );
}

fn fsm_auth_setup(fsm: &mut TaskFsm) {
    let _ = fsm.transition_to(TaskState::Running);
    let _ = fsm.transition_to(TaskState::WaitingAuth);
}

#[test]
fn test_task_fsm_driving_reviving_background_registry() {
    let mut registry = RevivingBackgroundWorkRegistry::default();
    let mut fsm = TaskFsm::new("bg_task_001".into());

    // Sync initial state
    fsm.sync_to_background_registry(&mut registry);
    let work = registry.list_work(None);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].id, "bg_task_001");
    assert_eq!(work[0].state, "pending");

    // Transition to Running and sync
    fsm.transition_to(TaskState::Running).unwrap();
    fsm.sync_to_background_registry(&mut registry);
    assert!(registry.has_running_work(None));

    // Transition to WaitingAuth
    fsm.transition_to(TaskState::WaitingAuth).unwrap();
    fsm.sync_to_background_registry(&mut registry);
    let work = registry.list_work(None);
    assert_eq!(work[0].state, "waiting_auth");

    // Reconstruct FSM from BackgroundWorkRecord
    let restored = TaskFsm::from_background_record(work[0]).expect("Should deserialize");
    assert_eq!(restored.task_id, "bg_task_001");
    assert_eq!(restored.state, TaskState::WaitingAuth);
}

#[test]
fn test_cron_5_field_parsing_and_schedules() {
    // Valid 5-field cron parsing
    assert!(compile_cron_matcher("*/15 * * * *").is_some());
    assert!(compile_cron_matcher("0 9 * * 1-5").is_some());
    assert!(compile_cron_matcher("@daily").is_some());
    assert!(compile_cron_matcher("CRON_TZ=America/New_York 0 9 * * 1-5").is_some());

    // Interval parsing
    assert_eq!(parse_every_interval_ms("@every 2h"), Some(7_200_000));
    assert!(CronRoutine::new("interval_job".into(), "@every 2h".into()).validate_expression());

    // Invalid expressions rejected
    assert!(compile_cron_matcher("* * * *").is_none()); // 4 fields
    assert!(compile_cron_matcher("60 * * * *").is_none()); // minute 60 > 59
    assert!(compile_cron_matcher("* 25 * * *").is_none()); // hour 25 > 23

    let mut routine = CronRoutine::new("cleanup_job".into(), "0 2 * * *".into())
        .with_name("Nightly Cleanup")
        .with_time_zone(Some("UTC".into()));

    assert!(routine.validate_expression());
    assert!(routine.compile().is_some());

    // Compute next run after 2026-11-06 01:00:00 UTC
    let baseline = Utc.with_ymd_and_hms(2026, 11, 6, 1, 0, 0).unwrap();
    let next_run = routine
        .compute_next_run(baseline)
        .expect("Should compute next run");

    // Expected next run is 2026-11-06 02:00:00 UTC
    let expected = Utc.with_ymd_and_hms(2026, 11, 6, 2, 0, 0).unwrap();
    assert_eq!(next_run, expected);
    assert_eq!(routine.next_run, Some(expected));

    // Preview 7 days
    let summary = routine.preview_next_7_days(baseline);
    assert_eq!(summary.scheduled_fires_next_7_days, 7); // 1 per day

    // Record execution
    let record = CronExecutionRecord {
        run_id: "run_01".into(),
        started_at: baseline,
        finished_at: Some(baseline),
        status: "success".into(),
        output_summary: Some("Cleaned 42 stale files".into()),
    };
    routine.record_execution(record);
    assert_eq!(routine.execution_records.len(), 1);
    assert_eq!(routine.last_run, Some(baseline));

    routine.toggle_enabled(false);
    assert!(!routine.enabled);
}

#[test]
fn test_approval_gate_10_minute_timeout_and_decisions() {
    assert_eq!(SAND_AUTO_REVIEW_APPROVAL_TTL_MS, 600_000); // 10 minutes exact

    let mut gate = ApprovalGate::new(
        "gate_1".into(),
        "task_42".into(),
        "shell_exec".into(),
        "rm -rf /tmp/test".into(),
    );

    assert_eq!(gate.ttl_ms, 600_000);
    assert!(gate.decision.is_none());
    assert!(!gate.is_approved());
    assert!(!gate.is_denied());
    assert!(!gate.is_expired());

    // Check countdown remaining seconds
    let now_ms = gate.created_at_ms + 100_000;
    assert_eq!(gate.remaining_seconds(now_ms), 500);

    // Decision: ApproveOnce
    assert!(gate.decide(ApprovalDecision::ApproveOnce).is_ok());
    assert!(gate.is_approved());
    assert!(!gate.is_denied());

    // Assert re-decision is strictly rejected
    let redecide_err = gate.decide(ApprovalDecision::AlwaysAllow);
    assert!(redecide_err.is_err(), "Re-decision must return Err");

    // Separate gate for AlwaysAllow
    let mut gate_always = ApprovalGate::new(
        "gate_always".into(),
        "task_42_always".into(),
        "shell_exec".into(),
        "ls /tmp".into(),
    );
    assert!(gate_always.decide(ApprovalDecision::AlwaysAllow).is_ok());
    assert!(gate_always.is_approved());
    assert!(gate_always.decide(ApprovalDecision::Deny { reason: None }).is_err());

    // Separate gate for Deny
    let mut gate_deny = ApprovalGate::new(
        "gate_2".into(),
        "task_43".into(),
        "fs_write".into(),
        "/etc/passwd".into(),
    );
    assert!(gate_deny.decide(ApprovalDecision::Deny {
        reason: Some("Operation too destructive".into()),
    }).is_ok());
    assert!(!gate_deny.is_approved());
    assert!(gate_deny.is_denied());
    assert!(gate_deny.decide(ApprovalDecision::ApproveOnce).is_err());

    // Test 10-Minute Timeout Auto-Expire
    let mut timeout_gate = ApprovalGate::new(
        "gate_2".into(),
        "task_99".into(),
        "network_access".into(),
        "connect to 1.1.1.1".into(),
    );

    // Before 10 minutes (9 minutes elapsed)
    let before_timeout = timeout_gate.created_at_ms + 540_000;
    assert!(!timeout_gate.check_expiration(before_timeout));
    assert!(timeout_gate.decision.is_none());

    // Exactly 10 minutes (600,000 ms elapsed)
    let at_timeout = timeout_gate.created_at_ms + 600_000;
    assert!(timeout_gate.check_expiration(at_timeout));
    assert_eq!(timeout_gate.decision, Some(ApprovalDecision::Timeout));
    assert!(timeout_gate.is_expired());
    assert_eq!(timeout_gate.remaining_seconds(at_timeout), 0);
}

#[tokio::test]
async fn test_integrated_task_approval_and_completion_pipeline() {
    let mut task_fsm = TaskFsm::new("pipeline_task_01".into());
    let mut registry = RevivingBackgroundWorkRegistry::default();

    // 1. Task enters Running
    task_fsm.transition_to(TaskState::Running).unwrap();
    task_fsm.sync_to_background_registry(&mut registry);
    assert_eq!(task_fsm.state, TaskState::Running);

    // 2. Action requires approval -> transitions to WaitingAuth
    task_fsm.transition_to(TaskState::WaitingAuth).unwrap();
    task_fsm.sync_to_background_registry(&mut registry);
    assert_eq!(task_fsm.state, TaskState::WaitingAuth);

    let mut gate = ApprovalGate::new(
        "gate_pipeline".into(),
        task_fsm.task_id.clone(),
        "cloud_sync".into(),
        "sync sensitive records".into(),
    );

    // 3. User Approves action
    gate.decide(ApprovalDecision::ApproveOnce).unwrap();
    assert!(gate.is_approved());

    // 4. Task resumes Running
    task_fsm.transition_to(TaskState::Running).unwrap();
    task_fsm.sync_to_background_registry(&mut registry);
    assert_eq!(task_fsm.state, TaskState::Running);

    // 5. Task Completes
    task_fsm.transition_to(TaskState::Completed).unwrap();
    task_fsm.sync_to_background_registry(&mut registry);
    assert_eq!(task_fsm.state, TaskState::Completed);
    assert!(task_fsm.is_terminal());

    let final_record = registry
        .list_work(None)
        .into_iter()
        .find(|w| w.id == "pipeline_task_01")
        .expect("Work record must exist");
    assert_eq!(final_record.state, "completed");
}
