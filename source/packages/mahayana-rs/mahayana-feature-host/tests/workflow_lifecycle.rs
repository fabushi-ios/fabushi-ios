use mahayana_feature_host::FeatureHostController;
use mahayana_host_protocol::{FeatureCommand, HostConfig, HostMode, SurfacePlatform};
use uuid::Uuid;

fn controller() -> FeatureHostController {
    FeatureHostController::create(
        HostConfig {
            profile_id: format!("workflow-crud-{}", Uuid::new_v4()),
            mode: HostMode::Test,
        },
        SurfacePlatform::Ios,
    )
    .expect("create canonical test-mode Feature Host")
}

#[test]
fn workflow_skill_crud_and_agent_enablement_round_trip_through_production_commands() {
    let host = controller();
    let agent_id = "mahayana-assistant".to_string();
    let workflow_id = "release-check".to_string();

    host.execute(FeatureCommand::WorkflowUpsert {
        request_id: "workflow-create".into(),
        agent_id: agent_id.clone(),
        id: Some(workflow_id.clone()),
        name: "Release check".into(),
        description: "Verify production evidence.".into(),
        body: "Inspect the release evidence before shipping.".into(),
        trigger: None,
        source_ref: None,
    })
    .expect("create workflow through canonical Host command");

    host.execute(FeatureCommand::WorkflowUpsert {
        request_id: "workflow-update".into(),
        agent_id: agent_id.clone(),
        id: Some(workflow_id.clone()),
        name: "Release check".into(),
        description: "Verify exact production evidence.".into(),
        body: "Inspect exact-head release evidence before shipping.".into(),
        trigger: None,
        source_ref: Some("local://release-check".into()),
    })
    .expect("update workflow through canonical Host command");

    host.execute(FeatureCommand::WorkflowSetEnabled {
        request_id: "workflow-disable".into(),
        agent_id: agent_id.clone(),
        id: workflow_id.clone(),
        enabled: false,
    })
    .expect("disable workflow through canonical Host command");

    let disabled = host
        .execute(FeatureCommand::WorkflowRun {
            request_id: "workflow-run-disabled".into(),
            agent_id: agent_id.clone(),
            id: workflow_id.clone(),
        })
        .expect_err("disabled workflow must not run");
    assert!(
        disabled.to_string().contains("workflow is disabled"),
        "unexpected disabled workflow error: {disabled}"
    );

    host.execute(FeatureCommand::WorkflowSetEnabled {
        request_id: "workflow-enable".into(),
        agent_id: agent_id.clone(),
        id: workflow_id.clone(),
        enabled: true,
    })
    .expect("enable workflow through canonical Host command");

    host.execute(FeatureCommand::WorkflowRun {
        request_id: "workflow-run-enabled".into(),
        agent_id: agent_id.clone(),
        id: workflow_id.clone(),
    })
    .expect("enabled workflow reaches the Agent execution owner");

    host.execute(FeatureCommand::WorkflowDelete {
        request_id: "workflow-delete".into(),
        agent_id: agent_id.clone(),
        id: workflow_id.clone(),
    })
    .expect("delete workflow through canonical Host command");

    let deleted = host
        .execute(FeatureCommand::WorkflowRun {
            request_id: "workflow-run-deleted".into(),
            agent_id,
            id: workflow_id,
        })
        .expect_err("deleted workflow must not run");
    assert!(
        deleted.to_string().contains("unknown workflow"),
        "unexpected deleted workflow error: {deleted}"
    );
}
