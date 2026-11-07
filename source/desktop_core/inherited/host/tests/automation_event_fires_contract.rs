use std::fs;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::automation_event_fires::{
    AutomationEventFires, DroppedFire, EventBatchExecutor, EventFireBatch,
    MAX_QUEUED_EVENT_FIRES_PER_AUTOMATION, MAX_REPORTED_DROPPED_FIRES,
};
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, FireAutomationOutcome,
};
use mahayana_host_runtime::extensions::transcript::automation_runtime::AutomationRuntime;
use serde_json::json;

fn root(label: &str) -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-automation-event-fire-{label}-{}-{n}",
        std::process::id()
    ))
}

#[test]
fn runtime_event_fire_debounces_and_coalesces_run_uuids_into_one_durable_run() {
    let root = root("batch");
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
                name: "Events".into(),
                prompt: "Review events".into(),
                trigger: json!({"type":"github","repo":"openai/example","events":["pr-opened"]}),
                is_enabled: Some(true),
            },
        )
        .expect("create");
    let automation_id = created[0].id.clone();
    let prompts = Arc::new(Mutex::new(Vec::new()));

    let execute = {
        let prompts = Arc::clone(&prompts);
        Arc::new(move |_agent: &str, _automation: &str, _name: &str, prompt: &str| {
            prompts.lock().expect("prompts").push(prompt.to_string());
            Ok(AutomationExecutionResult::Completed)
        })
    };

    let first_runtime = runtime.clone();
    let first_agent = agent.id.clone();
    let first_automation = automation_id.clone();
    let first_execute = Arc::clone(&execute);
    let first = thread::spawn(move || {
        first_runtime
            .enqueue_event_automation_fire_with(
                &first_agent,
                &first_automation,
                json!({"type":"issue","id":1}),
                Some("run-1".into()),
                first_execute,
            )
            .expect("first event fire")
    });

    thread::sleep(Duration::from_millis(40));

    let second_runtime = runtime.clone();
    let second_agent = agent.id.clone();
    let second_automation = automation_id.clone();
    let second_execute = Arc::clone(&execute);
    let second = thread::spawn(move || {
        second_runtime
            .enqueue_event_automation_fire_with(
                &second_agent,
                &second_automation,
                json!({"type":"issue","id":2}),
                Some("run-2".into()),
                second_execute,
            )
            .expect("second event fire")
    });

    assert_eq!(first.join().expect("first thread"), Some(FireAutomationOutcome::Ok));
    assert_eq!(second.join().expect("second thread"), Some(FireAutomationOutcome::Ok));
    let prompts = prompts.lock().expect("prompts");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("2 events"));

    let store = sessions.automation_store_for(&agent.id).expect("store");
    let runs = store.read_runs(&automation_id);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].trigger, "event");
    assert_eq!(runs[0].id, "run-1");
    assert_eq!(
        runs[0].coalesced_run_ids.as_deref(),
        Some(&["run-2".to_string()][..])
    );

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn event_fire_queue_sheds_oldest_overflow_and_deduplicates_drop_reporting() {
    let root = root("overflow");
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
                name: "Overflow".into(),
                prompt: "Review events".into(),
                trigger: json!({"type":"github","repo":"openai/example","events":["pr-opened"]}),
                is_enabled: Some(true),
            },
        )
        .expect("create");
    let automation = created[0].clone();

    let event_fires = AutomationEventFires::new(60_000);
    let drops = Arc::new(Mutex::new(Vec::<DroppedFire>::new()));
    event_fires.set_dropped_fire_reporter(Some({
        let drops = Arc::clone(&drops);
        Arc::new(move |drop| drops.lock().expect("drops").push(drop))
    }));
    let executor: EventBatchExecutor = Arc::new(|_batch: EventFireBatch| {
        Ok(Some(FireAutomationOutcome::Ok))
    });

    let mut receivers = Vec::new();
    for index in 0..=MAX_QUEUED_EVENT_FIRES_PER_AUTOMATION {
        receivers.push(event_fires.enqueue_event_automation_fire(
            &agent.id,
            automation.clone(),
            json!({"index":index}),
            Some(format!("overflow-{index}")),
            Arc::clone(&executor),
        ));
    }
    assert_eq!(
        receivers[0]
            .recv_timeout(Duration::from_secs(1))
            .expect("overflow outcome"),
        Some(FireAutomationOutcome::Error)
    );
    assert_eq!(drops.lock().expect("drops").len(), 1);

    let first_drop = { drops.lock().expect("drops")[0].clone() };
    event_fires.report_fire_dropped(first_drop);
    assert_eq!(drops.lock().expect("drops").len(), 1);

    for index in 0..(MAX_REPORTED_DROPPED_FIRES + 4) {
        event_fires.report_fire_dropped(DroppedFire {
            agent_id: agent.id.clone(),
            trigger: mahayana_host_runtime::extensions::transcript::automation_run_path::AutomationRunTrigger::Event,
            reason: "test".into(),
            scheduled_for_ms: None,
            run_uuid: Some(format!("reported-{index}")),
        });
    }
    assert_eq!(
        event_fires.reported_dropped_fire_uuid_count(),
        MAX_REPORTED_DROPPED_FIRES
    );
    assert_eq!(event_fires.pending_batch_count(), 1);
    event_fires.dispose();

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
