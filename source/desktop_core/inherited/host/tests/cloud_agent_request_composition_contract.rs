use std::collections::BTreeMap;

use mahayana_host_runtime::extensions::cloud_agents::cloud_agent_request_composition::{
    CloudAgentEnvironment, CloudAgentImageInput, SavedEnvironment, SavedEnvironmentRepo,
    SavedEnvironmentRepoConfig, build_cloud_agent_conversation_action,
    build_cloud_agent_requested_model, build_cloud_agent_user_message, build_repo_from_remote,
    normalize_repo_reference, resolve_cloud_agent_environment_fields,
    resolve_launch_repo_reference, sanitize_remote_url, sanitize_remote_url_for_http_url,
    to_start_repo_config,
};
use serde_json::json;

#[test]
fn requested_model_and_user_message_follow_frozen_contract() {
    let params = BTreeMap::from([("thinking".into(), "high".into())]);
    let error = build_cloud_agent_requested_model(None, Some(&params)).unwrap_err();
    assert_eq!(
        error.message,
        "'model' is required when 'model_params' are provided."
    );

    let requested =
        build_cloud_agent_requested_model(Some("  model-x  "), Some(&params)).unwrap().unwrap();
    assert_eq!(requested["modelId"], "model-x");
    assert_eq!(requested["maxMode"], true);
    assert_eq!(requested["parameters"][0]["id"], "thinking");
    assert_eq!(requested["parameters"][0]["value"], "high");

    let message = build_cloud_agent_user_message(
        "ship it",
        Some(json!("agent")),
        &[CloudAgentImageInput {
            data: vec![1, 2, 3],
            path: Some("/workspace/shot.png".into()),
            mime_type: Some("image/png".into()),
        }],
    );
    assert_eq!(message["text"], "ship it");
    assert_eq!(message["mode"], "agent");
    assert_eq!(
        message["selectedContext"]["selectedImages"][0]["dataOrBlobId"]["case"],
        "data"
    );
    assert_eq!(
        message["selectedContext"]["selectedImages"][0]["path"],
        "/workspace/shot.png"
    );

    let action = build_cloud_agent_conversation_action(message);
    assert_eq!(action["action"]["case"], "userMessageAction");
    assert_eq!(
        action["action"]["value"]["sendToInteractionListener"],
        true
    );
}

#[test]
fn repository_normalization_matches_https_ssh_and_short_github_forms() {
    assert_eq!(
        normalize_repo_reference("openai/codex"),
        "https://github.com/openai/codex"
    );
    assert_eq!(
        sanitize_remote_url("git@github.com:OpenAI/Codex.git"),
        "github.com/openai/codex"
    );
    assert_eq!(
        sanitize_remote_url("https://USER:secret@GitHub.com/OpenAI/Codex.git/"),
        "github.com/openai/codex"
    );
    assert_eq!(
        sanitize_remote_url_for_http_url("git@github.com:OpenAI/Codex.git"),
        "https://github.com/OpenAI/Codex"
    );

    let repo = build_repo_from_remote("OpenAI/Codex", Some("main")).unwrap();
    assert_eq!(repo.sanitized_repo_url, "github.com/openai/codex");
    assert_eq!(repo.http_repo_url, "https://github.com/OpenAI/Codex");
    assert_eq!(repo.base_branch.as_deref(), Some("main"));
}

fn saved_environment() -> SavedEnvironment {
    SavedEnvironment {
        public_id: "env-1".into(),
        name: "Production".into(),
        repo_config: Some(SavedEnvironmentRepoConfig {
            repos: vec![
                SavedEnvironmentRepo {
                    repo_url: "https://github.com/acme/primary.git".into(),
                    scm_repo_node_id: Some("node-1".into()),
                    git_enterprise_uuid: None,
                },
                SavedEnvironmentRepo {
                    repo_url: "git@github.com:acme/secondary.git".into(),
                    scm_repo_node_id: None,
                    git_enterprise_uuid: Some("enterprise-1".into()),
                },
            ],
        }),
    }
}

#[test]
fn saved_environment_enforces_primary_repository_semantics() {
    let saved = saved_environment();
    assert_eq!(
        resolve_launch_repo_reference(None, Some(&saved)).unwrap(),
        "https://github.com/acme/primary.git"
    );
    assert_eq!(
        resolve_launch_repo_reference(
            Some("git@github.com:acme/primary.git"),
            Some(&saved)
        )
        .unwrap(),
        "https://github.com/acme/primary.git"
    );

    let error = resolve_launch_repo_reference(
        Some("https://github.com/acme/secondary"),
        Some(&saved),
    )
    .unwrap_err();
    assert!(error.message.contains("is a secondary repo"));
    assert!(error.message.contains("primary repo"));

    let config = to_start_repo_config(saved.repo_config.as_ref()).unwrap();
    assert_eq!(config["repos"][0]["scmRepoNodeId"], "node-1");
    assert_eq!(config["repos"][1]["gitEnterpriseUuid"], "enterprise-1");
}

#[test]
fn private_worker_environment_fields_preserve_frozen_routing_labels() {
    let pool = resolve_cloud_agent_environment_fields(
        "https://github.com/Acme/Repo.git",
        Some(&CloudAgentEnvironment::Pool {
            name: Some("gpu".into()),
            team_id: Some(7),
        }),
    )
    .unwrap();
    assert_eq!(pool["usePrivateWorker"], true);
    assert_eq!(pool["labels"][0]["key"], "repo");
    assert_eq!(pool["labels"][0]["value"], "github.com/acme/repo");
    assert_eq!(pool["labels"][1]["value"], "gpu");

    let machine = resolve_cloud_agent_environment_fields(
        "https://github.com/acme/repo",
        Some(&CloudAgentEnvironment::Machine {
            name: "mac-pro".into(),
            team_id: None,
        }),
    )
    .unwrap();
    assert_eq!(machine["labels"][0]["value"], "mac-pro");
    assert_eq!(
        machine["labels"][1]["key"],
        "cursor.private_worker.shared_assignment_allowed"
    );
    assert_eq!(machine["labels"][1]["value"], "true");

    let error = resolve_cloud_agent_environment_fields(
        "https://github.com/acme/repo",
        Some(&CloudAgentEnvironment::Machine {
            name: "   ".into(),
            team_id: None,
        }),
    )
    .unwrap_err();
    assert_eq!(
        error.message,
        "A private-worker machine environment requires a registered name."
    );
}
