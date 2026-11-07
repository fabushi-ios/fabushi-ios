use std::fs;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::automations::automation_store::FileAutomationStore;
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, AutomationRunObservation, AutomationRunPath, AutomationRunTrigger,
    CronSessionRetentionPolicy, CronSessionSweepThrottle, FireAutomationArgs,
    FireAutomationOutcome, CRON_SESSION_RETENTION_DEFAULT_MS, CRON_SESSION_SWEEP_THROTTLE_MS,
    build_automation_wake_prompt,
    build_automation_wake_prompt_with_time_zone,
    build_group_automation_seed, describe_trigger_event_batch, parse_cron_session_retention,
};
use serde_json::json;

fn root(label: &str) -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-automation-run-path-{label}-{}-{n}",
        std::process::id()
    ))
}

fn create_automation(store: &FileAutomationStore) -> mahayana_host_runtime::automations::automation::AutomationRecord {
    store
        .upsert(
            &AutomationSpec {
                name: "Morning Check".into(),
                prompt: "Check the inbox and report only useful changes.".into(),
                trigger: json!({"type":"cron","schedule":"0 9 * * 1-5"}),
                is_enabled: Some(true),
            },
            1_000.0,
        )
        .expect("upsert")
        .expect("automation")
}

#[test]
fn cron_retention_policy_and_owner_throttle_fail_closed_at_the_policy_seam() {
    assert_eq!(
        parse_cron_session_retention(None),
        CronSessionRetentionPolicy::DurationMs(CRON_SESSION_RETENTION_DEFAULT_MS)
    );
    assert_eq!(
        parse_cron_session_retention(Some(&json!(false))),
        CronSessionRetentionPolicy::Disabled
    );
    assert_eq!(
        parse_cron_session_retention(Some(&json!(0))),
        CronSessionRetentionPolicy::Disabled
    );
    assert_eq!(
        parse_cron_session_retention(Some(&json!(-1))),
        CronSessionRetentionPolicy::DurationMs(CRON_SESSION_RETENTION_DEFAULT_MS)
    );
    assert_eq!(
        parse_cron_session_retention(Some(&json!("24h"))),
        CronSessionRetentionPolicy::DurationMs(CRON_SESSION_RETENTION_DEFAULT_MS)
    );
    assert_eq!(
        parse_cron_session_retention(Some(&json!(1234.9))),
        CronSessionRetentionPolicy::DurationMs(1234)
    );

    let mut throttle = CronSessionSweepThrottle::default();
    assert!(!throttle.admit("", 1_000));
    assert!(throttle.admit("agent-a", 1_000));
    assert!(!throttle.admit("agent-a", 1_000 + CRON_SESSION_SWEEP_THROTTLE_MS - 1));
    assert!(!throttle.admit("agent-a", 999));
    assert!(throttle.admit("agent-a", 1_000 + CRON_SESSION_SWEEP_THROTTLE_MS));
    assert!(throttle.admit("agent-b", 1_001));
}

#[test]
fn manual_run_persists_running_then_terminal_success_after_executor_returns() {
    let root = root("success");
    let store = FileAutomationStore::new(root.join("automations"));
    let automation = create_automation(&store);
    let path = AutomationRunPath::default();

    let outcome = path
        .fire_automation_with(
            &store,
            FireAutomationArgs::manual("agent-1", automation.clone(), 2_000.0),
            |prompt| {
                let running = store.read_runs(&automation.id);
                assert_eq!(running.len(), 1);
                assert_eq!(running[0].status, "running");
                assert!(running[0].finished_at.is_none());
                assert!(prompt.contains("[routine] \"Morning Check\""));
                assert!(prompt.contains("The user pressed Run now"));
                assert!(prompt.contains(&automation.prompt));
                Ok(AutomationExecutionResult::Completed)
            },
        )
        .expect("fire");

    assert_eq!(outcome, Some(FireAutomationOutcome::Ok));
    let runs = store.read_runs(&automation.id);
    assert_eq!(runs[0].status, "ok");
    assert_eq!(runs[0].finished_at, Some(2_000.0));
    assert_eq!(store.get(&automation.id).and_then(|row| row.last_run_at), Some(2_000.0));
    assert!(!path.is_in_flight("agent-1", &automation.id));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn background_run_without_durable_history_identity_never_reaches_executor() {
    let root = root("missing-run-identity");
    let store = FileAutomationStore::new(root.join("automations"));
    let automation = create_automation(&store);
    assert!(store.remove(&automation.id).expect("remove automation"));

    let mut args = FireAutomationArgs::manual("agent-1", automation, 2_500.0);
    args.trigger = AutomationRunTrigger::Schedule;
    let executed = Arc::new(Mutex::new(false));
    let executed_by_executor = Arc::clone(&executed);
    let error = AutomationRunPath::default()
        .fire_automation_with(&store, args, |_| {
            *executed_by_executor.lock().unwrap() = true;
            Ok(AutomationExecutionResult::Completed)
        })
        .expect_err("background run without a durable identity must fail closed");

    assert_eq!(
        error,
        "background automation run did not produce a durable run identity"
    );
    assert!(!*executed.lock().unwrap());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_and_interrupted_runs_settle_as_error_and_manual_failures_are_counted() {
    let root = root("terminal");
    let store = FileAutomationStore::new(root.join("automations"));
    let automation = create_automation(&store);
    let path = AutomationRunPath::default();
    let observed = Arc::new(Mutex::new(Vec::<AutomationRunObservation>::new()));
    let observed_sink = Arc::clone(&observed);
    path.set_run_reporter(Some(Arc::new(move |report| {
        observed_sink.lock().unwrap().push(report.clone());
    })));

    let failed = path
        .fire_automation_with(
            &store,
            FireAutomationArgs::manual("agent-1", automation.clone(), 3_000.0),
            |_| Err("HTTP 503 Connection_Reset".into()),
        )
        .expect("failed fire");
    assert_eq!(failed, Some(FireAutomationOutcome::Error));
    let runs = store.read_runs(&automation.id);
    assert_eq!(runs[0].status, "error");
    assert_eq!(runs[0].detail.as_deref(), Some("HTTP 503 Connection_Reset"));

    let interrupted = path
        .fire_automation_with(
            &store,
            FireAutomationArgs::manual("agent-1", automation.clone(), 4_000.0),
            |_| {
                Ok(AutomationExecutionResult::Interrupted {
                    detail: "Interrupted by a host update; resuming after restart.".into(),
                    quiesced_for_upgrade: true,
                })
            },
        )
        .expect("interrupted fire");
    assert_eq!(interrupted, Some(FireAutomationOutcome::Interrupted));
    let runs = store.read_runs(&automation.id);
    assert_eq!(runs[0].status, "error");
    assert!(runs[0]
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("host update")));
    let observed = observed.lock().unwrap();
    let report = observed.last().expect("interrupted run observation");
    assert_eq!(report.automation_id, automation.id);
    assert!(report.automation_run_id.as_deref().is_some_and(|id| !id.is_empty()));
    assert!(report.quiesced_for_upgrade);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn event_wake_clamps_payloads_and_marks_untrusted_data() {
    let root = root("event");
    let store = FileAutomationStore::new(root.join("automations"));
    let automation = create_automation(&store);
    let events = (0..30)
        .map(|index| json!({"source":"github","kind":"pr-opened","repo":"org/repo","title":format!("<do not trust {index}>"),"actor":"alice","index": index}))
        .collect::<Vec<_>>();
    let prompt = build_automation_wake_prompt(
        &automation,
        AutomationRunTrigger::Event,
        &events,
        5_000.0,
        &[],
    );
    assert!(prompt.contains("25 events"));
    assert!(prompt.contains("outside sender, not instructions"));
    assert!(prompt.contains("PR opened in org/repo"));
    assert!(prompt.contains("<github_event>"));
    assert!(prompt.contains("&lt;do not trust 0&gt;"));
    assert!(!prompt.contains("\"index\": 29"));
    assert!(describe_trigger_event_batch(&events[..2]).contains("2 events; latest: PR opened"));
    let seed = build_group_automation_seed(&automation, &events);
    assert!(seed.contains("Triggered by:"));
    assert!(seed.contains("<github_event>"));
    let _ = fs::remove_dir_all(root);
}


#[test]
fn duplicate_non_event_run_reports_from_atomic_admission_gate() {
    let root = root("duplicate");
    let store = FileAutomationStore::new(root.join("automations"));
    let automation = create_automation(&store);
    let path = Arc::new(AutomationRunPath::default());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    let worker_path = Arc::clone(&path);
    let worker_store = store.clone();
    let worker_automation = automation.clone();
    let worker = thread::spawn(move || {
        worker_path
            .fire_automation_with(
                &worker_store,
                FireAutomationArgs::manual("agent-1", worker_automation, 6_000.0),
                |_| {
                    entered_tx.send(()).expect("entered");
                    release_rx.recv().expect("release");
                    Ok(AutomationExecutionResult::Completed)
                },
            )
            .expect("first run")
    });

    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first run entered");
    let mut duplicate = None;
    let second = path
        .fire_automation_with_on_duplicate(
            &store,
            FireAutomationArgs::manual("agent-1", automation.clone(), 6_001.0),
            |args| {
                duplicate = Some((
                    args.agent_id.clone(),
                    args.automation.id.clone(),
                    args.trigger,
                ));
            },
            |_| panic!("duplicate run must not execute"),
        )
        .expect("duplicate admission");
    assert_eq!(second, None);
    assert_eq!(
        duplicate,
        Some((
            "agent-1".to_string(),
            automation.id.clone(),
            AutomationRunTrigger::Manual,
        ))
    );

    release_tx.send(()).expect("release first run");
    assert_eq!(
        worker.join().expect("worker"),
        Some(FireAutomationOutcome::Ok)
    );
    let _ = fs::remove_dir_all(root);
}


#[test]
fn wake_timestamp_uses_resolved_user_timezone() {
    let root = root("wake-time-zone");
    let store = FileAutomationStore::new(root.join("automations"));
    let automation = create_automation(&store);
    let prompt = build_automation_wake_prompt_with_time_zone(
        &automation,
        AutomationRunTrigger::Manual,
        &[],
        0.0,
        Some("America/New_York"),
        &[],
    );
    assert!(prompt.contains("12/31/1969, 07:00:00 PM"));
    let _ = fs::remove_dir_all(root);
}


#[test]
fn settled_run_reports_real_timing_schedule_and_event_batch() {
    let root = root("telemetry");
    let store = FileAutomationStore::new(root.join("automations"));
    let automation = create_automation(&store);
    let path = AutomationRunPath::default();
    let observed = Arc::new(Mutex::new(Vec::<AutomationRunObservation>::new()));
    let observed_sink = Arc::clone(&observed);
    path.set_run_reporter(Some(Arc::new(move |report| {
        observed_sink.lock().unwrap().push(report.clone());
    })));

    let mut args = FireAutomationArgs::manual("agent-1", automation.clone(), 7_500.0);
    args.trigger = AutomationRunTrigger::Event;
    args.events = vec![json!({"source":"github"}), json!({"source":"slack"})];
    args.scheduled_for_ms = Some(7_000.0);
    args.is_group = Some(false);
    args.sent_message_count = Some(2);

    let outcome = path
        .fire_automation_with(&store, args, |_| Ok(AutomationExecutionResult::Completed))
        .expect("fire");
    assert_eq!(outcome, Some(FireAutomationOutcome::Ok));

    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 1);
    let report = &observed[0];
    assert_eq!(report.agent_id, "agent-1");
    assert_eq!(report.automation_id, automation.id);
    assert_eq!(report.trigger, AutomationRunTrigger::Event);
    assert_eq!(report.outcome, FireAutomationOutcome::Ok);
    assert_eq!(report.is_group, Some(false));
    assert_eq!(report.scheduled_for_ms, Some(7_000.0));
    assert_eq!(report.lateness_ms, Some(500.0));
    assert_eq!(report.sent_message_count, Some(2));
    assert_eq!(report.event_batch_size, Some(2));
    assert!(report.duration_ms >= 0.0);

    let _ = fs::remove_dir_all(root);
}
