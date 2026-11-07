use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::transcript::ack_obligations::{
    ACK_REDRIVE_IDLE_DELAY_MS, MAX_ACK_REDRIVES, AckObligations, AckRedriveTrigger,
    build_ack_redrive_empty_delivery_report, build_ack_redrive_prompt,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-ack-manager-{label}-{}-{suffix}",
        std::process::id()
    ))
}

#[test]
fn send_ack_guard_backstops_early_exit_without_double_recording() {
    let root = temp_root("send-guard");
    let ack = Arc::new(AckObligations::new(&root));

    {
        let guard = ack.arm_send_guard("agent-a", 10.0, true);
        assert!(guard.is_armed());
    }
    let recovered = ack.store().get("agent-a").expect("guard records missing obligation");
    assert_eq!(recovered.created_at_ms, 10.0);
    assert_eq!(recovered.coalesced_count, 1.0);

    ack.store().clear("agent-a").expect("clear guarded obligation");
    {
        let guard = ack.arm_send_guard("agent-a", 20.0, true);
        let recorded = ack.record_send("agent-a", 20.0).expect("ordinary send record");
        assert!(recorded.created);
        drop(guard);
    }
    let explicit = ack.store().get("agent-a").expect("explicit obligation");
    assert_eq!(explicit.coalesced_count, 1.0);

    ack.store().clear("agent-a").expect("clear explicit obligation");
    {
        let mut guard = ack.arm_send_guard("agent-a", 30.0, true);
        guard.disarm();
    }
    assert!(ack.store().get("agent-a").is_none());

    {
        let _guard = ack.arm_send_guard("agent-a", 40.0, false);
    }
    assert!(ack.store().get("agent-a").is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn ack_tokens_are_agent_scoped_and_visible_send_fulfills_coalesced_obligation() {
    let root = temp_root("tokens");
    let ack = AckObligations::new(&root);
    let first = ack
        .record_send_and_mint_token("agent-a", 10.0)
        .expect("first reservation");
    let second = ack
        .record_send_and_mint_token("agent-a", 20.0)
        .expect("second reservation");
    assert!(first.created);
    assert!(!second.created);
    assert_eq!(second.obligation.coalesced_count, 2.0);

    assert!(!ack
        .fulfill_ack_obligation("agent-b", &first.ack_token)
        .expect("wrong agent"));
    assert!(ack.store().get("agent-a").is_some());

    assert!(ack
        .fulfill_ack_obligation("agent-a", &first.ack_token)
        .expect("fulfill"));
    assert!(ack.store().get("agent-a").is_none());
    assert!(ack.retire_ack_run_token("agent-a", Some(&first.ack_token)));
    assert!(ack.retire_ack_run_token("agent-a", Some(&second.ack_token)));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_enqueue_rollback_restores_previous_obligation_only_if_reservation_is_current() {
    let root = temp_root("rollback");
    let ack = AckObligations::new(&root);
    let first = ack
        .record_send_and_mint_token("agent-a", 10.0)
        .expect("first reservation");
    let second = ack
        .record_send_and_mint_token("agent-a", 20.0)
        .expect("second reservation");

    assert!(ack
        .rollback_ack_reservation("agent-a", &second.ack_token)
        .expect("rollback second"));
    let restored = ack.store().get("agent-a").expect("restored first");
    assert_eq!(restored.last_send_at_ms, 10.0);
    assert_eq!(restored.coalesced_count, 1.0);

    assert!(ack
        .fulfill_ack_obligation("agent-a", &first.ack_token)
        .expect("fulfill first"));
    assert!(ack.store().get("agent-a").is_none());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn deleting_agent_forgets_durable_obligation_and_every_scoped_run_token() {
    let root = temp_root("forget-agent");
    let ack = AckObligations::new(&root);
    let a1 = ack.record_send_and_mint_token("agent-a", 10.0).expect("a1");
    let a2 = ack.record_send_and_mint_token("agent-a", 20.0).expect("a2");
    let b = ack.record_send_and_mint_token("agent-b", 30.0).expect("b");

    assert!(ack.forget_agent("agent-a").expect("forget a"));
    assert!(ack.store().get("agent-a").is_none());
    assert!(ack.store().get("agent-b").is_some());
    assert!(!ack.retire_ack_run_token("agent-a", Some(&a1.ack_token)));
    assert!(!ack.retire_ack_run_token("agent-a", Some(&a2.ack_token)));
    assert!(ack.fulfill_ack_obligation("agent-b", &b.ack_token).expect("fulfill b"));
    assert!(ack.retire_ack_run_token("agent-b", Some(&b.ack_token)));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn ack_redrive_constants_and_prompt_keep_the_frozen_recovery_contract() {
    assert_eq!(MAX_ACK_REDRIVES, 3);
    assert_eq!(ACK_REDRIVE_IDLE_DELAY_MS, 5_000);
    let prompt = build_ack_redrive_prompt();
    assert!(prompt.contains("SendMessage"));
    assert!(prompt.contains("NEVER guess"));
    assert!(prompt.contains("ask them to resend"));
}


#[test]
fn accepted_send_can_be_recorded_before_the_runner_mints_its_turn_token() {
    let root = temp_root("split");
    let ack = AckObligations::new(&root);

    let first = ack.record_send("agent-a", 10.0).expect("record first");
    let second = ack.record_send("agent-a", 20.0).expect("coalesce second");
    assert!(first.created);
    assert!(!second.created);
    assert_eq!(second.obligation.coalesced_count, 2.0);

    let token = ack
        .mint_ack_run_token("agent-a")
        .expect("mint token")
        .expect("pending obligation token");
    assert!(ack
        .fulfill_ack_obligation("agent-a", &token)
        .expect("fulfill recorded send"));
    assert!(ack.store().get("agent-a").is_none());
    assert!(ack.retire_ack_run_token("agent-a", Some(&token)));

    assert_eq!(
        ack.mint_ack_run_token("agent-a").expect("no token"),
        None
    );

    let _ = fs::remove_dir_all(root);
}


#[test]
fn redrive_timers_are_per_agent_boot_and_idle_schedules_and_new_sends_cancel_them() {
    let root = temp_root("redrive-timers");
    let ack = AckObligations::new(&root);
    ack.record_send("agent-a", 10.0).expect("record a");
    ack.record_send("agent-b", 20.0).expect("record b");

    assert_eq!(ack.arm_boot_redrives(1_000), 2);
    let a = ack.redrive_schedule("agent-a").expect("boot a");
    let b = ack.redrive_schedule("agent-b").expect("boot b");
    assert_eq!(a.trigger, AckRedriveTrigger::Boot);
    assert_eq!(b.trigger, AckRedriveTrigger::Boot);
    assert_eq!(a.due_at_ms, 1_000 + ACK_REDRIVE_IDLE_DELAY_MS);
    assert_eq!(b.due_at_ms, 1_000 + ACK_REDRIVE_IDLE_DELAY_MS);
    assert_eq!(ack.take_due_redrive("agent-a", a.due_at_ms - 1), None);
    assert_eq!(
        ack.take_due_redrive("agent-a", a.due_at_ms),
        Some(AckRedriveTrigger::Boot)
    );
    assert!(ack.redrive_schedule("agent-a").is_none());
    assert!(ack.redrive_schedule("agent-b").is_some());

    assert!(ack.schedule_ack_redrive_after_idle("agent-a", 8_000));
    assert!(!ack.schedule_ack_redrive_after_idle("agent-a", 9_000));
    let idle = ack.redrive_schedule("agent-a").expect("idle schedule");
    assert_eq!(idle.trigger, AckRedriveTrigger::Idle);
    assert_eq!(idle.due_at_ms, 8_000 + ACK_REDRIVE_IDLE_DELAY_MS);

    ack.record_send("agent-a", 30.0).expect("new send clears timer");
    assert!(ack.redrive_schedule("agent-a").is_none());

    let token = ack
        .mint_ack_run_token("agent-b")
        .expect("mint")
        .expect("token");
    assert!(ack
        .fulfill_ack_obligation("agent-b", &token)
        .expect("fulfill"));
    assert!(ack.redrive_schedule("agent-b").is_none());

    let _ = fs::remove_dir_all(root);
}


#[test]
fn ack_redrive_empty_delivery_report_only_exists_while_delivery_is_still_owed() {
    let root = temp_root("empty-delivery");
    let ack = AckObligations::new(&root);
    ack.record_send("agent-a", 10.0).expect("record send");
    ack.record_redrive_attempt("agent-a").expect("redrive attempt");

    let obligation = ack.store().get("agent-a");
    let report = build_ack_redrive_empty_delivery_report(
        obligation.as_ref(),
        "agent-a",
        Some("stream-1"),
        Some("handoff-resume"),
        3,
        true,
        250,
    )
    .expect("outstanding delivery report");
    assert_eq!(report.conversation_id, "agent-a");
    assert_eq!(report.request_id.as_deref(), Some("stream-1"));
    assert_eq!(report.source, "ack_redrive");
    assert_eq!(report.request_source.as_deref(), Some("handoff-resume"));
    assert_eq!(report.redrive_attempts, Some(1));
    assert_eq!(report.tool_call_count, 3);
    assert!(report.stream_output_produced);
    assert_eq!(report.duration_ms, 250.0);
    assert!(report.ack_outstanding);

    let token = ack
        .mint_ack_run_token("agent-a")
        .expect("mint")
        .expect("token");
    assert!(ack
        .fulfill_ack_obligation("agent-a", &token)
        .expect("fulfill"));
    let cleared = ack.store().get("agent-a");
    assert!(build_ack_redrive_empty_delivery_report(
        cleared.as_ref(),
        "agent-a",
        Some("stream-2"),
        Some("handoff-resume"),
        0,
        false,
        100,
    )
    .is_none());

    let _ = fs::remove_dir_all(root);
}


#[test]
fn pre_minted_ack_tokens_are_reusable_only_by_their_own_agent() {
    let root = temp_root("owned-token");
    let ack = AckObligations::new(&root);
    ack.record_send("agent-a", 10.0).expect("record send");
    let token = ack
        .mint_ack_run_token("agent-a")
        .expect("mint")
        .expect("token");

    assert!(ack
        .is_ack_run_token_for_agent("agent-a", &token)
        .expect("matching token"));
    assert!(!ack
        .is_ack_run_token_for_agent("agent-b", &token)
        .expect("wrong agent token"));
    assert!(ack.retire_ack_run_token("agent-a", Some(&token)));

    let _ = fs::remove_dir_all(root);
}


#[test]
fn ack_owner_reports_frozen_created_coalesced_redrive_fulfilled_and_lost_outcomes() {
    let root = temp_root("telemetry");
    let ack = AckObligations::new(&root);
    let reports = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&reports);
    ack.set_telemetry_reporter(Some(Arc::new(move |report| captured.lock().unwrap().push(report.clone()))));
    ack.record_send("agent-a", 10.0).expect("created");
    ack.record_send("agent-a", 20.0).expect("coalesced");
    let ready = ack.prepare_redrive_with_telemetry("agent-a", true, AckRedriveTrigger::Idle, 30.0).expect("redrive");
    assert!(matches!(ready, mahayana_host_runtime::extensions::transcript::ack_obligations::AckRedrivePreparation::Ready(_)));
    let token = ack.mint_ack_run_token("agent-a").expect("token").expect("pending ack token");
    assert!(ack.fulfill_ack_obligation("agent-a", &token).expect("fulfilled"));
    ack.record_send("deleted", 40.0).expect("deleted created");
    let lost = ack.prepare_redrive_with_telemetry("deleted", false, AckRedriveTrigger::Boot, 55.0).expect("lost");
    assert!(matches!(lost, mahayana_host_runtime::extensions::transcript::ack_obligations::AckRedrivePreparation::LostAgentDeleted(_)));
    let reports = reports.lock().unwrap();
    let agent_a = reports.iter().filter(|report| report.conversation_id == "agent-a").collect::<Vec<_>>();
    assert_eq!(agent_a.len(), 4);
    assert_eq!(agent_a[0].outcome, "created");
    assert_eq!(agent_a[0].age_ms, Some(0.0));
    assert_eq!(agent_a[0].coalesced_count, Some(1));
    assert_eq!(agent_a[1].outcome, "coalesced");
    assert_eq!(agent_a[1].age_ms, Some(10.0));
    assert_eq!(agent_a[1].coalesced_count, Some(2));
    assert_eq!(agent_a[2].outcome, "redrive");
    assert_eq!(agent_a[2].reason.as_deref(), Some("idle"));
    assert_eq!(agent_a[2].redrive_attempts, Some(1));
    assert_eq!(agent_a[3].outcome, "fulfilled");
    assert_eq!(agent_a[3].coalesced_count, Some(2));
    assert_eq!(agent_a[3].redrive_attempts, Some(1));
    assert!(agent_a[3].time_to_first_visible_ack_ms.is_some());
    let deleted = reports.iter().find(|report| report.conversation_id == "deleted" && report.outcome == "lost").expect("agent-deleted loss telemetry");
    assert_eq!(deleted.reason.as_deref(), Some("agent_deleted"));
    assert_eq!(deleted.age_ms, Some(15.0));
    let _ = fs::remove_dir_all(root);
}
