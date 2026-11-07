use gpui_telegram_agent::{
    tasks::{
        approval::{ApprovalDecision, ApprovalGate, SAND_AUTO_REVIEW_APPROVAL_TTL_MS},
        cron::{compile_cron_matcher, parse_every_interval_ms, CronRoutine},
        fsm::{TaskFsm, TaskState},
    },
};
use gpui_telegram_core::error::TelegramError;
use mahayana_host_runtime::runner::background_work::{
    BackgroundWorkRecord, RevivingBackgroundWorkRegistry,
};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// 1. Task FSM 7-State Transition Matrix (Adversarial Exhaustive 49 Transitions)
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_task_fsm_exhaustive_49_transitions() {
    let all_states = [
        TaskState::Pending,
        TaskState::Running,
        TaskState::WaitingAuth,
        TaskState::WaitingConnection,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ];

    let mut valid_count = 0;
    let mut invalid_count = 0;

    for &from in &all_states {
        for &to in &all_states {
            let mut fsm = TaskFsm::new(format!("task_{:?}_{:?}", from, to));
            // Force initial state directly to test from every state
            fsm.state = from;
            fsm.state_history = vec![(from, 1000)];

            let result = fsm.transition_to(to);

            let is_expected_valid = match (from, to) {
                (TaskState::Pending, TaskState::Running) => true,
                (TaskState::Pending, TaskState::Cancelled) => true,

                (TaskState::Running, TaskState::WaitingAuth) => true,
                (TaskState::Running, TaskState::WaitingConnection) => true,
                (TaskState::Running, TaskState::Completed) => true,
                (TaskState::Running, TaskState::Failed) => true,
                (TaskState::Running, TaskState::Cancelled) => true,

                (TaskState::WaitingAuth, TaskState::Running) => true,
                (TaskState::WaitingAuth, TaskState::Cancelled) => true,

                (TaskState::WaitingConnection, TaskState::Running) => true,
                (TaskState::WaitingConnection, TaskState::Cancelled) => true,

                _ => false,
            };

            if is_expected_valid {
                valid_count += 1;
                assert!(
                    result.is_ok(),
                    "Expected valid transition from {:?} to {:?}, got {:?}",
                    from,
                    to,
                    result
                );
                assert_eq!(fsm.state, to);
                assert_eq!(fsm.state_history.len(), 2);
                assert_eq!(fsm.state_history[1].0, to);
            } else {
                invalid_count += 1;
                assert!(
                    result.is_err(),
                    "Expected invalid transition from {:?} to {:?} to return Err, but it succeeded!",
                    from,
                    to
                );
                match result {
                    Err(TelegramError::Validation(msg)) => {
                        assert!(msg.contains("Invalid task state transition"));
                    }
                    other => panic!(
                        "Expected TelegramError::Validation for {:?} -> {:?}, got {:?}",
                        from, to, other
                    ),
                }
                // State must NOT have changed
                assert_eq!(fsm.state, from);
                // History must NOT have grown
                assert_eq!(fsm.state_history.len(), 1);
            }
        }
    }

    assert_eq!(valid_count, 11, "Must have exactly 11 valid transitions");
    assert_eq!(invalid_count, 38, "Must have exactly 38 invalid transitions");
}

#[test]
fn test_adversarial_task_fsm_terminal_immutability() {
    let terminal_states = [TaskState::Completed, TaskState::Failed, TaskState::Cancelled];
    let all_states = [
        TaskState::Pending,
        TaskState::Running,
        TaskState::WaitingAuth,
        TaskState::WaitingConnection,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
    ];

    for &term in &terminal_states {
        let mut fsm = TaskFsm::new(format!("term_{:?}", term));
        fsm.state = term;
        assert!(fsm.is_terminal());
        assert!(!fsm.is_active());

        for &target in &all_states {
            let res = fsm.transition_to(target);
            assert!(
                res.is_err(),
                "Terminal state {:?} must NOT transition to {:?}",
                term,
                target
            );
            assert_eq!(fsm.state, term);
        }
    }
}

#[test]
fn test_adversarial_task_fsm_update_from_malformed_record() {
    let mut fsm = TaskFsm::new("fsm_record_test".into());
    assert_eq!(fsm.state, TaskState::Pending);

    // Record with invalid/unknown state
    let malformed_record = BackgroundWorkRecord {
        id: "fsm_record_test".into(),
        kind: "agent_task".into(),
        state: "unknown_bogus_state".into(),
        owner_id: None,
        metadata: None,
        abort: None,
    };
    // Should be safely ignored or handled without panic
    let update_res = fsm.update_from_background_record(&malformed_record);
    assert!(update_res.is_ok());
    assert_eq!(fsm.state, TaskState::Pending);

    // Constructing from malformed record returns None
    assert!(TaskFsm::from_background_record(&malformed_record).is_none());

    // Record trying an illegal transition (Pending -> Completed directly)
    let illegal_record = BackgroundWorkRecord {
        id: "fsm_record_test".into(),
        kind: "agent_task".into(),
        state: "completed".into(),
        owner_id: None,
        metadata: None,
        abort: None,
    };
    let illegal_res = fsm.update_from_background_record(&illegal_record);
    assert!(
        illegal_res.is_err(),
        "Illegal transition via background record must return error"
    );
    assert_eq!(fsm.state, TaskState::Pending);
}

// ---------------------------------------------------------------------------
// 2. ApprovalGate 10-Minute Timeout & Boundary Stress Testing
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_approval_gate_exact_millisecond_boundaries() {
    assert_eq!(SAND_AUTO_REVIEW_APPROVAL_TTL_MS, 600_000);

    // Subcase A: 599,999 ms (1ms before expiration)
    {
        let mut gate = ApprovalGate::new(
            "gate_edge_599999".into(),
            "task_edge".into(),
            "bash".into(),
            "echo 1".into(),
        );
        let t_before = gate.created_at_ms + 599_999;
        let expired = gate.check_expiration(t_before);
        assert!(
            !expired,
            "At 599,999ms, gate must NOT be expired (1ms remains)"
        );
        assert_eq!(gate.decision, None);
        assert!(!gate.is_expired());
        // Remaining seconds is 0 because integer division: (600000 - 599999)/1000 = 0
        assert_eq!(gate.remaining_seconds(t_before), 0);
    }

    // Subcase B: Exactly 600,000 ms (exact timeout boundary)
    {
        let mut gate = ApprovalGate::new(
            "gate_edge_600000".into(),
            "task_edge".into(),
            "bash".into(),
            "echo 2".into(),
        );
        let t_exact = gate.created_at_ms + 600_000;
        let expired = gate.check_expiration(t_exact);
        assert!(
            expired,
            "At exactly 600,000ms, gate MUST expire"
        );
        assert_eq!(gate.decision, Some(ApprovalDecision::Timeout));
        assert!(gate.is_expired());
        assert!(!gate.is_approved());
        assert!(!gate.is_denied());
        assert_eq!(gate.remaining_seconds(t_exact), 0);

        // Subsequent check_expiration call on already expired gate should return false
        assert!(
            !gate.check_expiration(t_exact + 1_000),
            "check_expiration should return false if decision is already set"
        );
    }

    // Subcase C: 600,001 ms (1ms past expiration)
    {
        let mut gate = ApprovalGate::new(
            "gate_edge_600001".into(),
            "task_edge".into(),
            "bash".into(),
            "echo 3".into(),
        );
        let t_after = gate.created_at_ms + 600_001;
        let expired = gate.check_expiration(t_after);
        assert!(
            expired,
            "At 600,001ms, gate MUST be expired"
        );
        assert_eq!(gate.decision, Some(ApprovalDecision::Timeout));
        assert!(gate.is_expired());
        assert_eq!(gate.remaining_seconds(t_after), 0);
    }
}

// ---------------------------------------------------------------------------
// 3. ApprovalGate Re-Decision Flaw Probe (Empirical Bug Demonstration)
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_approval_gate_redecision_behavior() {
    // Probe 1: Re-decision after Timeout
    let mut timeout_gate = ApprovalGate::new(
        "gate_redecision_timeout".into(),
        "task_sec_01".into(),
        "shell_exec".into(),
        "cat /etc/passwd".into(),
    );
    let expired_at = timeout_gate.created_at_ms + 600_000;
    assert!(timeout_gate.check_expiration(expired_at));
    assert!(timeout_gate.is_expired());

    // Adversarial action: Can an expired gate be revived and approved via decide()?
    let res1 = timeout_gate.decide(ApprovalDecision::ApproveOnce);
    assert!(res1.is_err(), "Re-decision after Timeout must return Err");
    assert!(timeout_gate.is_expired());
    assert!(!timeout_gate.is_approved());

    // Probe 2: Re-decision after explicit Deny
    let mut deny_gate = ApprovalGate::new(
        "gate_redecision_deny".into(),
        "task_sec_02".into(),
        "destructive_format".into(),
        "mkfs.ext4 /dev/sda".into(),
    );
    assert!(deny_gate.decide(ApprovalDecision::Deny {
        reason: Some("Security Policy Violation".into()),
    }).is_ok());
    assert!(deny_gate.is_denied());

    // Adversarial action: Can a Denied gate be overridden to ApproveOnce?
    let res2 = deny_gate.decide(ApprovalDecision::ApproveOnce);
    assert!(res2.is_err(), "Re-decision after Deny must return Err");
    assert!(deny_gate.is_denied());
    assert!(!deny_gate.is_approved());
}

// ---------------------------------------------------------------------------
// 4. 5-Field Cron Parsing Edge Cases & Fuzzing
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_cron_5_field_edge_cases_and_fuzzing() {
    // Valid complex patterns
    let valid_expressions = [
        "*/15 * * * *",              // 15-minute step
        "0 9 * * 1-5",               // Weekdays at 9am
        "1,15,30,45 * * * *",        // Minute comma list
        "0 0 1,15 * *",              // DOM comma list
        "*/5 8-18 * * 1-5",          // Multi-step & range combo
        "0 0 1 1-12/3 *",            // Month range with step
        "0 0 * * 0",                 // Sunday as 0
        "0 0 * * 7",                 // Sunday as 7 (mapped to 0)
        "CRON_TZ=UTC 0 12 * * *",    // TZ prefix
        "TZ=Europe/London 0 12 * * *", // Short TZ prefix
        "@hourly",
        "@daily",
        "@midnight",
        "@weekly",
        "@monthly",
        "@yearly",
        "@annually",
        "@every 15s",
        "@every 10m",
        "@every 24h",
        "@every 7d",
    ];

    for expr in &valid_expressions {
        let routine = CronRoutine::new("fuzz_valid".into(), expr.to_string());
        assert!(
            routine.validate_expression(),
            "Expected '{}' to be valid",
            expr
        );
    }

    // Invalid expressions (must be rejected)
    let invalid_expressions = [
        "* * * *",                  // 4 fields (too few)
        "* * * * * *",              // 6 fields (too many)
        "* * *",                    // 3 fields
        "",                         // empty string
        "   ",                      // whitespace only
        "60 * * * *",               // minute 60 (out of 0-59)
        "* 24 * * *",               // hour 24 (out of 0-23)
        "* * 0 * *",                // day of month 0 (out of 1-31)
        "* * 32 * *",               // day of month 32 (out of 1-31)
        "* * * 0 *",                // month 0 (out of 1-12)
        "* * * 13 *",               // month 13 (out of 1-12)
        "* * * * 8",                // day of week 8 (out of 0-7)
        "*/0 * * * *",              // step of 0
        "1-5/0 * * * *",            // range with step 0
        "5-1 * * * *",              // reversed range start > end
        "*/15/2 * * * *",           // multiple slashes
        "foo * * * *",              // non-numeric token
        "* bar * * *",              // non-numeric token
        "@every",                   // missing interval
        "@every 0s",                // 0 duration
        "@every -5m",               // negative duration
        "@every 10x",               // unknown unit
        "@every 10 m",              // spaces in duration
        "@every 10m extra",         // extra tokens
    ];

    for expr in &invalid_expressions {
        let routine = CronRoutine::new("fuzz_invalid".into(), expr.to_string());
        assert!(
            !routine.validate_expression(),
            "Expected invalid expression '{}' to be rejected",
            expr
        );
        assert!(
            compile_cron_matcher(expr).is_none(),
            "compile_cron_matcher must return None for '{}'",
            expr
        );
        assert!(
            parse_every_interval_ms(expr).is_none(),
            "parse_every_interval_ms must return None for '{}'",
            expr
        );
    }
}

// ---------------------------------------------------------------------------
// 5. High Concurrency: 100+ Concurrent Task FSM Transitions & Approval Requests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_adversarial_high_concurrency_128_tasks() {
    const TASK_COUNT: usize = 128;
    let registry = Arc::new(Mutex::new(RevivingBackgroundWorkRegistry::default()));

    let mut handles = Vec::with_capacity(TASK_COUNT);

    for i in 0..TASK_COUNT {
        let reg = Arc::clone(&registry);
        let task_id = format!("concurrent_task_{:03}", i);

        handles.push(tokio::spawn(async move {
            let mut fsm = TaskFsm::new(task_id);

            // 1. Pending -> Running
            fsm.transition_to(TaskState::Running).unwrap();
            {
                let mut guard = reg.lock().unwrap();
                fsm.sync_to_background_registry(&mut guard);
            }

            // Yield to test interleaving
            tokio::task::yield_now().await;

            // 2. Running -> WaitingAuth
            fsm.transition_to(TaskState::WaitingAuth).unwrap();
            {
                let mut guard = reg.lock().unwrap();
                fsm.sync_to_background_registry(&mut guard);
            }

            // 3. User Approval Simulation
            let mut gate = ApprovalGate::new(
                format!("gate_{}", fsm.task_id),
                fsm.task_id.clone(),
                "execute_action".into(),
                "args".into(),
            );
            gate.decide(ApprovalDecision::ApproveOnce).unwrap();
            assert!(gate.is_approved());

            tokio::task::yield_now().await;

            // 4. WaitingAuth -> Running
            fsm.transition_to(TaskState::Running).unwrap();
            {
                let mut guard = reg.lock().unwrap();
                fsm.sync_to_background_registry(&mut guard);
            }

            // 5. Running -> Completed
            fsm.transition_to(TaskState::Completed).unwrap();
            {
                let mut guard = reg.lock().unwrap();
                fsm.sync_to_background_registry(&mut guard);
            }

            assert!(fsm.is_terminal());
            assert_eq!(fsm.state_history.len(), 5);
        }));
    }

    for handle in handles {
        handle.await.expect("Task worker thread panicked!");
    }

    // Verify all 128 records registered in background registry with state "completed"
    let guard = registry.lock().unwrap();
    let works = guard.list_work(None);
    assert_eq!(works.len(), TASK_COUNT);

    for record in works {
        assert_eq!(
            record.state, "completed",
            "Task {} failed to end in completed state",
            record.id
        );
    }
}

#[tokio::test]
async fn test_adversarial_high_concurrency_128_approval_gates() {
    const GATE_COUNT: usize = 128;
    let mut handles = Vec::with_capacity(GATE_COUNT);

    for i in 0..GATE_COUNT {
        handles.push(tokio::spawn(async move {
            let mut gate = ApprovalGate::new(
                format!("gate_conc_{:03}", i),
                format!("task_conc_{:03}", i),
                "network_request".into(),
                format!("target_host_{}", i),
            );

            // Alternating patterns:
            if i % 3 == 0 {
                // Expire at exact boundary
                let expire_time = gate.created_at_ms + 600_000;
                assert!(gate.check_expiration(expire_time));
                assert!(gate.is_expired());
            } else if i % 3 == 1 {
                // ApproveOnce
                gate.decide(ApprovalDecision::ApproveOnce).unwrap();
                assert!(gate.is_approved());
            } else {
                // Deny
                gate.decide(ApprovalDecision::Deny {
                    reason: Some("Manual user rejection".into()),
                }).unwrap();
                assert!(gate.is_denied());
            }
        }));
    }

    for handle in handles {
        handle.await.expect("Approval worker thread panicked!");
    }
}
