use mahayana_feature_host::FeatureHostController;
use mahayana_host_protocol::{FeatureCommand, HostConfig, HostMode, SurfacePlatform};
use serde_json::json;
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


#[test]
fn installed_plugin_skills_are_replace_all_read_only_and_runnable() {
    let host = controller();
    let agent_id = "mahayana-assistant";
    host.sync_installed_plugin_skills(
        agent_id,
        json!([{
            "pluginId": "42",
            "pluginVersion": "0123456789abcdef0123456789abcdef01234567",
            "workflowId": "plugin-42-release-review",
            "skillId": "release-review",
            "name": "Release review",
            "description": "Review exact-head release evidence.",
            "markdown": "---\nname: Release review\ndescription: Review exact-head release evidence.\n---\nInspect the exact-head CI evidence before shipping.",
            "sourceRef": "https://github.com/example/plugin/blob/0123456789abcdef0123456789abcdef01234567/skills/release-review/SKILL.md",
            "publishedByCurrentUser": false
        }]),
    )
    .expect("sync installed plugin skills");
    host.execute(FeatureCommand::WorkflowRun {
        request_id: "plugin-skill-run".into(),
        agent_id: agent_id.into(),
        id: "plugin-42-release-review".into(),
    })
    .expect("installed plugin skill reaches canonical workflow runner");
    let disable = host.execute(FeatureCommand::WorkflowSetEnabled {
        request_id: "plugin-skill-disable".into(),
        agent_id: agent_id.into(),
        id: "plugin-42-release-review".into(),
        enabled: false,
    }).expect_err("plugin-managed skill enablement must be read-only");
    assert!(disable.to_string().contains("read-only"));
    let delete = host.execute(FeatureCommand::WorkflowDelete {
        request_id: "plugin-skill-delete".into(),
        agent_id: agent_id.into(),
        id: "plugin-42-release-review".into(),
    }).expect_err("plugin-managed skill deletion must be read-only");
    assert!(delete.to_string().contains("read-only"));
    host.sync_installed_plugin_skills(agent_id, json!([]))
        .expect("replace installed plugin skills with empty authoritative snapshot");
    let removed = host.execute(FeatureCommand::WorkflowRun {
        request_id: "plugin-skill-run-after-remove".into(),
        agent_id: agent_id.into(),
        id: "plugin-42-release-review".into(),
    }).expect_err("removed installed plugin skill must no longer be runnable");
    assert!(removed.to_string().contains("unknown workflow"));
}

#[test]
fn installed_plugin_skills_reject_mutable_or_mismatched_source_identity() {
    let host = controller();
    let agent_id = "mahayana-assistant";
    let version = "0123456789abcdef0123456789abcdef01234567";
    let bad_cases = [
        (
            "main",
            Some("https://github.com/example/plugin/blob/main/skills/release-review/SKILL.md"),
        ),
        (
            version,
            Some("https://github.com/example/plugin/blob/main/skills/release-review/SKILL.md"),
        ),
        (
            version,
            Some("https://github.com/example/plugin/blob/ffffffffffffffffffffffffffffffffffffffff/skills/release-review/SKILL.md"),
        ),
        (version, None),
    ];

    for (plugin_version, source_ref) in bad_cases {
        let error = host
            .sync_installed_plugin_skills(
                agent_id,
                json!([{
                    "pluginId": "42",
                    "pluginVersion": plugin_version,
                    "workflowId": "plugin-42-release-review",
                    "skillId": "release-review",
                    "name": "Release review",
                    "description": "Review exact-head release evidence.",
                    "markdown": "---\nname: Release review\ndescription: Review exact-head release evidence.\n---\nInspect the exact-head CI evidence before shipping.",
                    "sourceRef": source_ref,
                    "publishedByCurrentUser": false
                }]),
            )
            .expect_err("mutable or mismatched Marketplace Skill identity must fail closed");
        assert!(
            error.to_string().contains("immutable"),
            "unexpected validation error: {error}"
        );
    }
}
