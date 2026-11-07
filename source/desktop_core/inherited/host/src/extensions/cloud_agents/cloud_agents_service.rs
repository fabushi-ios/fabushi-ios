use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use prost::Message;
use serde_json::Value;
use uuid::Uuid;

use crate::cursor_backend::{
    CursorBackendError, resolve_sand_ghost_mode_header, send_cursor_unary,
};
use crate::extensions::auth::extension::HostAuthExtension;

use super::cloud_agent_launch_error::SandCloudAgentLaunchError;
use super::cloud_agent_poll_loop::{
    CLOUD_AGENT_POLL_RPC_TIMEOUT_MS, CloudAgentCompletionPoller, CloudAgentComposer,
    CloudAgentInfoFetcher, CloudAgentModelCatalogCache, CloudAgentPermanentErrorDetails,
    CloudAgentPollError, CloudAgentPr, CloudAgentRunStatus, CloudAgentTeamAdminPolicyCache,
    DetailedComposer, ModelCatalogLoader, SavedEnvironmentClient, TeamAdminPolicyLoader,
    resolve_saved_environment,
};
use super::cloud_agent_request_composition::{
    CloudAgentEnvironment, CloudAgentImageInput, SavedEnvironment, SavedEnvironmentRepo,
    SavedEnvironmentRepoConfig, build_cloud_agent_requested_model, build_cloud_agent_user_message,
    build_repo_from_remote, resolve_cloud_agent_environment_fields, resolve_launch_repo_reference,
};
use super::cloud_agent_wire::*;
use super::model_catalog_fetch::{
    SandModelCatalogEntry, fetch_sand_model_catalog,
};

pub const MAX_CLOUD_AGENT_FILES: usize = 300;
pub const CLOUD_AGENT_URL_PREFIX: &str = "https://cursor.com/agents/";

pub const START_BACKGROUND_COMPOSER_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/StartBackgroundComposerFromSnapshot";
pub const GET_BACKGROUND_COMPOSER_INFO_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/GetBackgroundComposerInfo";
pub const LIST_BACKGROUND_COMPOSERS_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/ListBackgroundComposers";
pub const ADD_ASYNC_FOLLOWUP_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/AddAsyncFollowupBackgroundComposer";
pub const PAUSE_BACKGROUND_COMPOSER_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/PauseBackgroundComposer";
pub const RENAME_BACKGROUND_COMPOSER_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/RenameBackgroundComposer";
pub const ARCHIVE_BACKGROUND_COMPOSER_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/ArchiveBackgroundComposer";
pub const DELETE_BACKGROUND_COMPOSER_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/DeleteBackgroundComposer";
pub const LIST_BACKGROUND_COMPOSER_ARTIFACTS_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/ListBackgroundComposerArtifacts";
pub const GET_BACKGROUND_COMPOSER_CONVERSATION_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/GetBackgroundComposerConversation";
pub const GET_PULL_REQUEST_MERGE_STATUS_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/GetPullRequestMergeStatus";
pub const GET_OPTIMIZED_DIFF_DETAILS_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/GetOptimizedDiffDetails";
pub const GET_ENVIRONMENT_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/GetEnvironment";
pub const LIST_ENVIRONMENTS_PATH: &str =
    "/aiserver.v1.BackgroundComposerService/ListEnvironments";
pub const GET_TEAMS_PATH: &str = "/aiserver.v1.DashboardService/GetTeams";
pub const GET_TEAM_ADMIN_SETTINGS_PATH: &str =
    "/aiserver.v1.DashboardService/GetTeamAdminSettingsOrEmptyIfNotInTeam";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudAgentBackendError {
    #[error("cloud agent resource not found: {0}")]
    NotFound(String),
    #[error("cloud agent backend is rate limited: {message}")]
    RateLimited {
        message: String,
        retry_after_ms: Option<u64>,
    },
    #[error("cloud agent backend failed: {0}")]
    Other(String),
}

impl CloudAgentBackendError {
    fn from_cursor(error: CursorBackendError) -> Self {
        match error {
            CursorBackendError::HttpStatus { status, body }
                if status == 403 || status == 404 =>
            {
                Self::NotFound(body)
            }
            CursorBackendError::HttpStatus { status: 429, body } => Self::RateLimited {
                message: body,
                retry_after_ms: None,
            },
            other => Self::Other(other.to_string()),
        }
    }

    fn as_poll_error(&self) -> CloudAgentPollError {
        match self {
            Self::RateLimited {
                message,
                retry_after_ms,
            } => CloudAgentPollError {
                message: message.clone(),
                is_rate_limit: true,
                retry_after_ms: *retry_after_ms,
            },
            other => CloudAgentPollError {
                message: other.to_string(),
                is_rate_limit: false,
                retry_after_ms: None,
            },
        }
    }
}

pub trait CloudAgentBackend: SavedEnvironmentClient + Send + Sync {
    fn start_background_composer(
        &self,
        request: StartBackgroundComposerRequestWire,
    ) -> Result<StartBackgroundComposerResponseWire, CloudAgentBackendError>;

    fn get_background_composer_info(
        &self,
        request: GetBackgroundComposerInfoRequestWire,
    ) -> Result<GetBackgroundComposerInfoResponseWire, CloudAgentBackendError>;

    fn list_background_composers(
        &self,
        request: ListBackgroundComposersRequestWire,
    ) -> Result<ListBackgroundComposersResponseWire, CloudAgentBackendError>;

    fn add_async_followup(
        &self,
        request: AddAsyncFollowupBackgroundComposerRequestWire,
    ) -> Result<AddAsyncFollowupBackgroundComposerResponseWire, CloudAgentBackendError>;

    fn pause_background_composer(
        &self,
        request: PauseBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError>;

    fn rename_background_composer(
        &self,
        request: RenameBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError>;

    fn archive_background_composer(
        &self,
        request: ArchiveBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError>;

    fn delete_background_composer(
        &self,
        request: DeleteBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError>;

    fn list_background_composer_artifacts(
        &self,
        request: ListBackgroundComposerArtifactsRequestWire,
    ) -> Result<ListBackgroundComposerArtifactsResponseWire, CloudAgentBackendError>;

    fn get_background_composer_conversation(
        &self,
        request: GetBackgroundComposerConversationRequestWire,
    ) -> Result<GetBackgroundComposerConversationResponseWire, CloudAgentBackendError>;

    fn get_pull_request_merge_status(
        &self,
        request: GetPullRequestMergeStatusRequestWire,
    ) -> Result<GetPullRequestMergeStatusResponseWire, CloudAgentBackendError>;

    fn get_optimized_diff_details(
        &self,
        request: GetOptimizedDiffDetailsRequestWire,
    ) -> Result<GetOptimizedDiffDetailsResponseWire, CloudAgentBackendError>;

    fn list_teams(&self) -> Result<Vec<TeamWire>, CloudAgentBackendError>;

    fn cloud_agents_disabled_by_team_admin(&self) -> Result<bool, CloudAgentBackendError>;
}

#[derive(Clone)]
pub struct CursorCloudAgentBackend {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
}

impl CursorCloudAgentBackend {
    pub fn new(backend_url: impl Into<String>, auth: Arc<HostAuthExtension>) -> Self {
        Self {
            backend_url: backend_url.into(),
            auth,
        }
    }

    fn unary<Req, Resp>(
        &self,
        path: &str,
        request: &Req,
        timeout_ms: u64,
    ) -> Result<Resp, CloudAgentBackendError>
    where
        Req: Message,
        Resp: Message + Default,
    {
        let access_token = self
            .auth
            .get_access_token()
            .map_err(|error| CloudAgentBackendError::Other(error.to_string()))?;
        let machine_id = self
            .auth
            .get_machine_id()
            .map_err(|error| CloudAgentBackendError::Other(error.to_string()))?;
        let ghost_mode =
            resolve_sand_ghost_mode_header(&self.backend_url, &access_token, &machine_id);
        let bytes = send_cursor_unary(
            &self.backend_url,
            &access_token,
            &machine_id,
            path,
            &request.encode_to_vec(),
            timeout_ms,
            ghost_mode,
        )
        .map_err(CloudAgentBackendError::from_cursor)?;
        Resp::decode(bytes.as_slice())
            .map_err(|error| CloudAgentBackendError::Other(error.to_string()))
    }

    fn unary_empty<Req: Message>(
        &self,
        path: &str,
        request: &Req,
        timeout_ms: u64,
    ) -> Result<(), CloudAgentBackendError> {
        #[derive(Clone, PartialEq, Message)]
        struct EmptyWire {}
        let _: EmptyWire = self.unary(path, request, timeout_ms)?;
        Ok(())
    }
}

impl SavedEnvironmentClient for CursorCloudAgentBackend {
    fn get_environment(&self, public_id: &str) -> Result<Option<SavedEnvironment>, String> {
        let response: GetEnvironmentResponseWire = self
            .unary(
                GET_ENVIRONMENT_PATH,
                &GetEnvironmentRequestWire {
                    public_id: public_id.to_string(),
                },
                CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
            )
            .map_err(|error| error.to_string())?;
        Ok(response.environment.map(saved_environment_from_wire))
    }

    fn list_environments(&self, limit: usize) -> Result<Vec<SavedEnvironment>, String> {
        let response: ListEnvironmentsResponseWire = self
            .unary(
                LIST_ENVIRONMENTS_PATH,
                &ListEnvironmentsRequestWire {
                    limit: Some(i32::try_from(limit).unwrap_or(i32::MAX)),
                    include_repository_scope_environments: Some(true),
                },
                CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
            )
            .map_err(|error| error.to_string())?;
        Ok(response
            .environments
            .into_iter()
            .map(saved_environment_from_wire)
            .collect())
    }
}

impl CloudAgentBackend for CursorCloudAgentBackend {
    fn start_background_composer(
        &self,
        request: StartBackgroundComposerRequestWire,
    ) -> Result<StartBackgroundComposerResponseWire, CloudAgentBackendError> {
        self.unary(
            START_BACKGROUND_COMPOSER_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn get_background_composer_info(
        &self,
        request: GetBackgroundComposerInfoRequestWire,
    ) -> Result<GetBackgroundComposerInfoResponseWire, CloudAgentBackendError> {
        self.unary(
            GET_BACKGROUND_COMPOSER_INFO_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn list_background_composers(
        &self,
        request: ListBackgroundComposersRequestWire,
    ) -> Result<ListBackgroundComposersResponseWire, CloudAgentBackendError> {
        self.unary(
            LIST_BACKGROUND_COMPOSERS_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn add_async_followup(
        &self,
        request: AddAsyncFollowupBackgroundComposerRequestWire,
    ) -> Result<AddAsyncFollowupBackgroundComposerResponseWire, CloudAgentBackendError> {
        self.unary(
            ADD_ASYNC_FOLLOWUP_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn pause_background_composer(
        &self,
        request: PauseBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        self.unary_empty(
            PAUSE_BACKGROUND_COMPOSER_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn rename_background_composer(
        &self,
        request: RenameBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        #[derive(Clone, PartialEq, Message)]
        struct RenameResponseWire {
            #[prost(string, tag = "1")]
            name: String,
        }
        let _: RenameResponseWire = self.unary(
            RENAME_BACKGROUND_COMPOSER_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )?;
        Ok(())
    }

    fn archive_background_composer(
        &self,
        request: ArchiveBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        #[derive(Clone, PartialEq, Message)]
        struct ArchiveResponseWire {
            #[prost(bool, tag = "1")]
            closed_pull_request: bool,
        }
        let _: ArchiveResponseWire = self.unary(
            ARCHIVE_BACKGROUND_COMPOSER_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )?;
        Ok(())
    }

    fn delete_background_composer(
        &self,
        request: DeleteBackgroundComposerRequestWire,
    ) -> Result<(), CloudAgentBackendError> {
        self.unary_empty(
            DELETE_BACKGROUND_COMPOSER_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn list_background_composer_artifacts(
        &self,
        request: ListBackgroundComposerArtifactsRequestWire,
    ) -> Result<ListBackgroundComposerArtifactsResponseWire, CloudAgentBackendError> {
        self.unary(
            LIST_BACKGROUND_COMPOSER_ARTIFACTS_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn get_background_composer_conversation(
        &self,
        request: GetBackgroundComposerConversationRequestWire,
    ) -> Result<GetBackgroundComposerConversationResponseWire, CloudAgentBackendError> {
        self.unary(
            GET_BACKGROUND_COMPOSER_CONVERSATION_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn get_pull_request_merge_status(
        &self,
        request: GetPullRequestMergeStatusRequestWire,
    ) -> Result<GetPullRequestMergeStatusResponseWire, CloudAgentBackendError> {
        self.unary(
            GET_PULL_REQUEST_MERGE_STATUS_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn get_optimized_diff_details(
        &self,
        request: GetOptimizedDiffDetailsRequestWire,
    ) -> Result<GetOptimizedDiffDetailsResponseWire, CloudAgentBackendError> {
        self.unary(
            GET_OPTIMIZED_DIFF_DETAILS_PATH,
            &request,
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )
    }

    fn list_teams(&self) -> Result<Vec<TeamWire>, CloudAgentBackendError> {
        let response: GetTeamsResponseWire = self.unary(
            GET_TEAMS_PATH,
            &GetTeamsRequestWire {
                active_only: Some(true),
            },
            CLOUD_AGENT_POLL_RPC_TIMEOUT_MS,
        )?;
        Ok(response.teams)
    }

    fn cloud_agents_disabled_by_team_admin(&self) -> Result<bool, CloudAgentBackendError> {
        let response: GetTeamAdminSettingsResponseWire = self.unary(
            GET_TEAM_ADMIN_SETTINGS_PATH,
            &GetTeamAdminSettingsRequestWire { team_id: None },
            super::cloud_agent_poll_loop::TEAM_ADMIN_POLICY_REQUEST_TIMEOUT_MS,
        )?;
        Ok(response
            .background_agent_settings
            .and_then(|settings| settings.disable_cloud_agents_in_sand)
            .unwrap_or(false))
    }
}

fn saved_environment_from_wire(value: LogicalEnvironmentWire) -> SavedEnvironment {
    SavedEnvironment {
        public_id: value.public_id,
        name: value.name,
        repo_config: value.repo_config.map(|config| SavedEnvironmentRepoConfig {
            repos: config
                .repos
                .into_iter()
                .map(|repo| SavedEnvironmentRepo {
                    repo_url: repo.repo_url,
                    scm_repo_node_id: nonempty(repo.scm_repo_node_id),
                    git_enterprise_uuid: repo.git_enterprise_uuid,
                })
                .collect(),
        }),
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn cloud_status_from_proto(value: i32) -> CloudAgentRunStatus {
    match value {
        1 => CloudAgentRunStatus::Running,
        2 => CloudAgentRunStatus::Finished,
        3 => CloudAgentRunStatus::Error,
        4 => CloudAgentRunStatus::Creating,
        5 => CloudAgentRunStatus::Expired,
        0 => CloudAgentRunStatus::Unspecified,
        other => CloudAgentRunStatus::Number(i64::from(other)),
    }
}

pub fn map_run_status(value: i32) -> &'static str {
    match value {
        1 => "running",
        2 => "finished",
        3 => "error",
        4 => "creating",
        5 => "expired",
        _ => "unknown",
    }
}

pub fn cloud_agent_url(bc_id: &str) -> String {
    format!("{CLOUD_AGENT_URL_PREFIX}{bc_id}")
}

fn wire_to_poll_detailed(value: DetailedBackgroundComposerWire) -> DetailedComposer {
    let composer = value.composer.map(|composer| CloudAgentComposer {
        status: Some(cloud_status_from_proto(composer.status)),
        pr_url: nonempty(composer.pr_url),
        branch_name: nonempty(composer.branch_name),
        commit_count: composer.commit_count.and_then(|n| u64::try_from(n).ok()),
        files_changed: composer.files_changed.and_then(|n| u64::try_from(n).ok()),
        lines_added: composer.lines_added.and_then(|n| u64::try_from(n).ok()),
        lines_removed: composer.lines_removed.and_then(|n| u64::try_from(n).ok()),
    });
    let prs = value
        .prs
        .into_iter()
        .map(|pr| CloudAgentPr {
            pr_url: pr.pr_url,
            branch_name: nonempty(pr.branch_name),
        })
        .collect();
    let permanent_error = value.permanent_error.and_then(|error| {
        error.details.map(|details| CloudAgentPermanentErrorDetails {
            title: details.title,
            detail: details.detail,
            rate_limit_reason: details.additional_info.get("rateLimitReason").cloned(),
        })
    });
    DetailedComposer {
        composer,
        prs,
        summary: value.summary,
        permanent_error,
    }
}

fn requested_model_wire(
    model_id: Option<&str>,
    params: Option<&BTreeMap<String, String>>,
) -> Result<Option<RequestedModelWire>, SandCloudAgentLaunchError> {
    let value = build_cloud_agent_requested_model(model_id, params)?;
    let Some(value) = value else {
        return Ok(None);
    };
    let model_id = value
        .get("modelId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let max_mode = value
        .get("maxMode")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let parameters = value
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|parameter| RequestedModelParameterWire {
            id: parameter
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            value: parameter
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    Ok(Some(RequestedModelWire {
        model_id,
        max_mode,
        parameters,
    }))
}

fn user_message_wire(prompt: &str, images: &[CloudAgentImageInput]) -> UserMessageWire {
    let json = build_cloud_agent_user_message(prompt, Some(Value::String("agent".into())), images);
    let message_id = json
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let selected_images = images
        .iter()
        .map(|image| SelectedImageWire {
            data: image.data.clone(),
            uuid: String::new(),
            path: image.path.clone().unwrap_or_default(),
            mime_type: image.mime_type.clone().unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    UserMessageWire {
        text: prompt.to_string(),
        message_id,
        selected_context: (!selected_images.is_empty()).then_some(SelectedContextWire {
            selected_images,
        }),
        mode: AGENT_MODE_AGENT,
    }
}

fn conversation_action_wire(
    prompt: &str,
    images: &[CloudAgentImageInput],
) -> ConversationActionWire {
    ConversationActionWire {
        user_message_action: Some(UserMessageActionWire {
            user_message: Some(user_message_wire(prompt, images)),
            send_to_interaction_listener: Some(true),
        }),
    }
}

fn repo_config_wire(
    saved: Option<&SavedEnvironment>,
) -> Option<EnvironmentRepoConfigWire> {
    let repos = saved
        .and_then(|saved| saved.repo_config.as_ref())
        .map(|config| {
            config
                .repos
                .iter()
                .map(|repo| EnvironmentRepoEntryWire {
                    repo_url: repo.repo_url.clone(),
                    scm_repo_node_id: repo.scm_repo_node_id.clone().unwrap_or_default(),
                    git_enterprise_uuid: repo.git_enterprise_uuid.clone(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    (!repos.is_empty()).then_some(EnvironmentRepoConfigWire { repos })
}

fn environment_wire_fields(
    repo_url: &str,
    environment: Option<&CloudAgentEnvironment>,
) -> Result<(Option<bool>, Vec<PrivateWorkerLabelWire>), SandCloudAgentLaunchError> {
    let value = resolve_cloud_agent_environment_fields(repo_url, environment)?;
    let use_private_worker = value
        .get("usePrivateWorker")
        .and_then(Value::as_bool);
    let labels = value
        .get("labels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|label| PrivateWorkerLabelWire {
            key: label
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            value: label
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
        .collect();
    Ok((use_private_worker, labels))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentLaunchArgs {
    pub prompt: String,
    pub repo_url: Option<String>,
    pub starting_ref: Option<String>,
    pub environment: Option<CloudAgentEnvironment>,
    pub model_id: Option<String>,
    pub model_params: BTreeMap<String, String>,
    pub images: Vec<CloudAgentImageInput>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentLaunchResult {
    pub bc_id: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentReplyArgs {
    pub bc_id: String,
    pub prompt: String,
    pub images: Vec<CloudAgentImageInput>,
    pub interrupt: bool,
    pub model_id: Option<String>,
    pub model_params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentSummary {
    pub bc_id: String,
    pub name: String,
    pub status: String,
    pub branch_name: String,
    pub pr_url: String,
    pub is_archived: bool,
    pub created_at_ms: u64,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentDetail {
    pub summary: CloudAgentSummary,
    pub files_changed: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentArtifact {
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentFileChange {
    pub path: String,
    pub added: i64,
    pub removed: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentInfo {
    pub bc_id: String,
    pub status: String,
    pub name: String,
    pub prompt: String,
    pub branch_name: String,
    pub pr_url: String,
    pub pr_state: String,
    pub pr_number: Option<i32>,
    pub files_changed: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub files: Vec<CloudAgentFileChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAgentTranscriptDump {
    pub status: String,
    pub line_count: usize,
    pub jsonl: String,
}

pub type CloudConversationTraceConverter =
    Arc<dyn Fn(&[Vec<u8>]) -> Result<Vec<Value>, String> + Send + Sync>;

pub struct SandCloudAgentManager {
    backend: Arc<dyn CloudAgentBackend>,
    launched_ids: Mutex<BTreeSet<String>>,
    completion_poller: Arc<CloudAgentCompletionPoller>,
    model_catalog: CloudAgentModelCatalogCache,
    team_admin_policy: CloudAgentTeamAdminPolicyCache,
    convert_conversation: CloudConversationTraceConverter,
}

impl SandCloudAgentManager {
    pub fn production(
        backend_url: String,
        auth: Arc<HostAuthExtension>,
        convert_conversation: CloudConversationTraceConverter,
    ) -> Arc<Self> {
        let cursor_backend = Arc::new(CursorCloudAgentBackend::new(
            backend_url.clone(),
            Arc::clone(&auth),
        ));
        Self::with_backend(
            cursor_backend,
            {
                let backend_url = backend_url.clone();
                let auth = Arc::clone(&auth);
                Arc::new(move || {
                    fetch_sand_model_catalog(&backend_url, Arc::clone(&auth))
                        .map_err(|error| error.to_string())
                })
            },
            convert_conversation,
        )
    }

    pub fn with_backend(
        backend: Arc<dyn CloudAgentBackend>,
        model_catalog_loader: ModelCatalogLoader,
        convert_conversation: CloudConversationTraceConverter,
    ) -> Arc<Self> {
        let started = Instant::now();
        let clock: Arc<dyn Fn() -> u64 + Send + Sync> =
            Arc::new(move || started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
        let sleep: Arc<dyn Fn(Duration) + Send + Sync> = Arc::new(std::thread::sleep);

        let backend_for_poll = Arc::clone(&backend);
        let get_info: CloudAgentInfoFetcher = Arc::new(move |bc_id, _timeout_ms| {
            let response = backend_for_poll
                .get_background_composer_info(GetBackgroundComposerInfoRequestWire {
                    bc_id: bc_id.to_string(),
                    include_diff: false,
                    do_not_throw_if_setup_not_finished: true,
                })
                .map_err(|error| error.as_poll_error())?;
            let detailed = response.composer.ok_or_else(|| CloudAgentPollError {
                message: format!("No cloud agent found for {bc_id}."),
                is_rate_limit: false,
                retry_after_ms: None,
            })?;
            Ok(wire_to_poll_detailed(detailed))
        });

        let backend_for_policy = Arc::clone(&backend);
        let policy_loader: TeamAdminPolicyLoader = Arc::new(move || {
            backend_for_policy
                .cloud_agents_disabled_by_team_admin()
                .map_err(|error| error.to_string())
        });

        Arc::new(Self {
            backend,
            launched_ids: Mutex::new(BTreeSet::new()),
            completion_poller: Arc::new(CloudAgentCompletionPoller::new(
                Arc::clone(&clock),
                sleep,
                get_info,
            )),
            model_catalog: CloudAgentModelCatalogCache::new(
                Arc::clone(&clock),
                model_catalog_loader,
            ),
            team_admin_policy: CloudAgentTeamAdminPolicyCache::new(clock, policy_loader),
            convert_conversation,
        })
    }

    pub fn dispose(&self) {
        self.completion_poller.dispose();
    }

    pub fn is_disabled_by_team_admin(&self) -> bool {
        self.team_admin_policy.is_disabled_by_team_admin()
    }

    pub fn prefetch_team_admin_policy(&self) {
        self.team_admin_policy.prefetch_team_admin_policy();
    }

    pub fn list_models(&self) -> Result<Vec<SandModelCatalogEntry>, String> {
        self.model_catalog.list_models()
    }

    pub fn launched_ids_snapshot(&self) -> BTreeSet<String> {
        self.launched_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn remember_managed_id(&self, bc_id: &str) {
        self.launched_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(bc_id.to_string());
    }

    pub fn is_managed_id(&self, bc_id: &str) -> bool {
        self.launched_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(bc_id)
    }

    pub fn forget_managed_id(&self, bc_id: &str) {
        self.launched_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(bc_id);
    }

    pub fn resolve_private_worker_team_id(
        &self,
        environment: Option<&CloudAgentEnvironment>,
    ) -> Result<Option<i32>, SandCloudAgentLaunchError> {
        match environment {
            None | Some(CloudAgentEnvironment::Cloud)
            | Some(CloudAgentEnvironment::Environment { .. }) => return Ok(None),
            Some(CloudAgentEnvironment::Pool { team_id: Some(id), .. })
            | Some(CloudAgentEnvironment::Machine { team_id: Some(id), .. }) => {
                return i32::try_from(*id)
                    .map(Some)
                    .map_err(|_| SandCloudAgentLaunchError::new("environment.team_id is out of range."));
            }
            _ => {}
        }

        let teams = self
            .backend
            .list_teams()
            .map_err(|error| SandCloudAgentLaunchError::new(error.to_string()))?
            .into_iter()
            .filter(|team| team.id > 0 && team.is_direct_member)
            .collect::<Vec<_>>();

        match teams.as_slice() {
            [team] => Ok(Some(team.id)),
            [] => {
                if matches!(environment, Some(CloudAgentEnvironment::Pool { .. })) {
                    Err(SandCloudAgentLaunchError::new(
                        "A self-hosted pool requires an active team, but this account has none.",
                    ))
                } else {
                    Ok(None)
                }
            }
            teams => Err(SandCloudAgentLaunchError::new(format!(
                "This account has multiple active teams. Set environment.team_id to one of: {}.",
                teams
                    .iter()
                    .map(|team| format!("{} ({})", team.name, team.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    pub fn launch(
        &self,
        args: CloudAgentLaunchArgs,
    ) -> Result<CloudAgentLaunchResult, SandCloudAgentLaunchError> {
        if matches!(args.environment, Some(CloudAgentEnvironment::Machine { .. }))
            && args
                .starting_ref
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(SandCloudAgentLaunchError::new(format!(
                "A named private worker runs on its own checkout, so it can't start from '{}'. Omit starting_ref (the worker uses its current branch), or use a cloud or pool environment to start from a specific ref.",
                args.starting_ref.as_deref().unwrap_or_default().trim()
            )));
        }

        let saved = if let Some(CloudAgentEnvironment::Environment { public_id, name }) =
            args.environment.as_ref()
        {
            Some(resolve_saved_environment(
                self.backend.as_ref(),
                public_id.as_deref(),
                name.as_deref(),
            )?)
        } else {
            None
        };

        let repo_reference =
            resolve_launch_repo_reference(args.repo_url.as_deref(), saved.as_ref())?;
        let repo = build_repo_from_remote(&repo_reference, args.starting_ref.as_deref())?;
        let requested_model = requested_model_wire(
            args.model_id.as_deref(),
            (!args.model_params.is_empty()).then_some(&args.model_params),
        )?;
        let bc_id = format!("bc-{}", Uuid::new_v4());
        let team_id = self.resolve_private_worker_team_id(args.environment.as_ref())?;
        let (use_private_worker, labels) =
            environment_wire_fields(&repo.http_repo_url, args.environment.as_ref())?;

        let response = self
            .backend
            .start_background_composer(StartBackgroundComposerRequestWire {
                snapshot_name_or_id: repo.sanitized_repo_url.clone(),
                repository_info: Some(HeadlessRepositoryInfoWire {
                    path_encryption_key: String::new(),
                    should_sync_index: false,
                }),
                snapshot_workspace_root_path: "/workspace".into(),
                auto_branch: true,
                return_immediately: true,
                devcontainer_starting_point: Some(DevcontainerStartingPointWire {
                    url: repo.http_repo_url.clone(),
                    r#ref: repo.base_branch.clone().unwrap_or_default(),
                    repo_config: repo_config_wire(saved.as_ref()),
                    environment_name: saved
                        .as_ref()
                        .map(|value| value.name.trim().to_string())
                        .filter(|value| !value.is_empty()),
                    environment_public_id: saved.as_ref().map(|value| value.public_id.clone()),
                }),
                repo_url: Some(repo.http_repo_url.clone()),
                bc_id: bc_id.clone(),
                source: Some(BACKGROUND_COMPOSER_SOURCE_GROK_BOT),
                add_initial_message_to_responses: Some(true),
                base_branch: repo.base_branch.clone(),
                auto_create_pr: Some(true),
                starting_message_type: Some(STARTING_MESSAGE_TYPE_USER_MESSAGE),
                conversation_action: Some(conversation_action_wire(
                    &args.prompt,
                    &args.images,
                )),
                name: args
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                use_private_worker,
                requested_models: requested_model.into_iter().collect(),
                labels,
                team_id,
            })
            .map_err(|error| SandCloudAgentLaunchError::new(error.to_string()))?;

        let id = response
            .composer
            .and_then(|composer| nonempty(composer.bc_id))
            .unwrap_or(bc_id);
        self.remember_managed_id(&id);
        Ok(CloudAgentLaunchResult {
            url: cloud_agent_url(&id),
            bc_id: id,
        })
    }

    pub fn await_completion(
        &self,
        bc_id: &str,
        wait_for_restart: bool,
    ) -> super::cloud_agent_poll_loop::CloudAgentWatchResult {
        self.completion_poller
            .await_completion(bc_id, wait_for_restart)
    }

    pub fn list(
        &self,
        limit: Option<usize>,
        include_archived: bool,
    ) -> Result<Vec<CloudAgentSummary>, CloudAgentBackendError> {
        let response = self.backend.list_background_composers(
            ListBackgroundComposersRequestWire {
                n: i32::try_from(limit.unwrap_or(20)).unwrap_or(i32::MAX),
                include_archived: None,
            },
        )?;
        Ok(response
            .composers
            .into_iter()
            .filter(|composer| include_archived || !composer.is_archived)
            .map(summary_from_wire)
            .collect())
    }

    pub fn get(&self, bc_id: &str) -> Result<Option<CloudAgentDetail>, CloudAgentBackendError> {
        let id = bc_id.trim();
        if id.is_empty() {
            return Ok(None);
        }
        let response = match self.backend.get_background_composer_info(
            GetBackgroundComposerInfoRequestWire {
                bc_id: id.to_string(),
                include_diff: false,
                do_not_throw_if_setup_not_finished: true,
            },
        ) {
            Ok(value) => value,
            Err(CloudAgentBackendError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(detailed) = response.composer else {
            return Ok(None);
        };
        let Some(composer) = detailed.composer.clone() else {
            return Ok(None);
        };
        let error = detailed
            .permanent_error
            .and_then(|error| error.details)
            .and_then(|details| {
                (details.additional_info.get("rateLimitReason").map(String::as_str)
                    == Some("sand_included_limit"))
                .then(|| {
                    [details.title.trim(), details.detail.trim()]
                        .into_iter()
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            })
            .filter(|value| !value.is_empty());

        Ok(Some(CloudAgentDetail {
            summary: summary_from_wire(composer.clone()),
            files_changed: nonnegative_u64(composer.files_changed),
            lines_added: nonnegative_u64(composer.lines_added),
            lines_removed: nonnegative_u64(composer.lines_removed),
            error,
        }))
    }

    pub fn reply(
        &self,
        args: CloudAgentReplyArgs,
    ) -> Result<String, SandCloudAgentLaunchError> {
        let requested_model = requested_model_wire(
            args.model_id.as_deref(),
            (!args.model_params.is_empty()).then_some(&args.model_params),
        )?;
        let response = self
            .backend
            .add_async_followup(AddAsyncFollowupBackgroundComposerRequestWire {
                bc_id: args.bc_id.clone(),
                synchronous: args.interrupt,
                followup_source: Some(BACKGROUND_COMPOSER_SOURCE_GROK_BOT),
                followup_conversation_action: Some(conversation_action_wire(
                    &args.prompt,
                    &args.images,
                )),
                requested_model,
            })
            .map_err(|error| SandCloudAgentLaunchError::new(error.to_string()))?;
        self.remember_managed_id(&args.bc_id);
        Ok(response.run_id)
    }

    pub fn cancel(&self, bc_id: &str) -> Result<(), CloudAgentBackendError> {
        self.backend
            .pause_background_composer(PauseBackgroundComposerRequestWire {
                bc_id: bc_id.to_string(),
                source: BACKGROUND_COMPOSER_SOURCE_GROK_BOT,
            })
    }

    pub fn rename(&self, bc_id: &str, title: &str) -> Result<(), CloudAgentBackendError> {
        self.backend
            .rename_background_composer(RenameBackgroundComposerRequestWire {
                bc_id: bc_id.to_string(),
                new_name: title.to_string(),
            })
    }

    pub fn set_archived(
        &self,
        bc_id: &str,
        archived: bool,
    ) -> Result<(), CloudAgentBackendError> {
        self.backend
            .archive_background_composer(ArchiveBackgroundComposerRequestWire {
                bc_id: bc_id.to_string(),
                unarchive: !archived,
                source: BACKGROUND_COMPOSER_SOURCE_GROK_BOT,
            })
    }

    pub fn delete(&self, bc_id: &str) -> Result<(), CloudAgentBackendError> {
        self.backend
            .delete_background_composer(DeleteBackgroundComposerRequestWire {
                bc_id: bc_id.to_string(),
            })
    }

    pub fn list_artifacts(
        &self,
        bc_id: &str,
    ) -> Result<Vec<CloudAgentArtifact>, CloudAgentBackendError> {
        let response = self.backend.list_background_composer_artifacts(
            ListBackgroundComposerArtifactsRequestWire {
                bc_id: bc_id.to_string(),
            },
        )?;
        Ok(response
            .artifacts
            .into_iter()
            .map(|artifact| CloudAgentArtifact {
                path: artifact.absolute_path,
                size_bytes: u64::try_from(artifact.size_bytes).unwrap_or(0),
            })
            .collect())
    }

    pub fn get_transcript_dump(
        &self,
        bc_id: &str,
    ) -> Result<Option<CloudAgentTranscriptDump>, CloudAgentBackendError> {
        let id = bc_id.trim();
        if id.is_empty() {
            return Ok(None);
        }
        let conversation = match self.backend.get_background_composer_conversation(
            GetBackgroundComposerConversationRequestWire {
                bc_id: id.to_string(),
            },
        ) {
            Ok(value) => value,
            Err(CloudAgentBackendError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let messages = (self.convert_conversation)(&conversation.conversation)
            .map_err(CloudAgentBackendError::Other)?;
        let lines = messages
            .into_iter()
            .map(|message| {
                serde_json::to_string(&message)
                    .map_err(|error| CloudAgentBackendError::Other(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut status = "unknown".to_string();
        if let Ok(response) = self.backend.get_background_composer_info(
            GetBackgroundComposerInfoRequestWire {
                bc_id: id.to_string(),
                include_diff: false,
                do_not_throw_if_setup_not_finished: true,
            },
        ) {
            if let Some(detailed) = response.composer {
                status = map_run_status(
                    detailed
                        .composer
                        .as_ref()
                        .map(|composer| composer.status)
                        .unwrap_or(detailed.status),
                )
                .to_string();
            }
        }
        Ok(Some(CloudAgentTranscriptDump {
            status,
            line_count: lines.len(),
            jsonl: if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            },
        }))
    }

    pub fn get_info(
        &self,
        bc_id: &str,
        include_files: bool,
    ) -> Result<Option<CloudAgentInfo>, CloudAgentBackendError> {
        let id = bc_id.trim();
        if id.is_empty() {
            return Ok(None);
        }
        let response = match self.backend.get_background_composer_info(
            GetBackgroundComposerInfoRequestWire {
                bc_id: id.to_string(),
                include_diff: false,
                do_not_throw_if_setup_not_finished: true,
            },
        ) {
            Ok(value) => value,
            Err(CloudAgentBackendError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(detailed) = response.composer else {
            return Ok(None);
        };
        let composer = detailed.composer.clone().unwrap_or_default();
        let pr = resolve_pr(&detailed);
        let live_pr_state = self.fetch_live_pr_state(&pr.url, &pr.state)?;
        let files = if include_files && composer.files_changed.unwrap_or(0) > 0 {
            self.fetch_file_changes(id)?
        } else {
            Vec::new()
        };
        Ok(Some(CloudAgentInfo {
            bc_id: id.to_string(),
            status: map_run_status(composer.status).to_string(),
            name: composer.name,
            prompt: detailed.prompt.as_ref().map(|prompt| prompt.text.clone()).unwrap_or_default(),
            branch_name: resolve_branch_name(&detailed),
            pr_url: pr.url,
            pr_state: live_pr_state.unwrap_or(pr.state),
            pr_number: pr.number,
            files_changed: nonnegative_u64(composer.files_changed),
            lines_added: nonnegative_u64(composer.lines_added),
            lines_removed: nonnegative_u64(composer.lines_removed),
            files,
        }))
    }

    pub fn fetch_live_pr_state(
        &self,
        pr_url: &str,
        stored_state: &str,
    ) -> Result<Option<String>, CloudAgentBackendError> {
        if pr_url.is_empty() || stored_state == "merged" {
            return Ok(None);
        }
        let value = self.backend.get_pull_request_merge_status(
            GetPullRequestMergeStatusRequestWire {
                pr_url: pr_url.to_string(),
            },
        )?;
        Ok(if value.is_merged {
            Some("merged".into())
        } else if value.is_closed {
            Some("closed".into())
        } else if value.is_draft {
            Some("draft".into())
        } else if value.state == "open" {
            Some("open".into())
        } else {
            None
        })
    }

    pub fn fetch_file_changes(
        &self,
        bc_id: &str,
    ) -> Result<Vec<CloudAgentFileChange>, CloudAgentBackendError> {
        let response = self.backend.get_optimized_diff_details(
            GetOptimizedDiffDetailsRequestWire {
                bc_id: bc_id.to_string(),
                exclude_before_after_diffs: true,
            },
        )?;
        let mut changes = Vec::new();
        for diff in response.diff.into_iter().flat_map(|diff| diff.diffs) {
            let from = normalize_diff_path(&diff.from);
            let to = normalize_diff_path(&diff.to);
            let path = if to.is_empty() { from } else { to };
            if path.is_empty() {
                continue;
            }
            changes.push(CloudAgentFileChange {
                path,
                added: i64::from(diff.added),
                removed: i64::from(diff.removed),
            });
            if changes.len() >= MAX_CLOUD_AGENT_FILES {
                break;
            }
        }
        Ok(changes)
    }
}

impl Drop for SandCloudAgentManager {
    fn drop(&mut self) {
        self.completion_poller.dispose();
    }
}

fn summary_from_wire(composer: BackgroundComposerWire) -> CloudAgentSummary {
    let created_at_ms = if composer.created_at_ms.is_finite() && composer.created_at_ms > 0.0 {
        composer.created_at_ms as u64
    } else {
        0
    };
    CloudAgentSummary {
        bc_id: composer.bc_id.clone(),
        name: composer.name,
        status: map_run_status(composer.status).to_string(),
        branch_name: composer.branch_name,
        pr_url: composer.pr_url,
        is_archived: composer.is_archived,
        created_at_ms,
        url: cloud_agent_url(&composer.bc_id),
    }
}

fn nonnegative_u64(value: Option<i32>) -> u64 {
    value.and_then(|value| u64::try_from(value).ok()).unwrap_or(0)
}

pub fn normalize_diff_path(value: &str) -> String {
    let value = value.trim();
    if value == "/dev/null" {
        String::new()
    } else {
        value.to_string()
    }
}

fn resolve_branch_name(detailed: &DetailedBackgroundComposerWire) -> String {
    detailed
        .composer
        .as_ref()
        .map(|composer| composer.branch_name.clone())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            detailed
                .prs
                .iter()
                .find_map(|pr| nonempty(pr.branch_name.clone()))
        })
        .unwrap_or_default()
}

struct ResolvedPr {
    url: String,
    state: String,
    number: Option<i32>,
}

fn proto_pr_state(value: Option<i32>) -> Option<String> {
    match value {
        Some(1) => Some("open".into()),
        Some(2) => Some("draft".into()),
        Some(3) => Some("merged".into()),
        Some(4) => Some("closed".into()),
        _ => None,
    }
}

fn resolve_pr(detailed: &DetailedBackgroundComposerWire) -> ResolvedPr {
    let composer_url = detailed
        .composer
        .as_ref()
        .map(|composer| composer.pr_url.clone())
        .unwrap_or_default();
    let primary = if !composer_url.is_empty() {
        detailed
            .prs
            .iter()
            .find(|pr| pr.pr_url.as_deref() == Some(composer_url.as_str()))
    } else {
        detailed
            .prs
            .iter()
            .find(|pr| pr.pull_number.is_some() || proto_pr_state(pr.pr_status).is_some())
    };
    let url = if !composer_url.is_empty() {
        composer_url
    } else {
        primary
            .and_then(|pr| pr.pr_url.clone())
            .unwrap_or_default()
    };
    let state = primary
        .and_then(|pr| proto_pr_state(pr.pr_status))
        .or_else(|| {
            detailed
                .composer
                .as_ref()
                .and_then(|composer| proto_pr_state(composer.pr_status))
        })
        .unwrap_or_else(|| {
            if url.is_empty() && primary.is_none() {
                "none".into()
            } else {
                "unknown".into()
            }
        });
    ResolvedPr {
        url,
        state,
        number: primary.and_then(|pr| pr.pull_number),
    }
}
