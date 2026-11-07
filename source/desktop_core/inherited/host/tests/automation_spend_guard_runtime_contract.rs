use std::fs;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::automation_event_fires::DroppedFire;
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, AutomationRunTrigger, FireAutomationOutcome,
};
use mahayana_host_runtime::extensions::transcript::automation_runtime::AutomationRuntime;
use mahayana_host_runtime::extensions::transcript::sand_automation_spend_guard::{
    SPEND_GUARD_IDLE_TTL_MS, SPEND_GUARD_PAUSE_DELAY_MS, SPEND_GUARD_SNOOZE_MS,
};
use serde_json::json;

fn root(label: &str) -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-spend-guard-runtime-{label}-{}-{n}",
        std::process::id()
    ))
}

fn prepare_away_agent(
    workers: &ProductionSessionWorkers,
    agent_id: &str,
    now_ms: f64,
) {
    let last_viewed = now_ms - SPEND_GUARD_IDLE_TTL_MS - 10_000.0;
    workers
        .mark_agent_viewed(agent_id, last_viewed, false)
        .expect("mark viewed");
    for index in 0..15 {
        workers
            .mark_agent_activity(agent_id, last_viewed + index as f64 + 1.0)
            .expect("mark unread activity");
    }
}

#[test]
fn background_run_nudges_then_pauses_and_resume_answer_restores_only_guard_paused_routines() {
    let root = root("pause-resume");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));
    let drops = Arc::new(Mutex::new(Vec::<DroppedFire>::new()));
    runtime.set_dropped_fire_reporter(Some({
        let drops = Arc::clone(&drops);
        Arc::new(move |drop| drops.lock().expect("drops").push(drop))
    }));

    let mut ids = Vec::new();
    for name in ["Daily one", "Daily two"] {
        let (created, _) = runtime
            .create_agent_automation(
                &agent.id,
                &AutomationSpec {
                    name: name.into(),
                    prompt: "Do the task".into(),
                    trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
                    is_enabled: Some(true),
                },
            )
            .expect("create automation");
        ids.push(created[0].id.clone());
    }

    let now_ms = 1_900_000_000_000.0;
    prepare_away_agent(&workers, &agent.id, now_ms);
    let first = runtime
        .run_background_automation_with(
            &agent.id,
            &ids[0],
            AutomationRunTrigger::Schedule,
            Vec::new(),
            Some("scheduled-run-1".into()),
            Vec::new(),
            now_ms,
            None,
            |prompt| {
                assert!(prompt.contains("The app has already asked them directly"));
                Ok(AutomationExecutionResult::Completed)
            },
        )
        .expect("first background run");
    assert_eq!(first, Some(FireAutomationOutcome::Ok));

    let state = workers
        .get_agent_automation_spend_guard_state(&agent.id)
        .expect("nudge state");
    assert_eq!(state.nudged_at_ms, Some(now_ms));
    assert_eq!(state.card_entry_ids.len(), 1);
    let nudge_entry_id = state.card_entry_ids[0].clone();
    let nudge_entry = workers
        .read_agent_transcript_entries(&agent.id)
        .expect("nudge transcript")
        .into_iter()
        .find(|entry| {
            entry.get("id").and_then(serde_json::Value::as_str)
                == Some(nudge_entry_id.as_str())
        })
        .expect("nudge card");
    let session_key = workers
        .read_session_work_admission(&agent.id)
        .expect("admission")
        .expect("admission record")
        .session_key;
    assert_eq!(
        nudge_entry.get("sessionKey").and_then(serde_json::Value::as_str),
        Some(session_key.as_str())
    );

    let called = Arc::new(AtomicBool::new(false));
    let called_in_run = Arc::clone(&called);
    let paused_at = now_ms + SPEND_GUARD_PAUSE_DELAY_MS + 1.0;
    let second = runtime
        .run_background_automation_with(
            &agent.id,
            &ids[0],
            AutomationRunTrigger::Schedule,
            Vec::new(),
            Some("scheduled-run-2".into()),
            Vec::new(),
            paused_at,
            None,
            move |_| {
                called_in_run.store(true, Ordering::SeqCst);
                Ok(AutomationExecutionResult::Completed)
            },
        )
        .expect("paused background run");
    assert_eq!(second, None);
    assert!(!called.load(Ordering::SeqCst));
    let drops = drops.lock().expect("drops");
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].reason, "user_away_paused");
    assert_eq!(drops[0].trigger, AutomationRunTrigger::Schedule);
    assert_eq!(drops[0].run_uuid.as_deref(), Some("scheduled-run-2"));
    drop(drops);

    let paused = workers
        .get_agent_automation_spend_guard_state(&agent.id)
        .expect("paused state");
    assert_eq!(paused.paused_automation_ids.len(), 2);
    assert_eq!(paused.card_entry_ids.len(), 2);
    assert!(paused.card_entry_ids.contains(&nudge_entry_id));
    let paused_entry_id = paused.card_entry_ids.last().expect("paused card").clone();
    let listed = runtime.get_agent_automations(&agent.id).expect("automations");
    assert!(listed.iter().all(|automation| !automation.is_enabled));

    let answer_at = paused_at + 10.0;
    let ack = runtime
        .handle_spend_guard_answer(
            &agent.id,
            &paused_entry_id,
            "spend-guard:resume",
            answer_at,
        )
        .expect("answer")
        .expect("ack");
    assert!(ack.contains("start the paused routines back up"));
    let resumed = runtime.get_agent_automations(&agent.id).expect("resumed");
    assert!(resumed.iter().all(|automation| automation.is_enabled));
    let state = workers
        .get_agent_automation_spend_guard_state(&agent.id)
        .expect("resumed state");
    assert_eq!(
        state.snoozed_until_ms,
        Some(answer_at + SPEND_GUARD_SNOOZE_MS)
    );
    assert!(state.paused_automation_ids.is_empty());
    assert!(state.card_entry_ids.is_empty());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn manual_run_is_not_blocked_by_spend_guard() {
    let root = root("manual");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));
    let (created, _) = runtime
        .create_agent_automation(
            &agent.id,
            &AutomationSpec {
                name: "Manual".into(),
                prompt: "Do it".into(),
                trigger: json!({"type":"cron","schedule":"@daily"}),
                is_enabled: Some(true),
            },
        )
        .expect("create");
    prepare_away_agent(&workers, &agent.id, 1_900_000_000_000.0);

    let outcome = runtime
        .run_agent_automation_now_with(&agent.id, &created[0].id, |_| {
            Ok(AutomationExecutionResult::Completed)
        })
        .expect("manual");
    assert_eq!(outcome, Some(FireAutomationOutcome::Ok));
    assert!(
        workers
            .get_agent_automation_spend_guard_state(&agent.id)
            .expect("guard")
            .nudged_at_ms
            .is_none()
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
