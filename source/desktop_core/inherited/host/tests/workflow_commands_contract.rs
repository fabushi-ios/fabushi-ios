use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::session::agent_session::SandAgentSessionStore;
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::extensions::transcript::workflow_commands::{
    WORKFLOW_INJECTED_BODY_LIMIT, WorkflowRunNowPlan, build_workflow_run_prompt,
    dispatch_workflow_command, expand_workflow_references, prepare_workflow_run_now,
};
use mahayana_host_runtime::workflows::workflow_store::WorkflowRecord;

fn temp_root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-workflow-commands-{label}-{}-{suffix}",
        std::process::id()
    ))
}

fn call(
    workers: Arc<ProductionSessionWorkers>,
    method: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    dispatch_workflow_command(workers, method, &args)
        .expect("workflow command handled")
        .expect("workflow command result")
}

#[test]
fn shipping_workflow_commands_delegate_crud_and_imports_to_session_store() {
    let root = temp_root("crud");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let session = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = session.create_session(None, "user", None).expect("agent");

    let created = call(
        Arc::clone(&workers),
        "createAgentWorkflow",
        serde_json::json!({
            "id": agent.id,
            "spec": {
                "name": "Research",
                "description": "Research workflow",
                "body": "Read sources and summarize.",
                "sourceRef": "local"
            }
        }),
    );
    assert_eq!(created.as_array().map(Vec::len), Some(1));
    let workflow_id = created[0]["id"].as_str().expect("workflow id").to_string();
    assert_eq!(created[0]["name"], "Research");
    assert_eq!(created[0]["sourceRef"], "local");
    assert_eq!(created[0]["isEnabledForAgent"], true);

    let listed = call(
        Arc::clone(&workers),
        "getAgentWorkflows",
        serde_json::json!({"id": agent.id}),
    );
    assert_eq!(listed[0]["id"], workflow_id);

    let updated = call(
        Arc::clone(&workers),
        "updateAgentWorkflow",
        serde_json::json!({
            "id": agent.id,
            "workflowId": workflow_id,
            "spec": {
                "name": "Research updated",
                "description": "Updated",
                "body": "Read more sources and summarize."
            }
        }),
    );
    assert_eq!(updated[0]["name"], "Research updated");

    let disabled = call(
        Arc::clone(&workers),
        "setAgentWorkflowEnabled",
        serde_json::json!({
            "id": agent.id,
            "workflowId": workflow_id,
            "isEnabled": false
        }),
    );
    assert_eq!(disabled[0]["isEnabledForAgent"], false);

    let disabled_record = call(
        Arc::clone(&workers),
        "getAgentWorkflow",
        serde_json::json!({
            "id": agent.id,
            "workflowId": workflow_id
        }),
    );
    assert_eq!(disabled_record["id"], workflow_id);
    assert_eq!(disabled_record["name"], "Research updated");
    assert_eq!(disabled_record["isEnabledForAgent"], false);

    let imported = call(
        Arc::clone(&workers),
        "importAgentWorkflowText",
        serde_json::json!({
            "id": agent.id,
            "name": "Imported text",
            "markdown": "---\nname: Imported\n---\nDo the imported thing."
        }),
    );
    assert_eq!(imported["result"]["imported"].as_array().map(Vec::len), Some(1));

    let linked = call(
        Arc::clone(&workers),
        "importAgentWorkflowUrl",
        serde_json::json!({
            "id": agent.id,
            "url": "https://example.test/SKILL.md",
            "name": "Live skill"
        }),
    );
    assert_eq!(linked["result"]["imported"].as_array().map(Vec::len), Some(1));
    assert!(linked["workflows"]
        .as_array()
        .expect("workflow list")
        .iter()
        .any(|workflow| workflow["sourceRef"] == "https://example.test/SKILL.md"));

    let after_delete = call(
        Arc::clone(&workers),
        "deleteAgentWorkflow",
        serde_json::json!({
            "id": agent.id,
            "workflowId": workflow_id
        }),
    );
    assert!(!after_delete
        .as_array()
        .expect("workflow list")
        .iter()
        .any(|workflow| workflow["id"] == workflow_id));

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn automation_backed_workflow_creation_is_owned_by_rust_session_store() {
    let root = temp_root("automation-create");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let session = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = session.create_session(None, "user", None).expect("agent");

    let created = call(
        Arc::clone(&workers),
        "createAgentWorkflow",
        serde_json::json!({
            "id": agent.id,
            "spec": {
                "name": "Scheduled",
                "body": "Run later",
                "trigger": {"schedule":"0 9 * * *","isEnabled":true}
            }
        }),
    );
    let created = created.as_array().expect("workflow list");
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["name"], "Scheduled");
    assert_eq!(created[0]["source"], "automation");
    assert_eq!(created[0]["trigger"]["schedule"], "0 9 * * *");
    assert_eq!(created[0]["trigger"]["isEnabled"], true);

    let listed = call(
        Arc::clone(&workers),
        "getAgentWorkflows",
        serde_json::json!({"id": agent.id}),
    );
    assert!(listed
        .as_array()
        .expect("workflow list")
        .iter()
        .any(|workflow| workflow["source"] == "automation"));

    let automation_id = created[0]["id"].as_str().expect("automation id");
    let plan = prepare_workflow_run_now(
        Arc::clone(&workers),
        &serde_json::json!({"id":agent.id,"workflowId":automation_id}),
    )
    .expect("automation plan")
    .expect("automation exists");
    assert_eq!(
        plan,
        WorkflowRunNowPlan::Automation {
            agent_id: agent.id.clone(),
            automation_id: automation_id.to_string(),
            automation_name: "Scheduled".to_string(),
        }
    );
    assert!(prepare_workflow_run_now(
        Arc::clone(&workers),
        &serde_json::json!({"id":agent.id,"workflowId":"missing"}),
    )
    .expect("missing workflow lookup")
    .is_none());

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}


#[test]
fn normal_send_expands_enabled_rich_text_workflow_reference() {
    let root = temp_root("normal-send-reference");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let session = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = session.create_session(None, "user", None).expect("agent");
    let created = call(
        Arc::clone(&workers),
        "createAgentWorkflow",
        serde_json::json!({
            "id": agent.id,
            "spec": {
                "name": "Research",
                "description": "Read sources",
                "body": "Collect evidence and summarize."
            }
        }),
    );
    let workflow_id = created[0]["id"].as_str().expect("workflow id");
    let rich_text = serde_json::json!({
        "type":"doc",
        "content":[{
            "type":"paragraph",
            "content":[{
                "type":"workflowReference",
                "attrs":{"id":workflow_id,"label":"Research"}
            }]
        }]
    })
    .to_string();

    let expanded = expand_workflow_references(
        Arc::clone(&workers),
        &agent.id,
        "also compare the results",
        Some(&rich_text),
    )
    .expect("workflow expansion");
    assert!(expanded.contains("The user invoked the \"Research\" workflow"));
    assert!(expanded.contains("Collect evidence and summarize."));
    assert!(expanded.ends_with("also compare the results"));

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn workflow_run_now_reference_plan_matches_frozen_rich_text_and_recipe_expansion() {
    let root = temp_root("run-now-reference");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(
        root.join("agents"),
        500,
    ));
    let session = SandAgentSessionStore::new(Arc::clone(&workers));
    let agent = session.create_session(None, "user", None).expect("agent");
    let created = call(
        Arc::clone(&workers),
        "createAgentWorkflow",
        serde_json::json!({
            "id": agent.id,
            "spec": {
                "name": "Research",
                "description": "Read sources",
                "body": "Collect evidence and summarize."
            }
        }),
    );
    let workflow_id = created[0]["id"].as_str().expect("workflow id").to_string();

    let plan = prepare_workflow_run_now(
        Arc::clone(&workers),
        &serde_json::json!({"id":agent.id,"workflowId":workflow_id}),
    )
    .expect("plan")
    .expect("workflow");
    let WorkflowRunNowPlan::Reference {
        agent_id,
        workflow_id,
        workflow_name,
        visible_prompt,
        rich_text,
        runtime_prompt,
    } = plan
    else {
        panic!("expected reference plan");
    };
    assert_eq!(agent_id, agent.id);
    assert_eq!(workflow_name, "Research");
    assert_eq!(visible_prompt, "@Research");
    assert!(rich_text.contains("\"type\":\"workflowReference\""));
    assert!(rich_text.contains(&format!("\"id\":\"{workflow_id}\"")));
    assert!(runtime_prompt.contains(
        "The user invoked the \"Research\" workflow (folder"
    ));
    assert!(runtime_prompt.contains("What it does: Read sources"));
    assert!(runtime_prompt.contains("Recipe to follow:\nCollect evidence and summarize."));
    assert!(runtime_prompt.ends_with("@Research"));

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn workflow_run_prompt_clamps_recipe_body_and_preserves_helper_location() {
    let body = "x".repeat(WORKFLOW_INJECTED_BODY_LIMIT + 50);
    let record = WorkflowRecord {
        id: "deep-research".into(),
        name: "Deep Research".into(),
        description: String::new(),
        body,
        trigger: None,
        source: "plugin".into(),
        source_ref: None,
        plugin_id: Some("plugin-deep-research".into()),
        published_by_current_user: true,
        disable_model_invocation: false,
        is_enabled_for_agent: true,
        created_at: 1.0,
        last_run_at: None,
        next_run_at: None,
        helper_scripts: vec!["collect.sh".into(), "parse.py".into()],
        file_path: std::path::PathBuf::from("/tmp/workflows/deep-research/SKILL.md"),
    };
    let prompt = build_workflow_run_prompt(&record);
    assert!(prompt.contains(
        "plugin skill id deep-research, file /tmp/workflows/deep-research/SKILL.md"
    ));
    assert!(prompt.contains(
        "Helper files live beside this workflow in /tmp/workflows/deep-research: collect.sh, parse.py."
    ));
    let recipe = prompt
        .split("Recipe to follow:\n")
        .nth(1)
        .expect("recipe")
        .split("\nHelper files")
        .next()
        .expect("recipe body");
    assert_eq!(recipe.chars().count(), WORKFLOW_INJECTED_BODY_LIMIT);
}
