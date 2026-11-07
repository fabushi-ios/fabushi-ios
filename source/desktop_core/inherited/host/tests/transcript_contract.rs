use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mahayana_host_runtime::extensions::transcript::prompt_acceptance_ledger::{
    AcceptanceLookup, AcceptanceStatus, MAX_RECORDS, PromptAcceptanceError,
    PromptAcceptanceLedger, SendAdmission, SendInput, canonical_send_input,
    send_input_digest,
};
use uuid::Uuid;

fn temp_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("fabushi-host-{name}-{}", Uuid::new_v4()))
}

fn sample_input(prompt: &str) -> SendInput {
    SendInput {
        agent_id: Some("agent-1".into()),
        prompt: prompt.into(),
        rich_text: None,
        reply_to_id: None,
        is_fork: false,
        attachment_paths: vec!["/tmp/a.txt".into()],
        attachment_names: vec!["a.txt".into()],
    }
}

#[test]
fn acceptance_digest_matches_canonical_wire_shape() {
    let input = sample_input("hello");
    assert_eq!(
        canonical_send_input(&input),
        r#"["agent-1","hello",null,null,false,["/tmp/a.txt"],["a.txt"]]"#
    );
    assert_eq!(
        send_input_digest(&input),
        "8609d62307b7aa0b2fef3270029a3b7c414928fc5b03b23eae366dd5a94aaf14"
    );
}

#[test]
fn acceptance_ledger_replays_nonce_outcome_and_rejects_digest_mismatch() {
    let root = temp_dir("acceptance");
    let clock = Arc::new(AtomicU64::new(1_000));
    let now = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    };
    let mut ledger = PromptAcceptanceLedger::with_clock(Some(&root), now);
    let digest = send_input_digest(&sample_input("hello"));

    assert_eq!(
        ledger.admit_send("host", "nonce-1", &digest).unwrap(),
        SendAdmission::Dispatch
    );
    let pending = ledger
        .record_pending(
            "host",
            "nonce-1",
            digest.clone(),
            "agent-1",
            Some("t1u".into()),
        )
        .unwrap();
    assert_eq!(pending.status, AcceptanceStatus::Pending);
    assert!(matches!(
        ledger.admit_send("host", "nonce-1", &digest).unwrap(),
        SendAdmission::Duplicate(record) if record.status == AcceptanceStatus::Pending
    ));

    let mismatch = send_input_digest(&sample_input("different"));
    assert!(matches!(
        ledger.admit_send("host", "nonce-1", &mismatch),
        Err(PromptAcceptanceError::DigestMismatch { .. })
    ));

    ledger.mark_accepted("host", "nonce-1");
    assert!(matches!(
        ledger.lookup("host", "nonce-1"),
        AcceptanceLookup::Found(record) if record.status == AcceptanceStatus::Accepted
    ));
    ledger.clear_unless_accepted("host", "nonce-1");
    assert!(matches!(
        ledger.lookup("host", "nonce-1"),
        AcceptanceLookup::Found(record) if record.status == AcceptanceStatus::Accepted
    ));

    clock.store(1_100, Ordering::SeqCst);
    ledger
        .record_pending("host", "nonce-2", digest.clone(), "agent-1", None)
        .unwrap();
    ledger.mark_rejected("host", "nonce-2", "OFFLINE");
    assert!(matches!(
        ledger.admit_send("host", "nonce-2", &digest),
        Err(PromptAcceptanceError::Rejected { code }) if code == "OFFLINE"
    ));

    ledger.dispose();
    drop(ledger);

    let mut reopened = PromptAcceptanceLedger::new(Some(&root));
    assert!(matches!(
        reopened.lookup("host", "nonce-1"),
        AcceptanceLookup::Found(record)
            if record.status == AcceptanceStatus::Accepted && record.echo_entry_id.as_deref() == Some("t1u")
    ));
    assert!(matches!(
        reopened.lookup("host", "nonce-2"),
        AcceptanceLookup::Found(record)
            if record.status == AcceptanceStatus::Rejected
    ));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn acceptance_ledger_tracks_eviction_and_corrupt_history_as_unknown_durability() {
    let root = temp_dir("gaps");
    let clock = Arc::new(AtomicU64::new(10));
    let now = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    };
    let mut ledger = PromptAcceptanceLedger::with_clock(Some(&root), now);
    let digest = send_input_digest(&sample_input("hello"));

    for index in 0..=MAX_RECORDS {
        clock.store(index as u64 + 10, Ordering::SeqCst);
        ledger
            .record_pending(
                "host",
                format!("nonce-{index}"),
                digest.clone(),
                "agent-1",
                None,
            )
            .unwrap();
        ledger.mark_accepted("host", &format!("nonce-{index}"));
    }
    assert_eq!(ledger.record_count(), MAX_RECORDS);
    assert!(ledger.gaps().eviction_horizon_ms.is_some());
    assert_eq!(
        ledger.lookup("host", "never-seen"),
        AcceptanceLookup::UnknownDurability
    );
    ledger.dispose();
    drop(ledger);

    let file = root.join("send-acceptance.json");
    fs::write(&file, "{not-json").unwrap();
    clock.store(9_999, Ordering::SeqCst);
    let now = {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    };
    let mut damaged = PromptAcceptanceLedger::with_clock(Some(&root), now);
    assert_eq!(
        damaged.lookup("host", "unknown"),
        AcceptanceLookup::UnknownDurability
    );
    assert_eq!(damaged.gaps().corrupt_reset_at_ms, Some(9_999));
    let has_backup = fs::read_dir(&root)
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains("corrupt-9999"));
    assert!(has_backup);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn run_scheduler_prioritizes_user_work_and_escapes_only_after_watchdog_grace() {
    use mahayana_host_runtime::extensions::transcript::run_scheduler::{
        ActivePhase, QueuedRun, RunLane, RunScheduler, RunSettlement, WatchdogStage,
    };

    fn task(id: &str, lane: RunLane, source: &str, at: u64) -> QueuedRun {
        QueuedRun {
            task_id: id.into(),
            lane,
            source: source.into(),
            enqueued_at_ms: at,
            accepted_at_ms: Some(at.saturating_sub(2)),
            ack_token: Some(format!("ack-{id}")),
        }
    }

    let mut scheduler = RunScheduler::new(100, 20);
    scheduler.enqueue("agent", task("background", RunLane::Background, "wake", 0)).unwrap();
    scheduler.enqueue("agent", task("group-user", RunLane::User, "group-member", 1)).unwrap();
    scheduler.enqueue("agent", task("direct-user", RunLane::User, "composer", 2)).unwrap();
    scheduler.enqueue("agent", task("agent-run", RunLane::Agent, "subagent", 3)).unwrap();

    let first = scheduler.start_next("agent", 10).expect("first run");
    assert_eq!(first.task_id, "direct-user");
    assert_eq!(first.lane, RunLane::User);
    assert_eq!(
        scheduler.queued_task_ids("agent"),
        vec!["group-user", "agent-run", "background"]
    );
    assert!(scheduler.watchdog_tick("agent", 99).is_none());

    scheduler.enqueue("agent", task("new-user", RunLane::User, "composer", 20)).unwrap();
    assert!(scheduler.watchdog_tick("agent", 109).is_none());
    let tripped = scheduler.watchdog_tick("agent", 120).expect("watchdog trip");
    assert_eq!(tripped.stage, WatchdogStage::Trip);
    assert_eq!(scheduler.active("agent").unwrap().phase, ActivePhase::Interrupted);
    assert!(scheduler.watchdog_tick("agent", 139).is_none());

    let escaped = scheduler.watchdog_tick("agent", 140).expect("watchdog escape");
    assert_eq!(escaped.stage, WatchdogStage::Escape);
    assert_eq!(escaped.ack_token.as_deref(), Some("ack-direct-user"));
    assert!(scheduler.active("agent").is_none());

    let successor = scheduler.start_next("agent", 141).expect("successor");
    assert_eq!(successor.task_id, "new-user");
    assert_eq!(successor.lane, RunLane::User);

    let late = scheduler.settle("agent", first.generation, 150);
    assert!(matches!(
        late,
        RunSettlement::ZombieSettled { watchdog, .. }
            if watchdog.stage == WatchdogStage::LateSettle
    ));
    assert_eq!(scheduler.active("agent").unwrap().generation, successor.generation);

    let current = scheduler.settle("agent", successor.generation, 160);
    assert!(matches!(current, RunSettlement::ActiveSettled { task_id, .. } if task_id == "new-user"));
    let group_member = scheduler.start_next("agent", 161).expect("remaining user lane");
    assert_eq!(group_member.task_id, "group-user");
    assert_eq!(group_member.lane, RunLane::User);
}

#[test]
fn run_scheduler_watchdog_ignores_background_backlog_without_waiting_user() {
    use mahayana_host_runtime::extensions::transcript::run_scheduler::{
        QueuedRun, RunLane, RunScheduler,
    };

    let mut scheduler = RunScheduler::new(50, 10);
    let mk = |id: &str, lane: RunLane| QueuedRun {
        task_id: id.into(),
        lane,
        source: "test".into(),
        enqueued_at_ms: 0,
        accepted_at_ms: None,
        ack_token: None,
    };
    scheduler.enqueue("agent", mk("a", RunLane::Agent)).unwrap();
    scheduler.enqueue("agent", mk("b", RunLane::Background)).unwrap();
    let active = scheduler.start_next("agent", 0).unwrap();
    assert_eq!(active.task_id, "a");
    assert!(scheduler.watchdog_tick("agent", 10_000).is_none());
    let diagnostics = scheduler.diagnostics(10_000);
    assert_eq!(diagnostics[0].depth_background, 1);
}


#[test]
fn send_pipeline_coalesces_inflight_nonce_and_preserves_accepted_outcome_on_failure() {
    use mahayana_host_runtime::extensions::transcript::prompt_acceptance_ledger::{
        AcceptanceLookup, AcceptanceStatus, PromptAcceptanceError,
    };
    use mahayana_host_runtime::extensions::transcript::send_pipeline::{
        SendBegin, SendEchoIdentity, SendPipelineState,
    };

    let root = temp_dir("send-pipeline");
    let mut ledger = PromptAcceptanceLedger::new(Some(&root));
    let mut pipeline = SendPipelineState::default();
    let input = sample_input("hello");

    let begin = pipeline
        .begin_send(&mut ledger, &input, Some("nonce-1"))
        .expect("begin");
    assert!(matches!(
        begin,
        SendBegin::Dispatch {
            client_nonce: Some(ref nonce),
            input_digest: Some(_),
        } if nonce == "nonce-1"
    ));
    assert!(pipeline.is_in_flight("nonce-1"));

    let different = sample_input("different while first is running");
    assert_eq!(
        pipeline
            .begin_send(&mut ledger, &input, Some("nonce-1"))
            .unwrap(),
        SendBegin::Coalesced {
            client_nonce: "nonce-1".into(),
        }
    );
    assert!(matches!(
        pipeline.begin_send(&mut ledger, &different, Some("nonce-1")),
        Err(PromptAcceptanceError::DigestMismatch { .. })
    ));

    let pending = pipeline
        .record_pending_acceptance(
            &mut ledger,
            "nonce-1",
            SendEchoIdentity {
                agent_id: "agent-1".into(),
                echo_entry_id: Some("t1u".into()),
            },
        )
        .unwrap();
    assert_eq!(pending.status, AcceptanceStatus::Pending);
    pipeline.mark_send_accepted(&mut ledger, Some("nonce-1"));
    pipeline.finish_send(&mut ledger, Some("nonce-1"), false);
    assert!(!pipeline.is_in_flight("nonce-1"));
    assert!(matches!(
        ledger.lookup("host", "nonce-1"),
        AcceptanceLookup::Found(record) if record.status == AcceptanceStatus::Accepted
    ));

    assert!(matches!(
        pipeline.begin_send(&mut ledger, &input, Some("nonce-1")).unwrap(),
        SendBegin::DuplicateNoop { record } if record.status == AcceptanceStatus::Accepted
    ));
    assert!(matches!(
        pipeline.begin_send(&mut ledger, &different, Some("nonce-1")),
        Err(PromptAcceptanceError::DigestMismatch { .. })
    ));

    assert_eq!(pipeline.next_turn_epoch("agent-1"), 1);
    assert_eq!(pipeline.next_turn_epoch("agent-1"), 2);
    assert_eq!(pipeline.current_turn_epoch("agent-1"), 2);
    let batch = pipeline.claim_attachment_batch_id("agent-1");
    assert_eq!(pipeline.claim_attachment_batch_id("agent-1"), batch);
    pipeline.clear_attachment_batch_id("agent-1");
    assert_ne!(pipeline.claim_attachment_batch_id("agent-1"), batch);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_unaccepted_send_is_cleared_for_retry() {
    use mahayana_host_runtime::extensions::transcript::prompt_acceptance_ledger::AcceptanceLookup;
    use mahayana_host_runtime::extensions::transcript::send_pipeline::{
        SendBegin, SendEchoIdentity, SendPipelineState,
    };

    let root = temp_dir("send-retry");
    let mut ledger = PromptAcceptanceLedger::new(Some(&root));
    let mut pipeline = SendPipelineState::default();
    let input = sample_input("retry me");

    assert!(matches!(
        pipeline.begin_send(&mut ledger, &input, Some("nonce-retry")).unwrap(),
        SendBegin::Dispatch { .. }
    ));
    pipeline
        .record_pending_acceptance(
            &mut ledger,
            "nonce-retry",
            SendEchoIdentity {
                agent_id: "agent-1".into(),
                echo_entry_id: Some("t2u".into()),
            },
        )
        .unwrap();
    pipeline.finish_send(&mut ledger, Some("nonce-retry"), false);
    assert_eq!(
        ledger.lookup("host", "nonce-retry"),
        AcceptanceLookup::NotFound
    );
    assert!(matches!(
        pipeline.begin_send(&mut ledger, &input, Some("nonce-retry")).unwrap(),
        SendBegin::Dispatch { .. }
    ));
    let _ = fs::remove_dir_all(root);
}


#[test]
fn run_lifecycle_settles_nested_sessions_only_at_outer_boundary() {
    use mahayana_host_runtime::extensions::transcript::run_lifecycle::RunLifecycleState;

    let mut lifecycle = RunLifecycleState::default();
    lifecycle.begin_session_run("agent", 100, false);
    lifecycle.begin_session_run("agent", 105, true);
    lifecycle.set_composing("agent", true);
    lifecycle.set_retrying("agent", true);
    lifecycle.set_activity("agent", Some("thinking"));
    assert_eq!(lifecycle.in_flight_count("agent"), 2);
    assert!(lifecycle.running_agent_ids().contains("agent"));

    lifecycle.track_turn_request_id("agent", "req-b");
    lifecycle.track_turn_request_id("agent", "req-a");
    lifecycle.track_turn_request_id("agent", "req-a");
    let usage = lifecycle.settle_turn_usage("agent", "composer");
    assert_eq!(usage.request_id.as_deref(), Some("req-a"));
    assert_eq!(usage.request_id_count, 2);
    assert_eq!(usage.turn_ended_seq, 1);
    lifecycle.track_turn_request_id("agent", "req-c");
    let usage2 = lifecycle.settle_turn_usage("agent", "");
    assert_eq!(usage2.turn_ended_seq, 2);
    assert_eq!(usage2.source, "turn");

    assert!(lifecycle.end_session_run("agent", 130).is_none());
    assert_eq!(lifecycle.in_flight_count("agent"), 1);
    assert!(!lifecycle.is_composing("agent"));
    assert!(!lifecycle.is_retrying("agent"));
    assert_eq!(lifecycle.activity("agent"), None);

    let completed = lifecycle.end_session_run("agent", 160).expect("outer completion");
    assert_eq!(completed.agent_id, "agent");
    assert_eq!(completed.duration_ms, 60);
    assert_eq!(completed.turn_worthy_begin_count, 1);
    assert!(lifecycle.running_agent_ids().is_empty());
    assert_eq!(lifecycle.active_run_session(), None);
}

#[test]
fn group_member_only_run_does_not_emit_user_turn_completion() {
    use mahayana_host_runtime::extensions::transcript::run_lifecycle::RunLifecycleState;

    let mut lifecycle = RunLifecycleState::default();
    lifecycle.begin_session_run("member", 10, true);
    assert!(lifecycle.end_session_run("member", 20).is_none());
    assert!(lifecycle.running_agent_ids().is_empty());
}
