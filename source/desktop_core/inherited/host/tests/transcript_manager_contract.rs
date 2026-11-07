use std::fs;
use std::future::{ready, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

use mahayana_host_runtime::agents::agent_profile::SandAgentProfile;
use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::extensions::session::agent_db_serde::AwaitingUserResponse;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::transcript::agent_lifecycle::AgentDeletionRuntimeDeps;
use mahayana_host_runtime::extensions::session::box_handoff_service::{
    BoxHandoffDeps, BoxHandoffService, HandoffDecision, HandoffRequest, HandoffStartResult,
    HandoffTelemetry, HandoffTrigger,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::client_side_tool_v2_producer::{
    ClientSideToolV2ProducedValue, ClientSideToolV2TransportKind,
};
use mahayana_host_runtime::extensions::transcript::pending_wake_rearm::{
    LostSubagentWake, PendingWakeReport, PendingWakeRuntimePort,
};
use mahayana_host_runtime::extensions::transcript::roster_emit::ProductionRosterEmit;
use mahayana_host_runtime::extensions::transcript::sand_pending_wake_store::{
    DurablePendingWakeMarker, PendingWakeKind, QuietWakeOrigin,
};
use mahayana_host_runtime::extensions::transcript::transcript_manager::{
    TranscriptManager, TranscriptTurnExecutionPort,
};
use mahayana_host_runtime::extensions::turn_execution::extension::turn_execution_extension;
use mahayana_host_runtime::extensions::turn_execution::turn_execution_service::TurnExecutor;
use serde_json::{json, Value};

fn temp_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = NEXT_TEMP_ROOT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fabushi-transcript-manager-{}-{suffix}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn manager_is_the_single_production_composition_owner() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));

    assert!(Arc::ptr_eq(&manager.session_workers(), &sessions));
    let runtime_a = manager.transcript_runtime();
    let runtime_b = manager.transcript_runtime();
    assert!(Arc::ptr_eq(&runtime_a, &runtime_b));

    let runners_a = manager.runner_registry();
    let runners_b = manager.runner_registry();
    assert!(Arc::ptr_eq(&runners_a, &runners_b));
    assert_eq!(runners_a.active_count(), 0);

    let group_a = manager.group_chat();
    let group_b = manager.group_chat();
    assert!(Arc::ptr_eq(&group_a, &group_b));

    let widgets_a = manager.widget_responses();
    let widgets_b = manager.widget_responses();
    assert!(Arc::ptr_eq(&widgets_a, &widgets_b));

    let roster = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&sessions),
        manager.transcript_runtime(),
        Arc::new(|_| {}),
    ));
    manager
        .bind_roster_emit(Arc::clone(&roster))
        .expect("bind manager roster");
    assert!(Arc::ptr_eq(
        &manager.roster_emit().expect("manager roster"),
        &roster,
    ));

    let workflows_a = manager.workflow_commands();
    let workflows_b = manager.workflow_commands();
    assert!(Arc::ptr_eq(&workflows_a, &workflows_b));

    let wakes_a = manager.background_wakes();
    let wakes_b = manager.background_wakes();
    assert!(Arc::ptr_eq(&wakes_a, &wakes_b));
    {
        let mut wakes = wakes_a.lock().expect("background wakes");
        mahayana_host_runtime::extensions::transcript::background_wakes::BackgroundWakes::enqueue(
            &mut wakes.pending_inbound,
            "agent-a",
            json!({"kind":"inbound"}),
        );
        wakes.dm_preempted_wake_agent_ids.insert("agent-a".into());
    }

    let rooms_a = manager.shared_rooms();
    let rooms_b = manager.shared_rooms();
    assert!(Arc::ptr_eq(&rooms_a, &rooms_b));

    let tool_call = manager
        .publish_client_side_tool_v2(
            "agent-tools",
            ClientSideToolV2ProducedValue::call("call-1", b"call".to_vec()),
        )
        .expect("manager-owned client tool call");
    let tool_result = manager
        .publish_client_side_tool_v2(
            "agent-tools",
            ClientSideToolV2ProducedValue::result("call-1", b"result".to_vec()),
        )
        .expect("manager-owned client tool result");
    assert_eq!(tool_call.kind, ClientSideToolV2TransportKind::Call);
    assert_eq!(tool_result.kind, ClientSideToolV2TransportKind::Result);
    assert_eq!(tool_call.epoch, tool_result.epoch);
    assert_eq!(tool_call.sequence, 1);
    assert_eq!(tool_result.sequence, 2);
    let tool_reset = manager
        .reset_client_side_tool_v2("agent-tools")
        .expect("manager-owned client tool reset");
    assert_eq!(tool_reset.kind, ClientSideToolV2TransportKind::Reset);
    assert_eq!(tool_reset.epoch, tool_call.epoch);
    assert_eq!(tool_reset.sequence, 3);

    let automation_config_changed = Arc::new(Mutex::new(0usize));
    let automation_config_changed_capture = Arc::clone(&automation_config_changed);
    manager.set_automation_config_changed_observer(Some(Arc::new(move || {
        *automation_config_changed_capture
            .lock()
            .expect("automation config observer") += 1;
    })));
    let listener_cards = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let listener_cards_capture = Arc::clone(&listener_cards);
    manager.set_listener_connect_observer(Some(Arc::new(move |agent_id, platform| {
        listener_cards_capture
            .lock()
            .expect("listener observer")
            .push((agent_id.to_string(), platform.to_string()));
    })));
    let channel_config_changed = Arc::new(Mutex::new(0usize));
    let channel_config_changed_capture = Arc::clone(&channel_config_changed);
    manager.set_channel_config_changed_observer(Some(Arc::new(move || {
        *channel_config_changed_capture
            .lock()
            .expect("channel config observer") += 1;
    })));
    assert!(
        manager
            .widget_responses()
            .bind_channel_config_changed(Arc::new(|| {}))
            .is_err(),
        "WidgetResponses channel-config signal must remain owned by TranscriptManager"
    );
    manager.emit_automation_config_changed();
    manager.emit_listener_connect_card("agent-observer", "slack");
    assert_eq!(*automation_config_changed.lock().expect("automation count"), 1);
    assert_eq!(
        listener_cards.lock().expect("listener cards").as_slice(),
        &[("agent-observer".to_string(), "slack".to_string())],
    );

    let ack_a = manager.ack_obligations();
    let ack_b = manager.ack_obligations();
    assert!(Arc::ptr_eq(&ack_a, &ack_b));
    ack_a.record_send("agent-a", 1.0).expect("durable ack");
    assert_eq!(ack_b.pending_obligations().len(), 1);

    let session = SandAgentSessionStore::new(Arc::clone(&sessions));
    let member_profile = |name: &str| SandAgentProfile {
        name: name.to_string(),
        description: String::new(),
        title: String::new(),
        avatar_shape: String::new(),
        avatar_color: String::new(),
    };
    let first_profile = member_profile("First member");
    let second_profile = member_profile("Second member");
    let first_group_member = session
        .create_session(Some(&first_profile), "user", None)
        .expect("first group member");
    let second_group_member = session
        .create_session(Some(&second_profile), "user", None)
        .expect("second group member");
    let created_group = manager
        .dispatch_session_gateway_call(
            "createGroup",
            &json!({
                "name": "Manager-owned room",
                "memberIds": [first_group_member.id, second_group_member.id],
            }),
        )
        .expect("group method handled")
        .expect("manager-owned group created");
    assert_eq!(created_group["agent"]["isGroup"], true);
    assert_eq!(
        created_group["agent"]["memberIds"].as_array().map(Vec::len),
        Some(2),
    );

    let agent = session.create_session(None, "user", None).expect("agent");
    let avatar = manager
        .dispatch_agent_lifecycle_gateway_call(
            &AgentDeletionRuntimeDeps::default(),
            None,
            "getAgentAvatar",
            &json!({"id": agent.id.clone()}),
        )
        .expect("agent lifecycle method handled")
        .expect("manager-owned lifecycle avatar");
    assert!(avatar.get("version").is_some());
    manager
        .switch_agent(&agent.id, 10.0)
        .expect("switch active agent");
    assert_eq!(
        sessions.memory_service().active_agent_id().as_deref(),
        Some(agent.id.as_str()),
    );
    sessions
        .append_agent_transcript_entries(
            &agent.id,
            &[json!({
                "id":"manager-secret-signal",
                "kind":"send-message",
                "message":{
                    "type":"secret-request",
                    "secretRequest":{
                        "label":"Manager-owned token",
                        "target":{"kind":"channel-credential","platform":"slack","field":"token"}
                    }
                }
            })],
        )
        .expect("append manager-owned secret request");
    assert!(manager
        .submit_secret_with(
            "manager-secret-signal",
            "manager-secret",
            &agent.id,
            11.0,
            |_| Ok(()),
        )
        .expect("manager-owned secret submission"));
    assert_eq!(*channel_config_changed.lock().expect("channel count"), 1);
    manager
        .set_window_focused(true, 123.0)
        .expect("manager window focus");
    assert_eq!(manager.window_focused_at_ms(), Some(123.0));
    assert_eq!(
        workflows_a.watched_agent_id().as_deref(),
        Some(agent.id.as_str())
    );

    let runtime = manager.transcript_runtime();
    runtime
        .session_runtime()
        .schedule_deferred_activation(&agent.id, None);
    assert_eq!(
        runtime.session_runtime().pending_activation_agent_id().as_deref(),
        Some(agent.id.as_str())
    );

    ack_a
        .record_send(&agent.id, 10.0)
        .expect("durable ack for redrive");
    assert!(ack_a.arm_redrive_timer(
        &agent.id,
        mahayana_host_runtime::extensions::transcript::ack_obligations::AckRedriveTrigger::Idle,
        10,
    ));
    assert!(ack_a.redrive_schedule(&agent.id).is_some());

    let handoff = BoxHandoffService::new(BoxHandoffDeps::default());
    let _ = handoff.start(HandoffRequest {
        agent_id: agent.id.clone(),
        instruction: "finish browser step".into(),
        telemetry: HandoffTelemetry::default(),
    });
    assert_eq!(handoff.pending_count(), 1);
    manager
        .set_handoff_service(handoff.clone())
        .expect("bind handoff");

    roster.set_outline_stream_coalescing_ms(60_000);
    roster.queue_outline_stream_update(&agent.id, "stream-item", json!({"text":"pending"}));
    assert!(roster.has_pending_outline_stream_update());

    assert!(!manager.is_disposed());
    manager.dispose();
    assert!(manager.is_disposed());
    assert!(manager.roster_emit().is_none());
    assert!(!roster.has_pending_outline_stream_update());
    manager.emit_automation_config_changed();
    manager.emit_channel_config_changed();
    manager.emit_listener_connect_card("agent-observer", "github");
    assert_eq!(*automation_config_changed.lock().expect("automation count"), 1);
    assert_eq!(*channel_config_changed.lock().expect("channel count"), 1);
    assert_eq!(listener_cards.lock().expect("listener cards").len(), 1);
    manager.dispose();
    assert_eq!(runners_a.active_count(), 0);
    assert!(workflows_a.watched_agent_id().is_none());
    assert!(runtime.session_runtime().pending_activation_agent_id().is_none());
    assert!(ack_a.redrive_schedule(&agent.id).is_none());
    {
        let wakes = wakes_a.lock().expect("background wakes after dispose");
        assert!(wakes.pending_inbound.is_empty());
        assert!(wakes.pending_channel_failures.is_empty());
        assert!(wakes.pending_event_wakes.is_empty());
        assert!(wakes.reviving_inbound_agent_ids.is_empty());
        assert!(wakes.reviving_channel_failure_agent_ids.is_empty());
        assert!(wakes.reviving_event_agent_ids.is_empty());
        assert!(wakes.dm_preempted_wake_agent_ids.is_empty());
    }
    assert_eq!(handoff.pending_count(), 0);
    assert_eq!(sessions.memory_service().active_agent_id(), None);
    assert_eq!(sessions.active_agent_store_owner_count(), 0);
    assert_eq!(sessions.active_agent_db_owner_count(), 0);
    assert_eq!(sessions.active_worker_count(), 0);

    let _ = fs::remove_dir_all(root);
}


struct ManagerFakeExecutor;

impl TurnExecutor for ManagerFakeExecutor {
    fn is_inference_ready(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(ready(true))
    }

    fn create_runner(&self, session: Value, hooks: Value) -> Value {
        json!({"kind":"runner","session":session,"hooks":hooks})
    }

    fn create_group_member_runner(
        &self,
        session: Value,
        hooks: Value,
        overrides: Value,
    ) -> Value {
        json!({
            "kind":"group",
            "session":session,
            "hooks":hooks,
            "overrides":overrides
        })
    }
}

#[test]
fn manager_owns_memory_gateway_and_invalidates_prompt_snapshot_after_mutation() {
    use mahayana_host_runtime::extensions::memory::memory_service::MemoryKind;

    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let session = SandAgentSessionStore::new(Arc::clone(&sessions));
    let agent = session.create_session(None, "user", None).expect("agent");
    let stored = sessions
        .memory_service()
        .store_for_agent(&agent.id)
        .add_memory("Remember this", 42, MemoryKind::Profile)
        .expect("memory write")
        .expect("memory record");
    sessions
        .set_agent_memory_prompt_snapshot(&agent.id, &json!({"fingerprint":"stale"}))
        .expect("snapshot");

    let listed = manager
        .dispatch_memory_gateway_call("getAgentMemories", &json!({"id":agent.id}))
        .expect("handled")
        .expect("list");
    assert_eq!(listed[0]["id"], stored.id);
    assert_eq!(listed[0]["content"], "Remember this");
    assert_eq!(listed[0]["createdAt"], 0);
    assert_eq!(listed[0]["kind"], "profile");

    let removed = manager
        .dispatch_memory_gateway_call(
            "deleteAgentMemory",
            &json!({"id":agent.id,"memoryId":stored.id}),
        )
        .expect("handled")
        .expect("delete");
    assert_eq!(removed, json!(true));
    assert!(sessions
        .get_agent_memory_prompt_snapshot(&agent.id)
        .expect("snapshot read")
        .is_none());

    sessions
        .memory_service()
        .store_for_agent(&agent.id)
        .add_memory("Clear this", 43, MemoryKind::Log)
        .expect("memory write")
        .expect("memory record");
    sessions
        .set_agent_memory_prompt_snapshot(&agent.id, &json!({"fingerprint":"stale-again"}))
        .expect("snapshot");
    assert_eq!(
        manager
            .dispatch_memory_gateway_call("clearAgentMemories", &json!({"id":agent.id}))
            .expect("handled")
            .expect("clear"),
        Value::Null,
    );
    assert!(sessions.memory_service().list(&agent.id).is_empty());
    assert!(sessions
        .get_agent_memory_prompt_snapshot(&agent.id)
        .expect("snapshot read")
        .is_none());

    sessions
        .begin_agent_delete(&agent.id)
        .expect("publish memory deletion fence");
    let blocked = manager
        .dispatch_memory_gateway_call("clearAgentMemories", &json!({"id":agent.id}))
        .expect("handled")
        .expect_err("memory gateway must honor deletion fence");
    assert!(blocked.contains("new session work is not admitted"));
    sessions
        .end_agent_delete(&agent.id)
        .expect("clear memory deletion fence");

    manager.dispose();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runner_registry_owns_turn_execution_and_manager_delegates_to_the_same_owner() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let runner_owner = manager.runner_registry();
    let (_, registry) = turn_execution_extension();
    let registry = Arc::new(Mutex::new(registry));
    manager.set_turn_execution(TranscriptTurnExecutionPort::new(Arc::clone(&registry)));

    assert!(runner_owner.turn_execution().is_some());
    assert!(!runner_owner.can_execute());
    assert!(!runner_owner.can_execute_group_member());
    assert!(!futures::executor::block_on(runner_owner.is_run_ready()));
    assert!(!manager.can_execute());
    assert!(!manager.can_execute_group_member());
    assert!(!futures::executor::block_on(manager.is_run_ready()));

    registry
        .lock()
        .expect("turn execution registry")
        .bind_executor(Box::new(ManagerFakeExecutor))
        .expect("bind executor");

    assert!(runner_owner.can_execute());
    assert!(runner_owner.can_execute_group_member());
    assert!(futures::executor::block_on(runner_owner.is_run_ready()));
    assert_eq!(
        runner_owner
            .create_runner(json!({"id":"owned-session"}), json!({"hook":"owned"}))
            .expect("owned runner")["kind"],
        "runner"
    );
    assert_eq!(
        runner_owner
            .create_group_member_runner(
                json!({"id":"owned-member"}),
                json!({"hook":"owned-group"}),
                json!({"model":"owned-override"}),
            )
            .expect("owned group runner")["kind"],
        "group"
    );

    assert!(manager.can_execute());
    assert!(manager.can_execute_group_member());
    assert!(futures::executor::block_on(manager.is_run_ready()));
    assert_eq!(
        manager
            .create_runner(json!({"id":"session"}), json!({"hook":"main"}))
            .expect("runner")["kind"],
        "runner"
    );
    assert_eq!(
        manager
            .create_group_member_runner(
                json!({"id":"member"}),
                json!({"hook":"group"}),
                json!({"model":"override"}),
            )
            .expect("group runner")["kind"],
        "group"
    );

    manager.dispose();
    let _ = fs::remove_dir_all(root);
}


#[derive(Default)]
struct ManagerPendingWakeRuntime {
    shell_watches: Mutex<Vec<(String, String)>>,
    reports: Mutex<Vec<PendingWakeReport>>,
}

impl PendingWakeRuntimePort for ManagerPendingWakeRuntime {
    fn can_execute(&self) -> bool {
        true
    }

    fn is_agent_gone(&self, _agent_id: &str) -> bool {
        false
    }

    fn is_group_session(&self, _agent_id: &str) -> Result<bool, String> {
        Ok(false)
    }

    fn cloud_watch_is_armed(&self, _agent_id: &str, _work_id: &str) -> bool {
        false
    }

    fn watch_cloud_agent(
        &self,
        _agent_id: &str,
        _work_id: &str,
        _quiet_origin: Option<&QuietWakeOrigin>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn watch_background_shell(
        &self,
        agent_id: &str,
        work_id: &str,
        _title: Option<&str>,
        _quiet_origin: Option<&QuietWakeOrigin>,
    ) -> Result<(), String> {
        self.shell_watches
            .lock()
            .expect("shell watches")
            .push((agent_id.to_string(), work_id.to_string()));
        Ok(())
    }

    fn deliver_recreate_interrupted_shell_notice(
        &self,
        _marker: &DurablePendingWakeMarker,
    ) -> Result<(), String> {
        Ok(())
    }

    fn revive_lost_subagent(&self, _wake: LostSubagentWake) -> Result<(), String> {
        Ok(())
    }

    fn revive_channel_delivery_failure(
        &self,
        _marker: &DurablePendingWakeMarker,
    ) -> Result<(), String> {
        Ok(())
    }

    fn emit_async_tasks_for_agent(&self, _agent_id: &str) {}

    fn report_pending_wake(&self, report: PendingWakeReport) {
        self.reports.lock().expect("pending-wake reports").push(report);
    }
}

#[test]
fn manager_owns_pending_wake_rearm_and_replays_durable_wakes() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let store = manager
        .transcript_runtime()
        .pending_wake_store()
        .cloned()
        .expect("production pending-wake store");
    store.mark_pending(DurablePendingWakeMarker {
        agent_id: "agent-pending".into(),
        kind: PendingWakeKind::Shell,
        work_id: "shell-pending".into(),
        marked_at_ms: chrono::Utc::now().timestamp_millis() as f64,
        quiet_origin: None,
        title: Some("Pending shell".into()),
        subagent_type: None,
        interrupted_by_recreate: false,
        delivery_operation_id: None,
        address_token: None,
        failure_reason: None,
        session_key: None,
    });

    let runtime = Arc::new(ManagerPendingWakeRuntime::default());
    manager
        .bind_pending_wake_runtime(runtime.clone())
        .expect("bind manager pending-wake owner");
    assert!(
        manager
            .bind_pending_wake_runtime(Arc::new(ManagerPendingWakeRuntime::default()))
            .is_err(),
        "TranscriptManager must reject a second pending-wake owner"
    );
    manager
        .rearm_pending_wakes()
        .expect("manager-owned pending-wake rearm");

    assert_eq!(
        runtime.shell_watches.lock().expect("shell watches").as_slice(),
        &[("agent-pending".to_string(), "shell-pending".to_string())]
    );
    assert!(
        runtime
            .reports
            .lock()
            .expect("pending-wake reports")
            .iter()
            .any(|report| report.outcome == "rearmed" && report.work_id == "shell-pending")
    );

    manager.dispose();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn manager_is_the_upgrade_resume_facade_for_shipping_host_lifecycle() {
    use mahayana_host_runtime::extensions::transcript::sand_upgrade_resume_store::UpgradeResumeMarker;

    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));

    assert!(!manager.is_quiescing_for_upgrade());
    manager.mark_upgrade_resume_pending(UpgradeResumeMarker {
        agent_id: "agent-upgrade".into(),
        marked_at_ms: 42.0,
        source: Some("background-revival".into()),
        automation_id: None,
        automation_run_id: None,
    });
    assert_eq!(
        manager.transcript_runtime().upgrade_resume_agent_ids(),
        vec!["agent-upgrade".to_string()]
    );

    let registry = manager.runner_registry();
    assert!(!registry.is_quiescing_for_upgrade());
    let summary = manager.quiesce_for_upgrade();
    assert!(summary.quiescing);
    assert_eq!(summary.running_turns, 0);
    assert!(manager.is_quiescing_for_upgrade());
    assert!(registry.is_quiescing_for_upgrade());
    manager.resume_after_recreate();
    assert!(!manager.is_quiescing_for_upgrade());
    assert!(!registry.is_quiescing_for_upgrade());

    manager.dispose();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn manager_emits_active_session_automations_through_the_roster_surface() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let captured = Arc::clone(&events);
    let roster = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&sessions),
        manager.transcript_runtime(),
        Arc::new(move |event| captured.lock().expect("events").push(event)),
    ));
    manager.bind_roster_emit(roster).expect("bind roster");

    let active = sessions
        .materialize_new_session(None, "user", None)
        .expect("active agent");
    let inactive = sessions
        .materialize_new_session(None, "user", None)
        .expect("inactive agent");
    manager.switch_agent(&active.id, 1_000.0).expect("activate agent");

    let store = SandAgentSessionStore::new(Arc::clone(&sessions));
    let automations = store
        .create_agent_automation(
            &active.id,
            &AutomationSpec {
                name: "Morning Check".into(),
                prompt: "Check the inbox.".into(),
                trigger: json!({"type":"cron","schedule":"0 9 * * 1-5"}),
                is_enabled: Some(true),
            },
        )
        .expect("create automation");
    assert_eq!(automations.len(), 1);

    assert!(manager.emit_automations(&active.id).expect("emit active"));
    let emitted = events.lock().expect("events").clone();
    let automation_event = emitted
        .iter()
        .rev()
        .find(|event| event.get("channel").and_then(Value::as_str) == Some("automations"))
        .expect("automation event");
    assert_eq!(
        automation_event
            .pointer("/payload/agentId")
            .and_then(Value::as_str),
        Some(active.id.as_str())
    );
    assert_eq!(
        automation_event
            .pointer("/payload/automations/0/name")
            .and_then(Value::as_str),
        Some("Morning Check")
    );

    let before = emitted.len();
    assert!(!manager.emit_automations(&inactive.id).expect("skip inactive"));
    assert_eq!(events.lock().expect("events").len(), before);

    manager.dispose();
    sessions.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn manager_owns_box_handoff_state_and_handback_settlement() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let sessions = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let manager = TranscriptManager::new(&root, Arc::clone(&sessions));
    let roster = Arc::new(ProductionRosterEmit::new(
        Arc::clone(&sessions),
        manager.transcript_runtime(),
        Arc::new(|_| {}),
    ));
    manager
        .bind_roster_emit(roster)
        .expect("bind manager roster");

    let handoff = BoxHandoffService::new(BoxHandoffDeps::default());
    manager
        .set_handoff_service(handoff)
        .expect("bind manager handoff owner");

    let record = sessions
        .materialize_new_session(None, "user", None)
        .expect("materialize agent");
    sessions
        .set_agent_awaiting_user_response(
            &record.id,
            Some(&AwaitingUserResponse {
                tab_id: "box-tab".into(),
                reason: "box-help".into(),
                since: 1.0,
            }),
        )
        .expect("awaiting state");

    let started = manager
        .start_box_handoff(HandoffRequest {
            agent_id: record.id.clone(),
            instruction: "Complete the browser step".into(),
            telemetry: HandoffTelemetry::default(),
        })
        .expect("start manager-owned handoff");
    let request_id = match started {
        HandoffStartResult::Started { request_id } => request_id,
        HandoffStartResult::AlreadyPending { .. } => panic!("unexpected existing handoff"),
    };
    assert_eq!(
        manager
            .box_handoff(&record.id)
            .expect("manager pending handoff")
            .request_id,
        request_id
    );

    let entry = json!({
        "id": "box-manager-request",
        "kind": "send-message",
        "message": {"type":"text","content":"Complete the browser step"},
        "timestampMs": 2,
        "boxRequestId": request_id,
        "boxInstruction": "Complete the browser step"
    });
    manager
        .transcript_runtime()
        .track_box_request_entry(&record.id, &entry);
    sessions
        .append_agent_transcript_entries(&record.id, &[entry])
        .expect("persist box request");

    let decision = manager
        .hand_back_forever_box(
            &record.id,
            HandoffTrigger::Name("button".into()),
        )
        .expect("manager hand back");
    let HandoffDecision::End(decision) = decision else {
        panic!("expected handoff settlement");
    };
    assert_eq!(decision.resolution, "completed");
    assert!(manager.box_handoff(&record.id).is_none());
    assert!(
        sessions
            .get_agent_awaiting_user_response(&record.id)
            .expect("awaiting state")
            .is_none()
    );
    let entries = sessions
        .read_agent_transcript_entries(&record.id)
        .expect("read transcript");
    let resolved = entries
        .iter()
        .find(|entry| {
            entry.get("id").and_then(Value::as_str) == Some("box-manager-request")
        })
        .expect("resolved box entry");
    assert_eq!(resolved["boxResolution"], "completed");

    manager.dispose();
    let _ = fs::remove_dir_all(root);
}
