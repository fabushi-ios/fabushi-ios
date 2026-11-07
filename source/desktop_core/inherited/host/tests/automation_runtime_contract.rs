use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::automations::automation::AutomationSpec;
use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::automation_run_path::{
    AutomationExecutionResult, FireAutomationOutcome,
};
use mahayana_host_runtime::extensions::transcript::automation_runtime::{
    AutomationLifecycleAction, AutomationRuntime, dispatch_automation_command,
};
use serde_json::json;

fn root(label: &str) -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-automation-runtime-{label}-{}-{n}",
        std::process::id()
    ))
}

#[test]
fn runtime_serializes_mutations_and_derives_lifecycle_actions() {
    let root = root("mutations");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));

    let spec = AutomationSpec {
        name: "Daily".into(),
        prompt: "Check updates".into(),
        trigger: json!({"type":"cron","schedule":"0 9 * * *"}),
        is_enabled: Some(true),
    };
    let (created, events) = runtime
        .create_agent_automation(&agent.id, &spec)
        .expect("create");
    assert_eq!(created.len(), 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Created);
    let id = created[0].id.clone();

    let (_, events) = runtime
        .update_agent_automation(
            &agent.id,
            &id,
            &AutomationSpec {
                prompt: "Check updates and summarize".into(),
                ..spec.clone()
            },
        )
        .expect("update");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Updated);

    let (_, events) = runtime
        .set_agent_automation_enabled(&agent.id, &id, false)
        .expect("disable");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Disabled);

    let (_, events) = runtime
        .delete_agent_automation(&agent.id, &id)
        .expect("delete");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Deleted);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_manual_run_uses_durable_run_path_and_terminal_executor() {
    let root = root("run");
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
                prompt: "Inspect the queue".into(),
                trigger: json!({"type":"cron","schedule":"@daily"}),
                is_enabled: Some(true),
            },
        )
        .expect("create");
    let id = created[0].id.clone();
    let store = sessions.automation_store_for(&agent.id).expect("store");

    let outcome = runtime
        .run_agent_automation_now_with(&agent.id, &id, |prompt| {
            assert!(prompt.contains("The user pressed Run now"));
            let runs = store.read_runs(&id);
            assert_eq!(runs[0].status, "running");
            Ok(AutomationExecutionResult::Completed)
        })
        .expect("run");

    assert_eq!(outcome, Some(FireAutomationOutcome::Ok));
    let runs = store.read_runs(&id);
    assert_eq!(runs[0].status, "ok");
    assert!(runs[0].finished_at.is_some());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn workflow_ui_mutation_uses_same_automation_lifecycle_owner() {
    let root = root("workflow-ui");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));

    let (workflows, events) = runtime
        .with_workflow_ui_mutation(&agent.id, |session| {
            session.create_agent_workflow(
                &agent.id,
                &mahayana_host_runtime::workflows::workflow_library::WorkflowSpec {
                    name: "Scheduled workflow".into(),
                    description: String::new(),
                    body: "Check the queue".into(),
                    trigger: Some(
                        mahayana_host_runtime::workflows::workflow_library::WorkflowTrigger {
                            schedule: "0 8 * * *".into(),
                            is_enabled: true,
                        },
                    ),
                    source_ref: None,
                },
            )
        })
        .expect("workflow mutation");
    assert_eq!(workflows.len(), 1);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Created);
    assert_eq!(
        events[0].source,
        mahayana_host_runtime::extensions::transcript::automation_runtime::AutomationLifecycleSource::WorkflowUi
    );

    let automation_id = workflows[0].id.clone();
    let (_, events) = runtime
        .with_workflow_ui_mutation(&agent.id, |session| {
            session.remove_agent_workflow(&agent.id, &automation_id)
        })
        .expect("delete workflow");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, AutomationLifecycleAction::Deleted);

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


fn automation_call(
    runtime: &AutomationRuntime,
    method: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    dispatch_automation_command(runtime, method, &args)
        .expect("automation command handled")
        .expect("automation command result")
}

#[test]
fn automation_gateway_commands_use_runtime_owner_and_renderer_shape() {
    let root = root("gateway");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let sessions = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = sessions.create_session(None, "user", None).expect("agent");
    let runtime = AutomationRuntime::new(Arc::clone(&workers));

    let created = automation_call(
        &runtime,
        "createAgentAutomation",
        json!({
            "id": agent.id,
            "spec": {
                "name": "Inbox review",
                "prompt": "Review the inbox",
                "trigger": {"type":"cron","schedule":"0 9 * * *"},
                "isEnabled": true
            }
        }),
    );
    let created = created.as_array().expect("automation list");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["name"], "Inbox review");
    assert_eq!(created[0]["prompt"], "Review the inbox");
    assert_eq!(created[0]["isEnabled"], true);
    assert_eq!(created[0]["trigger"]["type"], "cron");
    assert_eq!(created[0]["triggerDescription"], "Scheduled: 0 9 * * *");
    assert!(created[0]["runs"].as_array().expect("runs").is_empty());
    assert!(created[0]["createdAt"].is_number());
    let automation_id = created[0]["id"].as_str().expect("automation id").to_string();

    let listed = automation_call(
        &runtime,
        "getAgentAutomations",
        json!({"id": agent.id}),
    );
    assert_eq!(listed[0]["id"], automation_id);

    let disabled = automation_call(
        &runtime,
        "setAgentAutomationEnabled",
        json!({
            "id": agent.id,
            "automationId": automation_id,
            "isEnabled": false
        }),
    );
    assert_eq!(disabled[0]["isEnabled"], false);

    let updated = automation_call(
        &runtime,
        "updateAgentAutomation",
        json!({
            "id": agent.id,
            "automationId": automation_id,
            "spec": {
                "name": "Inbox review updated",
                "prompt": "Review mail and calendar",
                "trigger": {"type":"cron","schedule":"30 9 * * *"},
                "isEnabled": true
            }
        }),
    );
    assert_eq!(updated[0]["name"], "Inbox review updated");
    assert_eq!(updated[0]["prompt"], "Review mail and calendar");
    assert_eq!(updated[0]["schedule"], "30 9 * * *");

    let deleted = automation_call(
        &runtime,
        "deleteAgentAutomation",
        json!({"id": agent.id, "automationId": automation_id}),
    );
    assert!(deleted.as_array().expect("automation list").is_empty());

    assert!(dispatch_automation_command(
        &runtime,
        "runAgentAutomationNow",
        &json!({"id":agent.id,"automationId":"missing"}),
    )
    .is_none());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
