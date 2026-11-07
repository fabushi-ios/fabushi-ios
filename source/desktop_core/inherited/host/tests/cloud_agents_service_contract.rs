use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::cloud_agents::cloud_agent_poll_loop::SavedEnvironmentClient;
use mahayana_host_runtime::extensions::cloud_agents::cloud_agent_request_composition::{
    CloudAgentEnvironment, CloudAgentImageInput, SavedEnvironment, SavedEnvironmentRepoConfig,
};
use mahayana_host_runtime::extensions::cloud_agents::cloud_agent_wire::*;
use mahayana_host_runtime::extensions::cloud_agents::cloud_agents_service::*;
use mahayana_host_runtime::extensions::cloud_agents::model_catalog_fetch::SandModelCatalogEntry;
use serde_json::json;

#[derive(Default)]
struct FakeState {
    start: Option<StartBackgroundComposerRequestWire>,
    followup: Option<AddAsyncFollowupBackgroundComposerRequestWire>,
    paused: Vec<PauseBackgroundComposerRequestWire>,
    renamed: Vec<RenameBackgroundComposerRequestWire>,
    archived: Vec<ArchiveBackgroundComposerRequestWire>,
    deleted: Vec<DeleteBackgroundComposerRequestWire>,
}

struct FakeBackend {
    state: Arc<Mutex<FakeState>>,
    info: Mutex<Option<GetBackgroundComposerInfoResponseWire>>,
    list: Mutex<Vec<BackgroundComposerWire>>,
    environments: Mutex<Vec<SavedEnvironment>>,
    teams: Mutex<Vec<TeamWire>>,
    artifacts: Mutex<Vec<BackgroundComposerArtifactWire>>,
    conversation: Mutex<Vec<Vec<u8>>>,
    pr_status: Mutex<GetPullRequestMergeStatusResponseWire>,
    diff: Mutex<Vec<FileDiffWire>>,
}

impl FakeBackend {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState::default())),
            info: Mutex::new(None),
            list: Mutex::new(Vec::new()),
            environments: Mutex::new(Vec::new()),
            teams: Mutex::new(Vec::new()),
            artifacts: Mutex::new(Vec::new()),
            conversation: Mutex::new(Vec::new()),
            pr_status: Mutex::new(GetPullRequestMergeStatusResponseWire {
                is_merged: false,
                is_closed: false,
                state: "open".into(),
                is_draft: false,
            }),
            diff: Mutex::new(Vec::new()),
        }
    }
}

impl SavedEnvironmentClient for FakeBackend {
    fn get_environment(&self, public_id: &str) -> Result<Option<SavedEnvironment>, String> {
        Ok(self
            .environments
            .lock()
            .unwrap()
            .iter()
            .find(|value| value.public_id == public_id)
            .cloned())
    }

    fn list_environments(&self, _limit: usize) -> Result<Vec<SavedEnvironment>, String> {
        Ok(self.environments.lock().unwrap().clone())
    }
}

impl CloudAgentBackend for FakeBackend {
    fn start_background_composer(
        &self,
        request: StartBackgroundComposerRequestWire,
    ) -> Result<StartBackgroundComposerResponseWire, CloudAgentBackendError> {
        self.state.lock().unwrap().start = Some(request.clone());
        Ok(StartBackgroundComposerResponseWire {
            composer: Some(BackgroundComposerWire {
                bc_id: request.bc_id.clone(),
                ..BackgroundComposerWire::default()
            }),
            initial_run_id: Some("run-1".into()),
        })
    }

    fn get_background_composer_info(
        &self,
        _request: GetBackgroundComposerInfoRequestWire,
    ) -> Result<GetBackgroundComposerInfoResponseWire, CloudAgentBackendError> {
        Ok(self
            .info
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(GetBackgroundComposerInfoResponseWire { composer: None }))
    }

    fn list_background_composers(
        &self,
        _request: ListBackgroundComposersRequestWire,
    ) -> Result<ListBackgroundComposersResponseWire, CloudAgentBackendError> {
        Ok(ListBackgroundComposersResponseWire {
            composers: self.list.lock().unwrap().clone(),
        })
    }

    fn add_async_followup(
        &self,
        request: AddAsyncFollowupBackgroundComposerRequestWire,
    ) -> Result<AddAsyncFollowupBackgroundComposerResponseWire, CloudAgentBackendError> {
        self.state.lock().unwrap().followup = Some(request);
        Ok(AddAsyncFollowupBackgroundComposerResponseWire {
            run_id: "followup-1".into(),
        })
    }

    fn pause_background_composer(
        &self,
        request: PauseBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        self.state.lock().unwrap().paused.push(request);
        Ok(())
    }

    fn rename_background_composer(
        &self,
        request: RenameBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        self.state.lock().unwrap().renamed.push(request);
        Ok(())
    }

    fn archive_background_composer(
        &self,
        request: ArchiveBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        self.state.lock().unwrap().archived.push(request);
        Ok(())
    }

    fn delete_background_composer(
        &self,
        request: DeleteBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        self.state.lock().unwrap().deleted.push(request);
        Ok(())
    }

    fn list_background_composer_artifacts(
        &self,
        _request: ListBackgroundComposerArtifactsRequestWire,
    ) -> Result<ListBackgroundComposerArtifactsResponseWire, CloudAgentBackendError> {
        Ok(ListBackgroundComposerArtifactsResponseWire {
            artifacts: self.artifacts.lock().unwrap().clone(),
        })
    }

    fn get_background_composer_conversation(
        &self,
        _request: GetBackgroundComposerConversationRequestWire,
    ) -> Result<GetBackgroundComposerConversationResponseWire, CloudAgentBackendError> {
        Ok(GetBackgroundComposerConversationResponseWire {
            conversation: self.conversation.lock().unwrap().clone(),
        })
    }

    fn get_pull_request_merge_status(
        &self,
        _request: GetPullRequestMergeStatusRequestWire,
    ) -> Result<GetPullRequestMergeStatusResponseWire, CloudAgentBackendError> {
        Ok(self.pr_status.lock().unwrap().clone())
    }

    fn get_optimized_diff_details(
        &self,
        _request: GetOptimizedDiffDetailsRequestWire,
    ) -> Result<GetOptimizedDiffDetailsResponseWire, CloudAgentBackendError> {
        Ok(GetOptimizedDiffDetailsResponseWire {
            diff: Some(GitDiffWire {
                diffs: self.diff.lock().unwrap().clone(),
            }),
        })
    }

    fn list_teams(&self) -> Result<Vec<TeamWire>, CloudAgentBackendError> {
        Ok(self.teams.lock().unwrap().clone())
    }

    fn cloud_agents_disabled_by_team_admin(&self) -> Result<bool, CloudAgentBackendError> {
        Ok(false)
    }
}

fn manager(backend: Arc<FakeBackend>) -> Arc<SandCloudAgentManager> {
    SandCloudAgentManager::with_backend(
        backend,
        Arc::new(|| {
            Ok(vec![SandModelCatalogEntry {
                id: "model-x".into(),
                display_name: Some("Model X".into()),
                aliases: vec!["x".into()],
                params: Vec::new(),
                variants: Vec::new(),
            }])
        }),
        Arc::new(|messages| {
            Ok(messages
                .iter()
                .enumerate()
                .map(|(index, bytes)| json!({"index": index, "bytes": bytes}))
                .collect())
        }),
    )
}

#[test]
fn launch_projects_frozen_repo_model_image_environment_and_source_fields() {
    let backend = Arc::new(FakeBackend::new());
    backend.teams.lock().unwrap().push(TeamWire {
        name: "Acme".into(),
        id: 7,
        is_direct_member: true,
    });
    let service = manager(Arc::clone(&backend));

    let result = service
        .launch(CloudAgentLaunchArgs {
            prompt: "ship it".into(),
            repo_url: Some("Acme/Repo".into()),
            starting_ref: Some("main".into()),
            environment: Some(CloudAgentEnvironment::Pool {
                name: Some("gpu".into()),
                team_id: None,
            }),
            model_id: Some("model-x".into()),
            model_params: BTreeMap::from([("thinking".into(), "high".into())]),
            images: vec![CloudAgentImageInput {
                data: vec![1, 2, 3],
                path: Some("/workspace/shot.png".into()),
                mime_type: Some("image/png".into()),
            }],
            title: Some("  Fix CI  ".into()),
        })
        .unwrap();

    assert!(result.bc_id.starts_with("bc-"));
    assert_eq!(result.url, format!("https://cursor.com/agents/{}", result.bc_id));
    assert!(service.is_managed_id(&result.bc_id));

    let request = backend.state.lock().unwrap().start.clone().unwrap();
    assert_eq!(request.snapshot_name_or_id, "github.com/acme/repo");
    assert_eq!(request.snapshot_workspace_root_path, "/workspace");
    assert!(request.auto_branch);
    assert!(request.return_immediately);
    assert_eq!(request.source, Some(BACKGROUND_COMPOSER_SOURCE_GROK_BOT));
    assert_eq!(request.starting_message_type, Some(STARTING_MESSAGE_TYPE_USER_MESSAGE));
    assert_eq!(request.auto_create_pr, Some(true));
    assert_eq!(request.add_initial_message_to_responses, Some(true));
    assert_eq!(request.team_id, Some(7));
    assert_eq!(request.name.as_deref(), Some("Fix CI"));
    assert_eq!(request.requested_models[0].model_id, "model-x");
    assert_eq!(request.requested_models[0].parameters[0].id, "thinking");
    assert_eq!(request.labels[0].key, "repo");
    assert_eq!(request.labels[0].value, "github.com/acme/repo");
    assert_eq!(request.labels[1].key, "pool");
    assert_eq!(request.labels[1].value, "gpu");

    let starting = request.devcontainer_starting_point.unwrap();
    assert_eq!(starting.url, "https://github.com/Acme/Repo");
    assert_eq!(starting.r#ref, "main");
    let action = request.conversation_action.unwrap().user_message_action.unwrap();
    assert_eq!(action.send_to_interaction_listener, Some(true));
    let message = action.user_message.unwrap();
    assert_eq!(message.text, "ship it");
    assert_eq!(message.mode, AGENT_MODE_AGENT);
    assert_eq!(
        message.selected_context.unwrap().selected_images[0].data,
        vec![1, 2, 3]
    );
}

#[test]
fn saved_environment_and_named_machine_rules_match_frozen_manager() {
    let backend = Arc::new(FakeBackend::new());
    backend.environments.lock().unwrap().push(SavedEnvironment {
        public_id: "env-1".into(),
        name: "Prod".into(),
        repo_config: Some(SavedEnvironmentRepoConfig { repos: Vec::new() }),
    });
    let service = manager(backend);

    let error = service
        .launch(CloudAgentLaunchArgs {
            prompt: "x".into(),
            repo_url: Some("acme/repo".into()),
            starting_ref: Some("main".into()),
            environment: Some(CloudAgentEnvironment::Machine {
                name: "mac-pro".into(),
                team_id: None,
            }),
            model_id: None,
            model_params: BTreeMap::new(),
            images: Vec::new(),
            title: None,
        })
        .unwrap_err();
    assert!(error.message.contains("runs on its own checkout"));

    let missing = service
        .launch(CloudAgentLaunchArgs {
            prompt: "x".into(),
            repo_url: None,
            starting_ref: None,
            environment: Some(CloudAgentEnvironment::Environment {
                public_id: Some("missing".into()),
                name: None,
            }),
            model_id: None,
            model_params: BTreeMap::new(),
            images: Vec::new(),
            title: None,
        })
        .unwrap_err();
    assert!(missing.message.contains("No saved environment with id 'missing'"));
}

#[test]
fn list_get_reply_and_management_actions_use_single_backend_owner() {
    let backend = Arc::new(FakeBackend::new());
    backend.list.lock().unwrap().extend([
        BackgroundComposerWire {
            bc_id: "bc-open".into(),
            name: "Open".into(),
            branch_name: "work".into(),
            pr_url: "https://example/pr".into(),
            status: 1,
            created_at_ms: 12.0,
            ..BackgroundComposerWire::default()
        },
        BackgroundComposerWire {
            bc_id: "bc-old".into(),
            name: "Old".into(),
            is_archived: true,
            status: 2,
            ..BackgroundComposerWire::default()
        },
    ]);
    *backend.info.lock().unwrap() = Some(GetBackgroundComposerInfoResponseWire {
        composer: Some(DetailedBackgroundComposerWire {
            composer: Some(BackgroundComposerWire {
                bc_id: "bc-open".into(),
                name: "Open".into(),
                branch_name: "work".into(),
                pr_url: "https://example/pr".into(),
                status: 2,
                files_changed: Some(2),
                lines_added: Some(9),
                lines_removed: Some(3),
                ..BackgroundComposerWire::default()
            }),
            prompt: Some(HeadlessPromptWire { text: "prompt".into() }),
            status: 2,
            summary: Some("done".into()),
            permanent_error: None,
            prs: Vec::new(),
        }),
    });
    let service = manager(Arc::clone(&backend));

    let list = service.list(Some(20), false).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].bc_id, "bc-open");
    assert_eq!(list[0].status, "running");

    let detail = service.get("bc-open").unwrap().unwrap();
    assert_eq!(detail.files_changed, 2);
    assert_eq!(detail.lines_added, 9);

    let run_id = service
        .reply(CloudAgentReplyArgs {
            bc_id: "bc-open".into(),
            prompt: "continue".into(),
            images: Vec::new(),
            interrupt: true,
            model_id: None,
            model_params: BTreeMap::new(),
        })
        .unwrap();
    assert_eq!(run_id, "followup-1");
    assert!(service.is_managed_id("bc-open"));
    let followup = backend.state.lock().unwrap().followup.clone().unwrap();
    assert!(followup.synchronous);
    assert_eq!(followup.followup_source, Some(BACKGROUND_COMPOSER_SOURCE_GROK_BOT));

    service.cancel("bc-open").unwrap();
    service.rename("bc-open", "New").unwrap();
    service.set_archived("bc-open", true).unwrap();
    service.set_archived("bc-open", false).unwrap();
    service.delete("bc-open").unwrap();

    let state = backend.state.lock().unwrap();
    assert_eq!(state.paused[0].source, BACKGROUND_COMPOSER_SOURCE_GROK_BOT);
    assert_eq!(state.renamed[0].new_name, "New");
    assert!(!state.archived[0].unarchive);
    assert!(state.archived[1].unarchive);
    assert_eq!(state.deleted[0].bc_id, "bc-open");
}

#[test]
fn info_artifacts_diff_pr_and_transcript_dump_preserve_frozen_projections() {
    let backend = Arc::new(FakeBackend::new());
    *backend.info.lock().unwrap() = Some(GetBackgroundComposerInfoResponseWire {
        composer: Some(DetailedBackgroundComposerWire {
            composer: Some(BackgroundComposerWire {
                bc_id: "bc-1".into(),
                name: "Agent".into(),
                branch_name: "branch".into(),
                pr_url: "https://example/pr".into(),
                status: 2,
                files_changed: Some(3),
                lines_added: Some(11),
                lines_removed: Some(4),
                pr_status: Some(1),
                ..BackgroundComposerWire::default()
            }),
            prompt: Some(HeadlessPromptWire {
                text: "original".into(),
            }),
            status: 2,
            summary: None,
            permanent_error: None,
            prs: vec![BackgroundComposerPrWire {
                branch_name: "branch".into(),
                pull_number: Some(42),
                pr_status: Some(1),
                pr_url: Some("https://example/pr".into()),
            }],
        }),
    });
    backend.diff.lock().unwrap().extend([
        FileDiffWire {
            from: "/dev/null".into(),
            to: "new.rs".into(),
            added: 8,
            removed: 0,
        },
        FileDiffWire {
            from: "old.rs".into(),
            to: "/dev/null".into(),
            added: 0,
            removed: 4,
        },
    ]);
    backend.artifacts.lock().unwrap().push(BackgroundComposerArtifactWire {
        absolute_path: "/workspace/report.txt".into(),
        size_bytes: 123,
    });
    backend.conversation.lock().unwrap().extend([vec![1, 2], vec![3, 4]]);

    let service = manager(backend);
    let info = service.get_info("bc-1", true).unwrap().unwrap();
    assert_eq!(info.prompt, "original");
    assert_eq!(info.pr_state, "open");
    assert_eq!(info.pr_number, Some(42));
    assert_eq!(info.files[0].path, "new.rs");
    assert_eq!(info.files[1].path, "old.rs");

    let artifacts = service.list_artifacts("bc-1").unwrap();
    assert_eq!(artifacts[0].path, "/workspace/report.txt");
    assert_eq!(artifacts[0].size_bytes, 123);

    let dump = service.get_transcript_dump("bc-1").unwrap().unwrap();
    assert_eq!(dump.status, "finished");
    assert_eq!(dump.line_count, 2);
    assert!(dump.jsonl.contains("\"index\":0"));
    assert!(dump.jsonl.ends_with('\n'));
}

#[test]
fn model_catalog_and_private_worker_team_selection_are_manager_owned() {
    let backend = Arc::new(FakeBackend::new());
    backend.teams.lock().unwrap().extend([
        TeamWire {
            name: "One".into(),
            id: 1,
            is_direct_member: true,
        },
        TeamWire {
            name: "Ignored".into(),
            id: 2,
            is_direct_member: false,
        },
    ]);
    let service = manager(backend);
    assert_eq!(service.list_models().unwrap()[0].id, "model-x");
    assert_eq!(
        service
            .resolve_private_worker_team_id(Some(&CloudAgentEnvironment::Pool {
                name: None,
                team_id: None,
            }))
            .unwrap(),
        Some(1)
    );
}

#[test]
fn helper_projection_caps_diffs_and_normalizes_dev_null() {
    assert_eq!(map_run_status(1), "running");
    assert_eq!(map_run_status(5), "expired");
    assert_eq!(map_run_status(99), "unknown");
    assert_eq!(normalize_diff_path("/dev/null"), "");
    assert_eq!(normalize_diff_path(" src/lib.rs "), "src/lib.rs");
    assert_eq!(cloud_agent_url("bc-x"), "https://cursor.com/agents/bc-x");
    assert_eq!(MAX_CLOUD_AGENT_FILES, 300);
}


#[test]
fn managed_id_lifecycle_forgets_deleted_agents() {
    let service = manager(Arc::new(FakeBackend::new()));
    assert!(!service.is_managed_id("bc-managed"));
    service.remember_managed_id("bc-managed");
    assert!(service.is_managed_id("bc-managed"));
    assert!(service.launched_ids_snapshot().contains("bc-managed"));
    service.forget_managed_id("bc-managed");
    assert!(!service.is_managed_id("bc-managed"));
    assert!(!service.launched_ids_snapshot().contains("bc-managed"));
}
