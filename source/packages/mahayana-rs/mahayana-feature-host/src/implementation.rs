//! Product-level feature controller over the direct Mahayana Runtime Host.
//!
//! `HostMode::Test` is a deterministic in-process backend for fast E2E. It uses
//! real Rust commands, state, approvals, event ordering, and lifecycle without
//! network or simulator dependencies. Production provider mappings are added
//! feature-by-feature and must never silently fall back to test behavior.

use base64::Engine as _;
use flate2::Compression;
use flate2::write::GzEncoder;
use tar::{Builder as TarBuilder, HeaderMode};
use chrono::Datelike;
use chrono::NaiveDate;
use chrono::TimeZone;
use chrono::Timelike;
use chrono::Utc;

use crate::channel_management::AgentChannelStore;
use crate::client_side_tool_v2::{ClientSideToolV2Producer, FAMILY as CLIENT_SIDE_TOOL_V2_FAMILY};
use fabushi_messaging_core::BlobId;
use fabushi_messaging_core::ClientEnvelope as MessagingClientEnvelope;
use fabushi_messaging_core::FileBlobStore;
use fabushi_messaging_core::JsonFileStateStore;
use fabushi_messaging_core::MessagingService;
use fabushi_messaging_core::{AccessGrant, AccessScope, ActorId, FileAccessTokenStore};
#[cfg(feature = "production")]
use mahayana_core::ApprovalDecision as RuntimeApprovalDecision;
#[cfg(feature = "production")]
use mahayana_core::ApprovalId;
#[cfg(feature = "production")]
use mahayana_core::ConversationId;
use mahayana_core::MAHAYANA_AI_CONVERSATION_ID;
#[cfg(feature = "production")]
use mahayana_core::MessageRole as RuntimeMessageRole;
#[cfg(feature = "production")]
use mahayana_core::OperationId;
#[cfg(feature = "production")]
use mahayana_core::RuntimeActivityStatus;
#[cfg(feature = "production")]
use mahayana_core::RuntimeCommand;
#[cfg(feature = "production")]
use mahayana_core::RuntimeEvent;
#[cfg(feature = "production")]
use mahayana_core::RuntimeResponse;
#[cfg(feature = "production")]
use mahayana_core::capability::CapabilityAvailability;
#[cfg(feature = "production")]
use mahayana_core::capability::CapabilityKind;
#[cfg(feature = "production")]
use mahayana_host::HostCreateConfig;
#[cfg(feature = "production")]
use mahayana_host::MahayanaHost;
use mahayana_host_protocol::AgentBroadcastResult;
use mahayana_host_protocol::AgentMessageImage;
use mahayana_host_protocol::AgentMode;
use mahayana_host_protocol::AgentPeerMessage;
use mahayana_host_protocol::AgentStepStatus;
#[cfg(feature = "production")]
use mahayana_host_protocol::ApprovalDecision;
use mahayana_host_protocol::ApprovalResolution;
use mahayana_host_protocol::AsyncTaskKind;
use mahayana_host_protocol::AsyncTaskStatus;
use mahayana_host_protocol::AsyncTaskSummary;
use mahayana_host_protocol::AttachmentChunkResult;
use mahayana_host_protocol::AttachmentContext;
use mahayana_host_protocol::AttachmentImageResult;
use mahayana_host_protocol::AttachmentStored;
use mahayana_host_protocol::AttachmentTextResult;
use mahayana_host_protocol::AutoReviewBehavior;
use mahayana_host_protocol::AutoReviewRule;
use mahayana_host_protocol::AutomationRunStatus;
use mahayana_host_protocol::AutomationRunSummary;
use mahayana_host_protocol::AutomationSummary;
use mahayana_host_protocol::AutomationTrigger;
use mahayana_host_protocol::BotSummary;
use mahayana_host_protocol::COMPUTER_CONTROL_PROTOCOL_VERSION;
use mahayana_host_protocol::CapabilitySummary;
use mahayana_host_protocol::CommandAccepted;
use mahayana_host_protocol::ComputerActionResult;
use mahayana_host_protocol::ComputerControlOrigin;
use mahayana_host_protocol::ComputerControlTarget;
use mahayana_host_protocol::ComputerSnapshot;
use mahayana_host_protocol::ComputerTargetKind;
use mahayana_host_protocol::ConnectorAccountSummary;
use mahayana_host_protocol::ConnectorStatus;
use mahayana_host_protocol::ConnectorSummary;
use mahayana_host_protocol::ConnectorToolSummary;
use mahayana_host_protocol::ConnectorTransport;
use mahayana_host_protocol::ConversationMessage;
use mahayana_host_protocol::ConversationSummary;
use mahayana_host_protocol::DraftAction;
use mahayana_host_protocol::DraftSendState;
use mahayana_host_protocol::ErrorTray;
use mahayana_host_protocol::EventCard;
use mahayana_host_protocol::EventField;
use mahayana_host_protocol::FeatureCommand;
use mahayana_host_protocol::GroupMessage;
use mahayana_host_protocol::GroupSpeaker;
use mahayana_host_protocol::GroupSummary;
use mahayana_host_protocol::HOST_PROTOCOL_VERSION;
use mahayana_host_protocol::HostConfig;
use mahayana_host_protocol::HostEvent;
use mahayana_host_protocol::HostInfo;
use mahayana_host_protocol::HostMode;
use mahayana_host_protocol::ListenerIntegrationSummary;
use mahayana_host_protocol::ListenerPlatform;
use mahayana_host_protocol::LocalToolPermission;
use mahayana_host_protocol::MemoryKind;
use mahayana_host_protocol::MemoryRecord;
use mahayana_host_protocol::MessageDraft;
use mahayana_host_protocol::MessageRole;
use mahayana_host_protocol::ProductHostSettings;
use mahayana_host_protocol::SearchMediaMatch;
use mahayana_host_protocol::SearchMessageMatch;
use mahayana_host_protocol::SkillPublishState;
use mahayana_host_protocol::SkillSource;
use mahayana_host_protocol::SkillSummary;
use mahayana_host_protocol::SkillTeamSummary;
use mahayana_host_protocol::SubagentStatus;
use mahayana_host_protocol::SubagentSummary;
use mahayana_host_protocol::SurfacePlatform;
use mahayana_host_protocol::TranscriptReaction;
use mahayana_host_protocol::TEACH_MAX_DURATION_MS;
use mahayana_host_protocol::TeachEntryPoint;
use mahayana_host_protocol::TeachRecordingResult;
use mahayana_host_protocol::TeachRecordingStatus;
use mahayana_host_protocol::TranscriptCard;
use mahayana_host_protocol::UpdateState;
use mahayana_host_protocol::WorkflowSource;
use mahayana_host_protocol::WorkflowSummary;
use mahayana_host_protocol::WorkflowTrigger;
#[cfg(feature = "production")]
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::json;
use sha2::Digest as _;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

include!("automation_execution.rs");
include!("background_recovery.rs");

#[derive(Debug, thiserror::Error)]
pub enum FeatureHostError {
    #[cfg(feature = "production")]
    #[error(transparent)]
    Runtime(#[from] mahayana_host::HostError),
    #[error("production Runtime support is not compiled into this Host")]
    ProductionUnavailable,
    #[error("feature Host state mutex is poisoned")]
    StatePoisoned,
    #[error("feature Host is closed")]
    Closed,
    #[error("{0}")]
    Contract(String),
}

#[cfg_attr(not(feature = "production"), allow(dead_code))]
#[derive(Debug)]
struct PendingApproval {
    mini_app_id: String,
    capability: String,
    runtime_approval_id: Option<String>,
}

const GROUP_MAX_MEMBER_TURNS: usize = 10;
const GROUP_MAX_ROUNDS: usize = 3;
const GROUP_PROMPT_HISTORY_LIMIT: usize = 24;
const GROUP_MAX_MEMBERS: usize = 6;
const REMOTE_DEVICE_SECRET_MAX_ENTRIES: usize = 256;
const REMOTE_DEVICE_SECRET_MAX_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone)]
struct GroupRunState {
    run_id: String,
    round: usize,
    speaker_order: Vec<String>,
    speaker_index: usize,
    total_messages: usize,
    messages_this_round: usize,
}

#[derive(Debug, Clone)]
struct GroupOperationContext {
    run_id: String,
    group_id: String,
    member_id: String,
    member_name: String,
}

#[derive(Debug, Clone)]
struct BackgroundOperationContext {
    agent_id: String,
    agent_name: String,
    source: String,
    teach_artifact: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishedPluginSnapshot {
    plugin_id: String,
    plugin_version: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    published_by_current_user: bool,
    #[serde(default)]
    marketplace_team_id: Option<u64>,
    #[serde(default)]
    is_enabled_for_agent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishedWorkflowCacheMetadata {
    agent_id: String,
    original_workflow_id: String,
    promoted_workflow_id: String,
    plugin_id: String,
    plugin_version: String,
    plugin_name: String,
    display_name: String,
    description: String,
    marketplace_team_id: u64,
    enabled_before_publish: bool,
    #[serde(default)]
    unpublish_restore_prepared: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingBoxHandoff {
    request_id: String,
    agent_id: String,
    conversation_id: String,
    instruction: String,
    source_operation_id: String,
    resolving: bool,
}

#[derive(Debug, Clone)]
struct RemoteComputerLocalSession {
    device_id: String,
    client_id: String,
    expires_at_seconds: i64,
    generation: u64,
}

#[derive(Debug)]
struct TeachCaptureProcess {
    agent_id: String,
    entry_point: TeachEntryPoint,
    started_at_ms: i64,
    session_dir: PathBuf,
    video_path: PathBuf,
    child: Option<std::process::Child>,
}

enum MemoryAction {
    List { limit: usize },
    Add { content: String, kind: MemoryKind },
    Remove { id: String },
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeferredConversationActivation {
    generation: u64,
    conversation_id: String,
    shipped_through_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ConversationSessionState {
    active_conversation_id: Option<String>,
    pending_activation: Option<DeferredConversationActivation>,
    activation_generation: u64,
    scene_active: bool,
    focused_at_ms: Option<i64>,
}

impl ConversationSessionState {
    fn set_scene_active(&mut self, active: bool, now_ms: i64) {
        self.scene_active = active;
        self.focused_at_ms = active.then_some(now_ms);
    }

    fn note_contact(&mut self, now_ms: i64) {
        if self.scene_active {
            self.focused_at_ms = Some(now_ms);
        }
    }

    fn schedule_deferred_activation(
        &mut self,
        conversation_id: &str,
        shipped_through_id: Option<&str>,
    ) -> u64 {
        self.activation_generation = self.activation_generation.wrapping_add(1);
        if self.activation_generation == 0 {
            self.activation_generation = 1;
        }
        let generation = self.activation_generation;
        self.pending_activation = Some(DeferredConversationActivation {
            generation,
            conversation_id: conversation_id.to_string(),
            shipped_through_id: shipped_through_id.map(ToOwned::to_owned),
        });
        generation
    }

    fn invalidate_deferred_activation(&mut self) {
        self.pending_activation = None;
    }

    fn claim_deferred_activation(&mut self) -> Option<DeferredConversationActivation> {
        self.pending_activation.take()
    }

    fn switch_immediately(&mut self, conversation_id: &str) -> Option<String> {
        self.invalidate_deferred_activation();
        self.replace_active(conversation_id)
    }

    fn activate_claimed(&mut self, conversation_id: &str) -> Option<String> {
        self.replace_active(conversation_id)
    }

    fn replace_active(&mut self, conversation_id: &str) -> Option<String> {
        let previous = self
            .active_conversation_id
            .replace(conversation_id.to_string());
        previous.filter(|previous| previous != conversation_id)
    }

    fn mark_deleted(&mut self, conversation_id: &str) {
        if self
            .pending_activation
            .as_ref()
            .is_some_and(|pending| pending.conversation_id == conversation_id)
        {
            self.invalidate_deferred_activation();
        }
        if self.active_conversation_id.as_deref() == Some(conversation_id) {
            self.active_conversation_id = None;
        }
    }

    fn windowed_catch_up(
        shipped_through_id: Option<&str>,
        messages: &[ConversationMessage],
    ) -> Vec<ConversationMessage> {
        let start = match shipped_through_id {
            None => 0,
            Some(shipped_through_id) => {
                let Some(index) = messages
                    .iter()
                    .position(|message| message.id == shipped_through_id)
                else {
                    return Vec::new();
                };
                index + 1
            }
        };
        messages[start..].to_vec()
    }
}

fn bounded_conversation_window(
    messages: &[ConversationMessage],
    before_message_id: Option<&str>,
    limit: usize,
) -> (Vec<ConversationMessage>, Option<String>) {
    let end = match before_message_id {
        None => messages.len(),
        Some(before_message_id) => {
            let Some(index) = messages
                .iter()
                .position(|message| message.id == before_message_id)
            else {
                return (Vec::new(), None);
            };
            index
        }
    };
    let limit = limit.clamp(1, 200);
    let start = end.saturating_sub(limit);
    let window = messages[start..end].to_vec();
    let next_before_message_id = (start > 0)
        .then(|| window.first().map(|message| message.id.clone()))
        .flatten();
    (window, next_before_message_id)
}

const ASYNC_TASK_STALE_MAX_AGE_MS: i64 = 48 * 60 * 60 * 1_000;

fn async_task_key(agent_id: &str, kind: AsyncTaskKind, id: &str) -> String {
    let kind = match kind {
        AsyncTaskKind::Subagent => "subagent",
        AsyncTaskKind::Shell => "shell",
        AsyncTaskKind::CloudAgent => "cloud-agent",
    };
    format!("{agent_id}\u{1f}{kind}\u{1f}{id}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCloudAgentWake {
    pub agent_id: String,
    pub work_id: String,
    pub operation_id: String,
    pub title: String,
    pub started_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingAsyncTaskEntry {
    task: AsyncTaskSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingAsyncTasksFile {
    version: u8,
    #[serde(default)]
    entries: Vec<PendingAsyncTaskEntry>,
    #[serde(default)]
    tasks: Vec<AsyncTaskSummary>,
}

fn load_pending_async_tasks(
    path: &Path,
    now_ms: i64,
) -> (
    BTreeMap<String, AsyncTaskSummary>,
    BTreeMap<String, String>,
) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return (BTreeMap::new(), BTreeMap::new());
    };
    let Ok(file) = serde_json::from_str::<PendingAsyncTasksFile>(&raw) else {
        return (BTreeMap::new(), BTreeMap::new());
    };
    let mut entries = file.entries;
    if entries.is_empty() {
        entries.extend(file.tasks.into_iter().map(|task| PendingAsyncTaskEntry {
            task,
            operation_id: None,
        }));
    }
    let mut tasks = BTreeMap::new();
    let mut operation_ids = BTreeMap::new();
    for mut entry in entries {
        let task = &mut entry.task;
        if task.status != AsyncTaskStatus::Running
            || task.id.trim().is_empty()
            || task.parent_agent_id.trim().is_empty()
            || now_ms.saturating_sub(task.started_at_ms) > ASYNC_TASK_STALE_MAX_AGE_MS
        {
            continue;
        }
        if task.kind == AsyncTaskKind::Shell {
            let restart_detail = "reattached after a host restart";
            task.detail = Some(match task.detail.as_deref().filter(|detail| !detail.is_empty()) {
                Some(detail) if !detail.contains(restart_detail) => {
                    format!("{detail} · {restart_detail}")
                }
                Some(detail) => detail.to_string(),
                None => restart_detail.to_string(),
            });
        }
        if let Some(operation_id) = entry
            .operation_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            operation_ids.insert(async_task_key(&task.parent_agent_id, task.kind, &task.id), operation_id.to_string());
        }
        tasks.insert(async_task_key(&task.parent_agent_id, task.kind, &task.id), task.clone());
    }
    (tasks, operation_ids)
}

fn persist_pending_async_tasks(
    path: Option<&Path>,
    tasks: &BTreeMap<String, AsyncTaskSummary>,
    operation_ids: &BTreeMap<String, String>,
) -> Result<(), FeatureHostError> {
    let Some(path) = path else {
        return Ok(());
    };
    if tasks.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(FeatureHostError::Contract(format!(
                    "remove pending async task store: {error}"
                )));
            }
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create pending async task store directory: {error}"))
        })?;
    }
    let file = PendingAsyncTasksFile {
        version: 2,
        entries: tasks
            .iter()
            .map(|(key, task)| PendingAsyncTaskEntry {
                operation_id: operation_ids.get(key).cloned(),
                task: task.clone(),
            })
            .collect(),
        tasks: Vec::new(),
    };
    let bytes = serde_json::to_vec(&file).map_err(|error| {
        FeatureHostError::Contract(format!("serialize pending async tasks: {error}"))
    })?;
    let part = PathBuf::from(format!("{}.part", path.display()));
    std::fs::write(&part, bytes).map_err(|error| {
        FeatureHostError::Contract(format!("write pending async task store: {error}"))
    })?;
    match std::fs::rename(&part, path) {
        Ok(()) => Ok(()),
        Err(first_error) if path.exists() => {
            std::fs::remove_file(path).map_err(|error| {
                FeatureHostError::Contract(format!("replace pending async task store: {error}"))
            })?;
            std::fs::rename(&part, path).map_err(|_| {
                FeatureHostError::Contract(format!(
                    "commit pending async task store: {first_error}"
                ))
            })
        }
        Err(error) => Err(FeatureHostError::Contract(format!(
            "commit pending async task store: {error}"
        ))),
    }
}

#[derive(Debug)]
struct FeatureState {
    events: VecDeque<HostEvent>,
    conversation_session: ConversationSessionState,
    installed: BTreeMap<String, String>,
    pending_approvals: BTreeMap<String, PendingApproval>,
    operations: BTreeSet<String>,
    operation_agents: BTreeMap<String, String>,
    automation_operations: BTreeMap<String, (String, String)>,
    routine_executions: BTreeMap<String, RoutineExecution>,
    routine_operation_epochs: BTreeMap<String, u64>,
    routine_epoch: u64,
    routine_quiescing: bool,
    awaited_operations: BTreeSet<String>,
    operation_terminals: BTreeMap<String, Value>,
    background_operations: BTreeMap<String, BackgroundOperationContext>,
    background_recoveries: BTreeMap<String, BackgroundRecoveryExecution>,
    pending_box_handoffs: BTreeMap<String, PendingBoxHandoff>,
    remote_computer_sessions: BTreeMap<String, RemoteComputerLocalSession>,
    remote_computer_device_secrets: BTreeMap<String, String>,
    subagents: BTreeMap<String, SubagentSummary>,
    async_tasks: BTreeMap<String, AsyncTaskSummary>,
    async_task_operation_ids: BTreeMap<String, String>,
    peer_messages: Vec<AgentPeerMessage>,
    settings: ProductHostSettings,
    trays: Vec<ErrorTray>,
    sequence: u64,
    closed: bool,
    session_active: bool,
    auth_user: Option<Value>,
    automations: BTreeMap<String, AutomationSummary>,
    published_plugins_by_agent: BTreeMap<String, Vec<PublishedPluginSnapshot>>,
    connectors: BTreeMap<String, ConnectorSummary>,
    skills: BTreeMap<String, SkillSummary>,
    bots: BTreeMap<String, BotSummary>,
    groups: BTreeMap<String, GroupSummary>,
    group_runs: BTreeMap<String, GroupRunState>,
    group_operations: BTreeMap<String, GroupOperationContext>,
    listeners: BTreeMap<ListenerPlatform, ListenerIntegrationSummary>,
    pending_listener_resumes: BTreeSet<(String, ListenerPlatform)>,
    update_state: UpdateState,
}

impl Default for FeatureState {
    fn default() -> Self {
        Self {
            events: VecDeque::new(),
            conversation_session: ConversationSessionState::default(),
            installed: BTreeMap::new(),
            pending_approvals: BTreeMap::new(),
            operations: BTreeSet::new(),
            operation_agents: BTreeMap::new(),
            automation_operations: BTreeMap::new(),
            routine_executions: BTreeMap::new(),
            routine_operation_epochs: BTreeMap::new(),
            routine_epoch: 1,
            routine_quiescing: false,
            awaited_operations: BTreeSet::new(),
            operation_terminals: BTreeMap::new(),
            background_operations: BTreeMap::new(),
            background_recoveries: BTreeMap::new(),
            pending_box_handoffs: BTreeMap::new(),
            remote_computer_sessions: BTreeMap::new(),
            remote_computer_device_secrets: BTreeMap::new(),
            subagents: BTreeMap::new(),
            async_tasks: BTreeMap::new(),
            async_task_operation_ids: BTreeMap::new(),
            peer_messages: Vec::new(),
            settings: ProductHostSettings::default(),
            trays: Vec::new(),
            sequence: 0,
            closed: false,
            session_active: true,
            auth_user: None,
            automations: BTreeMap::new(),
            published_plugins_by_agent: BTreeMap::new(),
            connectors: default_connectors(),
            skills: default_skills(),
            bots: default_bots(),
            groups: BTreeMap::new(),
            group_runs: BTreeMap::new(),
            group_operations: BTreeMap::new(),
            listeners: default_listeners(),
            pending_listener_resumes: BTreeSet::new(),
            update_state: UpdateState::UpToDate {
                version: env!("CARGO_PKG_VERSION").into(),
            },
        }
    }
}

fn active_agent_id_for_state(state: &FeatureState) -> Option<String> {
    let conversation_id = state
        .conversation_session
        .active_conversation_id
        .as_deref()?;
    state
        .bots
        .values()
        .find(|bot| bot.conversation_id.as_deref() == Some(conversation_id))
        .map(|bot| bot.id.clone())
}

fn automation_listener_resume_keys(
    automation: &AutomationSummary,
) -> Vec<(String, ListenerPlatform)> {
    if !automation.enabled {
        return Vec::new();
    }
    let Some(trigger) = automation.trigger.as_ref() else {
        return Vec::new();
    };
    let agent_id = automation
        .agent_id
        .clone()
        .unwrap_or_else(|| "mahayana-assistant".into());
    let mut platforms = BTreeSet::new();
    automation_trigger_listener_platforms(trigger, &mut platforms);
    platforms
        .into_iter()
        .filter(|platform| matches!(platform, ListenerPlatform::Slack | ListenerPlatform::Github))
        .map(|platform| (agent_id.clone(), platform))
        .collect()
}

fn prune_pending_listener_resumes(state: &mut FeatureState) {
    let required = state
        .automations
        .values()
        .flat_map(automation_listener_resume_keys)
        .collect::<BTreeSet<_>>();
    state
        .pending_listener_resumes
        .retain(|key| required.contains(key));
}

fn take_pending_listener_resumes(
    state: &mut FeatureState,
    platform: ListenerPlatform,
) -> Vec<String> {
    let mut agents = state
        .pending_listener_resumes
        .iter()
        .filter(|(_, pending_platform)| *pending_platform == platform)
        .map(|(agent_id, _)| agent_id.clone())
        .collect::<Vec<_>>();
    agents.sort();
    agents.dedup();
    for agent_id in &agents {
        state
            .pending_listener_resumes
            .remove(&(agent_id.clone(), platform));
    }
    agents
}

fn queue_active_agent_automation_projection(state: &mut FeatureState, agent_id: &str) -> bool {
    if active_agent_id_for_state(state).as_deref() != Some(agent_id) {
        return false;
    }

    let mut automations = state
        .automations
        .values()
        .filter(|automation| automation.agent_id.as_deref() == Some(agent_id))
        .cloned()
        .collect::<Vec<_>>();
    automations.sort_by_key(|item| item.created_at_ms);
    state.events.push_back(HostEvent::TransportEvent {
        channel: "automations".into(),
        payload: json!({
            "agentId": agent_id,
            "automations": automations,
        }),
    });
    true
}

fn account_boundary_requires_runtime_reset(
    initialized: bool,
    active_account_id: &Option<String>,
    next_account_id: &Option<String>,
) -> bool {
    initialized && active_account_id != next_account_id
}

const APPROVAL_PRESENTATION_SECRET_KEYS: &[&str] = &[
    "authorization",
    "apikey",
    "api_key",
    "api-key",
    "credential",
    "password",
    "secret",
    "token",
];

fn approval_presentation_key_is_secret(value: &str) -> bool {
    let normalized = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    APPROVAL_PRESENTATION_SECRET_KEYS.iter().any(|key| {
        let key = key
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        normalized.contains(&key)
    })
}

fn approval_presentation_text_looks_sensitive(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("bearer ")
        || APPROVAL_PRESENTATION_SECRET_KEYS
            .iter()
            .any(|key| lower.contains(&key.to_ascii_lowercase()))
    {
        return true;
    }
    trimmed.chars().count() >= 24
        && !trimmed.chars().any(char::is_whitespace)
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_+/-=.".contains(ch))
}

fn approval_presentation_safe_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if approval_presentation_text_looks_sensitive(&normalized) {
        return "…".into();
    }
    normalized.chars().take(max_chars).collect()
}

fn approval_presentation_safe_value(value: &Value, key: Option<&str>, depth: usize) -> Value {
    if key.is_some_and(approval_presentation_key_is_secret) {
        return Value::String("…".into());
    }
    match value {
        Value::String(value) => Value::String(approval_presentation_safe_text(value, 240)),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::Array(items) if depth >= 3 => Value::String(format!("[{} items]", items.len())),
        Value::Object(_) if depth >= 3 => Value::String("{…}".into()),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(12)
                .map(|entry| approval_presentation_safe_value(entry, None, depth + 1))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .take(24)
                .map(|(entry_key, entry)| {
                    (
                        entry_key.chars().take(80).collect(),
                        approval_presentation_safe_value(entry, Some(entry_key), depth + 1),
                    )
                })
                .collect(),
        ),
    }
}

fn approval_presentation_safe_details(details: &Value) -> String {
    let safe = approval_presentation_safe_value(details, None, 0);
    serde_json::to_string(&safe)
        .unwrap_or_else(|_| "{…}".into())
        .chars()
        .take(1200)
        .collect()
}

pub struct FeatureHostController {
    config: HostConfig,
    info: HostInfo,
    #[cfg(feature = "production")]
    runtime: Option<MahayanaHost>,
    automation_path: Option<PathBuf>,
    bot_state_path: Option<PathBuf>,
    group_state_path: Option<PathBuf>,
    peer_messages_path: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    remote_device_state_path: Option<PathBuf>,
    async_tasks_path: Option<PathBuf>,
    channel_store: AgentChannelStore,
    test_auth_state_path: Option<PathBuf>,
    memory_root_path: Option<PathBuf>,
    workflow_root_path: Option<PathBuf>,
    teach_recording: Mutex<Option<TeachCaptureProcess>>,
    /// The authenticated account currently owning in-memory feature state.
    /// This is deliberately not a credential; it is only an identity marker
    /// used to prevent transcript/state reuse across account boundaries.
    active_account_id: Mutex<Option<String>>,
    // Startup is not an account replacement: the native Runtime may already
    // own a durable same-account checkpoint that must survive Host recreation.
    // Once the first persisted auth boundary is installed, later identity
    // changes use the destructive reset path.
    account_boundary_initialized: Mutex<bool>,
    client_side_tool_v2: Mutex<ClientSideToolV2Producer>,
    // Serializes routine admission with account replacement and native quiesce.
    routine_dispatch_lock: Mutex<()>,
    state: Mutex<FeatureState>,
}

impl FeatureHostController {
    pub fn create(config: HostConfig, platform: SurfacePlatform) -> Result<Self, FeatureHostError> {
        validate_config(&config)?;
        match config.mode {
            HostMode::Test => Ok(Self::create_test_backend(config, platform, None)),
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    Self::create_with_host_config(config, platform, HostCreateConfig::default())
                }
                #[cfg(not(feature = "production"))]
                {
                    Err(FeatureHostError::ProductionUnavailable)
                }
            }
        }
    }

    fn create_test_backend(
        config: HostConfig,
        platform: SurfacePlatform,
        test_data_dir: Option<&Path>,
    ) -> Self {
        let test_auth_state_path =
            test_data_dir.map(|data_dir| data_dir.join("test-auth-session.json"));
        let memory_root_path = Some(std::env::temp_dir().join(format!(
            "fabushi-feature-host-memory-{}-{}",
            config.profile_id,
            std::process::id()
        )));
        let workflow_root_path = Some(std::env::temp_dir().join(format!(
            "fabushi-feature-host-workflows-{}-{}",
            config.profile_id,
            std::process::id()
        )));
        let channel_store = AgentChannelStore::new(
            test_data_dir,
            test_data_dir.map(|_| "fabushi-test-channel-storage".to_string()),
        );
        let info = HostInfo {
            runtime_version: "mahayana-test-backend".to_string(),
            protocol_version: HOST_PROTOCOL_VERSION.to_string(),
            platform,
        };
        let mut state = FeatureState::default();
        if let Some(path) = test_auth_state_path.as_deref() {
            state.auth_user = load_test_auth_user(path);
        }
        sync_computer_control_policy(&state.settings);
        state.events.push_back(HostEvent::HostReady {
            timestamp: timestamp(),
            info: info.clone(),
        });
        Self {
            config,
            info,
            #[cfg(feature = "production")]
            runtime: None,
            automation_path: None,
            bot_state_path: None,
            group_state_path: None,
            peer_messages_path: None,
            settings_path: None,
            remote_device_state_path: None,
            async_tasks_path: test_data_dir.map(|data_dir| data_dir.join("pending-async-tasks.json")),
            channel_store,
            test_auth_state_path,
            memory_root_path,
            workflow_root_path,
            teach_recording: Mutex::new(None),
            active_account_id: Mutex::new(None),
            account_boundary_initialized: Mutex::new(false),
            client_side_tool_v2: Mutex::new(ClientSideToolV2Producer::new()),
            routine_dispatch_lock: Mutex::new(()),
            state: Mutex::new(state),
        }
    }

    #[cfg(feature = "production")]
    pub fn create_with_host_config(
        config: HostConfig,
        platform: SurfacePlatform,
        host_config: HostCreateConfig,
    ) -> Result<Self, FeatureHostError> {
        validate_config(&config)?;
        if config.mode == HostMode::Test {
            let test_data_dir = host_config.runtime.data_dir.clone();
            return Ok(Self::create_test_backend(
                config,
                platform,
                test_data_dir.as_deref(),
            ));
        }
        let automation_path = host_config.automation_path.clone().or_else(|| {
            host_config
                .runtime
                .data_dir
                .as_ref()
                .map(|data_dir| data_dir.join("automations.json"))
        });
        let bot_state_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("bots.json"));
        let group_state_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("groups.json"));
        let peer_messages_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("peer-messages.json"));
        let settings_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("settings.json"));
        let remote_device_state_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("remote-computer-device.json"));
        let async_tasks_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("pending-async-tasks.json"));
        let memory_root_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("agents"));
        let workflow_root_path = host_config
            .runtime
            .data_dir
            .as_ref()
            .map(|data_dir| data_dir.join("workflows"));
        let channel_store = AgentChannelStore::new(
            host_config.runtime.data_dir.as_deref(),
            host_config.product_storage_passphrase.clone(),
        );
        let runtime = MahayanaHost::create(host_config)?;
        let info = HostInfo {
            runtime_version: format!("mahayana-abi-{}", runtime.status().runtime_abi_version),
            protocol_version: HOST_PROTOCOL_VERSION.to_string(),
            platform,
        };
        let mut state = FeatureState::default();
        if let Some(path) = settings_path.as_deref() {
            state.settings = load_product_host_settings(path);
            // The bundled Computer Use MCP independently rereads this canonical
            // policy before every tool call. Persist defaults during startup so
            // a first-run profile is explicit rather than relying on fail-open
            // behavior while the settings UI has not yet written the file.
            persist_product_host_settings(path, &state.settings)?;
        }
        if let Some(path) = remote_device_state_path.as_deref() {
            state.remote_computer_device_secrets = load_remote_computer_device_secrets(path);
        }
        sync_computer_control_policy(&state.settings);
        state.events.push_back(HostEvent::HostReady {
            timestamp: timestamp(),
            info: info.clone(),
        });
        let controller = Self {
            config,
            info,
            runtime: Some(runtime),
            automation_path,
            bot_state_path,
            group_state_path,
            peer_messages_path,
            settings_path,
            remote_device_state_path,
            async_tasks_path,
            channel_store,
            test_auth_state_path: None,
            memory_root_path,
            workflow_root_path,
            teach_recording: Mutex::new(None),
            active_account_id: Mutex::new(None),
            account_boundary_initialized: Mutex::new(false),
            client_side_tool_v2: Mutex::new(ClientSideToolV2Producer::new()),
            routine_dispatch_lock: Mutex::new(()),
            state: Mutex::new(state),
        };
        controller.ensure_account_boundary(&controller.auth_status()?)?;
        controller.state()?.events.push_back(HostEvent::HostReady {
            timestamp: timestamp(),
            info: controller.info.clone(),
        });
        Ok(controller)
    }

    fn async_tasks_path_for_account(&self, account_id: Option<&str>) -> Option<PathBuf> {
        match (self.async_tasks_path.as_deref(), account_id) {
            (Some(base), Some(account_id)) if !account_id.is_empty() => {
                Some(account_scoped_path(base, account_id))
            }
            _ => None,
        }
    }

    #[cfg(feature = "production")]
    fn rearm_pending_async_operations(&self) -> Result<(), FeatureHostError> {
        let candidates = {
            let state = self.state()?;
            let routine_operations = state
                .routine_executions
                .values()
                .filter_map(|execution| execution.operation_id.as_deref())
                .collect::<BTreeSet<_>>();
            let background_operations = state
                .background_recoveries
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let mut candidates = BTreeMap::<String, (String, String)>::new();
            for (task_id, operation_id) in &state.async_task_operation_ids {
                if routine_operations.contains(operation_id.as_str())
                    || background_operations.contains(operation_id.as_str())
                {
                    continue;
                }
                let Some(task) = state.async_tasks.get(task_id) else {
                    continue;
                };
                if task.kind == AsyncTaskKind::CloudAgent {
                    continue;
                }
                let conversation_id = state
                    .bots
                    .get(&task.parent_agent_id)
                    .and_then(|bot| bot.conversation_id.clone())
                    .unwrap_or_else(|| MAHAYANA_AI_CONVERSATION_ID.to_string());
                candidates
                    .entry(operation_id.clone())
                    .or_insert_with(|| (task.parent_agent_id.clone(), conversation_id));
            }
            candidates
        };

        for (operation_id, (agent_id, conversation_id)) in candidates {
            {
                let mut state = self.state()?;
                state.operations.insert(operation_id.clone());
                state
                    .operation_agents
                    .insert(operation_id.clone(), agent_id.clone());
            }
            let resumed = self.runtime()?.resume_operation(
                mahayana_conversation::ResumeConversationOperationRequest {
                    conversation_id: ConversationId(conversation_id),
                    operation_id: OperationId(operation_id.clone()),
                    hidden: false,
                    show_assistant_output: false,
                    reply_to_message_id: None,
                    is_fork: false,
                    attachment_batch_id: None,
                },
            );
            if resumed.is_err() {
                let mut state = self.state()?;
                state.operations.remove(&operation_id);
                state.operation_agents.remove(&operation_id);
            }
        }
        Ok(())
    }

    pub fn react_to_message(
        &self,
        agent_id: &str,
        entry_id: &str,
        emoji: &str,
    ) -> Result<Value, FeatureHostError> {
        let agent_id = required(agent_id.to_string(), "reaction agent id")?;
        let entry_id = required(entry_id.to_string(), "reaction entry id")?;
        let emoji = emoji.trim().to_string();
        if emoji.is_empty() {
            return Ok(json!({"applied": false}));
        }
        #[cfg(feature = "production")]
        {
            self.require_authenticated_account()?;
            let target = {
                let state = self.state()?;
                ensure_open(&state)?;
                state
                    .bots
                    .get(&agent_id)
                    .cloned()
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown bot: {agent_id}")))?
            };
            let conversation_id = target.conversation_id.clone().ok_or_else(|| {
                FeatureHostError::Contract(format!("bot has no conversation: {}", target.id))
            })?;
            let runtime = self.runtime()?;
            let runtime_conversation_id = ConversationId(conversation_id.clone());
            let mut message = runtime
                .conversation_history(runtime_conversation_id.clone(), 500)?
                .into_iter()
                .find(|message| message.id.as_str() == entry_id)
                .ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "reaction entry is not in canonical transcript: {entry_id}"
                    ))
                })?;
            let is_user_message = matches!(message.role, RuntimeMessageRole::User);
            let mut reactions = message
                .metadata
                .get("reactions")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| {
                            let emoji = row.get("emoji")?.as_str()?.trim();
                            let by = row.get("by")?.as_str()?.trim();
                            (!emoji.is_empty() && !by.is_empty())
                                .then(|| (emoji.to_string(), by.to_string()))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let had = reactions
                .iter()
                .any(|(candidate, by)| candidate == &emoji && by == "me");
            if had {
                reactions.retain(|(candidate, by)| !(candidate == &emoji && by == "me"));
            } else {
                reactions.push((emoji.clone(), "me".into()));
            }

            if !message.metadata.is_object() {
                message.metadata = json!({});
            }
            let metadata = message
                .metadata
                .as_object_mut()
                .expect("reaction metadata must be an object");
            if reactions.is_empty() {
                metadata.remove("reactions");
            } else {
                metadata.insert(
                    "reactions".into(),
                    Value::Array(
                        reactions
                            .iter()
                            .map(|(emoji, by)| json!({"emoji": emoji, "by": by}))
                            .collect(),
                    ),
                );
            }
            if !runtime.replace_conversation_message(runtime_conversation_id, message.clone())? {
                return Ok(json!({"applied": false}));
            }

            let reaction_rows = reactions
                .iter()
                .map(|(emoji, by)| json!({"emoji": emoji, "by": by}))
                .collect::<Vec<_>>();
            let my_reactions = reactions
                .iter()
                .filter(|(_, by)| by == "me")
                .map(|(emoji, _)| Value::String(emoji.clone()))
                .collect::<Vec<_>>();
            {
                let mut state = self.state()?;
                ensure_open(&state)?;
                state.events.push_back(HostEvent::TransportEvent {
                    channel: "transcript.reaction".into(),
                    payload: json!({
                        "agentId": agent_id,
                        "entryId": entry_id,
                        "reactions": reaction_rows.clone(),
                        "myReactions": my_reactions.clone(),
                    }),
                });
            }

            let mut resume_operation_id = None;
            if !had && !is_user_message {
                let normalized = message.text.split_whitespace().collect::<Vec<_>>().join(" ");
                let quote = if normalized.is_empty() {
                    entry_id.clone()
                } else if normalized.chars().count() > 80 {
                    format!("{}…", normalized.chars().take(80).collect::<String>())
                } else {
                    normalized
                };
                let prompt = format!(
                    "[The user reacted {emoji} to your message: \"{quote}\". You don't need to reply; act on it only if it's useful (e.g. acknowledge, adjust, or continue).]"
                );
                resume_operation_id = self.schedule_background_agent_turn(
                    &target,
                    "message-reaction",
                    prompt,
                    format!("reaction:{}:{}", entry_id, now_millis()),
                    Vec::new(),
                )?;
            }
            return Ok(json!({
                "applied": true,
                "isAdding": !had,
                "reactions": reaction_rows,
                "myReactions": my_reactions,
                "resumeOperationId": resume_operation_id,
            }));
        }
        #[cfg(not(feature = "production"))]
        {
            let _ = (agent_id, entry_id, emoji);
            Err(FeatureHostError::ProductionUnavailable)
        }
    }

    pub fn pending_cloud_agent_wakes(
        &self,
    ) -> Result<Vec<NativeCloudAgentWake>, FeatureHostError> {
        let state = self.state()?;
        ensure_open(&state)?;
        let mut wakes = state
            .async_tasks
            .iter()
            .filter_map(|(key, task)| {
                if task.kind != AsyncTaskKind::CloudAgent {
                    return None;
                }
                let operation_id = state.async_task_operation_ids.get(key)?;
                Some(NativeCloudAgentWake {
                    agent_id: task.parent_agent_id.clone(),
                    work_id: task.id.clone(),
                    operation_id: operation_id.clone(),
                    title: task.label.clone(),
                    started_at_ms: task.started_at_ms,
                })
            })
            .collect::<Vec<_>>();
        wakes.sort_by(|left, right| {
            left.started_at_ms
                .cmp(&right.started_at_ms)
                .then_with(|| left.work_id.cmp(&right.work_id))
        });
        Ok(wakes)
    }

    pub fn settle_cloud_agent_wake(
        &self,
        agent_id: &str,
        work_id: &str,
        status: &str,
        result: &str,
    ) -> Result<bool, FeatureHostError> {
        let agent_id = required(agent_id.to_string(), "cloud agent parent id")?;
        let work_id = required(work_id.to_string(), "cloud agent work id")?;
        let status = match status.trim() {
            "completed" | "error" => status.trim().to_string(),
            other => {
                return Err(FeatureHostError::Contract(format!(
                    "cloud agent wake settlement requires completed/error status, got {other}"
                )));
            }
        };
        let task_key = async_task_key(&agent_id, AsyncTaskKind::CloudAgent, &work_id);
        let (task, target) = {
            let state = self.state()?;
            ensure_open(&state)?;
            let Some(task) = state.async_tasks.get(&task_key).cloned() else {
                return Ok(false);
            };
            if task.parent_agent_id != agent_id {
                return Err(FeatureHostError::Contract(
                    "cloud agent wake owner does not match settlement parent".into(),
                ));
            }
            (task, state.bots.get(&agent_id).cloned())
        };

        if let Some(target) = target {
            let outcome = if status == "error" { "failed" } else { "finished" };
            let result = if result.trim().is_empty() {
                "(the cloud agent finished without producing any output)"
            } else {
                result.trim()
            };
            let prompt = format!(
                "[A background task just completed] A background task you started has finished.\n\nBackground task \"{}\" (cursor-agent) {outcome}:\n{result}\n\nPick the work back up: review the result, then either keep going or wrap up. If this result is genuinely new and relevant to the user, or the user asked to be told when this finished, tell them with a SendMessage. Lead with the concrete thing that finished, not a bare pronoun like \"That\". If it is stale, irrelevant, already handled, or a duplicate, and the user was not waiting on it, stay silent and end the turn with no SendMessage. Keep your status current, and clear it once everything is done and you're idle.",
                task.label
            );
            self.schedule_background_agent_turn(
                &target,
                "subagent-revival",
                prompt,
                format!("cloud-agent-revival:{agent_id}:{work_id}"),
                Vec::new(),
            )?;
        }

        let account_id = self
            .active_account_id
            .lock()
            .map_err(|_| FeatureHostError::StatePoisoned)?
            .clone();
        let async_tasks_path = self.async_tasks_path_for_account(account_id.as_deref());
        let mut state = self.state()?;
        state.async_tasks.remove(&task_key);
        state.async_task_operation_ids.remove(&task_key);
        persist_pending_async_tasks(
            async_tasks_path.as_deref(),
            &state.async_tasks,
            &state.async_task_operation_ids,
        )?;
        let mut tasks = state
            .async_tasks
            .values()
            .filter(|task| task.parent_agent_id == agent_id)
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| {
            left.started_at_ms
                .cmp(&right.started_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        state.events.push_back(HostEvent::AsyncTaskChanged {
            timestamp: timestamp(),
            agent_id,
            tasks,
        });
        Ok(true)
    }

    pub fn async_tasks_for_agent(
        &self,
        agent_id: &str,
    ) -> Result<Vec<AsyncTaskSummary>, FeatureHostError> {
        let state = self.state()?;
        ensure_open(&state)?;
        let mut tasks = state
            .async_tasks
            .values()
            .filter(|task| task.parent_agent_id == agent_id)
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|left, right| {
            left.started_at_ms
                .cmp(&right.started_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(tasks)
    }

    pub fn info(&self) -> HostInfo {
        self.info.clone()
    }

    /// Returns the canonical Host-owned automation roster without creating a
    /// second renderer-side store. Command-palette consumers receive a snapshot
    /// of the same durable automation state used by automation commands.
    pub fn list_all_automations(&self) -> Result<Vec<AutomationSummary>, FeatureHostError> {
        let state = self.state()?;
        let mut automations = state.automations.values().cloned().collect::<Vec<_>>();
        automations.sort_by(|left, right| {
            left.created_at_ms
                .cmp(&right.created_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(automations)
    }

    /// Export one private workflow as the Desktop-compatible plugin-shaped
    /// tar.gz payload. Authentication and network mutation remain Coordinator-owned.
    pub fn export_workflow_publish_package(
        &self,
        agent_id: &str,
        workflow_id: &str,
    ) -> Result<Value, FeatureHostError> {
        if !is_safe_memory_agent_id(agent_id) || !is_safe_memory_agent_id(workflow_id) {
            return Err(FeatureHostError::Contract(
                "unsafe workflow publish identity".into(),
            ));
        }
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let agent_root = self
            .active_account_root(self.memory_root_path.as_deref())
            .ok_or_else(|| FeatureHostError::Contract("agent storage is unavailable".into()))?;
        if !self.state()?.bots.contains_key(agent_id) {
            return Err(FeatureHostError::Contract(format!(
                "unknown workflow owner: {agent_id}"
            )));
        }
        let summary = load_workflow_summary(workflow_root, &agent_root, agent_id, workflow_id)
            .ok_or_else(|| FeatureHostError::Contract(format!(
                "unknown private workflow: {workflow_id}"
            )))?;
        if summary.description.trim().is_empty() {
            return Err(FeatureHostError::Contract(
                "A description is required before publishing a skill.".into(),
            ));
        }
        let workflow_dir = workflow_root.join(workflow_id);
        let bytes = pack_workflow_plugin_artifact(
            &workflow_dir,
            workflow_id,
            &summary.name,
        )?;
        Ok(json!({
            "workflowId": summary.id,
            "name": normalize_marketplace_plugin_name(workflow_id),
            "displayName": summary.name,
            "description": summary.description,
            "pluginTarGzBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }))
    }

    pub fn sync_workflow_plugin_facts(
        &self,
        agent_id: &str,
        plugins: Value,
    ) -> Result<Value, FeatureHostError> {
        if !is_safe_memory_agent_id(agent_id) {
            return Err(FeatureHostError::Contract("unsafe published skill agent id".into()));
        }
        if !self.state()?.bots.contains_key(agent_id) {
            return Err(FeatureHostError::Contract(format!(
                "unknown workflow owner: {agent_id}"
            )));
        }
        let plugins: Vec<PublishedPluginSnapshot> = serde_json::from_value(plugins)
            .map_err(|error| FeatureHostError::Contract(format!(
                "invalid published skill projection: {error}"
            )))?;
        for plugin in &plugins {
            if plugin.plugin_id.trim().is_empty()
                || !is_exact_skill_publish_version(&plugin.plugin_version)
            {
                return Err(FeatureHostError::Contract(
                    "published skill projection has an invalid plugin identity".into(),
                ));
            }
        }
        let count = plugins.len();
        self.state()?
            .published_plugins_by_agent
            .insert(agent_id.to_string(), plugins);
        Ok(json!({"agentId": agent_id, "count": count}))
    }

    pub fn confirm_workflow_publish(
        &self,
        agent_id: &str,
        workflow_id: &str,
        plugin_id: &str,
        commit_sha: &str,
    ) -> Result<Value, FeatureHostError> {
        if !is_safe_memory_agent_id(agent_id)
            || !is_safe_memory_agent_id(workflow_id)
            || plugin_id.trim().is_empty()
            || !is_exact_skill_publish_version(commit_sha)
        {
            return Err(FeatureHostError::Contract(
                "unsafe skill publish confirmation identity".into(),
            ));
        }
        let snapshot = {
            let state = self.state()?;
            state
                .published_plugins_by_agent
                .get(agent_id)
                .and_then(|plugins| plugins.iter().find(|plugin| {
                    plugin.plugin_id == plugin_id
                        && plugin.plugin_version.eq_ignore_ascii_case(commit_sha)
                        && plugin.published_by_current_user
                }))
                .cloned()
        };
        let Some(snapshot) = snapshot else {
            return Ok(json!({"confirmed": false}));
        };
        let team_id = snapshot.marketplace_team_id.ok_or_else(|| {
            FeatureHostError::Contract(
                "Published skill confirmation has no team marketplace identity.".into(),
            )
        })?;
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let agent_root = self
            .active_account_root(self.memory_root_path.as_deref())
            .ok_or_else(|| FeatureHostError::Contract("agent storage is unavailable".into()))?;
        let cache_root = published_workflow_cache_root(workflow_root, plugin_id)?;

        if let Some(existing) = read_published_workflow_cache(&cache_root) {
            if existing.agent_id == agent_id
                && existing.original_workflow_id == workflow_id
                && existing.plugin_id == plugin_id
                && existing.plugin_version.eq_ignore_ascii_case(commit_sha)
                && cache_root.join("skill").join(WORKFLOW_FILENAME).is_file()
            {
                let private_dir = workflow_root.join(workflow_id);
                if private_dir.exists() {
                    std::fs::remove_dir_all(&private_dir).map_err(|error| {
                        FeatureHostError::Contract(format!(
                            "remove confirmed private workflow: {error}"
                        ))
                    })?;
                    forget_workflow_enablement(&agent_root, agent_id, workflow_id)?;
                }
                return Ok(json!({
                    "confirmed": true,
                    "promotedWorkflowId": existing.promoted_workflow_id,
                }));
            }
            return Err(FeatureHostError::Contract(
                "A different published skill cache already owns this plugin id.".into(),
            ));
        }

        let summary = load_workflow_summary(workflow_root, &agent_root, agent_id, workflow_id)
            .ok_or_else(|| FeatureHostError::Contract(
                "That private skill no longer exists.".into()
            ))?;
        let promoted_workflow_id = format!(
            "plugin-{}-{}",
            plugin_id,
            slugify_workflow_name(&summary.name)
        );
        let metadata = PublishedWorkflowCacheMetadata {
            agent_id: agent_id.to_string(),
            original_workflow_id: workflow_id.to_string(),
            promoted_workflow_id: promoted_workflow_id.clone(),
            plugin_id: plugin_id.to_string(),
            plugin_version: commit_sha.to_ascii_lowercase(),
            plugin_name: normalize_marketplace_plugin_name(workflow_id),
            display_name: summary.name.clone(),
            description: summary.description.clone(),
            marketplace_team_id: team_id,
            enabled_before_publish: is_workflow_enabled(&agent_root, agent_id, workflow_id),
            unpublish_restore_prepared: false,
        };
        let parent = cache_root.parent().ok_or_else(|| {
            FeatureHostError::Contract("Published skill cache path is invalid.".into())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create published skill cache parent: {error}"))
        })?;
        let temp_root = parent.join(format!(
            ".{}.tmp-{}",
            plugin_id,
            Uuid::new_v4()
        ));
        let cache_result = (|| {
            copy_workflow_tree(&workflow_root.join(workflow_id), &temp_root.join("skill"))?;
            write_published_workflow_cache(&temp_root, &metadata)?;
            if !temp_root.join("skill").join(WORKFLOW_FILENAME).is_file() {
                return Err(FeatureHostError::Contract(
                    "Published skill cache is missing SKILL.md.".into(),
                ));
            }
            std::fs::rename(&temp_root, &cache_root).map_err(|error| {
                FeatureHostError::Contract(format!("commit published skill cache: {error}"))
            })?;
            Ok::<(), FeatureHostError>(())
        })();
        if cache_result.is_err() {
            let _ = std::fs::remove_dir_all(&temp_root);
        }
        cache_result?;

        let private_dir = workflow_root.join(workflow_id);
        std::fs::remove_dir_all(&private_dir).map_err(|error| {
            FeatureHostError::Contract(format!("remove promoted private workflow: {error}"))
        })?;
        forget_workflow_enablement(&agent_root, agent_id, workflow_id)?;
        Ok(json!({
            "confirmed": true,
            "promotedWorkflowId": promoted_workflow_id,
        }))
    }

    pub fn confirm_published_workflow_resync(
        &self,
        agent_id: &str,
        workflow_id: &str,
        plugin_id: &str,
        commit_sha: &str,
    ) -> Result<Value, FeatureHostError> {
        if !is_safe_memory_agent_id(agent_id)
            || plugin_id.trim().is_empty()
            || !is_exact_skill_publish_version(commit_sha)
        {
            return Err(FeatureHostError::Contract(
                "unsafe skill resync confirmation identity".into(),
            ));
        }
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let snapshot = {
            let state = self.state()?;
            state
                .published_plugins_by_agent
                .get(agent_id)
                .and_then(|plugins| plugins.iter().find(|plugin| {
                    plugin.plugin_id == plugin_id
                        && plugin.plugin_version.eq_ignore_ascii_case(commit_sha)
                        && plugin.published_by_current_user
                }))
                .cloned()
        };
        let Some(snapshot) = snapshot else {
            return Ok(json!({"confirmed": false}));
        };
        let cache_root = published_workflow_cache_root(workflow_root, plugin_id)?;
        let mut metadata = read_published_workflow_cache(&cache_root)
            .ok_or_else(|| FeatureHostError::Contract(
                "That published skill's local cache is unavailable.".into()
            ))?;
        if metadata.agent_id != agent_id
            || metadata.promoted_workflow_id != workflow_id
            || metadata.plugin_id != plugin_id
        {
            return Err(FeatureHostError::Contract(
                "Published skill identity changed while syncing.".into(),
            ));
        }
        let team_id = snapshot.marketplace_team_id.ok_or_else(|| {
            FeatureHostError::Contract(
                "Published skill confirmation has no team marketplace identity.".into(),
            )
        })?;
        metadata.plugin_version = commit_sha.to_ascii_lowercase();
        metadata.marketplace_team_id = team_id;
        write_published_workflow_cache(&cache_root, &metadata)?;
        Ok(json!({
            "confirmed": true,
            "workflowId": workflow_id,
            "pluginId": plugin_id,
            "commitSha": metadata.plugin_version,
        }))
    }

    pub fn export_published_workflow_publish_package(
        &self,
        agent_id: &str,
        workflow_id: &str,
    ) -> Result<Value, FeatureHostError> {
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let agent_root = self
            .active_account_root(self.memory_root_path.as_deref())
            .ok_or_else(|| FeatureHostError::Contract("agent storage is unavailable".into()))?;
        let facts = self
            .state()?
            .published_plugins_by_agent
            .get(agent_id)
            .cloned()
            .unwrap_or_default();
        let (snapshot, metadata, cache_root) =
            find_published_workflow_cache(workflow_root, agent_id, workflow_id, &facts)
                .ok_or_else(|| FeatureHostError::Contract(
                    "That published skill is no longer installed.".into()
                ))?;
        if !snapshot.published_by_current_user {
            return Err(FeatureHostError::Contract(format!(
                "\"{}\" belongs to a plugin you did not publish.",
                metadata.display_name
            )));
        }
        let team_id = snapshot.marketplace_team_id.ok_or_else(|| {
            FeatureHostError::Contract(format!(
                "\"{}\" is not in a team marketplace.",
                metadata.display_name
            ))
        })?;
        let summary = load_workflow_summary(&cache_root, &agent_root, agent_id, "skill")
            .ok_or_else(|| FeatureHostError::Contract(
                "That published skill's local cache is unavailable.".into()
            ))?;
        let bytes = pack_workflow_plugin_artifact(
            &cache_root.join("skill"),
            &metadata.original_workflow_id,
            &summary.name,
        )?;
        Ok(json!({
            "workflowId": workflow_id,
            "pluginId": snapshot.plugin_id,
            "teamId": team_id,
            "name": metadata.plugin_name,
            "displayName": summary.name,
            "description": summary.description,
            "pluginTarGzBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }))
    }

    pub fn prepare_workflow_unpublish(
        &self,
        agent_id: &str,
        workflow_id: &str,
    ) -> Result<Value, FeatureHostError> {
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let agent_root = self
            .active_account_root(self.memory_root_path.as_deref())
            .ok_or_else(|| FeatureHostError::Contract("agent storage is unavailable".into()))?;
        let facts = self
            .state()?
            .published_plugins_by_agent
            .get(agent_id)
            .cloned()
            .unwrap_or_default();
        let (snapshot, mut metadata, cache_root) =
            find_published_workflow_cache(workflow_root, agent_id, workflow_id, &facts)
                .ok_or_else(|| FeatureHostError::Contract(
                    "That published skill is no longer installed.".into()
                ))?;
        if !snapshot.published_by_current_user {
            return Err(FeatureHostError::Contract(format!(
                "\"{}\" belongs to a plugin you did not publish.",
                metadata.display_name
            )));
        }
        let team_id = snapshot.marketplace_team_id.ok_or_else(|| {
            FeatureHostError::Contract(format!(
                "\"{}\" is not in a team marketplace.",
                metadata.display_name
            ))
        })?;
        let restored = workflow_root.join(&metadata.original_workflow_id);
        if restored.exists() {
            if !metadata.unpublish_restore_prepared
                || !restored.join(WORKFLOW_FILENAME).is_file()
                || !unpublish_recovery_marker_matches(&restored, &metadata)
            {
                return Err(FeatureHostError::Contract(format!(
                    "A private skill with id \"{}\" already exists; refusing to overwrite it.",
                    metadata.original_workflow_id
                )));
            }
            return Ok(json!({
                "pluginId": snapshot.plugin_id,
                "teamId": team_id,
                "restoredWorkflowId": metadata.original_workflow_id,
                "restoreReused": true,
            }));
        }

        // Persist the recovery intent before materializing the private copy.
        // If the remote unpublish later fails (or the app exits between these
        // steps), a retry can safely reuse or recreate only the copy that this
        // Host prepared, while unrelated pre-existing private skills still
        // fail closed above.
        if !metadata.unpublish_restore_prepared {
            metadata.unpublish_restore_prepared = true;
            write_published_workflow_cache(&cache_root, &metadata)?;
        }
        let temp_restore = workflow_root.join(format!(
            ".restore-{}-{}",
            metadata.original_workflow_id,
            Uuid::new_v4()
        ));
        let restore_result = (|| {
            copy_workflow_tree(&cache_root.join("skill"), &temp_restore)?;
            write_unpublish_recovery_marker(&temp_restore, &metadata)?;
            std::fs::rename(&temp_restore, &restored).map_err(|error| {
                FeatureHostError::Contract(format!("restore private skill: {error}"))
            })?;
            Ok::<(), FeatureHostError>(())
        })();
        if restore_result.is_err() {
            let _ = std::fs::remove_dir_all(&temp_restore);
        }
        restore_result?;
        set_workflow_enabled(
            &agent_root,
            agent_id,
            &metadata.original_workflow_id,
            metadata.enabled_before_publish,
        )?;
        Ok(json!({
            "pluginId": snapshot.plugin_id,
            "teamId": team_id,
            "restoredWorkflowId": metadata.original_workflow_id,
            "restoreReused": false,
        }))
    }

    pub fn complete_workflow_unpublish(
        &self,
        agent_id: &str,
        plugin_id: &str,
    ) -> Result<Value, FeatureHostError> {
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let cache_root = published_workflow_cache_root(workflow_root, plugin_id)?;
        if let Some(metadata) = read_published_workflow_cache(&cache_root) {
            let restored = workflow_root.join(&metadata.original_workflow_id);
            if unpublish_recovery_marker_matches(&restored, &metadata) {
                let marker = restored.join(PUBLISHED_WORKFLOW_RECOVERY_MARKER);
                if marker.exists() {
                    std::fs::remove_file(&marker).map_err(|error| {
                        FeatureHostError::Contract(format!(
                            "remove unpublish recovery marker: {error}"
                        ))
                    })?;
                }
            }
        }
        if cache_root.exists() {
            std::fs::remove_dir_all(&cache_root).map_err(|error| {
                FeatureHostError::Contract(format!("remove unpublished skill cache: {error}"))
            })?;
        }
        if let Some(plugins) = self
            .state()?
            .published_plugins_by_agent
            .get_mut(agent_id)
        {
            plugins.retain(|plugin| plugin.plugin_id != plugin_id);
        }
        Ok(json!({"pluginId": plugin_id, "removed": true}))
    }

    pub fn set_scene_active(&self, active: bool) -> Result<Value, FeatureHostError> {
        let now_ms = now_millis();
        let mut state = self.state()?;
        state.conversation_session.set_scene_active(active, now_ms);
        let focused_at_ms = state.conversation_session.focused_at_ms;
        drop(state);
        self.set_routine_quiescing(!active)?;
        Ok(json!({
            "active": active,
            "focusedAtMs": focused_at_ms,
        }))
    }

    pub fn note_scene_contact(&self) -> Result<(), FeatureHostError> {
        let now_ms = now_millis();
        self.state()?.conversation_session.note_contact(now_ms);
        Ok(())
    }

    /// Return UI-safe account state. Credentials stay inside the Rust product
    /// client and are never serialized across the presentation boundary.
    pub fn auth_status(&self) -> Result<Value, FeatureHostError> {
        match self.config.mode {
            HostMode::Test => {
                let state = self.state()?;
                Ok(match state.auth_user.as_ref() {
                    Some(user) => json!({
                        "@type": "mahayana.auth.status",
                        "loggedIn": true,
                        "provider": "test",
                        "user": user,
                    }),
                    None => json!({
                        "@type": "mahayana.auth.status",
                        "loggedIn": false,
                        "provider": "test",
                    }),
                })
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    // First paint must be local-first. Restoring the UI-safe
                    // account from the Rust-owned session is immediate and
                    // lets an offline macOS launch remain signed in. Network
                    // operations still validate the token at their boundary.
                    let session = if let Ok(session) = self
                        .runtime()?
                        .product_execute("mahayana.auth.session.restore", &json!({}))
                    {
                        session
                    } else {
                        self.runtime()?
                            .product_execute("mahayana.auth.status", &json!({}))
                            .map_err(FeatureHostError::from)?
                    };
                    self.ensure_account_boundary(&session)?;
                    Ok(session)
                }
                #[cfg(not(feature = "production"))]
                Err(FeatureHostError::ProductionUnavailable)
            }
        }
    }

    /// Issue a short-lived self-hosted messaging credential bound to the
    /// currently authenticated Fabushi account, one device, one client
    /// session, and an explicit set of scopes. The underlying account token is
    /// never exposed; only the derived messaging bearer token is returned once.
    pub fn issue_messaging_access(
        &self,
        device_id: String,
        session_id: String,
        requested_scopes: Vec<String>,
        ttl_ms: i64,
    ) -> Result<Value, FeatureHostError> {
        let device_id = required(device_id, "device id")?;
        let session_id = required(session_id, "session id")?;
        let auth = self.auth_status()?;
        if auth.get("loggedIn").and_then(Value::as_bool) != Some(true) {
            return Err(FeatureHostError::Contract(
                "messaging access requires an authenticated Fabushi account session".into(),
            ));
        }
        let user_id = stable_authenticated_account_id(&auth).ok_or_else(|| {
            FeatureHostError::Contract("authenticated account has no stable user id".into())
        })?;
        let digest = Sha256::digest(user_id.as_bytes());
        let account_fingerprint = digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let actor_id = ActorId::new(format!("human:account:{account_fingerprint}"));
        let mut scopes = std::collections::BTreeSet::new();
        for scope in requested_scopes {
            let parsed = match scope.as_str() {
                "messaging" => AccessScope::Messaging,
                "calls" => AccessScope::Calls,
                "blobsRead" => AccessScope::BlobsRead,
                "blobsWrite" => AccessScope::BlobsWrite,
                "payments" => AccessScope::Payments,
                "miniApps" => AccessScope::MiniApps,
                "administration" => AccessScope::Administration,
                _ => {
                    return Err(FeatureHostError::Contract(format!(
                        "unsupported messaging access scope: {scope}"
                    )));
                }
            };
            scopes.insert(parsed);
        }
        if scopes.is_empty() {
            scopes.extend([
                AccessScope::Messaging,
                AccessScope::Calls,
                AccessScope::BlobsRead,
                AccessScope::BlobsWrite,
            ]);
        }
        let now_ms = now_millis();
        let ttl_ms = ttl_ms.clamp(5 * 60 * 1000, 30 * 24 * 60 * 60 * 1000);
        let expires_at_ms = now_ms.saturating_add(ttl_ms);
        let root = self
            .memory_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("messaging storage is unavailable".into()))?;
        let access_store = FileAccessTokenStore::new(root.join("_messaging").join("access.json"));
        let grant_id = format!("grant:{}", Uuid::new_v4().simple());
        let issued = access_store
            .issue_random(AccessGrant {
                id: grant_id,
                actor_id: actor_id.clone(),
                device_id: device_id.clone(),
                session_id: session_id.clone(),
                scopes: scopes.clone(),
                issued_at_ms: now_ms,
                expires_at_ms: Some(expires_at_ms),
                revoked_at_ms: None,
            })
            .map_err(|error| FeatureHostError::Contract(error.to_string()))?;
        Ok(json!({
            "@type": "fabushi.messaging.access",
            "actorId": actor_id.0,
            "deviceId": device_id,
            "sessionId": session_id,
            "accessToken": issued.token,
            "expiresAtMs": expires_at_ms,
            "scopes": scopes,
        }))
    }

    pub fn password_login(
        &self,
        username: String,
        password: String,
    ) -> Result<Value, FeatureHostError> {
        let username = required(username, "username")?;
        let password = required(password, "password")?;
        #[cfg(not(feature = "production"))]
        let _ = &password;
        match self.config.mode {
            HostMode::Test => {
                let user = json!({
                    "id": "fast-e2e-user",
                    "username": username,
                    "nickname": "本地测试用户",
                });
                {
                    self.state()?.auth_user = Some(user.clone());
                }
                self.persist_test_auth_user(Some(&user))?;
                Ok(json!({
                    "@type": "mahayana.auth.session",
                    "loggedIn": true,
                    "provider": "test",
                    "sessionStored": true,
                    "user": user,
                }))
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let response = self
                        .runtime()?
                        .product_execute(
                            "mahayana.auth.password.login",
                            &json!({"username": username, "password": password}),
                        )
                        .map_err(FeatureHostError::from)?;
                    self.ensure_account_boundary(&response)?;
                    Ok(response)
                }
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn browser_login_start(&self) -> Result<Value, FeatureHostError> {
        match self.config.mode {
            HostMode::Test => Ok(json!({
                "attemptId": "test-browser-login",
                "loginUrl": "about:blank#fabushi-test-browser-login",
                "expiresAt": now_millis() / 1000 + 600,
                "pollAfterMs": 250,
            })),
            HostMode::Production => {
                #[cfg(feature = "production")]
                return self
                    .runtime()?
                    .product_execute(
                        "mahayana.auth.browser.start",
                        &json!({"platform": browser_login_platform(self.info.platform)}),
                    )
                    .map_err(FeatureHostError::from);
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn browser_login_reopen(&self, attempt_id: String) -> Result<Value, FeatureHostError> {
        let attempt_id = required(attempt_id, "attemptId")?;
        match self.config.mode {
            HostMode::Test => Ok(json!({
                "status": "pending",
                "attemptId": attempt_id,
                "loginUrl": "about:blank#fabushi-test-browser-login",
                "pollAfterMs": 120,
            })),
            HostMode::Production => {
                #[cfg(feature = "production")]
                return self
                    .runtime()?
                    .product_execute(
                        "mahayana.auth.browser.reopen",
                        &json!({"attemptId": attempt_id}),
                    )
                    .map_err(FeatureHostError::from);
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn browser_login_cancel(&self, attempt_id: String) -> Result<Value, FeatureHostError> {
        let attempt_id = required(attempt_id, "attemptId")?;
        match self.config.mode {
            HostMode::Test => Ok(json!({"status": "cancelled"})),
            HostMode::Production => {
                #[cfg(feature = "production")]
                return self
                    .runtime()?
                    .product_execute(
                        "mahayana.auth.browser.cancel",
                        &json!({"attemptId": attempt_id}),
                    )
                    .map_err(FeatureHostError::from);
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn browser_login_poll(&self, attempt_id: String) -> Result<Value, FeatureHostError> {
        let attempt_id = required(attempt_id, "attemptId")?;
        match self.config.mode {
            HostMode::Test => {
                if attempt_id != "test-browser-login" {
                    return Ok(json!({"status": "expired"}));
                }
                let user = json!({
                    "id": "fast-e2e-browser-user",
                    "email": "browser@example.test",
                    "nickname": "Browser 测试用户",
                });
                {
                    self.state()?.auth_user = Some(user.clone());
                }
                self.persist_test_auth_user(Some(&user))?;
                Ok(json!({
                    "status": "completed",
                    "provider": "browser",
                    "auth": {
                        "loggedIn": true,
                        "provider": "browser",
                        "user": user,
                    }
                }))
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let response = self
                        .runtime()?
                        .product_execute(
                            "mahayana.auth.browser.poll",
                            &json!({"attemptId": attempt_id}),
                        )
                        .map_err(FeatureHostError::from)?;
                    if auth_payload(&response)
                        .get("loggedIn")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        self.ensure_account_boundary(&response)?;
                    }
                    Ok(response)
                }
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn auth_providers(&self) -> Result<Value, FeatureHostError> {
        match self.config.mode {
            HostMode::Test => Ok(json!([
                {"id": "google", "displayName": "Google", "enabled": true},
                {"id": "apple", "displayName": "Apple", "enabled": true},
                {"id": "microsoft", "displayName": "Microsoft", "enabled": true},
                {"id": "github", "displayName": "GitHub", "enabled": true}
            ])),
            HostMode::Production => {
                #[cfg(feature = "production")]
                return self
                    .runtime()?
                    .product_execute("mahayana.auth.oauth.providers", &json!({}))
                    .map_err(FeatureHostError::from);
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn oauth_start(&self, provider: String) -> Result<Value, FeatureHostError> {
        let provider = required(provider, "provider")?;
        match self.config.mode {
            HostMode::Test => Ok(json!({
                "attemptId": format!("test-oauth-{provider}"),
                "provider": provider,
                "authorizationUrl": format!("about:blank#fabushi-test-oauth-{provider}"),
            })),
            HostMode::Production => {
                #[cfg(feature = "production")]
                return self
                    .runtime()?
                    .product_execute(
                        "mahayana.auth.oauth.start",
                        &json!({"provider": provider, "platform": "macos"}),
                    )
                    .map_err(FeatureHostError::from);
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn oauth_poll(&self, attempt_id: String) -> Result<Value, FeatureHostError> {
        let attempt_id = required(attempt_id, "attemptId")?;
        match self.config.mode {
            HostMode::Test => {
                let user = json!({
                    "id": "fast-e2e-oauth-user",
                    "email": "oauth@example.test",
                    "nickname": "OAuth 测试用户",
                });
                {
                    self.state()?.auth_user = Some(user.clone());
                }
                self.persist_test_auth_user(Some(&user))?;
                Ok(json!({
                    "attemptId": attempt_id,
                    "status": "completed",
                    "auth": {
                        "loggedIn": true,
                        "provider": "google",
                        "user": user,
                    }
                }))
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let response = self
                        .runtime()?
                        .product_execute(
                            "mahayana.auth.oauth.poll",
                            &json!({"attemptId": attempt_id}),
                        )
                        .map_err(FeatureHostError::from)?;
                    if auth_payload(&response)
                        .get("loggedIn")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        self.ensure_account_boundary(&response)?;
                    }
                    Ok(response)
                }
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn logout(&self) -> Result<Value, FeatureHostError> {
        match self.config.mode {
            HostMode::Test => {
                {
                    self.state()?.auth_user = None;
                }
                self.persist_test_auth_user(None)?;
                Ok(json!({
                    "@type": "mahayana.auth.session",
                    "loggedIn": false,
                    "revoked": true,
                }))
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let response = self.runtime()?.clear_session()?;
                    self.ensure_account_boundary(&response)?;
                    Ok(response)
                }
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    /// UI-safe, server-authoritative account model-usage projection.
    ///
    /// Production data is fetched only through the Rust-owned product session;
    /// no bearer/refresh credential crosses the Host boundary.
    pub fn usage_status(&self) -> Result<Value, FeatureHostError> {
        match self.config.mode {
            HostMode::Test => Ok(json!({
                "windowStart": 1_725_235_200_i64,
                "windowEnd": 1_725_840_000_i64,
                "tokenLimit": 100_000_i64,
                "usedTokens": 25_000_i64,
                "reservedTokens": 5_000_i64,
                "remainingTokens": 70_000_i64,
                "unlimited": false,
            })),
            HostMode::Production => {
                #[cfg(feature = "production")]
                return self
                    .runtime()?
                    .product_execute("mahayana.usage.status", &json!({}))
                    .map_err(FeatureHostError::from);
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    pub fn execute(&self, command: FeatureCommand) -> Result<CommandAccepted, FeatureHostError> {
        if let FeatureCommand::MessagingExecute {
            request_id,
            envelope,
        } = &command
        {
            return self.execute_messaging(request_id.clone(), envelope.clone());
        }
        if matches!(
            &command,
            FeatureCommand::AutomationList { .. }
                | FeatureCommand::AutomationUpsert { .. }
                | FeatureCommand::AutomationSetEnabled { .. }
                | FeatureCommand::AutomationDelete { .. }
                | FeatureCommand::AutomationRun { .. }
        ) {
            return self.execute_automation(command);
        }
        if matches!(
            &command,
            FeatureCommand::BotCreate { .. }
                | FeatureCommand::BotUpdate { .. }
                | FeatureCommand::BotClone { .. }
                | FeatureCommand::BotDelete { .. }
                | FeatureCommand::BotSetHidden { .. }
        ) {
            return self.execute_bot_profile(command);
        }
        if matches!(
            &command,
            FeatureCommand::GroupList { .. }
                | FeatureCommand::GroupCreate { .. }
                | FeatureCommand::GroupUpdate { .. }
                | FeatureCommand::GroupDelete { .. }
                | FeatureCommand::GroupSend { .. }
        ) {
            return self.execute_group_chat(command);
        }
        if matches!(
            &command,
            FeatureCommand::AgentSend { .. }
                | FeatureCommand::AgentBroadcast { .. }
                | FeatureCommand::AgentPeerHistory { .. }
        ) {
            return self.execute_agent_messaging(command);
        }
        if matches!(
            &command,
            FeatureCommand::SubagentList { .. } | FeatureCommand::AsyncTaskList { .. }
        ) {
            return self.execute_subagent_observation(command);
        }
        if matches!(
            &command,
            FeatureCommand::TeachStatus { .. }
                | FeatureCommand::TeachStart { .. }
                | FeatureCommand::TeachStop { .. }
        ) {
            return self.execute_teach(command);
        }
        if matches!(
            &command,
            FeatureCommand::ComputerStatus { .. }
                | FeatureCommand::ComputerScreenshot { .. }
                | FeatureCommand::ComputerAction { .. }
        ) {
            return self.execute_computer(command);
        }
        if matches!(
            &command,
            FeatureCommand::RemoteComputerRegister { .. }
                | FeatureCommand::RemoteComputerHeartbeat { .. }
                | FeatureCommand::RemoteComputerClients { .. }
                | FeatureCommand::RemoteComputerClientRevoke { .. }
                | FeatureCommand::RemoteComputerSessions { .. }
                | FeatureCommand::RemoteComputerSessionActivate { .. }
                | FeatureCommand::RemoteComputerSessionClose { .. }
                | FeatureCommand::RemoteComputerSignal { .. }
                | FeatureCommand::RemoteComputerSignalDrain { .. }
        ) {
            return self.execute_remote_computer(command);
        }
        if matches!(
            &command,
            FeatureCommand::MemoryList { .. }
                | FeatureCommand::MemoryAdd { .. }
                | FeatureCommand::MemoryRemove { .. }
                | FeatureCommand::MemoryClear { .. }
        ) {
            return self.execute_memory(command);
        }
        if matches!(
            &command,
            FeatureCommand::TrayList { .. }
                | FeatureCommand::TrayDismiss { .. }
                | FeatureCommand::TrayClear { .. }
                | FeatureCommand::TrayClearForAgent { .. }
        ) {
            return self.execute_tray(command);
        }
        if matches!(
            &command,
            FeatureCommand::WorkflowList { .. }
                | FeatureCommand::WorkflowUpsert { .. }
                | FeatureCommand::WorkflowSetEnabled { .. }
                | FeatureCommand::WorkflowDelete { .. }
                | FeatureCommand::WorkflowRun { .. }
                | FeatureCommand::WorkflowImportMarkdown { .. }
                | FeatureCommand::WorkflowImportLiveSource { .. }
        ) {
            return self.execute_workflow(command);
        }
        if matches!(
            &command,
            FeatureCommand::AttachmentUpload { .. }
                | FeatureCommand::AttachmentReadText { .. }
                | FeatureCommand::AttachmentReadChunk { .. }
                | FeatureCommand::AttachmentReadImage { .. }
        ) {
            return self.execute_attachment(command);
        }
        if matches!(
            &command,
            FeatureCommand::SearchMessages { .. } | FeatureCommand::SearchMedia { .. }
        ) {
            return self.execute_search(command);
        }
        if matches!(
            &command,
            FeatureCommand::McpList { .. }
                | FeatureCommand::McpApps { .. }
                | FeatureCommand::McpOauthLogin { .. }
                | FeatureCommand::McpOauthLogout { .. }
                | FeatureCommand::McpRemove { .. }
                | FeatureCommand::McpSetCustomInstructions { .. }
                | FeatureCommand::McpSetToolDisabled { .. }
                | FeatureCommand::McpRefresh { .. }
                | FeatureCommand::McpToolCall { .. }
        ) {
            return self.execute_mcp(command);
        }
        if matches!(
            &command,
            FeatureCommand::SettingsGet { .. }
                | FeatureCommand::SettingsUpdate { .. }
                | FeatureCommand::AuditList { .. }
        ) {
            return self.execute_settings_and_audit(command);
        }
        if matches!(&command, FeatureCommand::BoxHandoffResolve { .. }) {
            return self.resolve_box_handoff(command);
        }
        if is_product_surface_command(&command) {
            return self.execute_product_surface(command);
        }
        match self.config.mode {
            HostMode::Test => self.execute_test(command),
            HostMode::Production => self.execute_production(command),
        }
    }

    fn resolve_box_handoff(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let FeatureCommand::BoxHandoffResolve {
            request_id,
            handoff_request_id,
            agent_id,
            resolution,
        } = command
        else {
            unreachable!("box handoff resolver only accepts box.handoff.resolve");
        };
        let resolution = resolution.trim().to_string();
        if !matches!(
            resolution.as_str(),
            "completed" | "dismissed" | "viewer-closed"
        ) {
            return Err(FeatureHostError::Contract(
                "box handoff resolution must be completed, dismissed, or viewer-closed".into(),
            ));
        }
        let pending = {
            let mut state = self.state()?;
            let Some(pending) = state.pending_box_handoffs.get_mut(&agent_id) else {
                return Err(FeatureHostError::Contract(format!(
                    "no pending box handoff for agent {agent_id}"
                )));
            };
            if pending.request_id != handoff_request_id {
                return Err(FeatureHostError::Contract(
                    "box handoff request identity is stale".into(),
                ));
            }
            if pending.resolving {
                return Err(FeatureHostError::Contract(
                    "box handoff is already settling".into(),
                ));
            }
            pending.resolving = true;
            pending.clone()
        };

        match self.config.mode {
            HostMode::Test => {
                let mut state = self.state()?;
                state.pending_box_handoffs.remove(&agent_id);
                state.events.push_back(HostEvent::BoxHandoffResolved {
                    timestamp: timestamp(),
                    request_id: handoff_request_id,
                    agent_id,
                    resolution,
                    resume_operation_id: String::new(),
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let prompt = match resolution.as_str() {
                        "dismissed" => {
                            "[The user declined your request for help. Do not assume the requested step happened and do not immediately request the same help again. Continue another way if possible; if blocked, briefly tell the user what is blocked and wait.]"
                        }
                        "viewer-closed" => {
                            "[The user closed the handoff without explicitly confirming completion. Re-check the current state using read-only tools before acting. If you cannot tell whether the requested step finished, ask the user briefly instead of assuming.]"
                        }
                        _ => {
                            "[The user completed the requested handoff step and returned control. Re-check the current state with read-only tools first, then continue the task from the verified state.]"
                        }
                    };
                    let runtime = match self.runtime() {
                        Ok(runtime) => runtime,
                        Err(error) => {
                            if let Ok(mut state) = self.state() {
                                if let Some(live) =
                                    state.pending_box_handoffs.get_mut(&pending.agent_id)
                                {
                                    if live.request_id == pending.request_id {
                                        live.resolving = false;
                                    }
                                }
                            }
                            return Err(error);
                        }
                    };
                    let response = match runtime.execute(RuntimeCommand::SendMessage {
                        conversation_id: ConversationId(pending.conversation_id.clone()),
                        text: prompt.into(),
                        display_text: None,
                        client_message_id: Some(format!(
                            "box-handoff-resume:{}",
                            pending.request_id
                        )),
                        hidden: true,
                        show_assistant_output: false,
                        recovery_eligible: false,
                        reply_to_message_id: None,
                        is_fork: false,
                        attachment_batch_id: None,
                        selected_image_data_urls: Vec::new(),
                    }) {
                        Ok(response) => response,
                        Err(error) => {
                            if let Ok(mut state) = self.state() {
                                if let Some(live) =
                                    state.pending_box_handoffs.get_mut(&pending.agent_id)
                                {
                                    if live.request_id == pending.request_id {
                                        live.resolving = false;
                                    }
                                }
                            }
                            return Err(error.into());
                        }
                    };
                    let resume_operation_id = match response {
                        RuntimeResponse::Accepted { operation_id } => operation_id.to_string(),
                        other => {
                            if let Ok(mut state) = self.state() {
                                if let Some(live) =
                                    state.pending_box_handoffs.get_mut(&pending.agent_id)
                                {
                                    if live.request_id == pending.request_id {
                                        live.resolving = false;
                                    }
                                }
                            }
                            return Err(unexpected_response("box.handoff.resolve", other));
                        }
                    };
                    let mut state = self.state()?;
                    let still_current = !state.closed
                        && state
                            .pending_box_handoffs
                            .get(&pending.agent_id)
                            .is_some_and(|live| {
                                live.request_id == pending.request_id && live.resolving
                            });
                    if !still_current {
                        drop(state);
                        let _ = self
                            .runtime()?
                            .interrupt(OperationId(resume_operation_id.clone()));
                        return Err(FeatureHostError::Contract(
                            "box handoff changed while resume was being prepared".into(),
                        ));
                    }
                    state.pending_box_handoffs.remove(&pending.agent_id);
                    state.operations.insert(resume_operation_id.clone());
                    state
                        .operation_agents
                        .insert(resume_operation_id.clone(), pending.agent_id.clone());
                    state.events.push_back(HostEvent::BoxHandoffResolved {
                        timestamp: timestamp(),
                        request_id: pending.request_id,
                        agent_id: pending.agent_id,
                        resolution,
                        resume_operation_id: resume_operation_id.clone(),
                    });
                    state.events.push_back(HostEvent::OperationStarted {
                        timestamp: timestamp(),
                        operation_id: resume_operation_id.clone(),
                        label: "box-handoff-resume".into(),
                        interruptible: true,
                    });
                    Ok(CommandAccepted {
                        request_id,
                        operation_id: Some(resume_operation_id),
                    })
                }
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    fn execute_messaging(
        &self,
        request_id: String,
        envelope: Value,
    ) -> Result<CommandAccepted, FeatureHostError> {
        self.execute_messaging_sync(request_id.clone(), envelope)?;
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    /// Executes one messaging envelope against the exact same persistent
    /// `MessagingService` used by desktop and also returns the resulting server
    /// envelopes to native shells. The envelopes are still projected onto the
    /// regular FeatureHost event queue, so desktop/event-driven consumers retain
    /// their existing behavior while iOS and Android can update synchronously.
    pub fn execute_messaging_sync(
        &self,
        request_id: String,
        envelope: Value,
    ) -> Result<Vec<Value>, FeatureHostError> {
        static MESSAGING_IO_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _io_guard = MESSAGING_IO_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| FeatureHostError::Contract("messaging storage lock is poisoned".into()))?;
        let client_envelope: MessagingClientEnvelope =
            serde_json::from_value(envelope).map_err(|error| {
                FeatureHostError::Contract(format!("invalid messaging envelope: {error}"))
            })?;
        let root = self.messaging_root_for(&client_envelope)?;
        let messaging_root = root.join("_messaging");
        let store = JsonFileStateStore::new(messaging_root.join("snapshot.json"));
        let mut service = MessagingService::load_with_blob_store(
            store,
            FileBlobStore::new(messaging_root.join("blobs")),
        )
        .map_err(|error| FeatureHostError::Contract(error.to_string()))?;
        let responses = service
            .handle(client_envelope, now_millis())
            .map_err(|error| FeatureHostError::Contract(error.to_string()))?;
        let envelopes = responses
            .into_iter()
            .map(|response| {
                serde_json::to_value(response)
                    .map_err(|error| FeatureHostError::Contract(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut state = self.state()?;
        for envelope in &envelopes {
            state.events.push_back(HostEvent::MessagingEvent {
                timestamp: timestamp(),
                request_id: request_id.clone(),
                envelope: envelope.clone(),
            });
        }
        Ok(envelopes)
    }

    pub fn read_messaging_blob_range(
        &self,
        blob_id: &str,
        offset: u64,
        length: u64,
    ) -> Result<(fabushi_messaging_core::BlobMetadata, Vec<u8>), FeatureHostError> {
        let root = self
            .active_account_root(self.memory_root_path.as_deref())
            .ok_or_else(|| FeatureHostError::Contract("messaging storage is unavailable".into()))?;
        let blob_id = BlobId::new(blob_id.to_string())
            .map_err(|error| FeatureHostError::Contract(error.to_string()))?;
        let store = FileBlobStore::new(root.join("_messaging").join("blobs"));
        let metadata = store
            .metadata(&blob_id)
            .map_err(|error| FeatureHostError::Contract(error.to_string()))?;
        let bytes = store
            .read_range(&blob_id, offset, length.min(1024 * 1024))
            .map_err(|error| FeatureHostError::Contract(error.to_string()))?;
        Ok((metadata, bytes))
    }

    fn execute_automation(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        match command {
            FeatureCommand::AutomationList { agent_id, .. } => {
                let mut automations = self
                    .state()?
                    .automations
                    .values()
                    .filter(|automation| {
                        agent_id.as_ref().is_none_or(|agent_id| {
                            automation.agent_id.as_deref() == Some(agent_id.as_str())
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                automations.sort_by_key(|item| item.created_at_ms);
                self.state()?.events.push_back(HostEvent::AutomationListed {
                    timestamp: timestamp(),
                    automations,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::AutomationUpsert {
                id,
                agent_id,
                name,
                prompt,
                schedule,
                trigger,
                enabled,
                ..
            } => {
                let name = required(name, "automation name")?;
                let prompt = required(prompt, "automation prompt")?;
                let trigger = trigger.unwrap_or_else(|| AutomationTrigger::Schedule {
                    schedule: schedule.clone(),
                });
                let trigger = normalize_automation_trigger(trigger)?;
                let schedule = automation_trigger_legacy_schedule(&trigger);
                let now = now_millis();
                let mut state = self.state()?;
                let id = id
                    .filter(|id| is_safe_automation_id(id))
                    .unwrap_or_else(|| {
                        state.sequence += 1;
                        format!("routine-{}-{}", now, state.sequence)
                    });
                let previous = state.automations.get(&id).cloned();
                let requested_agent_id = match agent_id {
                    Some(agent_id) => Some(required(agent_id, "automation agent id")?),
                    None => None,
                };
                if let Some(agent_id) = requested_agent_id.as_deref() {
                    if !state.bots.contains_key(agent_id) {
                        return Err(FeatureHostError::Contract(format!(
                            "unknown automation agent: {agent_id}"
                        )));
                    }
                }
                if let (Some(previous), Some(agent_id)) =
                    (previous.as_ref(), requested_agent_id.as_deref())
                {
                    ensure_automation_agent_scope(previous, Some(agent_id))?;
                }
                let resolved_agent_id = requested_agent_id
                    .or_else(|| previous.as_ref().and_then(|item| item.agent_id.clone()));
                let action = if previous.is_some() {
                    "updated"
                } else {
                    "created"
                };
                let automation = AutomationSummary {
                    id: id.clone(),
                    agent_id: resolved_agent_id,
                    name,
                    prompt,
                    schedule: schedule.clone(),
                    trigger: Some(trigger.clone()),
                    enabled,
                    created_at_ms: previous.as_ref().map_or(now, |item| item.created_at_ms),
                    runs: previous.as_ref().map_or_else(Vec::new, |item| item.runs.clone()),
                    last_run_at_ms: previous.as_ref().and_then(|item| item.last_run_at_ms),
                    next_run_at_ms: automation_next_run(&trigger, &schedule, enabled, now),
                };
                state.automations.insert(id, automation.clone());
                self.persist_automations(&state.automations)?;
                state.events.push_back(HostEvent::AutomationChanged {
                    timestamp: timestamp(),
                    action: action.into(),
                    automation: automation.clone(),
                });
                drop(state);
                self.arm_listener_resume_after_automation_write(&automation)?;
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::AutomationSetEnabled {
                id,
                agent_id,
                enabled,
                ..
            } => {
                let mut state = self.state()?;
                let automation = state.automations.get_mut(&id).ok_or_else(|| {
                    FeatureHostError::Contract(format!("unknown automation: {id}"))
                })?;
                ensure_automation_agent_scope(automation, agent_id.as_deref())?;
                automation.enabled = enabled;
                let trigger =
                    automation
                        .trigger
                        .clone()
                        .unwrap_or_else(|| AutomationTrigger::Schedule {
                            schedule: automation.schedule.clone(),
                        });
                automation.next_run_at_ms =
                    automation_next_run(&trigger, &automation.schedule, enabled, now_millis());
                let automation = automation.clone();
                self.persist_automations(&state.automations)?;
                if !enabled {
                    prune_pending_listener_resumes(&mut state);
                }
                state.events.push_back(HostEvent::AutomationChanged {
                    timestamp: timestamp(),
                    action: if enabled { "resumed" } else { "paused" }.into(),
                    automation: automation.clone(),
                });
                drop(state);
                if enabled {
                    self.arm_listener_resume_after_automation_write(&automation)?;
                }
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::AutomationDelete { id, agent_id, .. } => {
                let mut state = self.state()?;
                let existing = state.automations.get(&id).ok_or_else(|| {
                    FeatureHostError::Contract(format!("unknown automation: {id}"))
                })?;
                ensure_automation_agent_scope(existing, agent_id.as_deref())?;
                let automation = state
                    .automations
                    .remove(&id)
                    .expect("automation checked above");
                self.persist_automations(&state.automations)?;
                prune_pending_listener_resumes(&mut state);
                state.events.push_back(HostEvent::AutomationChanged {
                    timestamp: timestamp(),
                    action: "deleted".into(),
                    automation,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::AutomationRun { id, agent_id, .. } => {
                self.execute_routine(request_id, id, agent_id, RoutineTrigger::Manual)
            }
            _ => unreachable!("non-automation command routed to automation executor"),
        }
    }

    fn execute_bot_profile(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let mut state = self.state()?;
        ensure_open(&state)?;
        let (action, bot) = match command {
            FeatureCommand::BotCreate {
                name,
                description,
                title,
                avatar,
                avatar_shape,
                avatar_color,
                ..
            } => {
                let name = clamp_line(&name, 72);
                if name.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "bot name must not be empty".into(),
                    ));
                }
                let id = next_id(&mut state, "agent");
                let bot = BotSummary {
                    id: id.clone(),
                    name,
                    description: clamp_block(&description, 2000),
                    title: title.trim().to_string(),
                    hidden: false,
                    avatar: sanitize_avatar_data_url(avatar)?,
                    avatar_shape: clean_optional_string(avatar_shape),
                    avatar_color: clean_optional_string(avatar_color),
                    notifications_enabled: true,
                    notify_on_updates: true,
                    unread: false,
                    conversation_id: Some(format!("codex:agent:{id}")),
                };
                state.bots.insert(id, bot.clone());
                ("created", bot)
            }
            FeatureCommand::BotUpdate {
                id,
                name,
                description,
                title,
                avatar,
                avatar_shape,
                avatar_color,
                notifications_enabled,
                notify_on_updates,
                unread,
                ..
            } => {
                let bot = state
                    .bots
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown bot: {id}")))?;
                if let Some(name) = name {
                    let name = clamp_line(&name, 72);
                    if name.is_empty() {
                        return Err(FeatureHostError::Contract(
                            "bot name must not be empty".into(),
                        ));
                    }
                    bot.name = name;
                }
                if let Some(description) = description {
                    bot.description = clamp_block(&description, 2000);
                }
                if let Some(title) = title {
                    bot.title = title.trim().to_string();
                }
                if avatar.is_some() {
                    bot.avatar = sanitize_avatar_data_url(avatar)?;
                }
                if avatar_shape.is_some() {
                    bot.avatar_shape = clean_optional_string(avatar_shape);
                }
                if avatar_color.is_some() {
                    bot.avatar_color = clean_optional_string(avatar_color);
                }
                if let Some(enabled) = notifications_enabled {
                    bot.notifications_enabled = enabled;
                }
                if let Some(enabled) = notify_on_updates {
                    bot.notify_on_updates = enabled;
                }
                if let Some(unread) = unread {
                    bot.unread = unread;
                }
                ("updated", bot.clone())
            }
            FeatureCommand::BotClone { id, .. } => {
                let source = state
                    .bots
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown bot: {id}")))?;
                let new_id = next_id(&mut state, "agent");
                let clone_name = clone_agent_display_name(&source.name);
                let bot = BotSummary {
                    id: new_id.clone(),
                    name: clone_name,
                    description: source.description,
                    title: source.title,
                    hidden: false,
                    avatar: source.avatar,
                    avatar_shape: source.avatar_shape,
                    avatar_color: source.avatar_color,
                    notifications_enabled: source.notifications_enabled,
                    notify_on_updates: source.notify_on_updates,
                    unread: false,
                    conversation_id: Some(format!("codex:agent:{new_id}")),
                };
                state.bots.insert(new_id, bot.clone());
                ("cloned", bot)
            }
            FeatureCommand::BotDelete { id, .. } => {
                if id == "mahayana-assistant" {
                    return Err(FeatureHostError::Contract(
                        "the primary Mahayana assistant cannot be deleted".into(),
                    ));
                }
                let bot = state
                    .bots
                    .remove(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown bot: {id}")))?;
                if let Some(conversation_id) = bot.conversation_id.as_deref() {
                    state.conversation_session.mark_deleted(conversation_id);
                }
                state
                    .pending_listener_resumes
                    .retain(|(agent_id, _)| agent_id != &id);
                ("deleted", bot)
            }
            FeatureCommand::BotSetHidden { id, hidden, .. } => {
                let bot = state
                    .bots
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown bot: {id}")))?;
                bot.hidden = hidden;
                ("updated", bot.clone())
            }
            _ => unreachable!("non-bot-profile command routed to bot executor"),
        };
        self.persist_bots(&state.bots)?;
        state.events.push_back(HostEvent::BotChanged {
            timestamp: timestamp(),
            action: action.into(),
            bot,
        });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_group_chat(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let mut state = self.state()?;
        ensure_open(&state)?;
        let mut kick_group_id: Option<String> = None;
        match command {
            FeatureCommand::GroupList { .. } => {
                let mut groups = state.groups.values().cloned().collect::<Vec<_>>();
                groups.sort_by_key(|group| group.created_at_ms);
                state.events.push_back(HostEvent::GroupListed {
                    timestamp: timestamp(),
                    groups,
                });
            }
            FeatureCommand::GroupCreate {
                name,
                description,
                avatar,
                avatar_shape,
                avatar_color,
                member_ids,
                ..
            } => {
                let name = clamp_line(&name, 72);
                if name.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "group name must not be empty".into(),
                    ));
                }
                let member_ids = validate_group_members(&state, member_ids)?;
                let now = now_millis();
                let id = next_id(&mut state, "group");
                let group = GroupSummary {
                    id: id.clone(),
                    name,
                    description: clamp_block(&description, 2000),
                    avatar: sanitize_avatar_data_url(avatar)?,
                    avatar_shape: clean_optional_string(avatar_shape),
                    avatar_color: clean_optional_string(avatar_color),
                    member_ids,
                    messages: Vec::new(),
                    created_at_ms: now,
                    updated_at_ms: now,
                };
                state.groups.insert(id, group.clone());
                self.persist_groups(&state.groups)?;
                state.events.push_back(HostEvent::GroupChanged {
                    timestamp: timestamp(),
                    action: "created".into(),
                    group,
                });
            }
            FeatureCommand::GroupUpdate {
                id,
                name,
                description,
                avatar,
                avatar_shape,
                avatar_color,
                member_ids,
                ..
            } => {
                let validated_members = member_ids
                    .map(|member_ids| validate_group_members(&state, member_ids))
                    .transpose()?;
                let group = state
                    .groups
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown group: {id}")))?;
                if let Some(name) = name {
                    let name = clamp_line(&name, 72);
                    if name.is_empty() {
                        return Err(FeatureHostError::Contract(
                            "group name must not be empty".into(),
                        ));
                    }
                    group.name = name;
                }
                if let Some(description) = description {
                    group.description = clamp_block(&description, 2000);
                }
                if avatar.is_some() {
                    group.avatar = sanitize_avatar_data_url(avatar)?;
                }
                if avatar_shape.is_some() {
                    group.avatar_shape = clean_optional_string(avatar_shape);
                }
                if avatar_color.is_some() {
                    group.avatar_color = clean_optional_string(avatar_color);
                }
                if let Some(member_ids) = validated_members {
                    group.member_ids = member_ids;
                }
                group.updated_at_ms = now_millis();
                let group = group.clone();
                self.persist_groups(&state.groups)?;
                state.events.push_back(HostEvent::GroupChanged {
                    timestamp: timestamp(),
                    action: "updated".into(),
                    group,
                });
            }
            FeatureCommand::GroupDelete { id, .. } => {
                let group = state
                    .groups
                    .remove(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown group: {id}")))?;
                self.persist_groups(&state.groups)?;
                state.events.push_back(HostEvent::GroupChanged {
                    timestamp: timestamp(),
                    action: "deleted".into(),
                    group,
                });
            }
            FeatureCommand::GroupSend { id, text, .. } => {
                let text = clamp_block(&text, 8000);
                if text.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "group message must not be empty".into(),
                    ));
                }
                let message_id = next_id(&mut state, "group-message");
                let now = now_millis();
                let group = state
                    .groups
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown group: {id}")))?;
                group.messages.push(GroupMessage {
                    id: message_id,
                    speaker: GroupSpeaker::User { name: None },
                    content: text,
                    created_at_ms: now,
                });
                if group.messages.len() > 500 {
                    let overflow = group.messages.len() - 500;
                    group.messages.drain(0..overflow);
                }
                group.updated_at_ms = now;
                let group = group.clone();
                let responder_ids = resolve_group_responders(&group, &state.bots);
                if !responder_ids.is_empty() {
                    let run_id = next_id(&mut state, "group-run");
                    state.group_runs.insert(
                        id.clone(),
                        GroupRunState {
                            run_id,
                            round: 0,
                            speaker_order: order_round_speakers(&responder_ids, 0),
                            speaker_index: 0,
                            total_messages: 0,
                            messages_this_round: 0,
                        },
                    );
                    kick_group_id = Some(id.clone());
                }
                self.persist_groups(&state.groups)?;
                state.events.push_back(HostEvent::GroupChanged {
                    timestamp: timestamp(),
                    action: "message".into(),
                    group,
                });
            }
            _ => unreachable!("non-group command routed to group executor"),
        }
        drop(state);
        let operation_id = match (self.config.mode, kick_group_id) {
            (HostMode::Production, Some(group_id)) => {
                #[cfg(feature = "production")]
                {
                    self.start_next_group_turn(&group_id)?
                }
                #[cfg(not(feature = "production"))]
                {
                    let _ = group_id;
                    None
                }
            }
            _ => None,
        };
        Ok(CommandAccepted {
            request_id,
            operation_id,
        })
    }

    fn execute_teach(&self, command: FeatureCommand) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        self.refresh_finished_teach_recording()?;
        match command {
            FeatureCommand::TeachStatus { .. } => {
                let status = {
                    let guard = self
                        .teach_recording
                        .lock()
                        .map_err(|_| FeatureHostError::StatePoisoned)?;
                    teach_recording_status(guard.as_ref())
                };
                self.state()?.events.push_back(HostEvent::TeachChanged {
                    timestamp: timestamp(),
                    status,
                    result: None,
                });
            }
            FeatureCommand::TeachStart {
                agent_id,
                entry_point,
                ..
            } => {
                if !is_safe_memory_agent_id(&agent_id)
                    || !self.state()?.bots.contains_key(&agent_id)
                {
                    return Err(FeatureHostError::Contract(format!(
                        "unknown teach agent: {agent_id}"
                    )));
                }
                let mut guard = self
                    .teach_recording
                    .lock()
                    .map_err(|_| FeatureHostError::StatePoisoned)?;
                if let Some(active) = guard.as_ref() {
                    if active.agent_id == agent_id {
                        let status = teach_recording_status(Some(active));
                        drop(guard);
                        self.state()?.events.push_back(HostEvent::TeachChanged {
                            timestamp: timestamp(),
                            status,
                            result: None,
                        });
                        return Ok(CommandAccepted {
                            request_id,
                            operation_id: None,
                        });
                    }
                    return Err(FeatureHostError::Contract(format!(
                        "teach recording is already active for {}",
                        active.agent_id
                    )));
                }

                let root = self
                    .active_account_root(self.memory_root_path.as_deref())
                    .ok_or_else(|| {
                        FeatureHostError::Contract("teach recording storage is unavailable".into())
                    })?;
                let started_at_ms = now_millis();
                let session_dir = root
                    .join(&agent_id)
                    .join("teach-sessions")
                    .join(format!("{started_at_ms}"));
                std::fs::create_dir_all(&session_dir).map_err(|error| {
                    FeatureHostError::Contract(format!("create teach session: {error}"))
                })?;
                let video_path = session_dir.join("demo.mp4");
                let child = if self.config.mode == HostMode::Production {
                    #[cfg(target_os = "ios")]
                    {
                        // iOS owns capture through the shipping WKWebView. The
                        // Host still owns session identity, storage and learning.
                        None
                    }
                    #[cfg(not(target_os = "ios"))]
                    {
                        Some(spawn_teach_capture(&video_path)?)
                    }
                } else {
                    None
                };
                *guard = Some(TeachCaptureProcess {
                    agent_id: agent_id.clone(),
                    entry_point,
                    started_at_ms,
                    session_dir,
                    video_path,
                    child,
                });
                let status = teach_recording_status(guard.as_ref());
                drop(guard);
                self.state()?.events.push_back(HostEvent::TeachChanged {
                    timestamp: timestamp(),
                    status,
                    result: None,
                });
            }
            FeatureCommand::TeachStop { agent_id, save, .. } => {
                let active = {
                    let mut guard = self
                        .teach_recording
                        .lock()
                        .map_err(|_| FeatureHostError::StatePoisoned)?;
                    let Some(active) = guard.as_ref() else {
                        let status = TeachRecordingStatus::default();
                        drop(guard);
                        self.state()?.events.push_back(HostEvent::TeachChanged {
                            timestamp: timestamp(),
                            status,
                            result: None,
                        });
                        return Ok(CommandAccepted {
                            request_id,
                            operation_id: None,
                        });
                    };
                    if active.agent_id != agent_id {
                        return Err(FeatureHostError::Contract(format!(
                            "teach recording belongs to {}, not {agent_id}",
                            active.agent_id
                        )));
                    }
                    guard.take().expect("teach recording existed")
                };
                let result = self.finalize_teach_capture(active, save)?;
                self.state()?.events.push_back(HostEvent::TeachChanged {
                    timestamp: timestamp(),
                    status: TeachRecordingStatus::default(),
                    result: Some(result),
                });
            }
            _ => unreachable!("non-teach command routed to teach executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn refresh_finished_teach_recording(&self) -> Result<(), FeatureHostError> {
        let finished = {
            let mut guard = self
                .teach_recording
                .lock()
                .map_err(|_| FeatureHostError::StatePoisoned)?;
            let Some(active) = guard.as_mut() else {
                return Ok(());
            };
            let process_finished = match active.child.as_mut() {
                Some(child) => child
                    .try_wait()
                    .map_err(|error| {
                        FeatureHostError::Contract(format!("poll teach capture: {error}"))
                    })?
                    .is_some(),
                None => now_millis() - active.started_at_ms >= TEACH_MAX_DURATION_MS,
            };
            process_finished.then(|| guard.take().expect("teach recording existed"))
        };
        if let Some(active) = finished {
            let result = self.finalize_teach_capture(active, true)?;
            self.state()?.events.push_back(HostEvent::TeachChanged {
                timestamp: timestamp(),
                status: TeachRecordingStatus::default(),
                result: Some(result),
            });
        }
        Ok(())
    }

    fn finalize_teach_capture(
        &self,
        mut active: TeachCaptureProcess,
        save: bool,
    ) -> Result<TeachRecordingResult, FeatureHostError> {
        if let Some(child) = active.child.as_mut() {
            stop_teach_capture(child)?;
        }
        let ended_at_ms = now_millis();
        let duration_ms = (ended_at_ms - active.started_at_ms).clamp(0, TEACH_MAX_DURATION_MS);
        if !save {
            let _ = std::fs::remove_dir_all(&active.session_dir);
            return Ok(TeachRecordingResult {
                agent_id: active.agent_id,
                video_path: active.video_path.to_string_lossy().to_string(),
                started_at_ms: active.started_at_ms,
                ended_at_ms,
                duration_ms,
                saved: false,
            });
        }

        if self.config.mode == HostMode::Test && !active.video_path.exists() {
            std::fs::write(&active.video_path, b"fabushi-test-teach-video").map_err(|error| {
                FeatureHostError::Contract(format!("write teach test fixture: {error}"))
            })?;
        }
        let metadata = std::fs::metadata(&active.video_path).map_err(|error| {
            FeatureHostError::Contract(format!("teach capture did not produce a video: {error}"))
        })?;
        if metadata.len() == 0 {
            return Err(FeatureHostError::Contract(
                "teach capture produced an empty video".into(),
            ));
        }

        let manifest = json!({
            "agentId": active.agent_id,
            "entryPoint": active.entry_point,
            "startedAtMs": active.started_at_ms,
            "endedAtMs": ended_at_ms,
            "durationMs": duration_ms,
            "videoPath": active.video_path,
        });
        std::fs::write(
            active.session_dir.join("session.json"),
            serde_json::to_vec_pretty(&manifest).map_err(|error| {
                FeatureHostError::Contract(format!("serialize teach manifest: {error}"))
            })?,
        )
        .map_err(|error| FeatureHostError::Contract(format!("write teach manifest: {error}")))?;

        if self.config.mode == HostMode::Production {
            #[cfg(not(target_os = "ios"))]
            {
                let _ = extract_teach_frames(&active.video_path, &active.session_dir.join("frames"));
            }
            // On iOS the native WKWebView capture owner writes both demo.mp4
            // and sampled JPEG frames into this Host-owned session directory.
        }
        let video_path = active.video_path.to_string_lossy().to_string();
        let _ = self.schedule_teach_learning(&active.agent_id, &video_path, &active.session_dir);
        Ok(TeachRecordingResult {
            agent_id: active.agent_id,
            video_path,
            started_at_ms: active.started_at_ms,
            ended_at_ms,
            duration_ms,
            saved: true,
        })
    }

    fn schedule_teach_learning(
        &self,
        agent_id: &str,
        video_path: &str,
        session_dir: &Path,
    ) -> Result<Option<String>, FeatureHostError> {
        let bot = self.state()?.bots.get(agent_id).cloned().ok_or_else(|| {
            FeatureHostError::Contract(format!("unknown teach agent: {agent_id}"))
        })?;
        let frames_dir = session_dir.join("frames").to_string_lossy().to_string();
        let prompt = format!(
            "[teach-recording] The user just demonstrated a repeatable task for you.\nRecording: {video_path}\nExtracted frames (when present): {frames_dir}\n\nStudy the demonstration carefully. Infer the intent, ordered steps, important UI landmarks, decision points, and safety checks. Return a reusable Markdown workflow/skill only: start with a concise # heading, then instructions another future run can follow. Do not merely summarize the recording and do not mention this hidden teach prompt."
        );
        if self.config.mode == HostMode::Test {
            return Ok(None);
        }
        #[cfg(feature = "production")]
        {
            self.schedule_recoverable_background_turn(
                &bot,
                "teach-recording",
                prompt,
                format!("teach:{}:{}", bot.id, now_millis()),
                Vec::new(),
                Some(video_path.to_string()),
            )
        }
        #[cfg(not(feature = "production"))]
        Ok(None)
    }

    fn persist_teach_workflow(
        &self,
        agent_id: &str,
        artifact: &str,
        markdown: &str,
    ) -> Result<WorkflowSummary, FeatureHostError> {
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let body = clamp_block(markdown, 100_000);
        if body.is_empty() {
            return Err(FeatureHostError::Contract(
                "teach learning returned an empty workflow".into(),
            ));
        }
        let name = derive_teach_workflow_name(&body);
        let base_id = slugify_teach_workflow_name(&name);
        let mut id = base_id.clone();
        let mut suffix = 2usize;
        while workflow_root.join(&id).exists() {
            id = format!("{base_id}-{suffix}");
            suffix += 1;
            if suffix > 999 {
                id = format!("{base_id}-{}", now_millis());
                break;
            }
        }
        let folder = workflow_root.join(&id);
        std::fs::create_dir_all(&folder).map_err(|error| {
            FeatureHostError::Contract(format!("create learned workflow: {error}"))
        })?;
        let description = "Learned from a recorded demonstration.".to_string();
        let metadata = json!({
            "name": name,
            "description": description,
            "metadata": { "source": artifact },
        });
        let yaml = serde_yaml::to_string(&metadata).map_err(|error| {
            FeatureHostError::Contract(format!("serialize learned workflow frontmatter: {error}"))
        })?;
        let file_path = folder.join("SKILL.md");
        std::fs::write(&file_path, format!("---\n{yaml}---\n{}\n", body.trim())).map_err(
            |error| FeatureHostError::Contract(format!("write learned workflow: {error}")),
        )?;
        let created_at = now_millis();
        let workflow = WorkflowSummary {
            id,
            name,
            description,
            body,
            trigger: None,
            source_ref: Some(artifact.to_string()),
            source: WorkflowSource::Workflow,
            plugin_id: None,
            published_by_current_user: false,
            is_enabled_for_agent: true,
            disable_model_invocation: None,
            schedule_description: None,
            created_at,
            last_run_at: None,
            next_run_at: None,
            helper_scripts: Vec::new(),
            file_path: file_path.to_string_lossy().to_string(),
        };
        if let Some(agent_root) = self.active_account_root(self.memory_root_path.as_deref()) {
            let _ = set_workflow_enabled(&agent_root, agent_id, &workflow.id, true);
        }
        Ok(workflow)
    }

    fn execute_subagent_observation(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        match command {
            FeatureCommand::SubagentList { agent_id, .. } => {
                let state = self.state()?;
                ensure_open(&state)?;
                let mut subagents = state
                    .subagents
                    .values()
                    .filter(|subagent| subagent.parent_agent_id == agent_id)
                    .cloned()
                    .collect::<Vec<_>>();
                subagents.sort_by_key(|subagent| subagent.started_at_ms);
                drop(state);
                self.state()?.events.push_back(HostEvent::SubagentListed {
                    timestamp: timestamp(),
                    agent_id,
                    subagents,
                });
            }
            FeatureCommand::AsyncTaskList { agent_id, .. } => {
                let state = self.state()?;
                ensure_open(&state)?;
                let mut tasks = state
                    .async_tasks
                    .values()
                    .filter(|task| task.parent_agent_id == agent_id)
                    .cloned()
                    .collect::<Vec<_>>();
                tasks.sort_by_key(|task| task.started_at_ms);
                drop(state);
                self.state()?.events.push_back(HostEvent::AsyncTaskListed {
                    timestamp: timestamp(),
                    agent_id,
                    tasks,
                });
            }
            _ => unreachable!("non-subagent command routed to subagent observer"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_agent_messaging(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        match command {
            FeatureCommand::AgentPeerHistory {
                agent_id, limit, ..
            } => {
                let state = self.state()?;
                ensure_open(&state)?;
                if !state.bots.contains_key(&agent_id) {
                    return Err(FeatureHostError::Contract(format!(
                        "unknown bot: {agent_id}"
                    )));
                }
                let mut messages = state
                    .peer_messages
                    .iter()
                    .filter(|message| {
                        message.from_agent_id == agent_id || message.target_id == agent_id
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                messages.sort_by_key(|message| message.created_at_ms);
                if messages.len() > limit.min(1000) {
                    let start = messages.len() - limit.min(1000);
                    messages = messages.split_off(start);
                }
                drop(state);
                self.state()?
                    .events
                    .push_back(HostEvent::AgentPeerHistoryListed {
                        timestamp: timestamp(),
                        agent_id,
                        messages,
                    });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::AgentSend {
                from_agent_id,
                target_id,
                text,
                images,
                priority,
                ..
            } => {
                let text = clamp_block(&text, 8000);
                if text.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "agent message must not be empty".into(),
                    ));
                }
                if from_agent_id == target_id {
                    return Err(FeatureHostError::Contract(
                        "an agent cannot message itself".into(),
                    ));
                }

                let mut kick_group: Option<String> = None;
                let direct_target = {
                    let mut state = self.state()?;
                    ensure_open(&state)?;
                    let sender = state.bots.get(&from_agent_id).cloned().ok_or_else(|| {
                        FeatureHostError::Contract(format!("unknown sender bot: {from_agent_id}"))
                    })?;
                    if let Some(target) = state.bots.get(&target_id).cloned() {
                        let peer = AgentPeerMessage {
                            id: next_id(&mut state, "agent-message"),
                            from_agent_id: sender.id.clone(),
                            from_agent_name: sender.name.clone(),
                            target_id: target.id.clone(),
                            target_name: target.name.clone(),
                            text: text.clone(),
                            images: images.clone(),
                            priority,
                            created_at_ms: now_millis(),
                        };
                        state.peer_messages.push(peer.clone());
                        if state.peer_messages.len() > 5000 {
                            let overflow = state.peer_messages.len() - 5000;
                            state.peer_messages.drain(0..overflow);
                        }
                        self.persist_peer_messages(&state.peer_messages)?;
                        state.events.push_back(HostEvent::AgentPeerMessageChanged {
                            timestamp: timestamp(),
                            message: peer,
                        });
                        Some((sender, target))
                    } else if let Some(group_snapshot) = state.groups.get(&target_id).cloned() {
                        if !group_snapshot
                            .member_ids
                            .iter()
                            .any(|id| id == &from_agent_id)
                        {
                            return Err(FeatureHostError::Contract(format!(
                                "agent {from_agent_id} is not a member of group {target_id}"
                            )));
                        }
                        let now = now_millis();
                        let message_id = next_id(&mut state, "group-message");
                        let group = state
                            .groups
                            .get_mut(&target_id)
                            .expect("group snapshot existed");
                        group.messages.push(GroupMessage {
                            id: message_id,
                            speaker: GroupSpeaker::Member {
                                id: sender.id.clone(),
                                name: sender.name.clone(),
                            },
                            content: text.clone(),
                            created_at_ms: now,
                        });
                        if group.messages.len() > 500 {
                            let overflow = group.messages.len() - 500;
                            group.messages.drain(0..overflow);
                        }
                        group.updated_at_ms = now;
                        let group = group.clone();
                        let mut responders = group
                            .member_ids
                            .iter()
                            .filter(|id| *id != &from_agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        let lower = text.to_lowercase();
                        let mentioned = responders
                            .iter()
                            .filter(|id| {
                                state.bots.get(*id).is_some_and(|bot| {
                                    group_member_handles(&bot.name)
                                        .iter()
                                        .any(|handle| has_group_mention_at(&lower, handle))
                                })
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        if !mentioned.is_empty() && !has_everyone_group_mention(&lower) {
                            responders = mentioned;
                        }
                        if !responders.is_empty() {
                            let run_id = next_id(&mut state, "group-run");
                            state.group_runs.insert(
                                target_id.clone(),
                                GroupRunState {
                                    run_id,
                                    round: 0,
                                    speaker_order: responders,
                                    speaker_index: 0,
                                    total_messages: 0,
                                    messages_this_round: 0,
                                },
                            );
                            kick_group = Some(target_id.clone());
                        }
                        self.persist_groups(&state.groups)?;
                        state.events.push_back(HostEvent::GroupChanged {
                            timestamp: timestamp(),
                            action: "message".into(),
                            group,
                        });
                        None
                    } else {
                        return Err(FeatureHostError::Contract(format!(
                            "unknown agent or group: {target_id}"
                        )));
                    }
                };

                if let Some(group_id) = kick_group {
                    if self.config.mode == HostMode::Production {
                        #[cfg(feature = "production")]
                        {
                            let _ = self.start_next_group_turn(&group_id)?;
                        }
                    }
                    return Ok(CommandAccepted {
                        request_id,
                        operation_id: None,
                    });
                }

                if let Some((sender, target)) = direct_target {
                    let wake_prompt = build_agent_inbound_wake_prompt(&sender, &text, priority);
                    let selected_image_data_urls = load_agent_inbound_image_data_urls(&images);
                    self.schedule_background_agent_turn(
                        &target,
                        if priority {
                            "agent-priority"
                        } else {
                            "agent-message"
                        },
                        wake_prompt,
                        format!("peer:{}:{}", sender.id, request_id),
                        selected_image_data_urls,
                    )?;
                }
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::AgentBroadcast {
                target_ids,
                message,
                ..
            } => {
                let message = clamp_block(&message, 8000);
                if message.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "broadcast message must not be empty".into(),
                    ));
                }
                let targets = {
                    let state = self.state()?;
                    ensure_open(&state)?;
                    match target_ids {
                        Some(ids) => {
                            let unique = ids.into_iter().collect::<BTreeSet<_>>();
                            unique
                                .into_iter()
                                .filter_map(|id| state.bots.get(&id).cloned())
                                .collect::<Vec<_>>()
                        }
                        None => state.bots.values().cloned().collect::<Vec<_>>(),
                    }
                };
                let total = targets.len();
                let mut scheduled = 0usize;
                for target in targets {
                    let prompt = build_admin_broadcast_wake_prompt(&message);
                    if self
                        .schedule_background_agent_turn(
                            &target,
                            "broadcast",
                            prompt,
                            format!("broadcast:{}:{}", target.id, request_id),
                            Vec::new(),
                        )
                        .is_ok()
                    {
                        scheduled += 1;
                    }
                }
                let result = AgentBroadcastResult { total, scheduled };
                self.state()?.events.push_back(HostEvent::AgentBroadcasted {
                    timestamp: timestamp(),
                    result,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            _ => unreachable!("non-agent-messaging command routed to agent messaging executor"),
        }
    }

    fn schedule_background_agent_turn(
        &self,
        target: &BotSummary,
        source: &str,
        prompt: String,
        client_message_id: String,
        selected_image_data_urls: Vec<String>,
    ) -> Result<Option<String>, FeatureHostError> {
        if self.config.mode == HostMode::Test {
            let operation_id = format!("background-test-{}-{}", target.id, now_millis());
            let mut state = self.state()?;
            state.events.push_back(HostEvent::AgentBackgroundStarted {
                timestamp: timestamp(),
                agent_id: target.id.clone(),
                agent_name: target.name.clone(),
                operation_id: operation_id.clone(),
                source: source.to_string(),
            });
            state.events.push_back(HostEvent::AgentBackgroundMessage {
                timestamp: timestamp(),
                agent_id: target.id.clone(),
                agent_name: target.name.clone(),
                operation_id: operation_id.clone(),
                source: source.to_string(),
                text: format!("{} received background work.", target.name),
            });
            state.events.push_back(HostEvent::AgentBackgroundFinished {
                timestamp: timestamp(),
                agent_id: target.id.clone(),
                agent_name: target.name.clone(),
                operation_id: operation_id.clone(),
                source: source.to_string(),
                error: None,
            });
            return Ok(Some(operation_id));
        }

        #[cfg(feature = "production")]
        {
            self.schedule_recoverable_background_turn(
                target,
                source,
                prompt,
                client_message_id,
                selected_image_data_urls,
                None,
            )
        }
        #[cfg(not(feature = "production"))]
        Err(FeatureHostError::ProductionUnavailable)
    }

    #[cfg(feature = "production")]
    fn schedule_recoverable_background_turn(
        &self,
        target: &BotSummary,
        source: &str,
        prompt: String,
        client_message_id: String,
        selected_image_data_urls: Vec<String>,
        teach_artifact: Option<String>,
    ) -> Result<Option<String>, FeatureHostError> {
        self.require_authenticated_account()?;
        let _gate = self.routine_dispatch_lock.lock().map_err(|_| FeatureHostError::StatePoisoned)?;
        let conversation_id = target.conversation_id.clone().ok_or_else(|| {
            FeatureHostError::Contract(format!("bot has no conversation: {}", target.id))
        })?;
        let account_key = self.routine_account_key()?;
        let operation_id = format!("background-operation:{}", Uuid::new_v4());
        {
            let mut state = self.state()?;
            ensure_open(&state)?;
            if state.routine_quiescing {
                return Err(FeatureHostError::Contract("background work is quiescing".into()));
            }
            if state.background_recoveries.len() >= BACKGROUND_RECOVERY_MAX_ACTIVE {
                return Err(FeatureHostError::Contract("background recovery capacity reached".into()));
            }
            let execution = BackgroundRecoveryExecution {
                operation_id: operation_id.clone(),
                conversation_id: conversation_id.clone(),
                account_key,
                epoch: state.routine_epoch,
                agent_id: target.id.clone(),
                agent_name: target.name.clone(),
                source: source.to_string(),
                teach_artifact: teach_artifact.clone(),
                delivered_message_fingerprints: BTreeSet::new(),
                phase: BackgroundRecoveryPhase::Dispatching,
            };
            state.background_recoveries.insert(operation_id.clone(), execution);
            state.background_operations.insert(
                operation_id.clone(),
                BackgroundOperationContext {
                    agent_id: target.id.clone(),
                    agent_name: target.name.clone(),
                    source: source.to_string(),
                    teach_artifact: teach_artifact.clone(),
                },
            );
            self.persist_background_recoveries(&state)?;
        }
        let accepted = match self.runtime()?.start_recoverable_message(mahayana_conversation::SendMessageRequest {
            conversation_id: ConversationId(conversation_id),
            operation_id: OperationId(operation_id.clone()),
            text: prompt,
            display_text: None,
            client_message_id: Some(client_message_id.clone()),
            hidden: true,
            show_assistant_output: true,
            recovery_eligible: true,
            reply_to_message_id: None,
            is_fork: false,
            attachment_batch_id: (!selected_image_data_urls.is_empty())
                .then(|| format!("attachment-batch:{client_message_id}")),
            selected_image_data_urls,
        }) {
            Ok(operation_id) => operation_id,
            Err(error) => {
                self.mark_background_recovery_required(&operation_id)?;
                return Err(error.into());
            }
        };
        if accepted.as_str() != operation_id {
            return Err(FeatureHostError::Contract("runtime changed preassigned background operation identity".into()));
        }
        let mut state = self.state()?;
        let current = state.background_recoveries.get_mut(&operation_id)
            .ok_or_else(|| FeatureHostError::Contract("background recovery disappeared during dispatch".into()))?;
        current.phase = BackgroundRecoveryPhase::Running;
        state.operations.insert(operation_id.clone());
        state.operation_agents.insert(operation_id.clone(), target.id.clone());
        self.persist_background_recoveries(&state)?;
        state.events.push_back(HostEvent::AgentBackgroundStarted {
            timestamp: timestamp(),
            agent_id: target.id.clone(),
            agent_name: target.name.clone(),
            operation_id: operation_id.clone(),
            source: source.to_string(),
        });
        Ok(Some(operation_id))
    }

    fn execute_computer(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let settings = self.state()?.settings.clone();
        match command {
            FeatureCommand::ComputerStatus { .. } => {
                let status = mahayana_computer::status(
                    settings.local_execution,
                    settings.route_egress_locally,
                    settings.remote_control_enabled,
                    settings.ai_computer_control_enabled,
                );
                self.state()?
                    .events
                    .push_back(HostEvent::ComputerStatusChanged {
                        timestamp: timestamp(),
                        request_id: request_id.clone(),
                        status,
                    });
            }
            FeatureCommand::ComputerScreenshot {
                origin,
                session_id,
                target,
                ..
            } => {
                self.ensure_computer_origin_allowed(
                    origin,
                    session_id.as_deref(),
                    &target,
                    &settings,
                )?;
                let snapshot = if self.config.mode == HostMode::Test {
                    test_computer_snapshot()
                } else {
                    mahayana_computer::capture_screen()
                        .map_err(|error| FeatureHostError::Contract(error.to_string()))?
                };
                let _ = self.append_action_audit(
                    "mahayana-assistant",
                    session_id.as_deref(),
                    json!({
                        "kind": "computerScreenshot",
                        "origin": computer_origin_label(origin),
                        "sessionId": session_id,
                        "capturedAtMs": snapshot.captured_at_ms,
                    }),
                );
                self.state()?
                    .events
                    .push_back(HostEvent::ComputerSnapshotCaptured {
                        timestamp: timestamp(),
                        request_id: request_id.clone(),
                        origin,
                        snapshot,
                    });
            }
            FeatureCommand::ComputerAction {
                origin,
                agent_id,
                session_id,
                target,
                action,
                then,
                ..
            } => {
                self.ensure_computer_origin_allowed(
                    origin,
                    session_id.as_deref(),
                    &target,
                    &settings,
                )?;
                let audit_agent_id = agent_id
                    .as_deref()
                    .filter(|id| is_safe_memory_agent_id(id))
                    .unwrap_or("mahayana-assistant");
                if let Some(agent_id) = agent_id.as_deref() {
                    if !self.state()?.bots.contains_key(agent_id) {
                        return Err(FeatureHostError::Contract(format!(
                            "unknown computer-control agent: {agent_id}"
                        )));
                    }
                }
                let mut actions = Vec::with_capacity(1 + then.len());
                actions.push(action);
                actions.extend(then);
                let result = if self.config.mode == HostMode::Test {
                    for action in &actions {
                        mahayana_computer::validate_action(action)
                            .map_err(|error| FeatureHostError::Contract(error.to_string()))?;
                    }
                    if actions.len() > mahayana_host_protocol::COMPUTER_MAX_ACTIONS_PER_CALL {
                        return Err(FeatureHostError::Contract(format!(
                            "at most {} computer actions may be batched",
                            mahayana_host_protocol::COMPUTER_MAX_ACTIONS_PER_CALL
                        )));
                    }
                    ComputerActionResult {
                        origin,
                        actions_executed: actions.len(),
                        snapshot: test_computer_snapshot(),
                    }
                } else {
                    mahayana_computer::execute(&actions, origin)
                        .map_err(|error| FeatureHostError::Contract(error.to_string()))?
                };
                let serialized_actions = serde_json::to_value(&actions).unwrap_or(Value::Null);
                self.append_action_audit(
                    audit_agent_id,
                    session_id.as_deref(),
                    json!({
                        "kind": "computerUse",
                        "origin": computer_origin_label(origin),
                        "sessionId": session_id,
                        "actionCount": result.actions_executed,
                        "actions": serialized_actions,
                        "status": "success",
                    }),
                )?;
                self.state()?
                    .events
                    .push_back(HostEvent::ComputerActionCompleted {
                        timestamp: timestamp(),
                        request_id: request_id.clone(),
                        result,
                    });
            }
            _ => unreachable!("non-computer command routed to computer executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_remote_computer(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let remote_enabled = self.state()?.settings.remote_control_enabled;
        let (method, action, payload, local_transition) = match command {
            FeatureCommand::RemoteComputerRegister {
                device_id,
                label,
                provider,
                platform,
                app_version,
                capabilities,
                ..
            } => {
                // Registration is device presence, not authorization to control.
                // Keeping it available while remote control is disabled lets the
                // signed-in account discover this installed desktop. Session
                // polling, activation, signaling, screenshots, and input remain
                // gated below by `remote_control_enabled`.
                let device_secret = self.remote_device_secret(&device_id, true)?;
                let provider = provider.unwrap_or_else(|| "fabushi-webrtc".to_string());
                let platform = platform.unwrap_or_else(|| "unknown".to_string());
                let app_version = app_version.unwrap_or_else(|| "unknown".to_string());
                (
                    "mahayana.remote.computer.register",
                    "registered",
                    json!({
                        "deviceId": device_id,
                        "label": label,
                        "deviceSecret": device_secret,
                        "provider": provider,
                        "platform": platform,
                        "appVersion": app_version,
                        "capabilities": capabilities,
                    }),
                    None,
                )
            }
            FeatureCommand::RemoteComputerHeartbeat { device_id, .. } => {
                let device_secret = self.remote_device_secret(&device_id, false)?;
                (
                    "mahayana.remote.computer.heartbeat",
                    "heartbeat",
                    json!({"deviceId": device_id, "deviceSecret": device_secret}),
                    None,
                )
            }
            FeatureCommand::RemoteComputerClients { device_id, .. } => (
                "mahayana.remote.computer.clients",
                "clients",
                json!({"deviceId": device_id}),
                None,
            ),
            FeatureCommand::RemoteComputerClientRevoke {
                device_id,
                client_id,
                ..
            } => (
                "mahayana.remote.computer.client.revoke",
                "clientRevoked",
                json!({"deviceId": device_id, "clientId": client_id}),
                Some(("revoke-client".to_string(), client_id)),
            ),
            FeatureCommand::RemoteComputerSessions { device_id, .. } => {
                if !remote_enabled {
                    return Err(FeatureHostError::Contract(
                        "remote computer control is disabled".into(),
                    ));
                }
                let device_secret = self.remote_device_secret(&device_id, false)?;
                (
                    "mahayana.remote.computer.sessions",
                    "sessions",
                    json!({"deviceId": device_id, "deviceSecret": device_secret}),
                    None,
                )
            }
            FeatureCommand::RemoteComputerSessionActivate {
                device_id,
                session_id,
                ..
            } => {
                if !remote_enabled {
                    return Err(FeatureHostError::Contract(
                        "remote computer control is disabled".into(),
                    ));
                }
                let device_secret = self.remote_device_secret(&device_id, false)?;
                (
                    "mahayana.remote.computer.session.activate",
                    "sessionActivated",
                    json!({"deviceId": device_id, "sessionId": session_id, "deviceSecret": device_secret}),
                    Some(("activate".to_string(), session_id)),
                )
            }
            FeatureCommand::RemoteComputerSessionClose {
                device_id,
                session_id,
                ..
            } => {
                let device_secret = self.remote_device_secret(&device_id, false)?;
                (
                    "mahayana.remote.computer.session.close",
                    "sessionClosed",
                    json!({"deviceId": device_id, "sessionId": session_id, "role": "desktop", "deviceSecret": device_secret}),
                    Some(("close".to_string(), session_id)),
                )
            }
            FeatureCommand::RemoteComputerSignal {
                device_id,
                session_id,
                kind,
                payload,
                ..
            } => {
                if !remote_enabled {
                    return Err(FeatureHostError::Contract(
                        "remote computer control is disabled".into(),
                    ));
                }
                let device_secret = self.remote_device_secret(&device_id, false)?;
                (
                    "mahayana.remote.computer.signal",
                    "signal",
                    json!({
                        "deviceId": device_id,
                        "sessionId": session_id,
                        "senderRole": "desktop",
                        "deviceSecret": device_secret,
                        "kind": kind,
                        "payload": payload,
                    }),
                    None,
                )
            }
            FeatureCommand::RemoteComputerSignalDrain {
                device_id,
                session_id,
                after_signal_id,
                ..
            } => {
                if !remote_enabled {
                    return Err(FeatureHostError::Contract(
                        "remote computer control is disabled".into(),
                    ));
                }
                let device_secret = self.remote_device_secret(&device_id, false)?;
                (
                    "mahayana.remote.computer.signals.drain",
                    "signals",
                    json!({
                        "deviceId": device_id,
                        "sessionId": session_id,
                        "receiverRole": "desktop",
                        "deviceSecret": device_secret,
                        "afterSignalId": after_signal_id.max(0),
                    }),
                    None,
                )
            }
            _ => unreachable!("non-remote-computer command routed to remote computer executor"),
        };

        let mut data = if self.config.mode == HostMode::Test {
            match action {
                "registered" => json!({
                    "deviceId": payload.get("deviceId"),
                    "label": payload.get("label"),
                    "pairingCode": "AB12CD34EF56",
                    "pairingExpiresAt": now_millis() / 1000 + 600,
                }),
                "heartbeat" => json!({"ok": true, "lastSeenAt": now_millis() / 1000}),
                "clients" => json!({"deviceId": payload.get("deviceId"), "clients": []}),
                "sessions" => json!({"deviceId": payload.get("deviceId"), "sessions": []}),
                "sessionActivated" => json!({
                    "sessionId": payload.get("sessionId"),
                    "clientId": "remote-client-test",
                    "expiresAt": now_millis() / 1000 + 7200,
                    "state": "active",
                }),
                "sessionClosed" => {
                    json!({"sessionId": payload.get("sessionId"), "state": "closed"})
                }
                "signals" => {
                    json!({"sessionId": payload.get("sessionId"), "signals": [], "lastSignalId": payload.get("afterSignalId")})
                }
                _ => json!({"ok": true}),
            }
        } else {
            #[cfg(feature = "production")]
            {
                self.runtime()?.product_execute(method, &payload)?
            }
            #[cfg(not(feature = "production"))]
            {
                return Err(FeatureHostError::ProductionUnavailable);
            }
        };

        if let Some((transition, id)) = local_transition {
            match transition.as_str() {
                "activate" => {
                    let client_id = data
                        .get("clientId")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            FeatureHostError::Contract(
                                "remote session activation did not return clientId".into(),
                            )
                        })?
                        .to_string();
                    let expires_at_seconds = data
                        .get("expiresAt")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| {
                            FeatureHostError::Contract(
                                "remote session activation did not return expiresAt".into(),
                            )
                        })?;
                    let device_id = payload
                        .get("deviceId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let generation = {
                        let mut state = self.state()?;
                        state.sequence = state.sequence.saturating_add(1);
                        let generation = state.sequence;
                        state.remote_computer_sessions.insert(
                            id,
                            RemoteComputerLocalSession {
                                device_id,
                                client_id,
                                expires_at_seconds,
                                generation,
                            },
                        );
                        generation
                    };
                    if let Some(object) = data.as_object_mut() {
                        object.insert("generation".into(), json!(generation));
                    }
                }
                "close" => {
                    self.state()?.remote_computer_sessions.remove(&id);
                }
                "revoke-client" => {
                    self.state()?
                        .remote_computer_sessions
                        .retain(|_, session| session.client_id != id);
                }
                _ => {}
            }
        }

        self.state()?
            .events
            .push_back(HostEvent::RemoteComputerChanged {
                timestamp: timestamp(),
                request_id: request_id.clone(),
                action: action.to_string(),
                data,
            });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn ensure_computer_origin_allowed(
        &self,
        origin: ComputerControlOrigin,
        session_id: Option<&str>,
        target: &ComputerControlTarget,
        settings: &ProductHostSettings,
    ) -> Result<(), FeatureHostError> {
        if target.protocol_version != COMPUTER_CONTROL_PROTOCOL_VERSION {
            return Err(FeatureHostError::Contract(format!(
                "unsupported computer-control protocol version: {}",
                target.protocol_version
            )));
        }
        if target.kind != ComputerTargetKind::Desktop {
            return Err(FeatureHostError::Contract(
                "desktop computer action cannot target a window or browser tab; use the target-specific capability".into(),
            ));
        }
        match origin {
            ComputerControlOrigin::LocalUi => {
                if !settings.local_execution {
                    return Err(FeatureHostError::Contract(
                        "local computer control is disabled in settings".into(),
                    ));
                }
            }
            ComputerControlOrigin::RemoteMobile => {
                if !settings.remote_control_enabled {
                    return Err(FeatureHostError::Contract(
                        "remote computer control is disabled in settings".into(),
                    ));
                }
                let session_id = session_id
                    .filter(|session| !session.trim().is_empty())
                    .ok_or_else(|| {
                        FeatureHostError::Contract(
                            "remote computer control requires an active paired session id".into(),
                        )
                    })?;
                let mut state = self.state()?;
                let now = now_millis() / 1000;
                state
                    .remote_computer_sessions
                    .retain(|_, session| session.expires_at_seconds > now);
                let Some(session) = state.remote_computer_sessions.get(session_id) else {
                    return Err(FeatureHostError::Contract(
                        "remote computer session is not active on this desktop".into(),
                    ));
                };
                if session.device_id.trim().is_empty() || session.client_id.trim().is_empty() {
                    return Err(FeatureHostError::Contract(
                        "remote computer session metadata is invalid".into(),
                    ));
                }
                if target.device_id.as_deref() != Some(session.device_id.as_str()) {
                    return Err(FeatureHostError::Contract(
                        "remote computer target device does not match the active paired session"
                            .into(),
                    ));
                }
                if target.generation != session.generation {
                    return Err(FeatureHostError::Contract(
                        "remote computer target generation is stale or mismatched".into(),
                    ));
                }
            }
            ComputerControlOrigin::Ai => {
                if !settings.local_execution || !settings.ai_computer_control_enabled {
                    return Err(FeatureHostError::Contract(
                        "AI computer control is disabled in settings".into(),
                    ));
                }
                match settings.local_tool_permission {
                    LocalToolPermission::Never => {
                        return Err(FeatureHostError::Contract(
                            "AI local-tool permission is set to Never".into(),
                        ));
                    }
                    LocalToolPermission::Ask => {
                        return Err(FeatureHostError::Contract(
                            "AI computer control requires an explicit approval while local-tool permission is Ask"
                                .into(),
                        ));
                    }
                    LocalToolPermission::Always => {}
                }
            }
        }
        Ok(())
    }

    fn execute_memory(&self, command: FeatureCommand) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let (agent_id, action) = match command {
            FeatureCommand::MemoryList {
                agent_id, limit, ..
            } => (agent_id, MemoryAction::List { limit }),
            FeatureCommand::MemoryAdd {
                agent_id,
                content,
                kind,
                ..
            } => (agent_id, MemoryAction::Add { content, kind }),
            FeatureCommand::MemoryRemove { agent_id, id, .. } => {
                (agent_id, MemoryAction::Remove { id })
            }
            FeatureCommand::MemoryClear { agent_id, .. } => (agent_id, MemoryAction::Clear),
            _ => unreachable!("non-memory command routed to memory executor"),
        };
        {
            let state = self.state()?;
            ensure_open(&state)?;
            if !state.bots.contains_key(&agent_id) {
                return Err(FeatureHostError::Contract(format!(
                    "unknown bot: {agent_id}"
                )));
            }
        }
        if !is_safe_memory_agent_id(&agent_id) {
            return Err(FeatureHostError::Contract(format!(
                "unsafe memory agent id: {agent_id}"
            )));
        }
        let root = self
            .memory_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("memory storage is unavailable".into()))?;
        let memory_dir = root.join(&agent_id).join("memory");
        match action {
            MemoryAction::List { limit } => {
                let memories = list_memories(&memory_dir, limit.min(1000))?;
                let count = count_memories(&memory_dir)?;
                self.state()?.events.push_back(HostEvent::MemoryListed {
                    timestamp: timestamp(),
                    agent_id,
                    memories,
                    count,
                    location: Some(memory_dir.to_string_lossy().into_owned()),
                });
            }
            MemoryAction::Add { content, kind } => {
                let memory = add_memory(&memory_dir, &content, now_millis(), kind)?;
                self.state()?.events.push_back(HostEvent::MemoryChanged {
                    timestamp: timestamp(),
                    agent_id,
                    action: if memory.is_some() {
                        "added"
                    } else {
                        "duplicate"
                    }
                    .into(),
                    memory,
                });
            }
            MemoryAction::Remove { id } => {
                let removed = remove_memory(&memory_dir, &id)?;
                self.state()?.events.push_back(HostEvent::MemoryChanged {
                    timestamp: timestamp(),
                    agent_id,
                    action: if removed { "removed" } else { "notFound" }.into(),
                    memory: None,
                });
            }
            MemoryAction::Clear => {
                if memory_dir.exists() {
                    std::fs::remove_dir_all(&memory_dir).map_err(|error| {
                        FeatureHostError::Contract(format!("clear memory: {error}"))
                    })?;
                }
                self.state()?.events.push_back(HostEvent::MemoryChanged {
                    timestamp: timestamp(),
                    agent_id,
                    action: "cleared".into(),
                    memory: None,
                });
            }
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_tray(&self, command: FeatureCommand) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let mut state = self.state()?;
        ensure_open(&state)?;
        match command {
            FeatureCommand::TrayList { .. } => {
                let trays = state.trays.clone();
                state.events.push_back(HostEvent::TrayListed {
                    timestamp: timestamp(),
                    trays,
                });
            }
            FeatureCommand::TrayDismiss { id, .. } => {
                let before = state.trays.len();
                state.trays.retain(|tray| tray.id != id);
                if state.trays.len() != before {
                    state.events.push_back(HostEvent::TrayChanged {
                        timestamp: timestamp(),
                        action: "dismissed".into(),
                        tray: None,
                        id: Some(id),
                    });
                }
            }
            FeatureCommand::TrayClear { .. } => {
                if !state.trays.is_empty() {
                    state.trays.clear();
                    state.events.push_back(HostEvent::TrayChanged {
                        timestamp: timestamp(),
                        action: "cleared".into(),
                        tray: None,
                        id: None,
                    });
                }
            }
            FeatureCommand::TrayClearForAgent { agent_id, .. } => {
                let removed = state
                    .trays
                    .iter()
                    .filter(|tray| tray.agent_id == agent_id)
                    .map(|tray| tray.id.clone())
                    .collect::<Vec<_>>();
                state.trays.retain(|tray| tray.agent_id != agent_id);
                for id in removed {
                    state.events.push_back(HostEvent::TrayChanged {
                        timestamp: timestamp(),
                        action: "dismissed".into(),
                        tray: None,
                        id: Some(id),
                    });
                }
            }
            _ => unreachable!("non-tray command routed to tray executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_workflow(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let workflow_root = self
            .workflow_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("workflow storage is unavailable".into()))?;
        let agent_root = self
            .memory_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("agent storage is unavailable".into()))?;
        let agent_id = match &command {
            FeatureCommand::WorkflowList { agent_id, .. }
            | FeatureCommand::WorkflowUpsert { agent_id, .. }
            | FeatureCommand::WorkflowSetEnabled { agent_id, .. }
            | FeatureCommand::WorkflowDelete { agent_id, .. }
            | FeatureCommand::WorkflowRun { agent_id, .. }
            | FeatureCommand::WorkflowImportMarkdown { agent_id, .. }
            | FeatureCommand::WorkflowImportLiveSource { agent_id, .. } => agent_id.clone(),
            _ => unreachable!("non-workflow command routed to workflow executor"),
        };
        {
            let state = self.state()?;
            ensure_open(&state)?;
            if !state.bots.contains_key(&agent_id) {
                return Err(FeatureHostError::Contract(format!(
                    "unknown bot: {agent_id}"
                )));
            }
        }
        if !is_safe_memory_agent_id(&agent_id) {
            return Err(FeatureHostError::Contract(format!(
                "unsafe workflow agent id: {agent_id}"
            )));
        }

        match command {
            FeatureCommand::WorkflowList { .. } => {
                let mut workflows = list_workflow_summaries(workflow_root, agent_root, &agent_id);
                let (automations, published_plugins) = {
                    let state = self.state()?;
                    let automations = state
                        .automations
                        .values()
                        .filter(|automation| {
                            automation
                                .agent_id
                                .as_deref()
                                .is_none_or(|owner| owner == agent_id.as_str())
                        })
                        .map(workflow_from_automation)
                        .collect::<Vec<_>>();
                    let published_plugins = state
                        .published_plugins_by_agent
                        .get(&agent_id)
                        .cloned()
                        .unwrap_or_default();
                    (automations, published_plugins)
                };
                workflows.extend(automations);
                workflows.extend(published_workflow_summaries(
                    workflow_root,
                    agent_root,
                    &agent_id,
                    &published_plugins,
                ));
                workflows.truncate(WORKFLOW_UI_LIMIT);
                self.state()?.events.push_back(HostEvent::WorkflowListed {
                    timestamp: timestamp(),
                    agent_id,
                    workflows,
                });
            }
            FeatureCommand::WorkflowUpsert {
                id,
                name,
                description,
                body,
                trigger,
                source_ref,
                ..
            } => {
                if let Some(mut trigger) = trigger {
                    trigger.schedule = normalize_automation_schedule(&trigger.schedule)?;
                    let now = now_millis();
                    let automation_id = id.clone().unwrap_or_else(|| slugify_workflow_name(&name));
                    let automation = {
                        let mut state = self.state()?;
                        let created_at_ms = state
                            .automations
                            .get(&automation_id)
                            .map(|automation| automation.created_at_ms)
                            .unwrap_or(now);
                        let last_run_at_ms = state
                            .automations
                            .get(&automation_id)
                            .and_then(|automation| automation.last_run_at_ms);
                        let runs = state
                            .automations
                            .get(&automation_id)
                            .map_or_else(Vec::new, |automation| automation.runs.clone());
                        let automation = AutomationSummary {
                            id: automation_id.clone(),
                            agent_id: Some(agent_id.clone()),
                            name: clamp_workflow_name(&name),
                            prompt: clamp_workflow_body(&body),
                            schedule: trigger.schedule.clone(),
                            trigger: Some(AutomationTrigger::Schedule {
                                schedule: trigger.schedule.clone(),
                            }),
                            enabled: trigger.is_enabled,
                            created_at_ms,
                            runs,
                            last_run_at_ms,
                            next_run_at_ms: trigger
                                .is_enabled
                                .then(|| next_automation_run(&trigger.schedule, now))
                                .flatten(),
                        };
                        state
                            .automations
                            .insert(automation_id.clone(), automation.clone());
                        self.persist_automations(&state.automations)?;
                        automation
                    };
                    let workflow_dir = workflow_root.join(&automation_id);
                    if workflow_dir.exists() {
                        let _ = std::fs::remove_dir_all(&workflow_dir);
                    }
                    let workflow = workflow_from_automation(&automation);
                    self.state()?.events.push_back(HostEvent::WorkflowChanged {
                        timestamp: timestamp(),
                        agent_id,
                        action: "saved".into(),
                        workflow: Some(workflow),
                        id: None,
                    });
                } else {
                    if let Some(existing_id) = id.as_deref() {
                        let removed_automation = {
                            let mut state = self.state()?;
                            if let Some(existing) = state.automations.get(existing_id) {
                                ensure_automation_agent_scope(existing, Some(agent_id.as_str()))?;
                            }
                            let removed = state.automations.remove(existing_id).is_some();
                            if removed {
                                self.persist_automations(&state.automations)?;
                            }
                            removed
                        };
                        if removed_automation {
                            // Converting a scheduled automation back into a trigger-less workflow.
                        }
                    }
                    let workflow = write_workflow(
                        workflow_root,
                        agent_root,
                        &agent_id,
                        id.as_deref(),
                        &name,
                        &description,
                        &body,
                        None,
                        source_ref.as_deref(),
                    )?;
                    self.state()?.events.push_back(HostEvent::WorkflowChanged {
                        timestamp: timestamp(),
                        agent_id,
                        action: "saved".into(),
                        workflow: Some(workflow),
                        id: None,
                    });
                }
            }
            FeatureCommand::WorkflowSetEnabled { id, enabled, .. } => {
                let automation = {
                    let mut state = self.state()?;
                    if let Some(automation) = state.automations.get_mut(&id) {
                        ensure_automation_agent_scope(automation, Some(agent_id.as_str()))?;
                        automation.enabled = enabled;
                        automation.next_run_at_ms = if enabled {
                            next_automation_run(&automation.schedule, now_millis())
                        } else {
                            None
                        };
                        let automation = automation.clone();
                        self.persist_automations(&state.automations)?;
                        Some(automation)
                    } else {
                        None
                    }
                };
                let workflow = if let Some(automation) = automation {
                    Some(workflow_from_automation(&automation))
                } else {
                    set_workflow_enabled(agent_root, &agent_id, &id, enabled)?;
                    load_workflow_summary(workflow_root, agent_root, &agent_id, &id)
                };
                self.state()?.events.push_back(HostEvent::WorkflowChanged {
                    timestamp: timestamp(),
                    agent_id,
                    action: "enabled".into(),
                    workflow,
                    id: Some(id),
                });
            }
            FeatureCommand::WorkflowDelete { id, .. } => {
                let removed_automation = {
                    let mut state = self.state()?;
                    if let Some(existing) = state.automations.get(&id) {
                        ensure_automation_agent_scope(existing, Some(agent_id.as_str()))?;
                    }
                    let removed = state.automations.remove(&id).is_some();
                    if removed {
                        self.persist_automations(&state.automations)?;
                    }
                    removed
                };
                if !removed_automation {
                    let path = workflow_root.join(&id);
                    if path.exists() {
                        std::fs::remove_dir_all(&path).map_err(|error| {
                            FeatureHostError::Contract(format!("delete workflow: {error}"))
                        })?;
                    }
                    forget_workflow_enablement(agent_root, &agent_id, &id)?;
                }
                self.state()?.events.push_back(HostEvent::WorkflowChanged {
                    timestamp: timestamp(),
                    agent_id,
                    action: "deleted".into(),
                    workflow: None,
                    id: Some(id),
                });
            }
            FeatureCommand::WorkflowRun { id, .. } => {
                if self.state()?.automations.contains_key(&id) {
                    return self.execute_automation(FeatureCommand::AutomationRun {
                        request_id,
                        id,
                        agent_id: Some(agent_id),
                    });
                }
                let workflow = load_workflow_summary(workflow_root, agent_root, &agent_id, &id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown workflow: {id}")))?;
                if !workflow.is_enabled_for_agent {
                    return Err(FeatureHostError::Contract(format!(
                        "workflow is disabled for {agent_id}: {id}"
                    )));
                }
                let visible_text = format!("@{}", workflow.name);
                let recipe = clamp_block(&workflow.body, WORKFLOW_INJECTED_BODY_LIMIT);
                let runtime_text = format!(
                    "[Workflow reference: {}]\nFollow this recipe for the current turn:\n{}\n\n[Visible user message]\n{}",
                    workflow.name, recipe, visible_text
                );
                match self.config.mode {
                    HostMode::Test => {
                        let mut state = self.state()?;
                        state.events.push_back(HostEvent::ChatMessage {
                            timestamp: timestamp(),
                            role: MessageRole::User,
                            text: visible_text,
                            operation_id: None,
                            message_id: None,
                            reply_to_message_id: None,
                            attachment_batch_id: None,
                            attachment: None,
                            branched: false,
                        });
                        state.events.push_back(HostEvent::ChatMessage {
                            timestamp: timestamp(),
                            role: MessageRole::Assistant,
                            text: format!("Running workflow: {}", workflow.name),
                            operation_id: None,
                            message_id: None,
                            reply_to_message_id: None,
                            attachment_batch_id: None,
                            attachment: None,
                            branched: false,
                        });
                        return Ok(CommandAccepted {
                            request_id,
                            operation_id: None,
                        });
                    }
                    HostMode::Production => {
                        #[cfg(feature = "production")]
                        {
                            let conversation_id = self
                                .state()?
                                .bots
                                .get(&agent_id)
                                .and_then(|bot| bot.conversation_id.clone())
                                .ok_or_else(|| {
                                    FeatureHostError::Contract(format!(
                                        "bot has no conversation: {agent_id}"
                                    ))
                                })?;
                            let (provider, model) = match self
                                .runtime()?
                                .execute(RuntimeCommand::Status)?
                            {
                                RuntimeResponse::Status(status) => (
                                    format!("{:?}", status.model_provider).to_lowercase(),
                                    status.model,
                                ),
                                other => return Err(unexpected_response("runtime.status", other)),
                            };
                            let response =
                                self.runtime()?.execute(RuntimeCommand::SendMessage {
                                    conversation_id: ConversationId(conversation_id),
                                    text: runtime_text,
                                    display_text: None,
                                    client_message_id: Some(request_id.clone()),
                                    hidden: true,
                                    show_assistant_output: false,
                                    recovery_eligible: false,
                                    reply_to_message_id: None,
                                    is_fork: false,
                                    attachment_batch_id: None,
                                    selected_image_data_urls: Vec::new(),
                                })?;
                            let operation_id = match response {
                                RuntimeResponse::Accepted { operation_id } => {
                                    operation_id.to_string()
                                }
                                other => return Err(unexpected_response("workflow.run", other)),
                            };
                            let mut state = self.state()?;
                            state.operations.insert(operation_id.clone());
                            state
                                .operation_agents
                                .insert(operation_id.clone(), agent_id);
                            state.events.push_back(HostEvent::ModelRouted {
                                timestamp: timestamp(),
                                operation_id: operation_id.clone(),
                                provider,
                                model,
                                mode: AgentMode::Agent,
                            });
                            state.events.push_back(HostEvent::ChatMessage {
                                timestamp: timestamp(),
                                role: MessageRole::User,
                                text: visible_text,
                                operation_id: None,
                                message_id: None,
                                reply_to_message_id: None,
                                attachment_batch_id: None,
                                attachment: None,
                                branched: false,
                            });
                            state.events.push_back(HostEvent::OperationStarted {
                                timestamp: timestamp(),
                                operation_id: operation_id.clone(),
                                label: format!("workflow:{}", workflow.id),
                                interruptible: true,
                            });
                            return Ok(CommandAccepted {
                                request_id,
                                operation_id: Some(operation_id),
                            });
                        }
                        #[cfg(not(feature = "production"))]
                        return Err(FeatureHostError::ProductionUnavailable);
                    }
                }
            }
            FeatureCommand::WorkflowImportMarkdown {
                markdown,
                fallback_name,
                ..
            } => {
                let parsed = parse_workflow_file(&markdown).ok_or_else(|| {
                    FeatureHostError::Contract("workflow markdown is empty".into())
                })?;
                let name = if parsed.name.is_empty() {
                    derive_workflow_name_from_markdown(&parsed.body)
                        .or(fallback_name.map(|name| clamp_workflow_name(&name)))
                        .unwrap_or_default()
                } else {
                    parsed.name.clone()
                };
                if name.is_empty() || parsed.body.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "workflow markdown must include a name and body".into(),
                    ));
                }
                let workflow = if let Some(mut trigger) = parsed.trigger.clone() {
                    trigger.schedule = normalize_automation_schedule(&trigger.schedule)?;
                    let now = now_millis();
                    let id = slugify_workflow_name(&name);
                    let automation = AutomationSummary {
                        id: id.clone(),
                        agent_id: Some(agent_id.clone()),
                        name: name.clone(),
                        prompt: parsed.body.clone(),
                        schedule: trigger.schedule.clone(),
                        trigger: Some(AutomationTrigger::Schedule {
                            schedule: trigger.schedule.clone(),
                        }),
                        enabled: trigger.is_enabled,
                        created_at_ms: now,
                        runs: Vec::new(),
                        last_run_at_ms: None,
                        next_run_at_ms: trigger
                            .is_enabled
                            .then(|| next_automation_run(&trigger.schedule, now))
                            .flatten(),
                    };
                    {
                        let mut state = self.state()?;
                        state.automations.insert(id, automation.clone());
                        self.persist_automations(&state.automations)?;
                    }
                    workflow_from_automation(&automation)
                } else {
                    write_workflow(
                        workflow_root,
                        agent_root,
                        &agent_id,
                        None,
                        &name,
                        &parsed.description,
                        &parsed.body,
                        None,
                        parsed.source_ref.as_deref(),
                    )?
                };
                self.state()?.events.push_back(HostEvent::WorkflowChanged {
                    timestamp: timestamp(),
                    agent_id,
                    action: "imported".into(),
                    workflow: Some(workflow),
                    id: None,
                });
            }
            FeatureCommand::WorkflowImportLiveSource {
                source,
                fallback_name,
                ..
            } => {
                let source = source.trim().to_string();
                if source.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "workflow live source must not be empty".into(),
                    ));
                }
                let name = fallback_name
                    .map(|name| clamp_workflow_name(&name))
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| derive_workflow_name_from_source(&source));
                let description = build_live_source_description(&name, &source);
                let body = build_live_source_pointer_body(&source);
                let workflow = write_workflow(
                    workflow_root,
                    agent_root,
                    &agent_id,
                    None,
                    &name,
                    &description,
                    &body,
                    None,
                    Some(&source),
                )?;
                self.state()?.events.push_back(HostEvent::WorkflowChanged {
                    timestamp: timestamp(),
                    agent_id,
                    action: "imported".into(),
                    workflow: Some(workflow),
                    id: None,
                });
            }
            _ => unreachable!("non-workflow command routed to workflow executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_attachment(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let agent_root = self
            .active_account_root(self.memory_root_path.as_deref())
            .ok_or_else(|| {
                FeatureHostError::Contract("attachment storage is unavailable".into())
            })?;
        match command {
            FeatureCommand::AttachmentUpload {
                agent_id,
                filename,
                mime_type,
                bytes_base64,
                ..
            } => {
                if !is_safe_memory_agent_id(&agent_id)
                    || !self.state()?.bots.contains_key(&agent_id)
                {
                    return Err(FeatureHostError::Contract(format!(
                        "unknown attachment owner: {agent_id}"
                    )));
                }
                let filename = filename.trim();
                if filename.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "attachment filename must not be empty".into(),
                    ));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(bytes_base64.trim())
                    .map_err(|error| {
                        FeatureHostError::Contract(format!("invalid attachment base64: {error}"))
                    })?;
                if bytes.is_empty() {
                    return Err(FeatureHostError::Contract("attachment is empty".into()));
                }
                let limit = attachment_byte_limit_for_name(filename);
                if bytes.len() as u64 > limit {
                    return Err(FeatureHostError::Contract(format!(
                        "attachment exceeds {} bytes",
                        limit
                    )));
                }
                let hash = format!("{:x}", Sha256::digest(&bytes));
                let extension = Path::new(filename)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .map(|extension| extension.to_ascii_lowercase())
                    .filter(|extension| {
                        extension
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric())
                    })
                    .filter(|extension| !extension.is_empty())
                    .unwrap_or_else(|| "bin".into());
                let attachments_dir = agent_root.join(&agent_id).join("attachments");
                std::fs::create_dir_all(&attachments_dir).map_err(|error| {
                    FeatureHostError::Contract(format!("create attachment directory: {error}"))
                })?;
                let path = attachments_dir.join(format!("{hash}.{extension}"));
                if !path.exists() {
                    std::fs::write(&path, &bytes).map_err(|error| {
                        FeatureHostError::Contract(format!("write attachment: {error}"))
                    })?;
                }
                let attachment = AttachmentStored {
                    id: hash.clone(),
                    agent_id,
                    name: filename.to_string(),
                    path: path.to_string_lossy().to_string(),
                    mime_type: clean_optional_string(mime_type)
                        .or_else(|| media_mime_type(filename).map(str::to_string)),
                    size_bytes: bytes.len() as u64,
                    hash,
                };
                self.state()?.events.push_back(HostEvent::AttachmentStored {
                    timestamp: timestamp(),
                    attachment,
                });
            }
            FeatureCommand::AttachmentReadText { agent_id, path, .. } => {
                let resolved = resolve_agent_attachment_path(&agent_root, &agent_id, &path)?;
                let metadata = std::fs::metadata(&resolved).map_err(|error| {
                    FeatureHostError::Contract(format!("read attachment metadata: {error}"))
                })?;
                let bytes = metadata.len();
                let preview = read_file_prefix(&resolved, ATTACHMENT_TEXT_PREVIEW_BYTE_CAP)?;
                let binary = !is_text_previewable_name(&resolved) || looks_like_binary(&preview);
                let result = AttachmentTextResult {
                    path: resolved.to_string_lossy().to_string(),
                    kind: if binary { "binary" } else { "text" }.into(),
                    text: (!binary).then(|| String::from_utf8_lossy(&preview).to_string()),
                    truncated: !binary && bytes > ATTACHMENT_TEXT_PREVIEW_BYTE_CAP as u64,
                    bytes,
                };
                self.state()?
                    .events
                    .push_back(HostEvent::AttachmentTextRead {
                        timestamp: timestamp(),
                        result,
                    });
            }
            FeatureCommand::AttachmentReadChunk {
                agent_id,
                path,
                offset,
                length,
                ..
            } => {
                let resolved = resolve_agent_attachment_path(&agent_root, &agent_id, &path)?;
                let metadata = std::fs::metadata(&resolved).map_err(|error| {
                    FeatureHostError::Contract(format!("read attachment metadata: {error}"))
                })?;
                let total_size = metadata.len();
                let start = offset.min(total_size);
                let length = length
                    .min(ATTACHMENT_CHUNK_MAX_BYTES as u64)
                    .min(total_size - start);
                let bytes = read_file_range(&resolved, start, length as usize)?;
                let result = AttachmentChunkResult {
                    path: resolved.to_string_lossy().to_string(),
                    bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    total_size,
                    mime: media_mime_type(resolved.to_string_lossy().as_ref()).map(str::to_string),
                };
                self.state()?
                    .events
                    .push_back(HostEvent::AttachmentChunkRead {
                        timestamp: timestamp(),
                        result,
                    });
            }
            FeatureCommand::AttachmentReadImage { agent_id, path, .. } => {
                let resolved = resolve_agent_attachment_path(&agent_root, &agent_id, &path)?;
                let mime = media_mime_type(resolved.to_string_lossy().as_ref())
                    .filter(|mime| mime.starts_with("image/"))
                    .ok_or_else(|| {
                        FeatureHostError::Contract("attachment is not a supported image".into())
                    })?;
                let bytes = std::fs::read(&resolved).map_err(|error| {
                    FeatureHostError::Contract(format!("read image attachment: {error}"))
                })?;
                if bytes.len() as u64 > ATTACHMENT_BYTE_LIMIT {
                    return Err(FeatureHostError::Contract(
                        "image attachment exceeds preview limit".into(),
                    ));
                }
                let (width, height) = image_dimensions(&bytes, mime);
                let result = AttachmentImageResult {
                    path: resolved.to_string_lossy().to_string(),
                    data_url: format!(
                        "data:{mime};base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(bytes)
                    ),
                    width,
                    height,
                };
                self.state()?
                    .events
                    .push_back(HostEvent::AttachmentImageRead {
                        timestamp: timestamp(),
                        result,
                    });
            }
            _ => unreachable!("non-attachment command routed to attachment executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_search(&self, command: FeatureCommand) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        {
            let state = self.state()?;
            ensure_open(&state)?;
        }
        match command {
            FeatureCommand::SearchMessages { query, limit, .. } => {
                let query = query.trim().to_lowercase();
                let limit = limit.clamp(1, AGENT_CONTENT_SEARCH_MAX_RESULTS);
                let mut matches = Vec::new();
                if !query.is_empty() && self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    {
                        let conversations =
                            match self.runtime()?.execute(RuntimeCommand::ListConversations)? {
                                RuntimeResponse::Conversations { data } => data,
                                other => {
                                    return Err(unexpected_response(
                                        "search.messages.conversations",
                                        other,
                                    ));
                                }
                            };
                        let bots = self.state()?.bots.values().cloned().collect::<Vec<_>>();
                        let bot_by_conversation = bots
                            .iter()
                            .filter_map(|bot| {
                                bot.conversation_id
                                    .as_ref()
                                    .map(|conversation_id| (conversation_id.clone(), bot))
                            })
                            .collect::<BTreeMap<_, _>>();
                        for conversation in conversations {
                            let response =
                                self.runtime()?
                                    .execute(RuntimeCommand::ConversationHistory {
                                        conversation_id: conversation.id.clone(),
                                        limit: Some(2_000),
                                    });
                            let messages = match response {
                                Ok(RuntimeResponse::History { data }) => data,
                                _ => continue,
                            };
                            let bot = bot_by_conversation.get(&conversation.id.0).copied();
                            let agent_id = bot
                                .map(|bot| bot.id.clone())
                                .unwrap_or_else(|| conversation.id.0.clone());
                            let agent_name = bot
                                .map(|bot| bot.name.clone())
                                .unwrap_or_else(|| conversation.title.clone());
                            let mut per_agent = 0usize;
                            for message in messages.into_iter().rev() {
                                if per_agent >= AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT {
                                    break;
                                }
                                let Some(snippet) = build_content_snippet(&message.text, &query)
                                else {
                                    continue;
                                };
                                let role = match message.role {
                                    RuntimeMessageRole::User => MessageRole::User,
                                    RuntimeMessageRole::Assistant
                                    | RuntimeMessageRole::Contact
                                    | RuntimeMessageRole::MiniApp
                                    | RuntimeMessageRole::System => MessageRole::Assistant,
                                };
                                matches.push(SearchMessageMatch {
                                    agent_id: agent_id.clone(),
                                    agent_name: agent_name.clone(),
                                    conversation_id: conversation.id.0.clone(),
                                    entry_id: message.id.to_string(),
                                    role,
                                    timestamp_ms: message.created_at_ms,
                                    snippet,
                                });
                                per_agent += 1;
                            }
                        }
                        matches.sort_by_key(|item| std::cmp::Reverse(item.timestamp_ms));
                        matches.truncate(limit);
                    }
                }
                self.state()?
                    .events
                    .push_back(HostEvent::SearchMessagesListed {
                        timestamp: timestamp(),
                        query,
                        matches,
                    });
            }
            FeatureCommand::SearchMedia { query, limit, .. } => {
                let query = query.trim().to_lowercase();
                let limit = limit.clamp(1, AGENT_CONTENT_SEARCH_MAX_RESULTS);
                let bots = self.state()?.bots.values().cloned().collect::<Vec<_>>();
                let mut matches = Vec::new();
                if let Some(agent_root) = self.active_account_root(self.memory_root_path.as_deref())
                {
                    for bot in bots {
                        collect_agent_media_matches(
                            &agent_root.join(&bot.id).join("attachments"),
                            &bot.id,
                            &bot.name,
                            &query,
                            &mut matches,
                        );
                    }
                }
                matches.sort_by_key(|item| std::cmp::Reverse(item.timestamp_ms));
                matches.truncate(limit);
                self.state()?
                    .events
                    .push_back(HostEvent::SearchMediaListed {
                        timestamp: timestamp(),
                        query,
                        matches,
                    });
            }
            _ => unreachable!("non-search command routed to search executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }


    /// Native settings surfaces use a direct MCP snapshot so they never drain
    /// the renderer's shared HostEvent queue while waiting for settings data.
    pub fn mcp_servers_snapshot(&self) -> Result<Vec<Value>, FeatureHostError> {
        {
            let state = self.state()?;
            ensure_open(&state)?;
        }
        if self.config.mode == HostMode::Production {
            #[cfg(feature = "production")]
            {
                self.require_authenticated_account()?;
                return match self.runtime()?.execute(RuntimeCommand::McpServers)? {
                    RuntimeResponse::McpServers { data } => Ok(data),
                    other => Err(unexpected_response("mcp.serversSnapshot", other)),
                };
            }
            #[cfg(not(feature = "production"))]
            return Err(FeatureHostError::ProductionUnavailable);
        }
        Ok(Vec::new())
    }

    pub fn set_mcp_tool_disabled_direct(
        &self,
        server: String,
        tool: String,
        disabled: bool,
    ) -> Result<Vec<String>, FeatureHostError> {
        {
            let state = self.state()?;
            ensure_open(&state)?;
        }
        let server = required(server, "MCP server")?;
        let tool = required(tool, "MCP tool")?;
        if self.config.mode == HostMode::Production {
            #[cfg(feature = "production")]
            {
                self.require_authenticated_account()?;
                return match self.runtime()?.execute(RuntimeCommand::McpSetToolDisabled {
                    server,
                    tool,
                    disabled,
                })? {
                    RuntimeResponse::McpToolDisabledUpdated { disabled_tools, .. } => Ok(disabled_tools),
                    other => Err(unexpected_response("mcp.setToolDisabledDirect", other)),
                };
            }
            #[cfg(not(feature = "production"))]
            return Err(FeatureHostError::ProductionUnavailable);
        }
        Ok(if disabled { vec![tool] } else { Vec::new() })
    }

    pub fn call_mcp_tool_direct(
        &self,
        server: String,
        tool: String,
        arguments: Value,
    ) -> Result<Value, FeatureHostError> {
        {
            let state = self.state()?;
            ensure_open(&state)?;
        }
        let server = required(server, "MCP server")?;
        let tool = required(tool, "MCP tool")?;
        if self.config.mode == HostMode::Production {
            #[cfg(feature = "production")]
            {
                self.require_authenticated_account()?;
                return match self.runtime()?.execute(RuntimeCommand::McpToolCall {
                    server,
                    tool,
                    arguments,
                })? {
                    RuntimeResponse::McpToolResult { result, .. } => Ok(result),
                    other => Err(unexpected_response("mcp.toolCallDirect", other)),
                };
            }
            #[cfg(not(feature = "production"))]
            return Err(FeatureHostError::ProductionUnavailable);
        }
        Ok(json!({"ok": true, "mock": true}))
    }

    fn execute_mcp(&self, command: FeatureCommand) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        {
            let state = self.state()?;
            ensure_open(&state)?;
        }
        match command {
            FeatureCommand::McpList { .. } => {
                let servers = if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    {
                        match self.runtime()?.execute(RuntimeCommand::McpServers)? {
                            RuntimeResponse::McpServers { data } => data,
                            other => return Err(unexpected_response("mcp.list", other)),
                        }
                    }
                    #[cfg(not(feature = "production"))]
                    Vec::new()
                } else {
                    Vec::new()
                };
                self.state()?.events.push_back(HostEvent::McpListed {
                    timestamp: timestamp(),
                    servers,
                });
            }
            FeatureCommand::McpApps { .. } => {
                let apps = if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    {
                        match self.runtime()?.execute(RuntimeCommand::McpApps)? {
                            RuntimeResponse::McpApps { data } => data,
                            other => return Err(unexpected_response("mcp.apps", other)),
                        }
                    }
                    #[cfg(not(feature = "production"))]
                    Vec::new()
                } else {
                    Vec::new()
                };
                self.state()?.events.push_back(HostEvent::McpAppsListed {
                    timestamp: timestamp(),
                    apps,
                });
            }
            FeatureCommand::McpOauthLogin { server, .. } => {
                let server = required(server, "MCP server")?;
                if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    {
                        let (server, authorization_url, removed) = match self
                            .runtime()?
                            .execute(RuntimeCommand::McpOauthLogin { server })?
                        {
                            RuntimeResponse::McpOauth {
                                server,
                                authorization_url,
                                removed,
                            } => (server, authorization_url, removed),
                            other => return Err(unexpected_response("mcp.oauthLogin", other)),
                        };
                        self.state()?.events.push_back(HostEvent::McpOauthChanged {
                            timestamp: timestamp(),
                            server,
                            authorization_url,
                            removed,
                        });
                    }
                    #[cfg(not(feature = "production"))]
                    return Err(FeatureHostError::ProductionUnavailable);
                } else {
                    self.state()?.events.push_back(HostEvent::McpOauthChanged {
                        timestamp: timestamp(),
                        server,
                        authorization_url: Some("https://example.test/mcp-oauth".into()),
                        removed: false,
                    });
                }
            }
            FeatureCommand::McpOauthLogout { server, .. } => {
                let server = required(server, "MCP server")?;
                if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    {
                        let (server, authorization_url, removed) = match self
                            .runtime()?
                            .execute(RuntimeCommand::McpOauthLogout { server })?
                        {
                            RuntimeResponse::McpOauth {
                                server,
                                authorization_url,
                                removed,
                            } => (server, authorization_url, removed),
                            other => return Err(unexpected_response("mcp.oauthLogout", other)),
                        };
                        self.state()?.events.push_back(HostEvent::McpOauthChanged {
                            timestamp: timestamp(),
                            server,
                            authorization_url,
                            removed,
                        });
                    }
                    #[cfg(not(feature = "production"))]
                    return Err(FeatureHostError::ProductionUnavailable);
                } else {
                    self.state()?.events.push_back(HostEvent::McpOauthChanged {
                        timestamp: timestamp(),
                        server,
                        authorization_url: None,
                        removed: true,
                    });
                }
            }
            FeatureCommand::McpRemove { server, .. } => {
                let server = required(server, "MCP server")?;
                if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    {
                        let removed = match self.runtime()?.execute(RuntimeCommand::McpRemove {
                            server: server.clone(),
                        })? {
                            RuntimeResponse::McpRemoved { removed, .. } => removed,
                            other => return Err(unexpected_response("mcp.remove", other)),
                        };
                        if !removed {
                            return Err(FeatureHostError::Contract(format!(
                                "MCP server is not a user-managed configuration: {server}"
                            )));
                        }
                    }
                    #[cfg(not(feature = "production"))]
                    return Err(FeatureHostError::ProductionUnavailable);
                }
                self.state()?.events.push_back(HostEvent::McpRefreshed {
                    timestamp: timestamp(),
                });
            }
            FeatureCommand::McpSetCustomInstructions {
                server,
                instructions,
                ..
            } => {
                let server = required(server, "MCP server")?;
                let instructions = clamp_block(&instructions, 20_000);
                if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    match self
                        .runtime()?
                        .execute(RuntimeCommand::McpSetCustomInstructions {
                            server: server.clone(),
                            instructions,
                        })? {
                        RuntimeResponse::McpCustomInstructionsUpdated { .. } => {}
                        other => {
                            return Err(unexpected_response("mcp.setCustomInstructions", other));
                        }
                    }
                    #[cfg(not(feature = "production"))]
                    return Err(FeatureHostError::ProductionUnavailable);
                }
                self.state()?.events.push_back(HostEvent::McpRefreshed {
                    timestamp: timestamp(),
                });
            }
            FeatureCommand::McpSetToolDisabled {
                server,
                tool,
                disabled,
                ..
            } => {
                let server = required(server, "MCP server")?;
                let tool = required(tool, "MCP tool")?;
                if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    match self
                        .runtime()?
                        .execute(RuntimeCommand::McpSetToolDisabled {
                            server: server.clone(),
                            tool,
                            disabled,
                        })? {
                        RuntimeResponse::McpToolDisabledUpdated { .. } => {}
                        other => return Err(unexpected_response("mcp.setToolDisabled", other)),
                    }
                    #[cfg(not(feature = "production"))]
                    return Err(FeatureHostError::ProductionUnavailable);
                }
                self.state()?.events.push_back(HostEvent::McpRefreshed {
                    timestamp: timestamp(),
                });
            }
            FeatureCommand::McpRefresh { .. } => {
                if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    match self.runtime()?.execute(RuntimeCommand::McpRefresh)? {
                        RuntimeResponse::McpRefreshed => {}
                        other => return Err(unexpected_response("mcp.refresh", other)),
                    }
                    #[cfg(not(feature = "production"))]
                    return Err(FeatureHostError::ProductionUnavailable);
                }
                self.state()?.events.push_back(HostEvent::McpRefreshed {
                    timestamp: timestamp(),
                });
            }
            FeatureCommand::McpToolCall {
                server,
                tool,
                arguments,
                ..
            } => {
                let server = required(server, "MCP server")?;
                let tool = required(tool, "MCP tool")?;
                let result = if self.config.mode == HostMode::Production {
                    #[cfg(feature = "production")]
                    {
                        match self.runtime()?.execute(RuntimeCommand::McpToolCall {
                            server: server.clone(),
                            tool: tool.clone(),
                            arguments,
                        })? {
                            RuntimeResponse::McpToolResult { result, .. } => result,
                            other => return Err(unexpected_response("mcp.toolCall", other)),
                        }
                    }
                    #[cfg(not(feature = "production"))]
                    Value::Null
                } else {
                    json!({"ok": true, "mock": true})
                };
                let _ = self.append_action_audit(
                    "mahayana-assistant",
                    None,
                    json!({
                        "kind": "mcpToolCall",
                        "serverIdentifier": server,
                        "toolName": tool,
                        "transport": "runtime",
                        "status": "success",
                    }),
                );
                self.state()?.events.push_back(HostEvent::McpToolResult {
                    timestamp: timestamp(),
                    server,
                    tool,
                    result,
                });
            }
            _ => unreachable!("non-MCP command routed to MCP executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn execute_settings_and_audit(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        match command {
            FeatureCommand::SettingsGet { .. } => {
                let settings = self.state()?.settings.clone();
                self.state()?.events.push_back(HostEvent::SettingsChanged {
                    timestamp: timestamp(),
                    settings,
                });
            }
            FeatureCommand::SettingsUpdate { mut settings, .. } => {
                settings.auto_review_rules = sanitize_auto_review_rules(settings.auto_review_rules);
                {
                    let mut state = self.state()?;
                    ensure_open(&state)?;
                    state.settings = settings.clone();
                }
                if let Some(path) = self.settings_path.as_deref() {
                    persist_product_host_settings(path, &settings)?;
                }
                sync_computer_control_policy(&settings);
                if !settings.remote_control_enabled {
                    self.state()?.remote_computer_sessions.clear();
                }
                self.state()?.events.push_back(HostEvent::SettingsChanged {
                    timestamp: timestamp(),
                    settings,
                });
            }
            FeatureCommand::AuditList {
                agent_id, limit, ..
            } => {
                if !is_safe_memory_agent_id(&agent_id)
                    || !self.state()?.bots.contains_key(&agent_id)
                {
                    return Err(FeatureHostError::Contract(format!(
                        "unknown audit agent: {agent_id}"
                    )));
                }
                let records = self
                    .memory_root_path
                    .as_deref()
                    .map(|root| {
                        read_action_audit(
                            &root.join(&agent_id).join("audit.jsonl"),
                            limit.min(1000),
                        )
                    })
                    .unwrap_or_default();
                self.state()?.events.push_back(HostEvent::AuditListed {
                    timestamp: timestamp(),
                    agent_id,
                    records,
                });
            }
            _ => unreachable!("non-settings command routed to settings executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn append_action_audit(
        &self,
        agent_id: &str,
        turn_id: Option<&str>,
        action: Value,
    ) -> Result<(), FeatureHostError> {
        if !is_safe_memory_agent_id(agent_id) {
            return Ok(());
        }
        let Some(root) = self.active_account_root(self.memory_root_path.as_deref()) else {
            return Ok(());
        };
        let path = root.join(agent_id).join("audit.jsonl");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                FeatureHostError::Contract(format!("create action audit directory: {error}"))
            })?;
        }
        let record = json!({
            "ts": timestamp(),
            "agentId": agent_id,
            "eventId": format!("audit-{}-{}", now_millis(), std::process::id()),
            "turnId": turn_id.unwrap_or(""),
            "action": action,
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| FeatureHostError::Contract(format!("open action audit: {error}")))?;
        writeln!(file, "{record}")
            .map_err(|error| FeatureHostError::Contract(format!("append action audit: {error}")))
    }

    fn execute_product_surface(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        match self.config.mode {
            HostMode::Test => self.execute_product_surface_test(command),
            HostMode::Production => {
                #[cfg(feature = "production")]
                return self.execute_product_surface_production(command);
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }
    }

    fn execute_product_surface_test(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let mut state = self.state()?;
        ensure_open(&state)?;
        match command {
            FeatureCommand::ConnectorList { .. } => {
                let connectors = state.connectors.values().cloned().collect();
                state.events.push_back(HostEvent::ConnectorListed {
                    timestamp: timestamp(),
                    connectors,
                });
            }
            FeatureCommand::ConnectorConnect {
                connector_id,
                account_label,
                ..
            } => {
                let connector_id = required(connector_id, "connectorId")?;
                let account_id = next_id(&mut state, "account");
                let connector = state.connectors.get_mut(&connector_id).ok_or_else(|| {
                    FeatureHostError::Contract(format!("unknown connector: {connector_id}"))
                })?;
                connector.accounts.push(ConnectorAccountSummary {
                    id: account_id,
                    label: account_label
                        .clone()
                        .filter(|label| !label.trim().is_empty())
                        .unwrap_or_else(|| "Personal".into()),
                    status: ConnectorStatus::Connected,
                    email: None,
                    team_managed: Some(false),
                    error: None,
                });
                connector.status = ConnectorStatus::Connected;
                let connector = connector.clone();
                state.events.push_back(HostEvent::ConnectorChanged {
                    timestamp: timestamp(),
                    action: "connected".into(),
                    connector,
                });
                if let Some(platform) = listener_platform_for_connector(&connector_id) {
                    if let Some(integration) = state.listeners.get_mut(&platform) {
                        integration.is_connected = true;
                        integration.account_label = account_label;
                        let integration = integration.clone();
                        state.events.push_back(HostEvent::ListenerChanged {
                            timestamp: timestamp(),
                            integration,
                        });
                    }
                }
            }
            FeatureCommand::ConnectorRenameAccount {
                connector_id,
                account_id,
                label,
                ..
            } => {
                let label = required(label, "account label")?;
                let connector = state.connectors.get_mut(&connector_id).ok_or_else(|| {
                    FeatureHostError::Contract(format!("unknown connector: {connector_id}"))
                })?;
                let account = connector
                    .accounts
                    .iter_mut()
                    .find(|account| account.id == account_id)
                    .ok_or_else(|| {
                        FeatureHostError::Contract(format!(
                            "unknown connector account: {account_id}"
                        ))
                    })?;
                if account.team_managed == Some(true) {
                    return Err(FeatureHostError::Contract(
                        "team-managed accounts cannot be renamed".into(),
                    ));
                }
                account.label = label;
                let connector = connector.clone();
                state.events.push_back(HostEvent::ConnectorChanged {
                    timestamp: timestamp(),
                    action: "updated".into(),
                    connector,
                });
            }
            FeatureCommand::ConnectorRemoveAccount {
                connector_id,
                account_id,
                ..
            } => {
                let connector = state.connectors.get_mut(&connector_id).ok_or_else(|| {
                    FeatureHostError::Contract(format!("unknown connector: {connector_id}"))
                })?;
                if connector
                    .accounts
                    .iter()
                    .any(|account| account.id == account_id && account.team_managed == Some(true))
                {
                    return Err(FeatureHostError::Contract(
                        "team-managed accounts cannot be removed".into(),
                    ));
                }
                let before = connector.accounts.len();
                connector
                    .accounts
                    .retain(|account| account.id != account_id);
                if before == connector.accounts.len() {
                    return Err(FeatureHostError::Contract(format!(
                        "unknown connector account: {account_id}"
                    )));
                }
                if connector.accounts.is_empty() {
                    connector.status = ConnectorStatus::Disconnected;
                }
                let connector = connector.clone();
                state.events.push_back(HostEvent::ConnectorChanged {
                    timestamp: timestamp(),
                    action: "removed".into(),
                    connector,
                });
            }
            FeatureCommand::ConnectorSetToolEnabled {
                connector_id,
                tool_id,
                enabled,
                ..
            } => {
                let connector = state.connectors.get_mut(&connector_id).ok_or_else(|| {
                    FeatureHostError::Contract(format!("unknown connector: {connector_id}"))
                })?;
                let tool = connector
                    .tools
                    .iter_mut()
                    .find(|tool| tool.id == tool_id)
                    .ok_or_else(|| {
                        FeatureHostError::Contract(format!("unknown connector tool: {tool_id}"))
                    })?;
                tool.enabled = enabled;
                let connector = connector.clone();
                state.events.push_back(HostEvent::ConnectorChanged {
                    timestamp: timestamp(),
                    action: "toolChanged".into(),
                    connector,
                });
            }
            FeatureCommand::SkillList { agent_id, .. } => {
                let skills = state
                    .skills
                    .values()
                    .filter(|skill| {
                        agent_id
                            .as_ref()
                            .is_none_or(|agent_id| skill.owner_agent_id.as_ref() == Some(agent_id))
                    })
                    .cloned()
                    .collect();
                state.events.push_back(HostEvent::SkillListed {
                    timestamp: timestamp(),
                    skills,
                    teams: default_skill_teams(),
                });
            }
            FeatureCommand::SkillUpsert {
                id,
                name,
                description,
                use_when,
                instructions,
                owner_agent_id,
                ..
            } => {
                let name = required(name, "skill name")?;
                let use_when = required(use_when, "skill useWhen")?;
                let instructions = required(instructions, "skill instructions")?;
                let id = id.unwrap_or_else(|| next_id(&mut state, "skill"));
                let action = if state.skills.contains_key(&id) {
                    "updated"
                } else {
                    "created"
                };
                let previous = state.skills.get(&id);
                if previous.is_some_and(|skill| skill.read_only == Some(true)) {
                    return Err(FeatureHostError::Contract(
                        "managed skills cannot be edited".into(),
                    ));
                }
                let skill = SkillSummary {
                    id: id.clone(),
                    name,
                    description,
                    use_when,
                    instructions,
                    source: previous.map_or(SkillSource::Private, |skill| skill.source),
                    publish_state: previous
                        .map_or(SkillPublishState::Local, |skill| skill.publish_state),
                    owner_agent_id,
                    team_id: previous.and_then(|skill| skill.team_id.clone()),
                    team_name: previous.and_then(|skill| skill.team_name.clone()),
                    read_only: previous.and_then(|skill| skill.read_only),
                    updated_at_ms: now_millis(),
                };
                state.skills.insert(id, skill.clone());
                state.events.push_back(HostEvent::SkillChanged {
                    timestamp: timestamp(),
                    action: action.into(),
                    skill,
                });
            }
            FeatureCommand::SkillDelete { id, .. } => {
                let skill = state
                    .skills
                    .get(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown skill: {id}")))?;
                if skill.read_only == Some(true) || skill.source == SkillSource::Team {
                    return Err(FeatureHostError::Contract(
                        "team-managed skills cannot be deleted".into(),
                    ));
                }
                let skill = state.skills.remove(&id).expect("skill checked above");
                state.events.push_back(HostEvent::SkillChanged {
                    timestamp: timestamp(),
                    action: "deleted".into(),
                    skill,
                });
            }
            FeatureCommand::SkillPublish { id, team_id, .. } => {
                let team = default_skill_teams()
                    .into_iter()
                    .find(|team| team.id == team_id)
                    .ok_or_else(|| {
                        FeatureHostError::Contract(format!("unknown skill team: {team_id}"))
                    })?;
                let skill = state
                    .skills
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown skill: {id}")))?;
                if skill.description.trim().is_empty() {
                    return Err(FeatureHostError::Contract(
                        "add a skill description before publishing".into(),
                    ));
                }
                skill.source = SkillSource::Team;
                skill.publish_state = SkillPublishState::Published;
                skill.team_id = Some(team.id);
                skill.team_name = Some(team.name);
                skill.updated_at_ms = now_millis();
                let skill = skill.clone();
                state.events.push_back(HostEvent::SkillChanged {
                    timestamp: timestamp(),
                    action: "published".into(),
                    skill,
                });
            }
            FeatureCommand::SkillUnpublish { id, .. } => {
                let skill = state
                    .skills
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown skill: {id}")))?;
                if skill.publish_state == SkillPublishState::Managed {
                    return Err(FeatureHostError::Contract(
                        "managed skills cannot be unpublished".into(),
                    ));
                }
                skill.source = SkillSource::Private;
                skill.publish_state = SkillPublishState::Local;
                skill.team_id = None;
                skill.team_name = None;
                skill.updated_at_ms = now_millis();
                let skill = skill.clone();
                state.events.push_back(HostEvent::SkillChanged {
                    timestamp: timestamp(),
                    action: "unpublished".into(),
                    skill,
                });
            }
            FeatureCommand::SkillSync { id, .. } => {
                let skill = state
                    .skills
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown skill: {id}")))?;
                if skill.team_id.is_none() {
                    return Err(FeatureHostError::Contract(
                        "only published skills can be synced".into(),
                    ));
                }
                skill.publish_state = SkillPublishState::Synced;
                skill.updated_at_ms = now_millis();
                let skill = skill.clone();
                state.events.push_back(HostEvent::SkillChanged {
                    timestamp: timestamp(),
                    action: "synced".into(),
                    skill,
                });
            }
            FeatureCommand::BotList { .. } => {
                let bots = state.bots.values().cloned().collect();
                state.events.push_back(HostEvent::BotListed {
                    timestamp: timestamp(),
                    bots,
                });
            }
            FeatureCommand::BotSetHidden { id, hidden, .. } => {
                let bot = state
                    .bots
                    .get_mut(&id)
                    .ok_or_else(|| FeatureHostError::Contract(format!("unknown bot: {id}")))?;
                bot.hidden = hidden;
                let bot = bot.clone();
                state.events.push_back(HostEvent::BotChanged {
                    timestamp: timestamp(),
                    action: "updated".into(),
                    bot,
                });
            }
            FeatureCommand::DraftResolve { draft, action, .. } => {
                let draft_id = draft.id().to_string();
                match action {
                    DraftAction::Discard => {
                        state.events.push_back(HostEvent::DraftChanged {
                            timestamp: timestamp(),
                            draft_id,
                            status: DraftSendState::Discarded,
                            error: None,
                        });
                    }
                    DraftAction::Send => {
                        validate_draft(&draft)?;
                        state.events.push_back(HostEvent::DraftChanged {
                            timestamp: timestamp(),
                            draft_id: draft_id.clone(),
                            status: DraftSendState::Sending,
                            error: None,
                        });
                        state.events.push_back(HostEvent::DraftChanged {
                            timestamp: timestamp(),
                            draft_id,
                            status: DraftSendState::Sent,
                            error: None,
                        });
                    }
                }
            }
            FeatureCommand::SecretProvide {
                secret_request_id,
                value,
                ..
            } => {
                if value.is_empty() {
                    return Err(FeatureHostError::Contract(
                        "secret value must not be empty".into(),
                    ));
                }
                state.events.push_back(HostEvent::SecretProvided {
                    timestamp: timestamp(),
                    secret_request_id,
                });
            }
            FeatureCommand::ListenerList { .. } => {
                let integrations = state.listeners.values().cloned().collect();
                state.events.push_back(HostEvent::ListenerListed {
                    timestamp: timestamp(),
                    integrations,
                });
            }
            FeatureCommand::ListenerConnect { platform, .. } => {
                let integration = state.listeners.get_mut(&platform).ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "unsupported listener platform: {platform:?}"
                    ))
                })?;
                integration.is_connected = true;
                integration.error = None;
                let integration = integration.clone();
                state.events.push_back(HostEvent::ListenerChanged {
                    timestamp: timestamp(),
                    integration,
                });
                if let Some(connector_id) = connector_for_listener_platform(platform) {
                    if let Some(connector) = state.connectors.get_mut(connector_id) {
                        connector.status = ConnectorStatus::Connected;
                        let connector = connector.clone();
                        state.events.push_back(HostEvent::ConnectorChanged {
                            timestamp: timestamp(),
                            action: "connected".into(),
                            connector,
                        });
                    }
                }
                let agent_ids = take_pending_listener_resumes(&mut state, platform);
                drop(state);
                self.dispatch_listener_connection_resumes(platform, agent_ids)?;
                return Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                });
            }
            FeatureCommand::ListenerDisconnect { platform, .. } => {
                let integration = state.listeners.get_mut(&platform).ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "unsupported listener platform: {platform:?}"
                    ))
                })?;
                integration.is_connected = false;
                integration.account_label = None;
                integration.error = None;
                let integration = integration.clone();
                state.events.push_back(HostEvent::ListenerChanged {
                    timestamp: timestamp(),
                    integration,
                });
                if let Some(connector_id) = connector_for_listener_platform(platform) {
                    if let Some(connector) = state.connectors.get_mut(connector_id) {
                        connector.status = ConnectorStatus::Disconnected;
                        connector.accounts.clear();
                        let connector = connector.clone();
                        state.events.push_back(HostEvent::ConnectorChanged {
                            timestamp: timestamp(),
                            action: "disconnected".into(),
                            connector,
                        });
                    }
                }
                state
                    .pending_listener_resumes
                    .retain(|(_, pending_platform)| *pending_platform != platform);
            }
            FeatureCommand::UpdateStatus { .. } => {
                let update_state = state.update_state.clone();
                state.events.push_back(HostEvent::UpdateChanged {
                    timestamp: timestamp(),
                    state: update_state,
                });
            }
            FeatureCommand::UpdateCheck { .. } => {
                state.update_state = UpdateState::Checking;
                let checking = state.update_state.clone();
                state.events.push_back(HostEvent::UpdateChanged {
                    timestamp: timestamp(),
                    state: checking,
                });
                state.update_state = UpdateState::UpToDate {
                    version: env!("CARGO_PKG_VERSION").into(),
                };
                let update_state = state.update_state.clone();
                state.events.push_back(HostEvent::UpdateChanged {
                    timestamp: timestamp(),
                    state: update_state,
                });
            }
            FeatureCommand::UpdateInstall { .. } => {
                let version = match &state.update_state {
                    UpdateState::Available { version, .. }
                    | UpdateState::Ready { version }
                    | UpdateState::Downloading { version, .. }
                    | UpdateState::Staging { version } => version.clone(),
                    _ => {
                        return Err(FeatureHostError::Contract(
                            "no update is available to install".into(),
                        ));
                    }
                };
                state.update_state = UpdateState::Downloading {
                    version: version.clone(),
                    progress: Some(100),
                };
                let downloading = state.update_state.clone();
                state.events.push_back(HostEvent::UpdateChanged {
                    timestamp: timestamp(),
                    state: downloading,
                });
                state.update_state = UpdateState::Ready { version };
                let ready = state.update_state.clone();
                state.events.push_back(HostEvent::UpdateChanged {
                    timestamp: timestamp(),
                    state: ready,
                });
            }
            _ => unreachable!("non-product-surface command routed to product executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn production_live_connector_sources(
        &self,
    ) -> Result<(Vec<Value>, Vec<Value>), FeatureHostError> {
        let servers = match self.runtime()?.execute(RuntimeCommand::McpServers)? {
            RuntimeResponse::McpServers { data } => data,
            other => return Err(unexpected_response("mahayana.mcp.servers", other)),
        };
        let apps = match self.runtime()?.execute(RuntimeCommand::McpApps) {
            Ok(RuntimeResponse::McpApps { data }) => data,
            Ok(other) => return Err(unexpected_response("mahayana.mcp.apps", other)),
            // Connector directory discovery is feature-gated in some Codex
            // builds. Live MCP tool metadata is still authoritative for linked
            // connectors, so an unavailable directory should not hide them.
            Err(_) => Vec::new(),
        };
        Ok((servers, apps))
    }

    #[cfg(feature = "production")]
    fn production_connector_snapshot(
        &self,
    ) -> Result<
        (
            Vec<ConnectorSummary>,
            BTreeMap<String, LiveConnectorProjection>,
        ),
        FeatureHostError,
    > {
        let payload = json!({"type": "connector.list", "requestId": "connector-snapshot"});
        let response = self
            .runtime()?
            .product_execute("mahayana.connector.list", &payload)?;
        let base: Vec<ConnectorSummary> =
            decode_product_field(response, "connectors", "mahayana.connector.list")?;
        let (servers, apps) = self.production_live_connector_sources()?;
        let live = live_connector_projections(&servers, &apps);
        Ok((merge_live_connectors(base, &live), live))
    }

    #[cfg(feature = "production")]
    fn emit_connector_snapshot_change(
        &self,
        connector_id: &str,
        action: &str,
    ) -> Result<(), FeatureHostError> {
        let (connectors, _) = self.production_connector_snapshot()?;
        let connector = connectors
            .into_iter()
            .find(|connector| connector.id == connector_id)
            .ok_or_else(|| {
                FeatureHostError::Contract(format!("unknown connector: {connector_id}"))
            })?;
        self.state()?.events.push_back(HostEvent::ConnectorChanged {
            timestamp: timestamp(),
            action: action.into(),
            connector,
        });
        Ok(())
    }

    #[cfg(feature = "production")]
    fn execute_live_product_surface_production(
        &self,
        command: &FeatureCommand,
    ) -> Result<Option<CommandAccepted>, FeatureHostError> {
        let request_id = command.request_id().to_string();
        match command {
            FeatureCommand::ConnectorList { .. } => {
                let (connectors, _) = self.production_connector_snapshot()?;
                let connected_listener_platforms = connectors
                    .iter()
                    .filter(|connector| connector.status == ConnectorStatus::Connected)
                    .filter_map(|connector| listener_platform_for_connector(&connector.id))
                    .collect::<BTreeSet<_>>();
                self.state()?.events.push_back(HostEvent::ConnectorListed {
                    timestamp: timestamp(),
                    connectors,
                });
                for platform in connected_listener_platforms {
                    self.consume_listener_connection_resumes(platform)?;
                }
                Ok(Some(CommandAccepted {
                    request_id,
                    operation_id: None,
                }))
            }
            FeatureCommand::ConnectorConnect {
                connector_id,
                account_label: _,
                ..
            } => {
                if connector_id == "git" {
                    let payload = serde_json::to_value(command).map_err(|error| {
                        FeatureHostError::Contract(format!("encode connector.connect: {error}"))
                    })?;
                    self.runtime()?
                        .product_execute("mahayana.connector.connect", &payload)?;
                    self.emit_connector_snapshot_change(connector_id, "connected")?;
                    return Ok(Some(CommandAccepted {
                        request_id,
                        operation_id: None,
                    }));
                }
                let (_, live) = self.production_connector_snapshot()?;
                let projection = live.get(connector_id).ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "{connector_id} is not installed or discoverable in the Codex connector runtime; add its plugin from Plugins first"
                    ))
                })?;
                if let Some(url) = projection.install_url.clone() {
                    if !url.starts_with("https://") {
                        return Err(FeatureHostError::Contract(
                            "connector authorization URL must use HTTPS".into(),
                        ));
                    }
                    self.state()?
                        .events
                        .push_back(HostEvent::ConnectorOauthRequested {
                            timestamp: timestamp(),
                            connector_id: connector_id.clone(),
                            authorization_url: url,
                        });
                    return Ok(Some(CommandAccepted {
                        request_id,
                        operation_id: None,
                    }));
                }
                let server = projection.server_name.clone().ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "{connector_id} does not expose an OAuth-capable MCP server"
                    ))
                })?;
                if projection.status == Some(ConnectorStatus::Connected) {
                    self.emit_connector_snapshot_change(connector_id, "connected")?;
                    return Ok(Some(CommandAccepted {
                        request_id,
                        operation_id: None,
                    }));
                }
                let authorization_url =
                    match self.runtime()?.execute(RuntimeCommand::McpOauthLogin {
                        server: server.clone(),
                    })? {
                        RuntimeResponse::McpOauth {
                            authorization_url: Some(url),
                            ..
                        } => url,
                        other => {
                            return Err(unexpected_response("mahayana.mcp.oauth.login", other));
                        }
                    };
                self.state()?
                    .events
                    .push_back(HostEvent::ConnectorOauthRequested {
                        timestamp: timestamp(),
                        connector_id: connector_id.clone(),
                        authorization_url,
                    });
                Ok(Some(CommandAccepted {
                    request_id,
                    operation_id: None,
                }))
            }
            FeatureCommand::ConnectorRenameAccount { connector_id, .. }
            | FeatureCommand::ConnectorSetToolEnabled { connector_id, .. } => {
                let method = product_surface_method(command);
                let payload = serde_json::to_value(command).map_err(|error| {
                    FeatureHostError::Contract(format!("encode {method}: {error}"))
                })?;
                self.runtime()?.product_execute(method, &payload)?;
                self.emit_connector_snapshot_change(
                    connector_id,
                    if matches!(command, FeatureCommand::ConnectorSetToolEnabled { .. }) {
                        "toolChanged"
                    } else {
                        "updated"
                    },
                )?;
                Ok(Some(CommandAccepted {
                    request_id,
                    operation_id: None,
                }))
            }
            FeatureCommand::ConnectorRemoveAccount {
                connector_id,
                account_id,
                ..
            } => {
                let (_, live) = self.production_connector_snapshot()?;
                if let Some(projection) = live.get(connector_id) {
                    if projection.server_name.as_deref() == Some("codex_apps") {
                        if let Some(url) = projection.install_url.clone() {
                            self.state()?
                                .events
                                .push_back(HostEvent::ConnectorOauthRequested {
                                    timestamp: timestamp(),
                                    connector_id: connector_id.clone(),
                                    authorization_url: url,
                                });
                        }
                        return Err(FeatureHostError::Contract(
                            "this linked ChatGPT App account is server-managed; its account page was opened because the current Codex connector API does not expose unlink"
                                .into(),
                        ));
                    }
                    if let Some(server) = projection.server_name.clone() {
                        match self
                            .runtime()?
                            .execute(RuntimeCommand::McpOauthLogout { server })?
                        {
                            RuntimeResponse::McpOauth { .. } => {}
                            other => {
                                return Err(unexpected_response(
                                    "mahayana.mcp.oauth.logout",
                                    other,
                                ));
                            }
                        }
                    }
                }
                let method = product_surface_method(command);
                let payload = serde_json::to_value(command).map_err(|error| {
                    FeatureHostError::Contract(format!("encode {method}: {error}"))
                })?;
                self.runtime()?.product_execute(method, &payload)?;
                let _ = account_id;
                self.emit_connector_snapshot_change(connector_id, "removed")?;
                Ok(Some(CommandAccepted {
                    request_id,
                    operation_id: None,
                }))
            }
            FeatureCommand::DraftResolve { draft, action, .. } => {
                let draft_id = draft.id().to_string();
                if *action == DraftAction::Discard {
                    self.state()?.events.push_back(HostEvent::DraftChanged {
                        timestamp: timestamp(),
                        draft_id,
                        status: DraftSendState::Discarded,
                        error: None,
                    });
                    return Ok(Some(CommandAccepted {
                        request_id,
                        operation_id: None,
                    }));
                }
                validate_draft(draft)?;
                let connector_id = match draft {
                    MessageDraft::Email { .. } => "gmail",
                    MessageDraft::Slack { .. } => "slack",
                };
                let (_, live) = self.production_connector_snapshot()?;
                let projection = live.get(connector_id).ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "{connector_id} connector is not installed; install and authorize it before sending"
                    ))
                })?;
                if projection.status != Some(ConnectorStatus::Connected) {
                    return Err(FeatureHostError::Contract(format!(
                        "{connector_id} connector is not authorized"
                    )));
                }
                let server = projection.server_name.clone().ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "{connector_id} connector does not expose an MCP server"
                    ))
                })?;
                let (tool, schema) =
                    projection_send_tool(projection, connector_id).ok_or_else(|| {
                        FeatureHostError::Contract(format!(
                            "{connector_id} connector has no compatible send tool"
                        ))
                    })?;
                let arguments = draft_tool_arguments(draft, schema)?;
                self.state()?.events.push_back(HostEvent::DraftChanged {
                    timestamp: timestamp(),
                    draft_id: draft_id.clone(),
                    status: DraftSendState::Sending,
                    error: None,
                });
                let result = self.runtime()?.execute(RuntimeCommand::McpToolCall {
                    server,
                    tool: tool.to_string(),
                    arguments,
                });
                match result {
                    Ok(RuntimeResponse::McpToolResult { .. }) => {
                        self.state()?.events.push_back(HostEvent::DraftChanged {
                            timestamp: timestamp(),
                            draft_id,
                            status: DraftSendState::Sent,
                            error: None,
                        });
                        Ok(Some(CommandAccepted {
                            request_id,
                            operation_id: None,
                        }))
                    }
                    Ok(other) => Err(unexpected_response("mahayana.mcp.tool.call", other)),
                    Err(error) => {
                        let message = error.to_string();
                        self.state()?.events.push_back(HostEvent::DraftChanged {
                            timestamp: timestamp(),
                            draft_id,
                            status: DraftSendState::Failed,
                            error: Some(message.clone()),
                        });
                        Err(error.into())
                    }
                }
            }
            FeatureCommand::ListenerList { .. } => {
                let payload = serde_json::to_value(command).map_err(|error| {
                    FeatureHostError::Contract(format!("encode listener.list: {error}"))
                })?;
                let response = self
                    .runtime()?
                    .product_execute("mahayana.listener.list", &payload)?;
                let mut integrations: Vec<ListenerIntegrationSummary> =
                    decode_product_field(response, "integrations", "mahayana.listener.list")?;
                let (connectors, _) = self.production_connector_snapshot()?;
                for integration in &mut integrations {
                    if integration.platform == ListenerPlatform::Git {
                        continue;
                    }
                    if let Some(connector_id) =
                        connector_for_listener_platform(integration.platform)
                    {
                        if let Some(connector) = connectors
                            .iter()
                            .find(|connector| connector.id == connector_id)
                        {
                            integration.is_connected =
                                connector.status == ConnectorStatus::Connected;
                            integration.account_label = connector
                                .accounts
                                .first()
                                .map(|account| account.label.clone());
                        }
                    }
                }
                let connected_platforms = integrations
                    .iter()
                    .filter(|integration| integration.is_connected)
                    .map(|integration| integration.platform)
                    .collect::<BTreeSet<_>>();
                self.state()?.events.push_back(HostEvent::ListenerListed {
                    timestamp: timestamp(),
                    integrations,
                });
                for platform in connected_platforms {
                    self.consume_listener_connection_resumes(platform)?;
                }
                Ok(Some(CommandAccepted {
                    request_id,
                    operation_id: None,
                }))
            }
            FeatureCommand::ListenerConnect { platform, .. } => {
                if *platform == ListenerPlatform::Git {
                    let payload = serde_json::to_value(command).map_err(|error| {
                        FeatureHostError::Contract(format!("encode listener.connect: {error}"))
                    })?;
                    let response = self
                        .runtime()?
                        .product_execute("mahayana.listener.connect", &payload)?;
                    let integration =
                        decode_product_field(response, "integration", "mahayana.listener.connect")?;
                    self.state()?.events.push_back(HostEvent::ListenerChanged {
                        timestamp: timestamp(),
                        integration,
                    });
                    self.consume_listener_connection_resumes(*platform)?;
                    return Ok(Some(CommandAccepted {
                        request_id,
                        operation_id: None,
                    }));
                }
                let connector_id = connector_for_listener_platform(*platform).ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "no connector exists for listener platform {platform:?}"
                    ))
                })?;
                let synthetic = FeatureCommand::ConnectorConnect {
                    request_id: request_id.clone(),
                    connector_id: connector_id.into(),
                    account_label: None,
                };
                self.execute_live_product_surface_production(&synthetic)?;
                Ok(Some(CommandAccepted {
                    request_id,
                    operation_id: None,
                }))
            }
            FeatureCommand::ListenerDisconnect { platform, .. } => {
                if *platform != ListenerPlatform::Git {
                    let connector_id =
                        connector_for_listener_platform(*platform).ok_or_else(|| {
                            FeatureHostError::Contract(format!(
                                "no connector exists for listener platform {platform:?}"
                            ))
                        })?;
                    let (connectors, live) = self.production_connector_snapshot()?;
                    let accounts = connectors
                        .iter()
                        .find(|connector| connector.id == connector_id)
                        .map(|connector| connector.accounts.clone())
                        .unwrap_or_default();
                    if accounts.is_empty() {
                        if let Some(projection) = live.get(connector_id) {
                            if projection.status == Some(ConnectorStatus::Connected) {
                                if let Some(server) = projection.server_name.clone() {
                                    match self
                                        .runtime()?
                                        .execute(RuntimeCommand::McpOauthLogout { server })?
                                    {
                                        RuntimeResponse::McpOauth { .. } => {}
                                        other => {
                                            return Err(unexpected_response(
                                                "mahayana.mcp.oauth.logout",
                                                other,
                                            ));
                                        }
                                    }
                                    self.emit_connector_snapshot_change(connector_id, "removed")?;
                                }
                            }
                        }
                    } else {
                        for account in accounts {
                            let synthetic = FeatureCommand::ConnectorRemoveAccount {
                                request_id: format!("{request_id}:{}", account.id),
                                connector_id: connector_id.into(),
                                account_id: account.id,
                            };
                            self.execute_live_product_surface_production(&synthetic)?;
                        }
                    }
                }
                let payload = serde_json::to_value(command).map_err(|error| {
                    FeatureHostError::Contract(format!("encode listener.disconnect: {error}"))
                })?;
                let response = self
                    .runtime()?
                    .product_execute("mahayana.listener.disconnect", &payload)?;
                let integration =
                    decode_product_field(response, "integration", "mahayana.listener.disconnect")?;
                {
                    let mut state = self.state()?;
                    state.events.push_back(HostEvent::ListenerChanged {
                        timestamp: timestamp(),
                        integration,
                    });
                    state
                        .pending_listener_resumes
                        .retain(|(_, pending_platform)| *pending_platform != *platform);
                }
                Ok(Some(CommandAccepted {
                    request_id,
                    operation_id: None,
                }))
            }
            _ => Ok(None),
        }
    }

    #[cfg(feature = "production")]
    fn execute_product_surface_production(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        if let Some(accepted) = self.execute_live_product_surface_production(&command)? {
            return Ok(accepted);
        }
        let request_id = command.request_id().to_string();
        let method = product_surface_method(&command);
        let payload = serde_json::to_value(&command).map_err(|error| {
            FeatureHostError::Contract(format!("encode {method} payload: {error}"))
        })?;

        if let FeatureCommand::DraftResolve {
            draft,
            action: DraftAction::Send,
            ..
        } = &command
        {
            validate_draft(draft)?;
            self.state()?.events.push_back(HostEvent::DraftChanged {
                timestamp: timestamp(),
                draft_id: draft.id().to_string(),
                status: DraftSendState::Sending,
                error: None,
            });
        }

        let response = self
            .runtime()?
            .product_execute(method, &payload)
            .map_err(FeatureHostError::from)?;
        let mut state = self.state()?;
        match command {
            FeatureCommand::ConnectorList { .. } => {
                let connectors = decode_product_field(response, "connectors", method)?;
                state.events.push_back(HostEvent::ConnectorListed {
                    timestamp: timestamp(),
                    connectors,
                });
            }
            FeatureCommand::ConnectorConnect { connector_id, .. } => {
                if let Some(url) = response
                    .get("authorizationUrl")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                {
                    state.events.push_back(HostEvent::ConnectorOauthRequested {
                        timestamp: timestamp(),
                        connector_id,
                        authorization_url: url,
                    });
                }
                if response.get("connector").is_some() || response.get("id").is_some() {
                    let connector = decode_product_field(response, "connector", method)?;
                    state.events.push_back(HostEvent::ConnectorChanged {
                        timestamp: timestamp(),
                        action: "connected".into(),
                        connector,
                    });
                }
            }
            FeatureCommand::ConnectorRenameAccount { .. }
            | FeatureCommand::ConnectorRemoveAccount { .. }
            | FeatureCommand::ConnectorSetToolEnabled { .. } => {
                let action = match command {
                    FeatureCommand::ConnectorRemoveAccount { .. } => "removed",
                    FeatureCommand::ConnectorSetToolEnabled { .. } => "toolChanged",
                    _ => "updated",
                };
                let connector = decode_product_field(response, "connector", method)?;
                state.events.push_back(HostEvent::ConnectorChanged {
                    timestamp: timestamp(),
                    action: action.into(),
                    connector,
                });
            }
            FeatureCommand::SkillList { .. } => {
                let skills = decode_product_field(response.clone(), "skills", method)?;
                let teams = response
                    .get("teams")
                    .cloned()
                    .map(|value| decode_product_value(value, method))
                    .transpose()?
                    .unwrap_or_default();
                state.events.push_back(HostEvent::SkillListed {
                    timestamp: timestamp(),
                    skills,
                    teams,
                });
            }
            FeatureCommand::SkillUpsert { .. }
            | FeatureCommand::SkillDelete { .. }
            | FeatureCommand::SkillPublish { .. }
            | FeatureCommand::SkillUnpublish { .. }
            | FeatureCommand::SkillSync { .. } => {
                let action = match command {
                    FeatureCommand::SkillDelete { .. } => "deleted".to_string(),
                    FeatureCommand::SkillPublish { .. } => "published".to_string(),
                    FeatureCommand::SkillUnpublish { .. } => "unpublished".to_string(),
                    FeatureCommand::SkillSync { .. } => "synced".to_string(),
                    _ => response
                        .get("action")
                        .and_then(Value::as_str)
                        .unwrap_or("updated")
                        .to_string(),
                };
                let skill = decode_product_field(response, "skill", method)?;
                state.events.push_back(HostEvent::SkillChanged {
                    timestamp: timestamp(),
                    action,
                    skill,
                });
            }
            FeatureCommand::BotList { .. } => {
                let mut bots: Vec<BotSummary> = decode_product_field(response, "bots", method)?;
                let mut known = bots
                    .iter()
                    .map(|bot| bot.id.clone())
                    .collect::<BTreeSet<_>>();
                for bot in state.bots.values() {
                    if known.insert(bot.id.clone()) {
                        bots.push(bot.clone());
                    }
                }
                state.events.push_back(HostEvent::BotListed {
                    timestamp: timestamp(),
                    bots,
                });
            }
            FeatureCommand::BotSetHidden { .. } => {
                let bot = decode_product_field(response, "bot", method)?;
                state.events.push_back(HostEvent::BotChanged {
                    timestamp: timestamp(),
                    action: "updated".into(),
                    bot,
                });
            }
            FeatureCommand::DraftResolve { draft, action, .. } => {
                let status = response
                    .get("status")
                    .cloned()
                    .map(|value| decode_product_value(value, method))
                    .transpose()?
                    .unwrap_or(match action {
                        DraftAction::Send => DraftSendState::Sent,
                        DraftAction::Discard => DraftSendState::Discarded,
                    });
                state.events.push_back(HostEvent::DraftChanged {
                    timestamp: timestamp(),
                    draft_id: draft.id().to_string(),
                    status,
                    error: response
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                });
            }
            FeatureCommand::SecretProvide {
                secret_request_id, ..
            } => {
                state.events.push_back(HostEvent::SecretProvided {
                    timestamp: timestamp(),
                    secret_request_id,
                });
            }
            FeatureCommand::ListenerList { .. } => {
                let integrations = decode_product_field(response, "integrations", method)?;
                state.events.push_back(HostEvent::ListenerListed {
                    timestamp: timestamp(),
                    integrations,
                });
            }
            FeatureCommand::ListenerConnect { .. } | FeatureCommand::ListenerDisconnect { .. } => {
                let integration = decode_product_field(response, "integration", method)?;
                state.events.push_back(HostEvent::ListenerChanged {
                    timestamp: timestamp(),
                    integration,
                });
            }
            FeatureCommand::UpdateStatus { .. }
            | FeatureCommand::UpdateCheck { .. }
            | FeatureCommand::UpdateInstall { .. } => {
                let update_state = decode_product_field(response, "state", method)?;
                state.events.push_back(HostEvent::UpdateChanged {
                    timestamp: timestamp(),
                    state: update_state,
                });
            }
            _ => unreachable!("non-product-surface command routed to product executor"),
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    fn listener_connected_for_automation_write(
        &self,
        platform: ListenerPlatform,
    ) -> Option<bool> {
        match self.config.mode {
            HostMode::Test => self
                .state()
                .ok()?
                .listeners
                .get(&platform)
                .map(|integration| integration.is_connected),
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let connector_id = connector_for_listener_platform(platform)?;
                    let (connectors, _) = self.production_connector_snapshot().ok()?;
                    connectors
                        .iter()
                        .find(|connector| connector.id == connector_id)
                        .map(|connector| connector.status == ConnectorStatus::Connected)
                }
                #[cfg(not(feature = "production"))]
                {
                    let _ = platform;
                    None
                }
            }
        }
    }

    fn arm_listener_resume_after_automation_write(
        &self,
        automation: &AutomationSummary,
    ) -> Result<(), FeatureHostError> {
        let keys = automation_listener_resume_keys(automation);
        let mut state = self.state()?;
        prune_pending_listener_resumes(&mut state);
        drop(state);

        for (agent_id, platform) in keys {
            let Some(is_connected) = self.listener_connected_for_automation_write(platform) else {
                // Match Desktop fail-soft listener lookup: an unavailable connection
                // projection must not block or mutate a successfully persisted routine.
                continue;
            };
            let mut state = self.state()?;
            if is_connected {
                state.pending_listener_resumes.remove(&(agent_id, platform));
                continue;
            }
            if !state
                .pending_listener_resumes
                .insert((agent_id.clone(), platform))
            {
                continue;
            }
            state.events.push_back(HostEvent::TranscriptCard {
                timestamp: timestamp(),
                entry_id: format!(
                    "listener-connect:{agent_id}:{}",
                    listener_platform_slug(platform)
                ),
                operation_id: None,
                card: TranscriptCard::ListenerConnect {
                    platform,
                    reason: Some("so this routine can fire".into()),
                    connected: false,
                    pending: Some(true),
                },
            });
        }
        Ok(())
    }

    fn dispatch_listener_connection_resumes(
        &self,
        platform: ListenerPlatform,
        agent_ids: Vec<String>,
    ) -> Result<(), FeatureHostError> {
        for agent_id in agent_ids {
            match self.config.mode {
                HostMode::Test => {
                    self.state()?.events.push_back(HostEvent::TransportEvent {
                        channel: "listener-resume".into(),
                        payload: json!({
                            "agentId": agent_id,
                            "platform": listener_platform_slug(platform),
                            "hidden": true,
                        }),
                    });
                }
                HostMode::Production => {
                    #[cfg(feature = "production")]
                    {
                        self.dispatch_hidden_listener_resume(&agent_id, platform)?;
                    }
                    #[cfg(not(feature = "production"))]
                    return Err(FeatureHostError::ProductionUnavailable);
                }
            }
        }
        Ok(())
    }

    fn consume_listener_connection_resumes(
        &self,
        platform: ListenerPlatform,
    ) -> Result<(), FeatureHostError> {
        let agent_ids = {
            let mut state = self.state()?;
            take_pending_listener_resumes(&mut state, platform)
        };
        self.dispatch_listener_connection_resumes(platform, agent_ids)
    }

    fn poll_pending_listener_connection_resumes(&self) -> Result<(), FeatureHostError> {
        let pending_platforms = {
            let state = self.state()?;
            state
                .pending_listener_resumes
                .iter()
                .map(|(_, platform)| *platform)
                .collect::<BTreeSet<_>>()
        };
        if pending_platforms.is_empty() {
            return Ok(());
        }

        match self.config.mode {
            HostMode::Test => {
                let connected = {
                    let state = self.state()?;
                    pending_platforms
                        .into_iter()
                        .filter(|platform| {
                            state
                                .listeners
                                .get(platform)
                                .is_some_and(|integration| integration.is_connected)
                        })
                        .collect::<Vec<_>>()
                };
                for platform in connected {
                    self.consume_listener_connection_resumes(platform)?;
                }
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let Ok((connectors, _)) = self.production_connector_snapshot() else {
                        return Ok(());
                    };
                    let connected = pending_platforms
                        .into_iter()
                        .filter(|platform| {
                            connector_for_listener_platform(*platform)
                                .and_then(|connector_id| {
                                    connectors
                                        .iter()
                                        .find(|connector| connector.id == connector_id)
                                })
                                .is_some_and(|connector| {
                                    connector.status == ConnectorStatus::Connected
                                })
                        })
                        .collect::<Vec<_>>();
                    for platform in connected {
                        self.consume_listener_connection_resumes(platform)?;
                    }
                }
                #[cfg(not(feature = "production"))]
                {
                    return Err(FeatureHostError::ProductionUnavailable);
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "production")]
    fn dispatch_hidden_listener_resume(
        &self,
        agent_id: &str,
        platform: ListenerPlatform,
    ) -> Result<(), FeatureHostError> {
        self.require_authenticated_account()?;
        let conversation_id = if agent_id == "mahayana-assistant" {
            ConversationId(MAHAYANA_AI_CONVERSATION_ID.to_string())
        } else {
            let state = self.state()?;
            ConversationId(
                state
                    .bots
                    .get(agent_id)
                    .and_then(|bot| bot.conversation_id.clone())
                    .ok_or_else(|| {
                        FeatureHostError::Contract(format!(
                            "listener resume owner no longer exists: {agent_id}"
                        ))
                    })?,
            )
        };
        let (provider, routed_model) = match self.runtime()?.execute(RuntimeCommand::Status)? {
            RuntimeResponse::Status(status) => (
                format!("{:?}", status.model_provider).to_lowercase(),
                status.model,
            ),
            other => return Err(unexpected_response("runtime.status", other)),
        };
        let request_id = format!(
            "listener-resume:{agent_id}:{}:{}",
            listener_platform_slug(platform),
            now_millis()
        );
        let prompt = format!(
            "[MAHAYANA_HIDDEN_CONTEXT] {} is now connected. Continue the interrupted routine setup that was waiting for this listener. Do not fire the saved automation merely because the connection completed.",
            listener_platform_display(platform)
        );
        let response = self.runtime()?.execute(RuntimeCommand::SendMessage {
            conversation_id,
            text: prompt,
            display_text: None,
            client_message_id: Some(request_id),
            hidden: true,
            show_assistant_output: false,
            recovery_eligible: true,
            reply_to_message_id: None,
            is_fork: false,
            attachment_batch_id: None,
            selected_image_data_urls: Vec::new(),
        })?;
        let operation_id = match response {
            RuntimeResponse::Accepted { operation_id } => operation_id.to_string(),
            other => return Err(unexpected_response("listener.resume", other)),
        };
        let mut state = self.state()?;
        state.operations.insert(operation_id.clone());
        state
            .operation_agents
            .insert(operation_id.clone(), agent_id.to_string());
        state.events.push_back(HostEvent::OperationStarted {
            timestamp: timestamp(),
            operation_id: operation_id.clone(),
            label: "listener-connect-resume".into(),
            interruptible: true,
        });
        state.events.push_back(HostEvent::ModelRouted {
            timestamp: timestamp(),
            operation_id,
            provider,
            model: routed_model,
            mode: AgentMode::Agent,
        });
        Ok(())
    }

    /// Delivers a verified listener event into the automation engine. This is
    /// intentionally not a renderer command: only the backend relay/native
    /// listener bridge may call it, so web content cannot forge trigger events.
    pub fn ingest_listener_event(&self, event: EventCard) -> Result<usize, FeatureHostError> {
        let serialized = serde_json::to_string(&event).map_err(|error| {
            FeatureHostError::Contract(format!("encode listener event: {error}"))
        })?;
        if serialized.len() > 64 * 1024 {
            return Err(FeatureHostError::Contract("listener event exceeds 64 KiB".into()));
        }
        let matching_ids = {
            let state = self.state()?;
            ensure_open(&state)?;
            state.automations.values().filter(|automation| {
                automation.enabled && automation.trigger.as_ref().is_some_and(|trigger| {
                    automation_trigger_matches_event(trigger, &event, &serialized)
                })
            }).map(|automation| automation.id.clone()).collect::<Vec<_>>()
        };
        for id in &matching_ids {
            self.execute_routine(
                format!("listener-{}", Uuid::new_v4()), id.clone(), None,
                RoutineTrigger::Event(event.clone()),
            )?;
        }
        Ok(matching_ids.len())
    }

    pub fn register_awaited_operation(&self, operation_id: &str) -> Result<(), FeatureHostError> {
        let operation_id = operation_id.trim();
        if operation_id.is_empty() {
            return Err(FeatureHostError::Contract(
                "operationId is required for terminal settlement".into(),
            ));
        }
        let mut state = self.state()?;
        if !state.operations.contains(operation_id) {
            return Err(FeatureHostError::Contract(format!(
                "operation is not active: {operation_id}"
            )));
        }
        state.awaited_operations.insert(operation_id.to_string());
        Ok(())
    }

    pub fn cancel_awaited_operation(&self, operation_id: &str) -> Result<(), FeatureHostError> {
        let mut state = self.state()?;
        state.awaited_operations.remove(operation_id);
        state.operation_terminals.remove(operation_id);
        Ok(())
    }

    pub fn await_operation_step(
        &self,
        operation_id: &str,
        timeout: Duration,
    ) -> Result<Value, FeatureHostError> {
        let operation_id = operation_id.trim();
        if operation_id.is_empty() {
            return Err(FeatureHostError::Contract(
                "operationId is required for terminal settlement".into(),
            ));
        }

        {
            let mut state = self.state()?;
            if let Some(terminal) = state.operation_terminals.remove(operation_id) {
                state.awaited_operations.remove(operation_id);
                return Ok(terminal);
            }
            if !state.awaited_operations.contains(operation_id) {
                return Err(FeatureHostError::Contract(format!(
                    "operation is not registered for terminal settlement: {operation_id}"
                )));
            }
        }

        match self.config.mode {
            HostMode::Test => {}
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    if let Some(event) = self.receive_production(timeout)? {
                        // Awaiting a terminal request must never steal the renderer-visible
                        // event stream. Re-enqueue the translated event after using the
                        // Runtime receive path as the canonical event pump.
                        self.state()?.events.push_back(event);
                    }
                }
                #[cfg(not(feature = "production"))]
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }

        let mut state = self.state()?;
        if let Some(terminal) = state.operation_terminals.remove(operation_id) {
            state.awaited_operations.remove(operation_id);
            Ok(terminal)
        } else {
            Ok(json!({"status": "pending"}))
        }
    }

    fn advance_deferred_conversation_activation(
        &self,
    ) -> Result<Option<HostEvent>, FeatureHostError> {
        let claim = {
            let mut state = self.state()?;
            state.conversation_session.claim_deferred_activation()
        };
        let Some(claim) = claim else {
            return Ok(None);
        };

        match self.config.mode {
            HostMode::Test => {
                let mut state = self.state()?;
                let previous = state
                    .conversation_session
                    .activate_claimed(&claim.conversation_id);
                state.events.push_back(HostEvent::ConversationActivated {
                    timestamp: timestamp(),
                    conversation_id: claim.conversation_id,
                    previous_conversation_id: previous,
                    generation: Some(claim.generation),
                });
                Ok(state.events.pop_front())
            }
            HostMode::Production => {
                #[cfg(feature = "production")]
                {
                    let catch_up = match self.production_read_conversation_window(
                        &claim.conversation_id,
                        None,
                        claim.shipped_through_id.as_deref(),
                        None,
                    ) {
                        Ok(messages) => messages,
                        Err(error) => {
                            return Ok(Some(HostEvent::ConversationActivationFailed {
                                timestamp: timestamp(),
                                conversation_id: claim.conversation_id,
                                generation: claim.generation,
                                message: error.to_string(),
                            }));
                        }
                    };
                    {
                        let mut state = self.state()?;
                        let previous = state
                            .conversation_session
                            .activate_claimed(&claim.conversation_id);
                        if !catch_up.is_empty() {
                            state.events.push_back(HostEvent::ConversationAppended {
                                timestamp: timestamp(),
                                conversation_id: claim.conversation_id.clone(),
                                messages: catch_up,
                            });
                        }
                        state.events.push_back(HostEvent::ConversationActivated {
                            timestamp: timestamp(),
                            conversation_id: claim.conversation_id.clone(),
                            previous_conversation_id: previous,
                            generation: Some(claim.generation),
                        });
                    }
                    let _ = self.production_list_conversations_from_runtime(
                        format!("session-activation-roster-{}", claim.generation),
                        None,
                    );
                    Ok(self.state()?.events.pop_front())
                }
                #[cfg(not(feature = "production"))]
                {
                    Err(FeatureHostError::ProductionUnavailable)
                }
            }
        }
    }

    pub fn receive(&self) -> Result<Option<HostEvent>, FeatureHostError> {
        self.receive_with_timeout(Duration::ZERO)
    }

    pub fn receive_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<HostEvent>, FeatureHostError> {
        self.advance_pending_routine()?;
        self.fire_due_automation()?;
        if let Some(event) = self.state()?.events.pop_front() {
            return Ok(Some(event));
        }
        if let Some(event) = self.advance_deferred_conversation_activation()? {
            return Ok(Some(event));
        }
        self.poll_pending_listener_connection_resumes()?;
        if let Some(event) = self.state()?.events.pop_front() {
            return Ok(Some(event));
        }
        match self.config.mode {
            HostMode::Test => Ok(None),
            HostMode::Production => self.receive_production(timeout),
        }
    }

    fn fire_due_automation(&self) -> Result<(), FeatureHostError> {
        let now = now_millis();
        let due = {
            let state = self.state()?;
            if state.closed || state.routine_quiescing
                || (self.config.mode == HostMode::Production && !state.session_active) {
                return Ok(());
            }
            state.automations.values().find(|automation| {
                automation.enabled && automation.next_run_at_ms.is_some_and(|next| next <= now)
            }).map(|automation| (automation.id.clone(), automation.agent_id.clone()))
        };
        if let Some((id, agent_id)) = due {
            self.execute_routine(format!("scheduled-{id}-{now}"), id, agent_id, RoutineTrigger::Schedule)?;
        }
        Ok(())
    }

    fn persist_test_auth_user(&self, user: Option<&Value>) -> Result<(), FeatureHostError> {
        let Some(path) = self.test_auth_state_path.as_deref() else {
            return Ok(());
        };
        persist_test_auth_user(path, user)
    }

    fn persist_automations(
        &self,
        automations: &BTreeMap<String, AutomationSummary>,
    ) -> Result<(), FeatureHostError> {
        let Some(path) = self.active_account_root(self.automation_path.as_deref()) else {
            return Ok(());
        };
        persist_automations(&path, automations)
    }

    fn persist_bots(&self, bots: &BTreeMap<String, BotSummary>) -> Result<(), FeatureHostError> {
        let Some(path) = self.active_account_root(self.bot_state_path.as_deref()) else {
            return Ok(());
        };
        persist_bots(&path, bots)
    }

    fn persist_groups(
        &self,
        groups: &BTreeMap<String, GroupSummary>,
    ) -> Result<(), FeatureHostError> {
        let Some(path) = self.active_account_root(self.group_state_path.as_deref()) else {
            return Ok(());
        };
        persist_groups(&path, groups)
    }

    fn persist_peer_messages(&self, messages: &[AgentPeerMessage]) -> Result<(), FeatureHostError> {
        let Some(path) = self.active_account_root(self.peer_messages_path.as_deref()) else {
            return Ok(());
        };
        persist_peer_messages(&path, messages)
    }

    fn active_account_root(&self, base: Option<&Path>) -> Option<PathBuf> {
        let base = base?;
        #[cfg(feature = "production")]
        {
            // The production feature is also enabled by the cross-platform
            // test harness, while HostMode::Test deliberately has no live
            // MahayanaHost or product account. Keep that harness on its
            // isolated temporary root; real production hosts use the
            // account-scoped branch below.
            if self.config.mode == HostMode::Test {
                Some(base.to_path_buf())
            } else {
                let account_id = self
                    .active_account_id
                    .lock()
                    .ok()
                    .and_then(|account| account.clone())?;
                Some(account_scoped_path(base, &account_id))
            }
        }
        #[cfg(not(feature = "production"))]
        {
            Some(base.to_path_buf())
        }
    }

    #[cfg(feature = "production")]
    fn messaging_root_for(
        &self,
        envelope: &MessagingClientEnvelope,
    ) -> Result<PathBuf, FeatureHostError> {
        let auth_status = self.auth_status()?;
        let auth = auth_payload(&auth_status);
        if auth.get("loggedIn").and_then(Value::as_bool) != Some(true) {
            return Err(FeatureHostError::Contract(
                "messaging commands require an authenticated Fabushi account session".into(),
            ));
        }
        let account_id = auth_account_id(auth).ok_or_else(|| {
            FeatureHostError::Contract("authenticated account has no stable user id".into())
        })?;
        let expected_actor = actor_id_for_account_id(&account_id);
        if envelope.context.actor_id != expected_actor {
            return Err(FeatureHostError::Contract(
                "messaging actor does not match the authenticated Fabushi account".into(),
            ));
        }
        let base = self
            .memory_root_path
            .as_deref()
            .ok_or_else(|| FeatureHostError::Contract("messaging storage is unavailable".into()))?;
        Ok(account_scoped_path(base, &account_id))
    }

    #[cfg(not(feature = "production"))]
    fn messaging_root_for(
        &self,
        _envelope: &MessagingClientEnvelope,
    ) -> Result<PathBuf, FeatureHostError> {
        self.memory_root_path
            .clone()
            .ok_or_else(|| FeatureHostError::Contract("messaging storage is unavailable".into()))
    }


    fn channel_account_id(&self) -> Result<String, FeatureHostError> {
        if self.config.mode == HostMode::Test {
            return Ok(format!("test:{}", self.config.profile_id));
        }
        #[cfg(feature = "production")]
        {
            self.active_account_id
                .lock()
                .map_err(|_| FeatureHostError::StatePoisoned)?
                .clone()
                .ok_or_else(|| {
                    FeatureHostError::Contract(
                        "channel management requires an authenticated Fabushi account".into(),
                    )
                })
        }
        #[cfg(not(feature = "production"))]
        {
            Err(FeatureHostError::ProductionUnavailable)
        }
    }

    fn require_channel_agent(&self, agent_id: &str) -> Result<String, FeatureHostError> {
        let agent_id = required(agent_id.to_string(), "channel agent id")?;
        let state = self.state()?;
        ensure_open(&state)?;
        if !state.bots.contains_key(&agent_id) {
            return Err(FeatureHostError::Contract(format!(
                "unknown bot: {agent_id}"
            )));
        }
        Ok(agent_id)
    }

    /// Return only UI-safe channel metadata. Connector credentials stay in the
    /// encrypted Rust-owned channel secret store and are never serialized.
    pub fn agent_channels(&self, agent_id: &str) -> Result<Value, FeatureHostError> {
        let agent_id = self.require_channel_agent(agent_id)?;
        let account_id = self.channel_account_id()?;
        let connections = self
            .channel_store
            .list_connections(&account_id, &agent_id)
            .map_err(FeatureHostError::Contract)?;
        serde_json::to_value(connections)
            .map_err(|error| FeatureHostError::Contract(format!("encode channel connections: {error}")))
    }

    pub fn connect_agent_channel(
        &self,
        agent_id: &str,
        platform: &str,
        token: &str,
    ) -> Result<Value, FeatureHostError> {
        let agent_id = self.require_channel_agent(agent_id)?;
        let platform = required(platform.to_string(), "channel platform")?;
        if token.trim().is_empty() {
            return Err(FeatureHostError::Contract(
                "channel credential must not be empty".into(),
            ));
        }
        let account_id = self.channel_account_id()?;
        let connections = self
            .channel_store
            .connect(&account_id, &agent_id, &platform, token)
            .map_err(FeatureHostError::Contract)?;
        serde_json::to_value(connections)
            .map_err(|error| FeatureHostError::Contract(format!("encode channel connections: {error}")))
    }

    pub fn disconnect_agent_channel(
        &self,
        agent_id: &str,
        platform: &str,
    ) -> Result<Value, FeatureHostError> {
        let agent_id = self.require_channel_agent(agent_id)?;
        let platform = required(platform.to_string(), "channel platform")?;
        let account_id = self.channel_account_id()?;
        let connections = self
            .channel_store
            .disconnect(&account_id, &agent_id, &platform)
            .map_err(FeatureHostError::Contract)?;
        serde_json::to_value(connections)
            .map_err(|error| FeatureHostError::Contract(format!("encode channel connections: {error}")))
    }

    pub fn refresh_agent_channel(
        &self,
        agent_id: &str,
        platform: &str,
    ) -> Result<Value, FeatureHostError> {
        let agent_id = self.require_channel_agent(agent_id)?;
        let platform = required(platform.to_string(), "channel platform")?;
        let account_id = self.channel_account_id()?;
        let connections = self
            .channel_store
            .refresh(&account_id, &agent_id, &platform)
            .map_err(FeatureHostError::Contract)?;
        serde_json::to_value(connections)
            .map_err(|error| FeatureHostError::Contract(format!("encode channel connections: {error}")))
    }

    #[cfg(feature = "production")]
    fn ensure_account_boundary(&self, response: &Value) -> Result<(), FeatureHostError> {
        let _routine_gate = self.routine_dispatch_lock.lock()
            .map_err(|_| FeatureHostError::StatePoisoned)?;
        let auth = auth_payload(response);
        let logged_in = auth.get("loggedIn").and_then(Value::as_bool) == Some(true);
        let next_account_id = logged_in.then(|| auth_account_id(auth)).flatten();
        let initialized = *self
            .account_boundary_initialized
            .lock()
            .map_err(|_| FeatureHostError::StatePoisoned)?;
        let (previous_account_id, changed, reset_runtime) = {
            let active = self
                .active_account_id
                .lock()
                .map_err(|_| FeatureHostError::StatePoisoned)?;
            (
                active.clone(),
                *active != next_account_id,
                account_boundary_requires_runtime_reset(initialized, &active, &next_account_id),
            )
        };
        if changed {
            if reset_runtime {
                self.retire_routines_for_account_change()?;
                self.retire_background_recoveries_for_account_change()?;
                self.runtime()?.reset_session()?;
            }
            let account_id = next_account_id.as_deref();
            let automations = self
                .automation_path
                .as_deref()
                .map(|path| {
                    account_id
                        .map(|id| load_automations(&account_scoped_path(path, id)))
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let mut bots = default_bots();
            if let (Some(path), Some(account_id)) = (self.bot_state_path.as_deref(), account_id) {
                bots.extend(load_bots(&account_scoped_path(path, account_id)));
            }
            let groups = self
                .group_state_path
                .as_deref()
                .map(|path| {
                    account_id
                        .map(|id| load_groups(&account_scoped_path(path, id)))
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let peer_messages = self
                .peer_messages_path
                .as_deref()
                .map(|path| {
                    account_id
                        .map(|id| load_peer_messages(&account_scoped_path(path, id)))
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let remote_device_secrets = self
                .remote_device_state_path
                .as_deref()
                .map(|path| {
                    account_id
                        .map(|id| {
                            load_remote_computer_device_secrets(&account_scoped_path(path, id))
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let next_async_tasks_path =
                self.async_tasks_path_for_account(next_account_id.as_deref());
            let (restored_async_tasks, restored_async_task_operation_ids) = if initialized {
                (BTreeMap::new(), BTreeMap::new())
            } else {
                next_async_tasks_path
                    .as_deref()
                    .map(|path| load_pending_async_tasks(path, now_millis()))
                    .unwrap_or_default()
            };
            if initialized {
                let previous_async_tasks_path =
                    self.async_tasks_path_for_account(previous_account_id.as_deref());
                persist_pending_async_tasks(
                    previous_async_tasks_path.as_deref(),
                    &BTreeMap::new(),
                    &BTreeMap::new(),
                )?;
                if next_async_tasks_path != previous_async_tasks_path {
                    persist_pending_async_tasks(
                        next_async_tasks_path.as_deref(),
                        &BTreeMap::new(),
                        &BTreeMap::new(),
                    )?;
                }
            } else {
                persist_pending_async_tasks(
                    next_async_tasks_path.as_deref(),
                    &restored_async_tasks,
                    &restored_async_task_operation_ids,
                )?;
            }
            let mut state = self.state()?;
            state.events.clear();
            state.conversation_session = ConversationSessionState::default();
            state.pending_approvals.clear();
            state.operations.clear();
            state.operation_agents.clear();
            state.automation_operations.clear();
            state.routine_executions.clear();
            state.routine_epoch = state.routine_epoch.wrapping_add(1).max(1);
            state.awaited_operations.clear();
            state.operation_terminals.clear();
            state.background_operations.clear();
            state.background_recoveries.clear();
            state.async_tasks = restored_async_tasks;
            state.async_task_operation_ids = restored_async_task_operation_ids;
            state.subagents.clear();
            state.automations = automations;
            state.published_plugins_by_agent.clear();
            state.bots = bots;
            state.peer_messages = peer_messages;
            state.groups.clear();
            state.groups = groups;
            state.group_runs.clear();
            state.group_operations.clear();
            state.remote_computer_device_secrets = remote_device_secrets;
            state.auth_user = None;
            state.session_active = logged_in;
            drop(state);
            *self
                .active_account_id
                .lock()
                .map_err(|_| FeatureHostError::StatePoisoned)? = next_account_id.clone();
            self.restore_routine_executions()?;
            self.restore_background_recoveries()?;
        }
        {
            let mut state = self.state()?;
            state.session_active = logged_in;
            state.auth_user = if logged_in {
                auth.get("user").cloned()
            } else {
                None
            };
        }
        *self
            .account_boundary_initialized
            .lock()
            .map_err(|_| FeatureHostError::StatePoisoned)? = true;
        if logged_in {
            // A signed-in Host is not ready for chat until the real Mahayana
            // provider session exists. This runs on restored sessions and on
            // fresh password/browser/OAuth login, so the first chat.send only
            // submits work; it never becomes the trigger that starts the
            // provider process/thread.
            self.runtime()?
                .warmup_conversation(ConversationId(MAHAYANA_AI_CONVERSATION_ID.to_string()))?;
            if !self.state()?.routine_quiescing {
                self.resume_suspended_background_operations()?;
            }
            if changed && !initialized {
                self.rearm_pending_async_operations()?;
            }
        }
        Ok(())
    }

    fn persist_remote_device_secrets(
        &self,
        secrets: &BTreeMap<String, String>,
    ) -> Result<(), FeatureHostError> {
        let Some(path) = self.active_account_root(self.remote_device_state_path.as_deref()) else {
            return Ok(());
        };
        persist_remote_computer_device_secrets(&path, secrets)
    }

    fn remote_device_secret(
        &self,
        device_id: &str,
        create: bool,
    ) -> Result<String, FeatureHostError> {
        if !is_safe_memory_agent_id(device_id) {
            return Err(FeatureHostError::Contract(format!(
                "unsafe remote computer device id: {device_id}"
            )));
        }
        let mut state = self.state()?;
        if let Some(secret) = state.remote_computer_device_secrets.get(device_id) {
            return Ok(secret.clone());
        }
        if !create {
            return Err(FeatureHostError::Contract(
                "remote computer must be registered by this desktop before use".into(),
            ));
        }
        if state.remote_computer_device_secrets.len() >= REMOTE_DEVICE_SECRET_MAX_ENTRIES {
            return Err(FeatureHostError::Contract(
                "too many local remote computer device identities are stored; reset an old profile before registering another".into(),
            ));
        }
        let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        state
            .remote_computer_device_secrets
            .insert(device_id.to_string(), secret.clone());
        let secrets = state.remote_computer_device_secrets.clone();
        drop(state);
        self.persist_remote_device_secrets(&secrets)?;
        Ok(secret)
    }

    /// Main settings are canonical on iOS; the Host receives this bounded
    /// runtime projection before execution so approval matching has one owner.
    pub fn set_auto_review_rules_direct(
        &self,
        rules: Vec<AutoReviewRule>,
    ) -> Result<Vec<AutoReviewRule>, FeatureHostError> {
        let rules = sanitize_auto_review_rules(rules);
        let mut state = self.state()?;
        ensure_open(&state)?;
        state.settings.auto_review_rules = rules.clone();
        Ok(rules)
    }

    pub fn resolve_approval(&self, resolution: ApprovalResolution) -> Result<(), FeatureHostError> {
        let pending = {
            let mut state = self.state()?;
            ensure_open(&state)?;
            state
                .pending_approvals
                .remove(&resolution.approval_id)
                .ok_or_else(|| {
                    FeatureHostError::Contract(format!(
                        "unknown approval: {}",
                        resolution.approval_id
                    ))
                })?
        };

        #[cfg(not(feature = "production"))]
        let _ = &pending;

        if self.config.mode == HostMode::Production {
            #[cfg(feature = "production")]
            {
                if let Some(runtime_approval_id) = pending.runtime_approval_id {
                    let decision = match resolution.decision {
                        ApprovalDecision::AllowOnce => RuntimeApprovalDecision::Accept,
                        ApprovalDecision::AllowSession => RuntimeApprovalDecision::AcceptForSession,
                        ApprovalDecision::Deny => RuntimeApprovalDecision::Decline,
                    };
                    self.runtime()?.resolve_approval(
                        ApprovalId(runtime_approval_id),
                        decision,
                        json!({
                            "miniAppId": pending.mini_app_id,
                            "capability": pending.capability,
                        }),
                    )?;
                }
            }
            #[cfg(not(feature = "production"))]
            {
                return Err(FeatureHostError::ProductionUnavailable);
            }
        }

        self.state()?.events.push_back(HostEvent::ApprovalResolved {
            timestamp: timestamp(),
            approval_id: resolution.approval_id,
            decision: resolution.decision,
        });
        Ok(())
    }

    pub fn interrupt(&self, operation_id: &str) -> Result<(), FeatureHostError> {
        {
            let state = self.state()?;
            ensure_open(&state)?;
            if !state.operations.contains(operation_id) {
                return Err(FeatureHostError::Contract(format!(
                    "unknown operation: {operation_id}"
                )));
            }
        }

        if self.config.mode == HostMode::Production {
            #[cfg(feature = "production")]
            if !operation_id.starts_with("host-task-") {
                self.runtime()?
                    .interrupt(OperationId(operation_id.to_string()))?;
            }
            #[cfg(not(feature = "production"))]
            return Err(FeatureHostError::ProductionUnavailable);
        }

        let mut state = self.state()?;
        state.operations.remove(operation_id);
        state.operation_agents.remove(operation_id);
        state.events.push_back(HostEvent::OperationInterrupted {
            timestamp: timestamp(),
            operation_id: operation_id.to_string(),
            reason: Some("interrupted by user".into()),
        });
        Ok(())
    }

    pub fn close(&self) -> Result<(), FeatureHostError> {
        self.set_routine_quiescing(true)?;
        let operation_ids = {
            let mut state = self.state()?;
            if state.closed {
                return Ok(());
            }
            state.closed = true;
            state.conversation_session.invalidate_deferred_activation();
            state.conversation_session.active_conversation_id = None;
            state.conversation_session.scene_active = false;
            state.conversation_session.focused_at_ms = None;
            state.pending_approvals.clear();
            state.awaited_operations.clear();
            state.operation_terminals.clear();
            state.pending_box_handoffs.clear();
            state.pending_listener_resumes.clear();
            state.remote_computer_sessions.clear();
            state.group_runs.clear();
            state.group_operations.clear();
            let suspended_routine_operations = state
                .routine_executions
                .values()
                .filter(|execution| execution.phase == RoutinePhase::Suspended)
                .filter_map(|execution| execution.operation_id.as_ref())
                .cloned()
                .collect::<BTreeSet<_>>();
            let suspended_background_operations = state
                .background_recoveries
                .values()
                .filter(|execution| execution.phase == BackgroundRecoveryPhase::Suspended)
                .map(|execution| execution.operation_id.clone())
                .collect::<BTreeSet<_>>();
            let operation_ids = state
                .operations
                .iter()
                .filter(|operation_id| {
                    !suspended_routine_operations.contains(*operation_id)
                        && !suspended_background_operations.contains(*operation_id)
                })
                .cloned()
                .collect::<Vec<_>>();
            state.background_operations.clear();
            state.operations.clear();
            state.operation_agents.clear();
            *self
                .client_side_tool_v2
                .lock()
                .map_err(|_| FeatureHostError::StatePoisoned)? = ClientSideToolV2Producer::new();
            state.events.push_back(HostEvent::HostClosed {
                timestamp: timestamp(),
            });
            operation_ids
        };

        if self.config.mode == HostMode::Production {
            #[cfg(feature = "production")]
            for operation_id in operation_ids {
                if !operation_id.starts_with("host-task-") {
                    let _ = self.runtime()?.interrupt(OperationId(operation_id));
                }
            }
            #[cfg(not(feature = "production"))]
            return Err(FeatureHostError::ProductionUnavailable);
        }

        let active_teach = self
            .teach_recording
            .lock()
            .map_err(|_| FeatureHostError::StatePoisoned)?
            .take();
        if let Some(mut active) = active_teach {
            if let Some(child) = active.child.as_mut() {
                stop_teach_capture(child)?;
            }
            let _ = std::fs::remove_dir_all(&active.session_dir);
        }
        Ok(())
    }

    #[cfg(feature = "production")]
    fn runtime(&self) -> Result<&MahayanaHost, FeatureHostError> {
        self.runtime
            .as_ref()
            .ok_or(FeatureHostError::ProductionUnavailable)
    }

    #[cfg(feature = "production")]
    fn require_authenticated_account(&self) -> Result<(), FeatureHostError> {
        let auth_status = self.auth_status()?;
        let auth = auth_payload(&auth_status);
        if auth.get("loggedIn").and_then(Value::as_bool) != Some(true) {
            return Err(FeatureHostError::Contract(
                "this operation requires an authenticated Fabushi account session".into(),
            ));
        }
        if auth_account_id(auth).is_none() {
            return Err(FeatureHostError::Contract(
                "authenticated account has no stable user id".into(),
            ));
        }
        Ok(())
    }

    #[cfg(not(feature = "production"))]
    fn execute_production(
        &self,
        _command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        Err(FeatureHostError::ProductionUnavailable)
    }

    #[cfg(not(feature = "production"))]
    fn receive_production(
        &self,
        _timeout: Duration,
    ) -> Result<Option<HostEvent>, FeatureHostError> {
        Err(FeatureHostError::ProductionUnavailable)
    }

    #[cfg(feature = "production")]
    fn start_next_group_turn(&self, group_id: &str) -> Result<Option<String>, FeatureHostError> {
        let prepared = {
            let state = self.state()?;
            let Some(run) = state.group_runs.get(group_id).cloned() else {
                return Ok(None);
            };
            let Some(member_id) = run.speaker_order.get(run.speaker_index).cloned() else {
                return Ok(None);
            };
            let Some(group) = state.groups.get(group_id).cloned() else {
                return Ok(None);
            };
            let Some(member) = state.bots.get(&member_id).cloned() else {
                return Ok(None);
            };
            let Some(conversation_id) = member.conversation_id.clone() else {
                return Err(FeatureHostError::Contract(format!(
                    "group member {} has no conversation id",
                    member.id
                )));
            };
            let peers = group
                .member_ids
                .iter()
                .filter(|id| **id != member.id)
                .filter_map(|id| state.bots.get(id).cloned())
                .collect::<Vec<_>>();
            let new_messages = group_messages_since_member_last_spoke(&group.messages, &member.id);
            let system_prompt = build_group_member_system_prompt(&member, &group, &peers);
            let turn_prompt = build_group_turn_prompt(&member, &group, &peers, new_messages);
            let account_memory_root = self.active_account_root(self.memory_root_path.as_deref());
            let account_workflow_root =
                self.active_account_root(self.workflow_root_path.as_deref());
            let memory_prompt = account_memory_root
                .as_deref()
                .map(|root| render_memory_system_prompt(&root.join(&member.id).join("memory")))
                .unwrap_or_default();
            let workflow_catalog = match (
                account_workflow_root.as_deref(),
                account_memory_root.as_deref(),
            ) {
                (Some(workflow_root), Some(agent_root)) => {
                    render_workflow_catalog(workflow_root, agent_root, &member.id)
                }
                _ => String::new(),
            };
            let mut context_sections = vec![system_prompt];
            if !memory_prompt.is_empty() {
                context_sections.push(format!("[Persistent agent memory]\n{memory_prompt}"));
            }
            if !workflow_catalog.is_empty() {
                context_sections.push(format!("[Available workflows]\n{workflow_catalog}"));
            }
            context_sections.push(turn_prompt);
            let runtime_text = format!(
                "[MAHAYANA_HIDDEN_CONTEXT]\n{}",
                context_sections.join("\n\n")
            );
            (
                GroupOperationContext {
                    run_id: run.run_id,
                    group_id: group.id,
                    member_id: member.id,
                    member_name: member.name,
                },
                conversation_id,
                runtime_text,
            )
        };
        let (context, conversation_id, runtime_text) = prepared;
        let response = self.runtime()?.execute(RuntimeCommand::SendMessage {
            conversation_id: ConversationId(conversation_id),
            text: runtime_text,
            display_text: None,
            client_message_id: Some(format!(
                "{}:{}:{}",
                context.run_id, context.group_id, context.member_id
            )),
            hidden: true,
            show_assistant_output: false,
            recovery_eligible: false,
            reply_to_message_id: None,
            is_fork: false,
            attachment_batch_id: None,
            selected_image_data_urls: Vec::new(),
        })?;
        let operation_id = match response {
            RuntimeResponse::Accepted { operation_id } => operation_id.to_string(),
            other => return Err(unexpected_response("group.member.turn", other)),
        };
        let mut state = self.state()?;
        state.group_operations.insert(operation_id.clone(), context);
        Ok(Some(operation_id))
    }

    #[cfg(feature = "production")]
    fn advance_group_run_after_turn(
        &self,
        context: &GroupOperationContext,
    ) -> Result<Option<String>, FeatureHostError> {
        let should_continue = {
            let mut state = self.state()?;
            let Some(snapshot) = state.group_runs.get(&context.group_id).cloned() else {
                return Ok(None);
            };
            if snapshot.run_id != context.run_id {
                return Ok(None);
            }
            let mut next = snapshot;
            next.speaker_index += 1;
            let mut done = next.total_messages >= GROUP_MAX_MEMBER_TURNS;
            if !done && next.speaker_index >= next.speaker_order.len() {
                if next.messages_this_round == 0 {
                    done = true;
                } else {
                    next.round += 1;
                    if next.round >= GROUP_MAX_ROUNDS {
                        done = true;
                    } else if let Some(group) = state.groups.get(&context.group_id) {
                        let responders = resolve_group_responders(group, &state.bots);
                        next.speaker_order = order_round_speakers(&responders, next.round);
                        next.speaker_index = 0;
                        next.messages_this_round = 0;
                        if next.speaker_order.is_empty() {
                            done = true;
                        }
                    } else {
                        done = true;
                    }
                }
            }
            if done {
                state.group_runs.remove(&context.group_id);
                false
            } else {
                state.group_runs.insert(context.group_id.clone(), next);
                true
            }
        };
        if should_continue {
            self.start_next_group_turn(&context.group_id)
        } else {
            Ok(None)
        }
    }

    #[cfg(feature = "production")]
    fn translate_runtime_event(
        &self,
        event: RuntimeEvent,
    ) -> Result<Option<HostEvent>, FeatureHostError> {
        if self.stale_routine_runtime_event(&event)? {
            return Ok(None);
        }
        let event = match event {
            RuntimeEvent::Ready { .. } => None,
            RuntimeEvent::MessageDelta {
                operation_id,
                delta,
                ..
            } => {
                let operation_id = operation_id.to_string();
                let group_context = self.state()?.group_operations.get(&operation_id).cloned();
                if let Some(context) = group_context {
                    Some(HostEvent::GroupDelta {
                        timestamp: timestamp(),
                        group_id: context.group_id,
                        member_id: context.member_id,
                        member_name: context.member_name,
                        operation_id,
                        delta,
                    })
                } else if let Some(context) = self
                    .state()?
                    .background_operations
                    .get(&operation_id)
                    .cloned()
                {
                    Some(HostEvent::AgentBackgroundDelta {
                        timestamp: timestamp(),
                        agent_id: context.agent_id,
                        agent_name: context.agent_name,
                        operation_id,
                        source: context.source,
                        delta,
                    })
                } else {
                    Some(HostEvent::ChatDelta {
                        timestamp: timestamp(),
                        operation_id,
                        delta,
                    })
                }
            }
            RuntimeEvent::MessageCompleted {
                operation_id,
                message,
                ..
            } => {
                let operation_id = operation_id.to_string();
                let group_context = self.state()?.group_operations.get(&operation_id).cloned();
                if let Some(context) = group_context {
                    if message.role != RuntimeMessageRole::Assistant {
                        None
                    } else {
                        let content = message.text.trim();
                        if is_group_pass_content(content) {
                            None
                        } else {
                            let mut state = self.state()?;
                            let message_id = next_id(&mut state, "group-message");
                            let now = now_millis();
                            let Some(group) = state.groups.get_mut(&context.group_id) else {
                                return Ok(None);
                            };
                            group.messages.push(GroupMessage {
                                id: message_id,
                                speaker: GroupSpeaker::Member {
                                    id: context.member_id.clone(),
                                    name: context.member_name.clone(),
                                },
                                content: content.to_string(),
                                created_at_ms: now,
                            });
                            if group.messages.len() > 500 {
                                let overflow = group.messages.len() - 500;
                                group.messages.drain(0..overflow);
                            }
                            group.updated_at_ms = now;
                            let group = group.clone();
                            if let Some(run) = state.group_runs.get_mut(&context.group_id) {
                                if run.run_id == context.run_id
                                    && run.total_messages < GROUP_MAX_MEMBER_TURNS
                                {
                                    run.total_messages += 1;
                                    run.messages_this_round += 1;
                                }
                            }
                            self.persist_groups(&state.groups)?;
                            Some(HostEvent::GroupChanged {
                                timestamp: timestamp(),
                                action: "message".into(),
                                group,
                            })
                        }
                    }
                } else if let Some(context) = self
                    .state()?
                    .background_operations
                    .get(&operation_id)
                    .cloned()
                {
                    if message.role == RuntimeMessageRole::Assistant {
                        let fingerprint = background_message_fingerprint(
                            &message.text,
                            message.metadata.get("generatedSend").and_then(Value::as_bool) == Some(true),
                        );
                        if self.state()?.background_recoveries
                            .get(&operation_id)
                            .is_some_and(|execution| execution.delivered_message_fingerprints.contains(&fingerprint))
                        {
                            return Ok(None);
                        }
                        if let Some(artifact) = context.teach_artifact.as_deref() {
                            if !message.text.trim().is_empty() {
                                match self.persist_teach_workflow(
                                    &context.agent_id,
                                    artifact,
                                    &message.text,
                                ) {
                                    Ok(workflow) => {
                                        self.state()?.events.push_back(
                                            HostEvent::WorkflowChanged {
                                                timestamp: timestamp(),
                                                agent_id: context.agent_id.clone(),
                                                action: "learned".into(),
                                                workflow: Some(workflow),
                                                id: None,
                                            },
                                        );
                                    }
                                    Err(error) => {
                                        let mut state = self.state()?;
                                        push_error_tray(
                                            &mut state,
                                            context.agent_id.clone(),
                                            "Teach workflow could not be saved".into(),
                                            Some(error.to_string()),
                                            Some(operation_id.clone()),
                                            Some(format!("teach-workflow:{}", context.agent_id)),
                                        );
                                    }
                                }
                            }
                        }
                        {
                            let mut state = self.state()?;
                            if let Some(execution) = state.background_recoveries.get_mut(&operation_id) {
                                execution.delivered_message_fingerprints.insert(fingerprint);
                                self.persist_background_recoveries(&state)?;
                            }
                        }
                        Some(HostEvent::AgentBackgroundMessage {
                            timestamp: timestamp(),
                            agent_id: context.agent_id,
                            agent_name: context.agent_name,
                            operation_id,
                            source: context.source,
                            text: message.text,
                        })
                    } else {
                        None
                    }
                } else {
                    let mut cards = transcript_cards_from_metadata(&message.metadata);
                    let message_id = message.id.to_string();
                    if message.text.trim().is_empty() && !cards.is_empty() {
                        let first = cards.remove(0);
                        let mut state = self.state()?;
                        for (index, card) in cards.into_iter().enumerate() {
                            state.events.push_back(HostEvent::TranscriptCard {
                                timestamp: timestamp(),
                                entry_id: format!("{message_id}-card-{}", index + 1),
                                operation_id: Some(operation_id.clone()),
                                card,
                            });
                        }
                        Some(HostEvent::TranscriptCard {
                            timestamp: timestamp(),
                            entry_id: format!("{message_id}-card-0"),
                            operation_id: Some(operation_id),
                            card: first,
                        })
                    } else {
                        if !cards.is_empty() {
                            let mut state = self.state()?;
                            for (index, card) in cards.into_iter().enumerate() {
                                state.events.push_back(HostEvent::TranscriptCard {
                                    timestamp: timestamp(),
                                    entry_id: format!("{message_id}-card-{index}"),
                                    operation_id: Some(operation_id.clone()),
                                    card,
                                });
                            }
                        }
                        let role = match message.role {
                            RuntimeMessageRole::User => MessageRole::User,
                            RuntimeMessageRole::Assistant
                            | RuntimeMessageRole::Contact
                            | RuntimeMessageRole::MiniApp
                            | RuntimeMessageRole::System => MessageRole::Assistant,
                        };
                        let reply_to_message_id = message
                            .metadata
                            .get("replyToMessageId")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        let attachment_batch_id = message
                            .metadata
                            .get("attachmentBatchId")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                        let attachment = message.metadata.get("generatedAttachment").cloned();
                        let branched = message
                            .metadata
                            .get("branched")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        Some(HostEvent::ChatMessage {
                            timestamp: timestamp(),
                            role,
                            text: message.text,
                            message_id: Some(message_id),
                            operation_id: Some(operation_id),
                            reply_to_message_id,
                            attachment_batch_id,
                            attachment,
                            branched,
                        })
                    }
                }
            }
            RuntimeEvent::ApprovalRequested {
                operation_id,
                approval_id,
                title,
                details,
            } => Some(self.translate_runtime_approval(
                operation_id.to_string(),
                approval_id,
                title,
                details,
            )?),
            RuntimeEvent::OperationCompleted { operation_id } => {
                let operation_id = operation_id.to_string();
                self.finish_automation_operation(
                    &operation_id, AutomationRunStatus::Ok, None, "completed",
                )?;
                let group_context = self.state()?.group_operations.remove(&operation_id);
                if let Some(context) = group_context {
                    let _ = self.advance_group_run_after_turn(&context)?;
                    None
                } else if let Some(context) =
                    self.state()?.background_operations.remove(&operation_id)
                {
                    self.settle_background_recovery(&operation_id)?;
                    let mut state = self.state()?;
                    state.operations.remove(&operation_id);
                    state.operation_agents.remove(&operation_id);
                    Some(HostEvent::AgentBackgroundFinished {
                        timestamp: timestamp(),
                        agent_id: context.agent_id,
                        agent_name: context.agent_name,
                        operation_id,
                        source: context.source,
                        error: None,
                    })
                } else {
                    let mut state = self.state()?;
                    state.operations.remove(&operation_id);
                    let terminal_agent_id = state.operation_agents.remove(&operation_id);
                    if state.awaited_operations.contains(&operation_id) {
                        state
                            .operation_terminals
                            .insert(operation_id.clone(), json!({"status": "completed"}));
                    }
                    if let Some(agent_id) = terminal_agent_id {
                        let _ = queue_active_agent_automation_projection(&mut state, &agent_id);
                    }
                    Some(HostEvent::OperationCompleted {
                        timestamp: timestamp(),
                        operation_id,
                    })
                }
            }
            RuntimeEvent::OperationInterrupted {
                operation_id,
                reason,
            } => {
                let operation_id = operation_id.to_string();
                self.finish_automation_operation(
                    &operation_id, AutomationRunStatus::Error, Some(reason.clone()), "interrupted",
                )?;
                if let Some(context) = self.state()?.background_operations.remove(&operation_id) {
                    self.settle_background_recovery(&operation_id)?;
                    let mut state = self.state()?;
                    state.operations.remove(&operation_id);
                    state.operation_agents.remove(&operation_id);
                    Some(HostEvent::AgentBackgroundFinished {
                        timestamp: timestamp(),
                        agent_id: context.agent_id,
                        agent_name: context.agent_name,
                        operation_id,
                        source: context.source,
                        error: Some(reason),
                    })
                } else {
                let mut state = self.state()?;
                if !state.operations.remove(&operation_id) {
                    None
                } else {
                    state.operation_agents.remove(&operation_id);
                    if state.awaited_operations.contains(&operation_id) {
                        state.operation_terminals.insert(
                            operation_id.clone(),
                            json!({
                                "status": "interrupted",
                                "reason": reason.clone(),
                            }),
                        );
                    }
                    Some(HostEvent::OperationInterrupted {
                        timestamp: timestamp(),
                        operation_id,
                        reason: Some(reason),
                    })
                }
                }
            }
            RuntimeEvent::OperationFailed {
                operation_id,
                code,
                message,
            } => {
                let operation_id = operation_id.to_string();
                let group_context = self.state()?.group_operations.remove(&operation_id);
                if let Some(context) = group_context {
                    let group = {
                        let mut state = self.state()?;
                        let group = state.groups.get(&context.group_id).cloned();
                        push_error_tray(
                            &mut state,
                            context.member_id.clone(),
                            format!("{} failed in {}", context.member_name, context.group_id),
                            Some(message.clone()),
                            Some(operation_id.clone()),
                            Some(format!("group:{}:{}", context.group_id, code)),
                        );
                        group
                    };
                    let _ = self.advance_group_run_after_turn(&context)?;
                    group.map(|group| HostEvent::GroupChanged {
                        timestamp: timestamp(),
                        action: format!("turnFailed:{code}:{message}"),
                        group,
                    })
                } else if let Some(context) =
                    self.state()?.background_operations.remove(&operation_id)
                {
                    self.settle_background_recovery(&operation_id)?;
                    let mut state = self.state()?;
                    state.operations.remove(&operation_id);
                    state.operation_agents.remove(&operation_id);
                    push_error_tray(
                        &mut state,
                        context.agent_id.clone(),
                        format!("{} background task failed", context.agent_name),
                        Some(message.clone()),
                        Some(operation_id.clone()),
                        Some(format!("background:{}:{code}", context.agent_id)),
                    );
                    Some(HostEvent::AgentBackgroundFinished {
                        timestamp: timestamp(),
                        agent_id: context.agent_id,
                        agent_name: context.agent_name,
                        operation_id,
                        source: context.source,
                        error: Some(message),
                    })
                } else {
                    self.finish_automation_operation(
                        &operation_id, AutomationRunStatus::Error,
                        Some(format!("{code}: {message}")), "failed",
                    )?;
                    let mut state = self.state()?;
                    state.operations.remove(&operation_id);
                    if state.awaited_operations.contains(&operation_id) {
                        state.operation_terminals.insert(
                            operation_id.clone(),
                            json!({
                                "status": "failed",
                                "code": code.clone(),
                                "message": message.clone(),
                            }),
                        );
                    }
                    let agent_id = state
                        .operation_agents
                        .remove(&operation_id)
                        .unwrap_or_else(|| "mahayana-assistant".into());
                    let agent_name = state
                        .bots
                        .get(&agent_id)
                        .map(|bot| bot.name.clone())
                        .unwrap_or_else(|| "Agent".into());
                    push_error_tray(
                        &mut state,
                        agent_id.clone(),
                        format!("{agent_name} task failed"),
                        Some(message.clone()),
                        Some(operation_id.clone()),
                        Some(format!("agent-run:{agent_id}:{code}")),
                    );
                    Some(HostEvent::OperationFailed {
                        timestamp: timestamp(),
                        operation_id,
                        code,
                        message,
                    })
                }
            }
            RuntimeEvent::ModelUsageUpdated {
                operation_id,
                usage,
            } => {
                let tokens = usage.total.unwrap_or_else(|| usage.last.clone());
                Some(HostEvent::UsageUpdated {
                    timestamp: timestamp(),
                    operation_id: operation_id.to_string(),
                    input_tokens: tokens.input_tokens,
                    cached_input_tokens: tokens.cached_input_tokens,
                    output_tokens: tokens.output_tokens,
                    reasoning_tokens: tokens.reasoning_output_tokens,
                    total_tokens: tokens.total_tokens,
                    context_window: usage.model_context_window,
                })
            }
            RuntimeEvent::PluginProgress {
                operation_id,
                plugin_id,
                tool,
                progress,
                total,
                message,
            } => Some(HostEvent::AgentStep {
                timestamp: timestamp(),
                operation_id: Some(operation_id.to_string()),
                step_id: format!("{plugin_id}:{tool}"),
                kind: "tool".into(),
                title: tool,
                detail: Some(message),
                status: if total > 0 && progress >= total {
                    AgentStepStatus::Completed
                } else {
                    AgentStepStatus::Running
                },
                progress: Some(progress),
                total: Some(total),
            }),
            RuntimeEvent::AgentActivity {
                operation_id,
                step_id,
                kind,
                title,
                detail,
                status,
                metadata,
            } => {
                let operation_id = operation_id.to_string();
                let agent_id = {
                    let state = self.state()?;
                    activity_parent_agent_id(&state, &operation_id)
                };
                if kind == "box_handoff_request" {
                    let provider_metadata = metadata
                        .as_ref()
                        .and_then(|value| value.get("provider"))
                        .and_then(|value| value.as_object());
                    let instruction = provider_metadata
                        .and_then(|value| value.get("instruction"))
                        .and_then(Value::as_str)
                        .or(detail.as_deref())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            FeatureHostError::Contract(
                                "box handoff request is missing an instruction".into(),
                            )
                        })?
                        .to_string();
                    let optional_field = |name: &str| {
                        provider_metadata
                            .and_then(|value| value.get(name))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string)
                    };
                    let reason = optional_field("reason");
                    let domain = optional_field("domain");
                    let idp_domain = optional_field("idp_domain");
                    let mut state = self.state()?;
                    let conversation_id = state
                        .bots
                        .get(&agent_id)
                        .and_then(|bot| bot.conversation_id.clone())
                        .unwrap_or_else(|| MAHAYANA_AI_CONVERSATION_ID.to_string());
                    let pending = if let Some(existing) = state.pending_box_handoffs.get(&agent_id)
                    {
                        existing.clone()
                    } else {
                        let request_id = next_id(&mut state, "box-handoff");
                        let pending = PendingBoxHandoff {
                            request_id,
                            agent_id: agent_id.clone(),
                            conversation_id,
                            instruction: instruction.clone(),
                            source_operation_id: operation_id.clone(),
                            resolving: false,
                        };
                        state
                            .pending_box_handoffs
                            .insert(agent_id.clone(), pending.clone());
                        pending
                    };
                    return Ok(Some(HostEvent::BoxHandoffRequested {
                        timestamp: timestamp(),
                        request_id: pending.request_id,
                        agent_id: pending.agent_id,
                        operation_id: pending.source_operation_id,
                        instruction: pending.instruction,
                        reason,
                        domain,
                        idp_domain,
                    }));
                }
                if kind == "tool" {
                    if status == RuntimeActivityStatus::Completed
                        && metadata
                            .as_ref()
                            .and_then(|value| value.get("tool"))
                            .and_then(Value::as_str)
                            == Some("react_to_message")
                    {
                        if let Some(output) = metadata
                            .as_ref()
                            .and_then(|value| value.get("output"))
                            .filter(|output| {
                                output.get("applied").and_then(Value::as_bool) == Some(true)
                            })
                        {
                            let entry_id = output
                                .get("messageId")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .ok_or_else(|| {
                                    FeatureHostError::Contract(
                                        "react_to_message completion is missing messageId".into(),
                                    )
                                })?
                                .to_string();
                            let reactions =
                                output.get("reactions").cloned().unwrap_or_else(|| json!([]));
                            let my_reactions =
                                output.get("myReactions").cloned().unwrap_or_else(|| json!([]));
                            self.state()?.events.push_back(HostEvent::TransportEvent {
                                channel: "transcript.reaction".into(),
                                payload: json!({
                                    "agentId": agent_id.clone(),
                                    "entryId": entry_id,
                                    "reactions": reactions,
                                    "myReactions": my_reactions,
                                }),
                            });
                        }
                    }
                    if let Some(tool_call_id) = metadata
                        .as_ref()
                        .and_then(|value| value.get("toolCallId"))
                        .and_then(Value::as_str)
                    {
                        let transport = {
                            let mut producer = self
                                .client_side_tool_v2
                                .lock()
                                .map_err(|_| FeatureHostError::StatePoisoned)?;
                            match status {
                                RuntimeActivityStatus::Running => {
                                    producer.publish_call(&agent_id, tool_call_id)
                                }
                                RuntimeActivityStatus::Completed
                                | RuntimeActivityStatus::Failed => {
                                    producer.publish_result(&agent_id, tool_call_id)
                                }
                                _ => None,
                            }
                        };
                        if let Some(payload) = transport {
                            self.state()?.events.push_back(HostEvent::TransportEvent {
                                channel: CLIENT_SIDE_TOOL_V2_FAMILY.into(),
                                payload,
                            });
                        }
                    }
                }
                if kind == "subagent" {
                    let account_id = self
                        .active_account_id
                        .lock()
                        .map_err(|_| FeatureHostError::StatePoisoned)?
                        .clone();
                    let async_tasks_path =
                        self.async_tasks_path_for_account(account_id.as_deref());
                    let mut state = self.state()?;
                    let changed = update_subagents_from_activity(
                        &mut state,
                        &agent_id,
                        &operation_id,
                        &title,
                        detail.as_deref(),
                        status,
                        metadata.as_ref(),
                    );
                    for subagent in changed {
                        if subagent.status == SubagentStatus::Running {
                            state.async_task_operation_ids.insert(
                                async_task_key(&subagent.parent_agent_id, AsyncTaskKind::Subagent, &subagent.id),
                                operation_id.clone(),
                            );
                        } else {
                            state.async_task_operation_ids.remove(&async_task_key(
                                &subagent.parent_agent_id,
                                AsyncTaskKind::Subagent,
                                &subagent.id,
                            ));
                        }
                        state.events.push_back(HostEvent::SubagentChanged {
                            timestamp: timestamp(),
                            subagent,
                        });
                    }
                    persist_pending_async_tasks(
                        async_tasks_path.as_deref(),
                        &state.async_tasks,
                        &state.async_task_operation_ids,
                    )?;
                    let mut tasks = state
                        .async_tasks
                        .values()
                        .filter(|task| task.parent_agent_id == agent_id)
                        .cloned()
                        .collect::<Vec<_>>();
                    tasks.sort_by_key(|task| task.started_at_ms);
                    state.events.push_back(HostEvent::AsyncTaskChanged {
                        timestamp: timestamp(),
                        agent_id: agent_id.clone(),
                        tasks,
                    });
                }
                if matches!(
                    kind.as_str(),
                    "shell" | "command" | "local-exec" | "exec" | "cloud-agent" | "cloud_agent"
                ) {
                    let account_id = self
                        .active_account_id
                        .lock()
                        .map_err(|_| FeatureHostError::StatePoisoned)?
                        .clone();
                    let async_tasks_path =
                        self.async_tasks_path_for_account(account_id.as_deref());
                    let task_kind = if matches!(kind.as_str(), "cloud-agent" | "cloud_agent") {
                        AsyncTaskKind::CloudAgent
                    } else {
                        AsyncTaskKind::Shell
                    };
                    let resource_id = if task_kind == AsyncTaskKind::CloudAgent {
                        cloud_task_resource_id(metadata.as_ref())
                    } else {
                        None
                    };
                    let task_id = match task_kind {
                        AsyncTaskKind::CloudAgent => resource_id.clone(),
                        AsyncTaskKind::Shell => Some(format!("{operation_id}:{step_id}")),
                        AsyncTaskKind::Subagent => None,
                    };
                    if let Some(task_id) = task_id {
                        let task_key = async_task_key(&agent_id, task_kind, &task_id);
                        let mut state = self.state()?;
                        if status == RuntimeActivityStatus::Running {
                            state.async_tasks.insert(
                                task_key.clone(),
                                AsyncTaskSummary {
                                    kind: task_kind,
                                    id: task_id.clone(),
                                    parent_agent_id: agent_id.clone(),
                                    label: title.clone(),
                                    status: AsyncTaskStatus::Running,
                                    started_at_ms: now_millis(),
                                    detail: detail.clone(),
                                    subagent_type: None,
                                    resource_id: resource_id.clone(),
                                },
                            );
                            state
                                .async_task_operation_ids
                                .insert(task_key.clone(), operation_id.clone());
                        } else {
                            state.async_tasks.remove(&task_key);
                            state.async_task_operation_ids.remove(&task_key);
                        }
                        persist_pending_async_tasks(
                            async_tasks_path.as_deref(),
                            &state.async_tasks,
                            &state.async_task_operation_ids,
                        )?;
                        let mut tasks = state
                            .async_tasks
                            .values()
                            .filter(|task| task.parent_agent_id == agent_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        tasks.sort_by_key(|task| task.started_at_ms);
                        state.events.push_back(HostEvent::AsyncTaskChanged {
                            timestamp: timestamp(),
                            agent_id: agent_id.clone(),
                            tasks,
                        });
                    }
                }
                if matches!(kind.as_str(), "shell" | "command" | "local-exec" | "exec")
                    && status != RuntimeActivityStatus::Running
                {
                    let _ = self.append_action_audit(
                        &agent_id,
                        Some(&operation_id),
                        json!({
                            "kind": "shellCommand",
                            "command": detail.clone().unwrap_or_else(|| title.clone()),
                            "shellKind": kind,
                            "target": "runtime",
                            "status": match status {
                                RuntimeActivityStatus::Completed => "completed",
                                RuntimeActivityStatus::Failed => "failed",
                                RuntimeActivityStatus::Running => "running",
                            },
                        }),
                    );
                }
                if kind == "computer" && status != RuntimeActivityStatus::Running {
                    let computer_metadata = metadata
                        .as_ref()
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    let _ = self.append_action_audit(
                        &agent_id,
                        Some(&operation_id),
                        json!({
                            "kind": "computerUse",
                            "origin": computer_metadata
                                .get("origin")
                                .and_then(Value::as_str)
                                .unwrap_or("ai"),
                            "actions": computer_metadata.get("arguments").cloned().unwrap_or(Value::Null),
                            "detail": detail.clone(),
                            "title": title.clone(),
                            "status": match status {
                                RuntimeActivityStatus::Completed => "completed",
                                RuntimeActivityStatus::Failed => "failed",
                                RuntimeActivityStatus::Running => "running",
                            },
                        }),
                    );
                }
                Some(HostEvent::AgentStep {
                    timestamp: timestamp(),
                    operation_id: Some(operation_id),
                    step_id,
                    kind,
                    title,
                    detail,
                    status: match status {
                        RuntimeActivityStatus::Running => AgentStepStatus::Running,
                        RuntimeActivityStatus::Completed => AgentStepStatus::Completed,
                        RuntimeActivityStatus::Failed => AgentStepStatus::Failed,
                    },
                    progress: None,
                    total: None,
                })
            }
            RuntimeEvent::ProviderDegraded { provider, message } => Some(HostEvent::AgentStep {
                timestamp: timestamp(),
                operation_id: None,
                step_id: format!("provider:{provider}"),
                kind: "provider".into(),
                title: format!("{provider} 服务降级"),
                detail: Some(message),
                status: AgentStepStatus::Failed,
                progress: None,
                total: None,
            }),
            RuntimeEvent::Lagged { skipped } => Some(HostEvent::AgentStep {
                timestamp: timestamp(),
                operation_id: None,
                step_id: "runtime:event-lag".into(),
                kind: "runtime".into(),
                title: "事件流正在追赶".into(),
                detail: Some(format!("跳过 {skipped} 个过期事件")),
                status: AgentStepStatus::Failed,
                progress: None,
                total: None,
            }),
        };
        Ok(event)
    }

    #[cfg(feature = "production")]
    fn translate_runtime_approval(
        &self,
        operation_id: String,
        approval_id: ApprovalId,
        title: String,
        details: serde_json::Value,
    ) -> Result<HostEvent, FeatureHostError> {
        let approval_key = approval_id.to_string();
        let mini_app_id = details
            .get("pluginId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("runtime")
            .to_string();
        let capability = details
            .get("capability")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(title.as_str())
            .to_string();
        let display_title = approval_presentation_safe_text(&title, 240);
        let reason = details
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .map(|value| approval_presentation_safe_text(value, 1200))
            .unwrap_or_else(|| approval_presentation_safe_details(&details));

        let settings = self.state()?.settings.clone();
        let is_local_tool_request = details.get("command").is_some()
            || details
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| matches!(kind, "command" | "local-tool" | "local_tool"));
        let has_ask_match = settings.auto_review_rules.iter().any(|rule| {
            rule.behavior == AutoReviewBehavior::Ask
                && auto_review_rule_matches(rule, &title, &details)
        });
        let has_allow_match = !has_ask_match
            && settings.auto_review_rules.iter().any(|rule| {
                rule.behavior == AutoReviewBehavior::Allow
                    && auto_review_rule_matches(rule, &title, &details)
            });
        let auto_decision = if is_local_tool_request
            && (!settings.local_execution
                || settings.local_tool_permission == LocalToolPermission::Never)
        {
            Some(ApprovalDecision::Deny)
        } else if is_local_tool_request
            && settings.local_tool_permission == LocalToolPermission::Always
            && !has_ask_match
        {
            Some(ApprovalDecision::AllowSession)
        } else if has_allow_match {
            Some(ApprovalDecision::AllowOnce)
        } else {
            None
        };
        if let Some(decision) = auto_decision {
            let runtime_decision = match decision {
                ApprovalDecision::AllowOnce => RuntimeApprovalDecision::Accept,
                ApprovalDecision::AllowSession => RuntimeApprovalDecision::AcceptForSession,
                ApprovalDecision::Deny => RuntimeApprovalDecision::Decline,
            };
            self.runtime()?.resolve_approval(
                ApprovalId(approval_key.clone()),
                runtime_decision,
                json!({
                    "source": "fabushi-auto-review",
                    "capability": capability,
                    "reason": reason,
                }),
            )?;
            let agent_id = details
                .get("agentId")
                .and_then(Value::as_str)
                .unwrap_or("mahayana-assistant");
            let _ = self.append_action_audit(
                agent_id,
                None,
                json!({
                    "kind": "autoReview",
                    "approvalId": approval_key,
                    "decision": match decision {
                        ApprovalDecision::AllowOnce => "allow-once",
                        ApprovalDecision::AllowSession => "allow-session",
                        ApprovalDecision::Deny => "deny",
                    },
                    "title": display_title,
                    "capability": approval_presentation_safe_text(&capability, 160),
                }),
            );
            self.state()?.events.push_back(HostEvent::AgentStep {
                timestamp: timestamp(),
                operation_id: None,
                step_id: format!("auto-review:{approval_key}"),
                kind: "auto-review".into(),
                title: match decision {
                    ApprovalDecision::AllowOnce => "自动审批：本次允许".into(),
                    ApprovalDecision::AllowSession => "自动审批：按本机权限允许".into(),
                    ApprovalDecision::Deny => "自动审批：已拒绝".into(),
                },
                detail: Some(title),
                status: AgentStepStatus::Completed,
                progress: None,
                total: None,
            });
            return Ok(HostEvent::ApprovalResolved {
                timestamp: timestamp(),
                approval_id: approval_key,
                decision,
            });
        }

        self.state()?.pending_approvals.insert(
            approval_key.clone(),
            PendingApproval {
                mini_app_id: mini_app_id.clone(),
                capability: capability.clone(),
                runtime_approval_id: Some(approval_id.to_string()),
            },
        );
        Ok(HostEvent::ApprovalRequested {
            timestamp: timestamp(),
            operation_id: Some(operation_id),
            approval_id: approval_key,
            mini_app_id,
            capability,
            reason,
            kind: details
                .get("kind")
                .and_then(Value::as_str)
                .map(|value| approval_presentation_safe_text(value, 120)),
            subject: details
                .get("subject")
                .or_else(|| details.get("command"))
                .and_then(Value::as_str)
                .map(|value| approval_presentation_safe_text(value, 500)),
            detail: details
                .get("detail")
                .and_then(Value::as_str)
                .map(|value| approval_presentation_safe_text(value, 1200)),
            proposed_rule: details
                .get("proposedRule")
                .and_then(Value::as_str)
                .map(|value| approval_presentation_safe_text(value, 500)),
            location: details
                .get("location")
                .and_then(Value::as_str)
                .map(|value| approval_presentation_safe_text(value, 300)),
        })
    }

    #[cfg(feature = "production")]
    fn receive_production(&self, timeout: Duration) -> Result<Option<HostEvent>, FeatureHostError> {
        for index in 0..16 {
            // Block only for the first runtime event. Once awakened, drain already-queued
            // events without adding latency between streamed events.
            let receive_timeout = if index == 0 { timeout } else { Duration::ZERO };
            let Some(event) = self.runtime()?.receive(receive_timeout)? else {
                return Ok(None);
            };
            if let Some(event) = self.translate_runtime_event(event)? {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }

    #[cfg(feature = "production")]
    fn production_long_task(
        &self,
        request_id: String,
        label: String,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let label = required(label, "operation label")?;
        let mut state = self.state()?;
        let operation_id = next_id(&mut state, "host-task");
        state.operations.insert(operation_id.clone());
        state.events.push_back(HostEvent::OperationStarted {
            timestamp: timestamp(),
            operation_id: operation_id.clone(),
            label,
            interruptible: true,
        });
        Ok(CommandAccepted {
            request_id,
            operation_id: Some(operation_id),
        })
    }

    #[cfg(feature = "production")]
    fn production_clear_session(
        &self,
        request_id: String,
    ) -> Result<CommandAccepted, FeatureHostError> {
        self.runtime()?.clear_session()?;
        let mut state = self.state()?;
        state.session_active = false;
        state.conversation_session = ConversationSessionState::default();
        state.events.push_back(HostEvent::SessionCleared {
            timestamp: timestamp(),
        });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn production_capability_request(
        &self,
        request_id: String,
        mini_app_id: String,
        capability: String,
        reason: String,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let mini_app_id = required(mini_app_id, "miniAppId")?;
        let capability = required(capability, "capability")?;
        let reason = required(reason, "reason")?;
        let mut state = self.state()?;
        if !state.installed.contains_key(&mini_app_id) {
            return Err(FeatureHostError::Contract(format!(
                "MiniApp is not installed: {mini_app_id}"
            )));
        }
        let approval_id = next_id(&mut state, "approval");
        state.pending_approvals.insert(
            approval_id.clone(),
            PendingApproval {
                mini_app_id: mini_app_id.clone(),
                capability: capability.clone(),
                runtime_approval_id: None,
            },
        );
        state.events.push_back(HostEvent::ApprovalRequested {
            timestamp: timestamp(),
            operation_id: None,
            approval_id,
            mini_app_id,
            capability,
            reason,
            kind: Some("capability".into()),
            subject: None,
            detail: None,
            proposed_rule: None,
            location: None,
        });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn production_open(
        &self,
        request_id: String,
        mini_app_id: String,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let mini_app_id = required(mini_app_id, "miniAppId")?;
        if !self.state()?.installed.contains_key(&mini_app_id) {
            return Err(FeatureHostError::Contract(format!(
                "MiniApp is not installed: {mini_app_id}"
            )));
        }
        let html = match self.runtime()?.execute(RuntimeCommand::PluginUi {
            plugin_id: mini_app_id.clone(),
        })? {
            RuntimeResponse::PluginUi { html, .. } => html,
            other => return Err(unexpected_response("miniapp.open", other)),
        };
        self.state()?.events.push_back(HostEvent::MiniAppOpened {
            timestamp: timestamp(),
            mini_app_id,
            html: Some(html),
        });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn production_install(
        &self,
        request_id: String,
        mini_app_id: String,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let mini_app_id = required(mini_app_id, "miniAppId")?;
        let response = self.runtime()?.execute(RuntimeCommand::ListCapabilities {
            query: Some(format!("miniapp.{mini_app_id}")),
        })?;
        let available = match response {
            RuntimeResponse::Capabilities { data } => data.into_iter().any(|capability| {
                capability.plugin_id.as_deref() == Some(mini_app_id.as_str())
                    && capability.is_invokable()
            }),
            other => return Err(unexpected_response("marketplace.install", other)),
        };
        if !available {
            return Err(FeatureHostError::Contract(format!(
                "MiniApp is unavailable in the production Runtime: {mini_app_id}"
            )));
        }
        let version = "bundled".to_string();
        let mut state = self.state()?;
        state.installed.insert(mini_app_id.clone(), version.clone());
        state.events.push_back(HostEvent::MarketplaceInstalled {
            timestamp: timestamp(),
            mini_app_id,
            version,
        });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn mcp_instruction_context(&self) -> Result<Option<String>, FeatureHostError> {
        let instructions = match self
            .runtime()?
            .execute(RuntimeCommand::McpCustomInstructions)?
        {
            RuntimeResponse::McpCustomInstructions { instructions } => instructions,
            other => return Err(unexpected_response("mcp.customInstructions", other)),
        };
        Ok(render_mcp_instruction_context(&instructions))
    }

    #[cfg(feature = "production")]
    fn production_chat(
        &self,
        request_id: String,
        text: String,
        agent_id: Option<String>,
        requested_conversation_id: Option<String>,
        mode: AgentMode,
        mode_statement: Option<String>,
        model: Option<String>,
        attachments: Vec<AttachmentContext>,
        reply_to_message_id: Option<String>,
        is_fork: bool,
    ) -> Result<CommandAccepted, FeatureHostError> {
        self.require_authenticated_account()?;
        let text = required(text, "chat text")?;
        let bot_conversation_id = if let Some(agent_id) = agent_id.as_deref() {
            self.state()?
                .bots
                .get(agent_id)
                .and_then(|bot| bot.conversation_id.clone())
        } else {
            None
        };
        if let Some(mini_app_id) = agent_id
            .as_deref()
            .filter(|id| *id != "mahayana-assistant" && bot_conversation_id.is_none())
        {
            match self
                .runtime()?
                .execute(RuntimeCommand::ApproveLocalPluginTool {
                    plugin_id: mini_app_id.to_string(),
                    tool: "chat".to_string(),
                })? {
                RuntimeResponse::LocalPluginToolApproved { .. } => {}
                other => return Err(unexpected_response("miniapp.chat.approve", other)),
            }
            let response = self
                .runtime()?
                .execute(RuntimeCommand::CallLocalPluginTool {
                    plugin_id: mini_app_id.to_string(),
                    tool: "chat".to_string(),
                    arguments: json!({"message": text}),
                })?;
            let reply = match response {
                RuntimeResponse::LocalPluginToolResult { result, .. } => result
                    .pointer("/content/0/text")
                    .and_then(Value::as_str)
                    .filter(|reply| !reply.is_empty())
                    .unwrap_or("已收到。请选择应用内的快捷操作继续。")
                    .to_string(),
                other => return Err(unexpected_response("miniapp.chat", other)),
            };
            let mut state = self.state()?;
            state.events.push_back(HostEvent::ChatMessage {
                timestamp: timestamp(),
                role: MessageRole::User,
                text,
                operation_id: None,
                message_id: None,
                reply_to_message_id: None,
                attachment_batch_id: None,
                attachment: None,
                branched: false,
            });
            state.events.push_back(HostEvent::ChatMessage {
                timestamp: timestamp(),
                role: MessageRole::Assistant,
                text: reply,
                operation_id: None,
                message_id: None,
                reply_to_message_id: None,
                attachment_batch_id: None,
                attachment: None,
                branched: false,
            });
            return Ok(CommandAccepted {
                request_id,
                operation_id: None,
            });
        }
        let conversation_id = requested_conversation_id
            .map(ConversationId)
            .or_else(|| bot_conversation_id.map(ConversationId))
            .unwrap_or_else(|| ConversationId(MAHAYANA_AI_CONVERSATION_ID.to_string()));
        let (provider, routed_model) = match self.runtime()?.execute(RuntimeCommand::Status)? {
            RuntimeResponse::Status(status) => (
                format!("{:?}", status.model_provider).to_lowercase(),
                status.model,
            ),
            other => return Err(unexpected_response("runtime.status", other)),
        };
        let media_channels = crate::send_message_shaping::split_send_media_channels(&attachments);
        let selected_image_data_urls = selected_image_data_urls(&media_channels.image_attachments);
        let memory_agent_id = agent_id.as_deref().unwrap_or("mahayana-assistant");
        let identity_context = {
            let state = self.state()?;
            render_native_turn_identity_context(
                state.auth_user.as_ref(),
                state.bots.get(memory_agent_id),
            )
        };
        let mut runtime_text = compose_agent_input(
            &text,
            mode,
            mode_statement.as_deref(),
            &media_channels.file_attachments,
        );
        if let Some(identity_context) = identity_context {
            runtime_text = format!("{identity_context}\n\n[Current turn]\n{runtime_text}");
        }
        runtime_text = crate::send_message_shaping::append_selected_video_context(
            runtime_text,
            &media_channels.selected_videos,
        );
        if let Some(mcp_context) = self.mcp_instruction_context()? {
            runtime_text = format!(
                "{mcp_context}

[Current turn]
{runtime_text}"
            );
        }
        if is_safe_memory_agent_id(memory_agent_id) {
            if let Some(root) = self.active_account_root(self.memory_root_path.as_deref()) {
                let memory_dir = root.join(memory_agent_id).join("memory");
                let memory_prompt = render_memory_system_prompt(&memory_dir);
                if !memory_prompt.is_empty() {
                    runtime_text = format!(
                        "[Persistent agent memory]\n{memory_prompt}\n\n[Current turn]\n{runtime_text}"
                    );
                }
            }
            let account_workflow_root =
                self.active_account_root(self.workflow_root_path.as_deref());
            let account_memory_root = self.active_account_root(self.memory_root_path.as_deref());
            if let (Some(workflow_root), Some(agent_root)) = (
                account_workflow_root.as_deref(),
                account_memory_root.as_deref(),
            ) {
                let workflow_catalog =
                    render_workflow_catalog(workflow_root, agent_root, memory_agent_id);
                if !workflow_catalog.is_empty() {
                    runtime_text =
                        format!("[Available workflows]\n{workflow_catalog}\n\n{runtime_text}");
                }
            }
        }
        let response = self.runtime()?.execute(RuntimeCommand::SendMessage {
            conversation_id,
            text: runtime_text,
            display_text: Some(text.clone()),
            client_message_id: Some(request_id.clone()),
            hidden: false,
            show_assistant_output: false,
            recovery_eligible: attachments.is_empty(),
            reply_to_message_id: reply_to_message_id
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            is_fork,
            attachment_batch_id: (!attachments.is_empty())
                .then(|| format!("attachment-batch:{request_id}")),
            selected_image_data_urls,
        })?;
        let operation_id = match response {
            RuntimeResponse::Accepted { operation_id } => operation_id.to_string(),
            other => return Err(unexpected_response("chat.send", other)),
        };
        let mut state = self.state()?;
        state.operations.insert(operation_id.clone());
        state.operation_agents.insert(
            operation_id.clone(),
            agent_id
                .clone()
                .unwrap_or_else(|| "mahayana-assistant".into()),
        );
        state.events.push_back(HostEvent::ChatMessage {
            timestamp: timestamp(),
            role: MessageRole::User,
            text,
            operation_id: None,
            message_id: None,
            reply_to_message_id: None,
            attachment_batch_id: None,
            attachment: None,
            branched: false,
        });
        state.events.push_back(HostEvent::OperationStarted {
            timestamp: timestamp(),
            operation_id: operation_id.clone(),
            label: "chat-response".into(),
            interruptible: true,
        });
        state.events.push_back(HostEvent::ModelRouted {
            timestamp: timestamp(),
            operation_id: operation_id.clone(),
            provider,
            model: routed_model.clone(),
            mode,
        });
        if let Some(preferred_model) = model.filter(|preferred| preferred != &routed_model) {
            state.events.push_back(HostEvent::AgentStep {
                timestamp: timestamp(),
                operation_id: Some(operation_id.clone()),
                step_id: format!("{operation_id}:model-preference"),
                kind: "model".into(),
                title: format!("使用已配置模型 {routed_model}"),
                detail: Some(format!(
                    "本次偏好 {preferred_model}；当前 Runtime 不支持会话中热切换"
                )),
                status: AgentStepStatus::Completed,
                progress: None,
                total: None,
            });
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: Some(operation_id),
        })
    }

    #[cfg(feature = "production")]
    fn production_list_conversations(
        &self,
        request_id: String,
        query: Option<String>,
    ) -> Result<CommandAccepted, FeatureHostError> {
        self.require_authenticated_account()?;
        self.production_list_conversations_from_runtime(request_id, query)
    }

    #[cfg(feature = "production")]
    fn production_list_conversations_from_runtime(
        &self,
        request_id: String,
        query: Option<String>,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let conversations = match self.runtime()?.execute(RuntimeCommand::ListConversations)? {
            RuntimeResponse::Conversations { data } => data,
            other => return Err(unexpected_response("conversation.list", other)),
        };
        let query = query.map(|query| query.to_lowercase());
        let conversations = conversations
            .into_iter()
            .filter(|conversation| {
                query.as_ref().is_none_or(|query| {
                    conversation.title.to_lowercase().contains(query)
                        || conversation.id.0.to_lowercase().contains(query)
                })
            })
            .map(|conversation| ConversationSummary {
                id: conversation.id.0,
                title: conversation.title,
                kind: conversation.peer.provider_key().into(),
                pinned: conversation.pinned,
                unread_count: conversation.unread_count,
                updated_at_ms: conversation.updated_at_ms,
            })
            .collect();
        self.state()?
            .events
            .push_back(HostEvent::ConversationListed {
                timestamp: timestamp(),
                conversations,
            });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn production_read_conversation_messages(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<ConversationMessage>, FeatureHostError> {
        let limit = u32::try_from(limit.clamp(1, 500)).map_err(|_| {
            FeatureHostError::Contract("conversation history limit exceeds u32".into())
        })?;
        let messages = match self
            .runtime()?
            .execute(RuntimeCommand::ConversationHistory {
                conversation_id: ConversationId(conversation_id.to_string()),
                limit: Some(limit),
            })? {
            RuntimeResponse::History { data } => data,
            other => return Err(unexpected_response("conversation.history", other)),
        };
        Ok(messages
            .into_iter()
            .map(|message| ConversationMessage {
                id: message.id.0,
                role: match message.role {
                    RuntimeMessageRole::User => MessageRole::User,
                    _ => MessageRole::Assistant,
                },
                reply_to_message_id: message
                    .metadata
                    .get("replyToMessageId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                branched: message
                    .metadata
                    .get("branched")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                reactions: message
                    .metadata
                    .get("reactions")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|row| {
                                let emoji = row.get("emoji")?.as_str()?.trim();
                                let by = row.get("by")?.as_str()?.trim();
                                (!emoji.is_empty() && !by.is_empty()).then(|| TranscriptReaction {
                                    emoji: emoji.to_string(),
                                    by: by.to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                text: message.text,
                created_at_ms: message.created_at_ms,
            })
            .collect())
    }

    #[cfg(feature = "production")]
    fn production_read_conversation_window(
        &self,
        conversation_id: &str,
        before_message_id: Option<&str>,
        after_message_id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<ConversationMessage>, FeatureHostError> {
        let messages = match self
            .runtime()?
            .execute(RuntimeCommand::ConversationHistoryWindow {
                conversation_id: ConversationId(conversation_id.to_string()),
                before_message_id: before_message_id.map(ToOwned::to_owned),
                after_message_id: after_message_id.map(ToOwned::to_owned),
                limit,
            })? {
            RuntimeResponse::History { data } => data,
            other => return Err(unexpected_response("conversation.historyWindow", other)),
        };
        Ok(messages
            .into_iter()
            .map(|message| ConversationMessage {
                id: message.id.0,
                role: match message.role {
                    RuntimeMessageRole::User => MessageRole::User,
                    _ => MessageRole::Assistant,
                },
                reply_to_message_id: message
                    .metadata
                    .get("replyToMessageId")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned),
                branched: message
                    .metadata
                    .get("branched")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                reactions: message
                    .metadata
                    .get("reactions")
                    .and_then(Value::as_array)
                    .map(|rows| {
                        rows.iter()
                            .filter_map(|row| {
                                let emoji = row.get("emoji")?.as_str()?.trim();
                                let by = row.get("by")?.as_str()?.trim();
                                (!emoji.is_empty() && !by.is_empty()).then(|| TranscriptReaction {
                                    emoji: emoji.to_string(),
                                    by: by.to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                text: message.text,
                created_at_ms: message.created_at_ms,
            })
            .collect())
    }

    #[cfg(feature = "production")]
    fn production_open_conversation(
        &self,
        request_id: String,
        conversation_id: String,
    ) -> Result<CommandAccepted, FeatureHostError> {
        self.require_authenticated_account()?;
        self.production_open_conversation_from_runtime(request_id, conversation_id)
    }

    #[cfg(feature = "production")]
    fn production_open_conversation_from_runtime(
        &self,
        request_id: String,
        conversation_id: String,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let conversation_id = required(conversation_id, "conversationId")?;
        let messages = self.production_read_conversation_messages(&conversation_id, 200)?;
        {
            let mut state = self.state()?;
            let previous = state
                .conversation_session
                .switch_immediately(&conversation_id);
            state.events.push_back(HostEvent::ConversationOpened {
                timestamp: timestamp(),
                conversation_id: conversation_id.clone(),
                messages,
            });
            state.events.push_back(HostEvent::ConversationActivated {
                timestamp: timestamp(),
                conversation_id: conversation_id.clone(),
                previous_conversation_id: previous,
                generation: None,
            });
        }
        let _ = self.production_list_conversations_from_runtime(
            format!("session-explicit-switch-{request_id}"),
            None,
        );
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn production_open_conversation_windowed(
        &self,
        request_id: String,
        conversation_id: String,
        before_message_id: Option<String>,
        limit: usize,
    ) -> Result<CommandAccepted, FeatureHostError> {
        self.require_authenticated_account()?;
        let conversation_id = required(conversation_id, "conversationId")?;
        let limit = limit.clamp(1, 200);
        let mut messages = self.production_read_conversation_window(
            &conversation_id,
            before_message_id.as_deref(),
            None,
            Some(u32::try_from(limit + 1).unwrap_or(201)),
        )?;
        let has_older = messages.len() > limit;
        if has_older {
            messages.remove(0);
        }
        let next_before_message_id = has_older
            .then(|| messages.first().map(|message| message.id.clone()))
            .flatten();
        let shipped_through_id = messages.last().map(|message| message.id.clone());
        let mut state = self.state()?;
        let was_active = state.conversation_session.active_conversation_id.as_deref()
            == Some(conversation_id.as_str());
        state.events.push_back(HostEvent::ConversationWindowOpened {
            timestamp: timestamp(),
            request_id: Some(request_id.clone()),
            conversation_id: conversation_id.clone(),
            messages,
            next_before_message_id,
        });
        if !was_active {
            state
                .conversation_session
                .schedule_deferred_activation(&conversation_id, shipped_through_id.as_deref());
        }
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn production_list_capabilities(
        &self,
        request_id: String,
        query: Option<String>,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let response = self
            .runtime()?
            .execute(RuntimeCommand::ListCapabilities { query })?;
        let data = match response {
            RuntimeResponse::Capabilities { data } => data,
            other => return Err(unexpected_response("capability.list", other)),
        };
        let capabilities = data
            .into_iter()
            .map(|capability| CapabilitySummary {
                id: capability.id,
                title: capability.title,
                kind: match capability.kind {
                    CapabilityKind::Agent => "agent",
                    CapabilityKind::Bot => "bot",
                    CapabilityKind::Plugin => "plugin",
                    CapabilityKind::MiniApp => "miniApp",
                    CapabilityKind::Application => "application",
                    CapabilityKind::Contact => "contact",
                }
                .into(),
                mention: capability.mention,
                conversation_id: capability.conversation_id.to_string(),
                provider: capability.provider,
                plugin_id: capability.plugin_id,
                description: capability.description,
                required_permissions: capability.required_permissions,
                availability: match capability.availability {
                    CapabilityAvailability::Ready => "ready",
                    CapabilityAvailability::PermissionRequired => "permissionRequired",
                    CapabilityAvailability::Unavailable => "unavailable",
                }
                .into(),
                unavailable_reason: capability.unavailable_reason,
            })
            .collect();
        self.state()?.events.push_back(HostEvent::CapabilityListed {
            timestamp: timestamp(),
            capabilities,
        });
        Ok(CommandAccepted {
            request_id,
            operation_id: None,
        })
    }

    #[cfg(feature = "production")]
    fn execute_production(
        &self,
        command: FeatureCommand,
    ) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        {
            let state = self.state()?;
            ensure_open(&state)?;
        }
        match command {
            FeatureCommand::ChatSend {
                text,
                agent_id,
                conversation_id,
                mode,
                mode_statement,
                model,
                attachments,
                reply_to_message_id,
                is_fork,
                ..
            } => self.production_chat(
                request_id,
                text,
                agent_id,
                conversation_id,
                mode,
                mode_statement,
                model,
                attachments,
                reply_to_message_id,
                is_fork,
            ),
            FeatureCommand::ConversationList { query, .. } => {
                self.production_list_conversations(request_id, query)
            }
            FeatureCommand::ConversationOpen {
                conversation_id, ..
            } => self.production_open_conversation(request_id, conversation_id),
            FeatureCommand::ConversationOpenWindowed {
                conversation_id,
                before_message_id,
                limit,
                ..
            }
            | FeatureCommand::ConversationOpenTail {
                conversation_id,
                before_message_id,
                limit,
                ..
            } => self.production_open_conversation_windowed(
                request_id,
                conversation_id,
                before_message_id,
                limit,
            ),
            FeatureCommand::CapabilityList { query, .. } => {
                self.production_list_capabilities(request_id, query)
            }
            FeatureCommand::MarketplaceInstall { mini_app_id, .. } => {
                self.production_install(request_id, mini_app_id)
            }
            FeatureCommand::MiniAppOpen { mini_app_id, .. } => {
                self.production_open(request_id, mini_app_id)
            }
            FeatureCommand::CapabilityRequest {
                mini_app_id,
                capability,
                reason,
                ..
            } => self.production_capability_request(request_id, mini_app_id, capability, reason),
            FeatureCommand::RuntimeLongTask { label, .. } => {
                self.production_long_task(request_id, label)
            }
            FeatureCommand::SessionClear { .. } => self.production_clear_session(request_id),
            _ => unreachable!(
                "automation and product-surface commands are intercepted before production dispatch"
            ),
        }
    }

    fn execute_test(&self, command: FeatureCommand) -> Result<CommandAccepted, FeatureHostError> {
        let request_id = command.request_id().to_string();
        let mut state = self.state()?;
        ensure_open(&state)?;
        match command {
            FeatureCommand::ChatSend {
                text,
                agent_id,
                mode,
                model,
                attachments,
                ..
            } => {
                let text = required(text, "chat text")?;
                let operation_id = next_id(&mut state, "chat");
                state.events.push_back(HostEvent::ChatMessage {
                    timestamp: timestamp(),
                    role: MessageRole::User,
                    text: text.clone(),
                    operation_id: None,
                    message_id: None,
                    reply_to_message_id: None,
                    attachment_batch_id: None,
                    attachment: None,
                    branched: false,
                });
                state.events.push_back(HostEvent::OperationStarted {
                    timestamp: timestamp(),
                    operation_id: operation_id.clone(),
                    label: "chat-response".into(),
                    interruptible: false,
                });
                state.events.push_back(HostEvent::ModelRouted {
                    timestamp: timestamp(),
                    operation_id: operation_id.clone(),
                    provider: "mahayana-test".into(),
                    model: model.unwrap_or_else(|| "auto".into()),
                    mode,
                });
                state.events.push_back(HostEvent::AgentStep {
                    timestamp: timestamp(),
                    operation_id: Some(operation_id.clone()),
                    step_id: format!("{operation_id}:context"),
                    kind: "context".into(),
                    title: if attachments.is_empty() {
                        "分析请求".into()
                    } else {
                        format!("读取 {} 个附件", attachments.len())
                    },
                    detail: None,
                    status: AgentStepStatus::Completed,
                    progress: Some(1),
                    total: Some(1),
                });
                state.events.push_back(HostEvent::ChatMessage {
                    timestamp: timestamp(),
                    role: MessageRole::Assistant,
                    text: agent_id
                        .filter(|id| id != "mahayana-assistant")
                        .map(|id| format!("{id}机器人收到：{text}"))
                        .unwrap_or_else(|| format!("收到：{text}")),
                    operation_id: Some(operation_id.clone()),
                    message_id: None,
                    reply_to_message_id: None,
                    attachment_batch_id: None,
                    attachment: None,
                    branched: false,
                });
                state.events.push_back(HostEvent::UsageUpdated {
                    timestamp: timestamp(),
                    operation_id: operation_id.clone(),
                    input_tokens: text.chars().count() as i64,
                    cached_input_tokens: 0,
                    output_tokens: 8,
                    reasoning_tokens: 0,
                    total_tokens: text.chars().count() as i64 + 8,
                    context_window: Some(128_000),
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: Some(operation_id),
                })
            }
            FeatureCommand::ConversationList { query, .. } => {
                let mut conversations = vec![ConversationSummary {
                    id: MAHAYANA_AI_CONVERSATION_ID.into(),
                    title: "大乘助手".into(),
                    kind: "codex".into(),
                    pinned: true,
                    unread_count: 0,
                    updated_at_ms: 0,
                }];
                if let Some(query) = query {
                    conversations.retain(|item| item.title.contains(&query));
                }
                state.events.push_back(HostEvent::ConversationListed {
                    timestamp: timestamp(),
                    conversations,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::ConversationOpen {
                conversation_id, ..
            } => {
                let previous = state
                    .conversation_session
                    .switch_immediately(&conversation_id);
                state.events.push_back(HostEvent::ConversationOpened {
                    timestamp: timestamp(),
                    conversation_id: conversation_id.clone(),
                    messages: Vec::new(),
                });
                state.events.push_back(HostEvent::ConversationActivated {
                    timestamp: timestamp(),
                    conversation_id,
                    previous_conversation_id: previous,
                    generation: None,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::ConversationOpenWindowed {
                request_id,
                conversation_id,
                before_message_id,
                limit,
            }
            | FeatureCommand::ConversationOpenTail {
                request_id,
                conversation_id,
                before_message_id,
                limit,
            } => {
                let (messages, next_before_message_id) =
                    bounded_conversation_window(&[], before_message_id.as_deref(), limit);
                let shipped_through_id = messages.last().map(|message| message.id.clone());
                let was_active = state.conversation_session.active_conversation_id.as_deref()
                    == Some(conversation_id.as_str());
                state.events.push_back(HostEvent::ConversationWindowOpened {
                    timestamp: timestamp(),
                    request_id: Some(request_id.clone()),
                    conversation_id: conversation_id.clone(),
                    messages,
                    next_before_message_id,
                });
                if !was_active {
                    state.conversation_session.schedule_deferred_activation(
                        &conversation_id,
                        shipped_through_id.as_deref(),
                    );
                }
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::CapabilityList { query, .. } => {
                let mut capabilities = vec![CapabilitySummary {
                    id: "agent.mahayana".into(),
                    title: "大乘助手".into(),
                    kind: "agent".into(),
                    mention: "@agent.mahayana".into(),
                    conversation_id: MAHAYANA_AI_CONVERSATION_ID.into(),
                    provider: "codex".into(),
                    plugin_id: None,
                    description: "大乘共享智能代理".into(),
                    required_permissions: Vec::new(),
                    availability: "ready".into(),
                    unavailable_reason: None,
                }];
                capabilities.extend(state.installed.keys().map(|plugin_id| CapabilitySummary {
                    id: format!("miniapp.{plugin_id}"),
                    title: plugin_id.clone(),
                    kind: "miniApp".into(),
                    mention: format!("@miniapp.{plugin_id}"),
                    conversation_id: format!("miniapp:{plugin_id}"),
                    provider: "miniapp".into(),
                    plugin_id: Some(plugin_id.clone()),
                    description: "大乘共享插件、小程序、应用或机器人能力".into(),
                    required_permissions: Vec::new(),
                    availability: "ready".into(),
                    unavailable_reason: None,
                }));
                if let Some(query) = query {
                    let query = query.to_lowercase();
                    capabilities.retain(|item| {
                        item.id.to_lowercase().contains(&query)
                            || item.title.to_lowercase().contains(&query)
                            || item.description.to_lowercase().contains(&query)
                    });
                }
                state.events.push_back(HostEvent::CapabilityListed {
                    timestamp: timestamp(),
                    capabilities,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::MarketplaceInstall { mini_app_id, .. } => {
                let mini_app_id = required(mini_app_id, "miniAppId")?;
                let version = "1.0.0".to_string();
                state.installed.insert(mini_app_id.clone(), version.clone());
                state.events.push_back(HostEvent::MarketplaceInstalled {
                    timestamp: timestamp(),
                    mini_app_id,
                    version,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::MiniAppOpen { mini_app_id, .. } => {
                let mini_app_id = required(mini_app_id, "miniAppId")?;
                if !state.installed.contains_key(&mini_app_id) {
                    return Err(FeatureHostError::Contract(format!(
                        "MiniApp is not installed: {mini_app_id}"
                    )));
                }
                state.events.push_back(HostEvent::MiniAppOpened {
                    timestamp: timestamp(),
                    mini_app_id,
                    html: Some(
                        "<!doctype html><html><body><h1>测试 MiniApp</h1></body></html>".into(),
                    ),
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::CapabilityRequest {
                mini_app_id,
                capability,
                reason,
                ..
            } => {
                let mini_app_id = required(mini_app_id, "miniAppId")?;
                let capability = required(capability, "capability")?;
                let reason = required(reason, "reason")?;
                let approval_id = next_id(&mut state, "approval");
                state.pending_approvals.insert(
                    approval_id.clone(),
                    PendingApproval {
                        mini_app_id: mini_app_id.clone(),
                        capability: capability.clone(),
                        runtime_approval_id: None,
                    },
                );
                state.events.push_back(HostEvent::ApprovalRequested {
                    timestamp: timestamp(),
                    operation_id: None,
                    approval_id,
                    mini_app_id,
                    capability,
                    reason,
                    kind: Some("capability".into()),
                    subject: None,
                    detail: None,
                    proposed_rule: None,
                    location: None,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            FeatureCommand::RuntimeLongTask { label, .. } => {
                let label = required(label, "operation label")?;
                let operation_id = next_id(&mut state, "operation");
                state.operations.insert(operation_id.clone());
                state.events.push_back(HostEvent::OperationStarted {
                    timestamp: timestamp(),
                    operation_id: operation_id.clone(),
                    label,
                    interruptible: true,
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: Some(operation_id),
                })
            }
            FeatureCommand::SessionClear { .. } => {
                state.session_active = false;
                state.conversation_session = ConversationSessionState::default();
                state.events.push_back(HostEvent::SessionCleared {
                    timestamp: timestamp(),
                });
                Ok(CommandAccepted {
                    request_id,
                    operation_id: None,
                })
            }
            _ => unreachable!(
                "automation and product-surface commands are intercepted before test dispatch"
            ),
        }
    }

    fn state(&self) -> Result<MutexGuard<'_, FeatureState>, FeatureHostError> {
        self.state
            .lock()
            .map_err(|_| FeatureHostError::StatePoisoned)
    }
}

fn transcript_cards_from_metadata(metadata: &Value) -> Vec<TranscriptCard> {
    if let Some(cards) = metadata.get("cards").and_then(Value::as_array) {
        return cards.iter().filter_map(decode_transcript_card).collect();
    }
    for field in ["transcriptCard", "card", "artifact"] {
        if let Some(card) = metadata.get(field).and_then(decode_transcript_card) {
            return vec![card];
        }
    }
    decode_transcript_card(metadata).into_iter().collect()
}

fn decode_transcript_card(value: &Value) -> Option<TranscriptCard> {
    let mut value = value.clone();
    let object = value.as_object_mut()?;
    if !object.contains_key("kind") {
        let kind = object.get("type").cloned()?;
        object.insert("kind".into(), kind);
    }
    serde_json::from_value(value).ok()
}

fn is_product_surface_command(command: &FeatureCommand) -> bool {
    matches!(
        command,
        FeatureCommand::ConnectorList { .. }
            | FeatureCommand::ConnectorConnect { .. }
            | FeatureCommand::ConnectorRenameAccount { .. }
            | FeatureCommand::ConnectorRemoveAccount { .. }
            | FeatureCommand::ConnectorSetToolEnabled { .. }
            | FeatureCommand::SkillList { .. }
            | FeatureCommand::SkillUpsert { .. }
            | FeatureCommand::SkillDelete { .. }
            | FeatureCommand::SkillPublish { .. }
            | FeatureCommand::SkillUnpublish { .. }
            | FeatureCommand::SkillSync { .. }
            | FeatureCommand::BotList { .. }
            | FeatureCommand::BotSetHidden { .. }
            | FeatureCommand::DraftResolve { .. }
            | FeatureCommand::SecretProvide { .. }
            | FeatureCommand::ListenerList { .. }
            | FeatureCommand::ListenerConnect { .. }
            | FeatureCommand::ListenerDisconnect { .. }
            | FeatureCommand::UpdateStatus { .. }
            | FeatureCommand::UpdateCheck { .. }
            | FeatureCommand::UpdateInstall { .. }
    )
}

#[cfg(feature = "production")]
#[derive(Debug, Clone, Default)]
struct LiveConnectorProjection {
    server_name: Option<String>,
    connector_id: Option<String>,
    install_url: Option<String>,
    status: Option<ConnectorStatus>,
    accounts: BTreeMap<String, ConnectorAccountSummary>,
    tools: BTreeMap<String, ConnectorToolSummary>,
    tool_schemas: BTreeMap<String, Value>,
}

#[cfg(feature = "production")]
fn connector_key_from_name(value: &str) -> Option<&'static str> {
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized.contains("gmail") || normalized.contains("googlemail") {
        Some("gmail")
    } else if normalized.contains("github") {
        Some("github")
    } else if normalized.contains("slack") {
        Some("slack")
    } else if normalized.contains("microsoftteams") || normalized == "teams" {
        Some("teams")
    } else if normalized.contains("linear") {
        Some("linear")
    } else if normalized.contains("sentry") {
        Some("sentry")
    } else if normalized.contains("pagerduty") {
        Some("pagerduty")
    } else if normalized == "git" {
        Some("git")
    } else {
        None
    }
}

#[cfg(feature = "production")]
fn connector_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut needs_dash = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if needs_dash && !slug.is_empty() {
                slug.push('-');
            }
            needs_dash = false;
            slug.push(character.to_ascii_lowercase());
        } else {
            needs_dash = true;
        }
    }
    if slug.is_empty() { "app".into() } else { slug }
}

#[cfg(feature = "production")]
fn connector_status_from_auth(auth_status: Option<&str>, has_tools: bool) -> ConnectorStatus {
    match auth_status.unwrap_or_default() {
        "notLoggedIn" => ConnectorStatus::AuthRequired,
        "oAuth" | "bearerToken" => ConnectorStatus::Connected,
        "unsupported" if has_tools => ConnectorStatus::Connected,
        "unknown" if has_tools => ConnectorStatus::Connected,
        _ if has_tools => ConnectorStatus::Connected,
        _ => ConnectorStatus::Disconnected,
    }
}

#[cfg(feature = "production")]
fn live_connector_projections(
    servers: &[Value],
    apps: &[Value],
) -> BTreeMap<String, LiveConnectorProjection> {
    let mut live = BTreeMap::<String, LiveConnectorProjection>::new();
    for app in apps {
        let name = app.get("name").and_then(Value::as_str).unwrap_or_default();
        let id = app.get("id").and_then(Value::as_str).unwrap_or_default();
        let Some(key) = connector_key_from_name(name).or_else(|| connector_key_from_name(id))
        else {
            continue;
        };
        let entry = live.entry(key.to_string()).or_default();
        entry.connector_id = (!id.is_empty()).then(|| id.to_string());
        entry.install_url = app
            .get("installUrl")
            .and_then(Value::as_str)
            .map(str::to_string);
        if app.get("isAccessible").and_then(Value::as_bool) == Some(true) {
            entry.status = Some(ConnectorStatus::Connected);
        }
    }
    for server in servers {
        let server_name = server
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let auth_status = server.get("authStatus").and_then(Value::as_str);
        let tools = server
            .get("tools")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let direct_key = connector_key_from_name(server_name);
        if let Some(key) = direct_key {
            let entry = live.entry(key.to_string()).or_default();
            entry.server_name = Some(server_name.to_string());
            entry.status = Some(connector_status_from_auth(auth_status, !tools.is_empty()));
        }
        for (wire_name, tool) in tools {
            let meta = tool.get("_meta").and_then(Value::as_object);
            let connector_name = meta
                .and_then(|meta| meta.get("connector_name"))
                .and_then(Value::as_str);
            let connector_id = meta
                .and_then(|meta| meta.get("connector_id"))
                .and_then(Value::as_str);
            let Some(key) = connector_name
                .and_then(connector_key_from_name)
                .or_else(|| connector_id.and_then(connector_key_from_name))
                .or(direct_key)
            else {
                continue;
            };
            let entry = live.entry(key.to_string()).or_default();
            entry.server_name = Some(server_name.to_string());
            entry.status = Some(ConnectorStatus::Connected);
            if let Some(connector_id) = connector_id {
                entry.connector_id = Some(connector_id.to_string());
                if entry.install_url.is_none() {
                    let display = connector_name.unwrap_or(key);
                    entry.install_url = Some(format!(
                        "https://chatgpt.com/apps/{}/{}",
                        connector_slug(display),
                        connector_id
                    ));
                }
            }
            let tool_id = tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(wire_name.as_str())
                .to_string();
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let read_only = tool
                .pointer("/annotations/readOnlyHint")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if let Some(schema) = tool
                .get("inputSchema")
                .or_else(|| tool.get("input_schema"))
                .cloned()
            {
                entry.tool_schemas.insert(tool_id.clone(), schema);
            }
            entry.tools.insert(
                tool_id.clone(),
                ConnectorToolSummary {
                    id: tool_id.clone(),
                    name: tool
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|title| !title.trim().is_empty())
                        .unwrap_or(tool_id.as_str())
                        .to_string(),
                    description,
                    enabled: true,
                    requires_approval: Some(!read_only),
                },
            );
            let link_id = meta
                .and_then(|meta| meta.get("link_id"))
                .and_then(Value::as_str);
            let owner = meta
                .and_then(|meta| meta.get("link_owner_profile"))
                .and_then(Value::as_object);
            if link_id.is_some() || owner.is_some() {
                let account_id = link_id
                    .map(|link_id| format!("mcp:{key}:{link_id}"))
                    .unwrap_or_else(|| format!("mcp:{key}"));
                let email = owner
                    .and_then(|owner| owner.get("email"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let label = owner
                    .and_then(|owner| {
                        owner
                            .get("name")
                            .or_else(|| owner.get("nickname"))
                            .and_then(Value::as_str)
                    })
                    .map(str::to_string)
                    .or_else(|| email.clone())
                    .or_else(|| connector_name.map(str::to_string))
                    .unwrap_or_else(|| key.to_string());
                entry.accounts.insert(
                    account_id.clone(),
                    ConnectorAccountSummary {
                        id: account_id,
                        label,
                        status: ConnectorStatus::Connected,
                        email,
                        team_managed: Some(false),
                        error: None,
                    },
                );
            }
        }
    }
    live
}

#[cfg(feature = "production")]
fn projection_send_tool<'a>(
    projection: &'a LiveConnectorProjection,
    connector_id: &str,
) -> Option<(&'a str, Option<&'a Value>)> {
    let preferred: &[&str] = match connector_id {
        "gmail" => &[
            "gmail.send_email",
            "send_email",
            "gmail.send_draft",
            "send_draft",
        ],
        "slack" => &[
            "slack.send_message",
            "slack.post_message",
            "send_message",
            "post_message",
        ],
        _ => &[],
    };
    for candidate in preferred {
        if let Some((tool_id, _)) = projection
            .tools
            .iter()
            .find(|(tool_id, _)| tool_id.as_str() == *candidate || tool_id.ends_with(candidate))
        {
            return Some((tool_id.as_str(), projection.tool_schemas.get(tool_id)));
        }
    }
    None
}

#[cfg(feature = "production")]
fn schema_property_is_array(schema: Option<&Value>, name: &str) -> bool {
    schema
        .and_then(|schema| schema.pointer(&format!("/properties/{name}/type")))
        .and_then(Value::as_str)
        == Some("array")
}

#[cfg(feature = "production")]
fn draft_tool_arguments(
    draft: &MessageDraft,
    schema: Option<&Value>,
) -> Result<Value, FeatureHostError> {
    let properties = schema
        .and_then(|schema| schema.get("properties"))
        .and_then(Value::as_object);
    match draft {
        MessageDraft::Email {
            from,
            to,
            cc,
            subject,
            body,
            ..
        } => {
            if to.is_empty() {
                return Err(FeatureHostError::Contract(
                    "email draft requires at least one recipient".into(),
                ));
            }
            let mut arguments = serde_json::Map::new();
            if schema_property_is_array(schema, "to") {
                arguments.insert("to".into(), json!(to));
            } else {
                arguments.insert("to".into(), Value::String(to.join(", ")));
            }
            arguments.insert("subject".into(), Value::String(subject.clone()));
            if properties.is_some_and(|properties| properties.contains_key("payload")) {
                arguments.insert(
                    "payload".into(),
                    json!({
                        "mime_type": "text/plain",
                        "charset": "UTF-8",
                        "body": {"content": body}
                    }),
                );
            } else if properties.is_some_and(|properties| properties.contains_key("message")) {
                arguments.insert("message".into(), Value::String(body.clone()));
            } else {
                arguments.insert("body".into(), Value::String(body.clone()));
            }
            if let Some(cc) = cc.as_ref().filter(|cc| !cc.is_empty()) {
                if schema_property_is_array(schema, "cc") {
                    arguments.insert("cc".into(), json!(cc));
                } else {
                    arguments.insert("cc".into(), Value::String(cc.join(", ")));
                }
            }
            if let Some(from) = from.as_ref().filter(|from| !from.trim().is_empty()) {
                let key = if properties
                    .is_some_and(|properties| properties.contains_key("from_address"))
                {
                    "from_address"
                } else {
                    "from"
                };
                if properties.is_none_or(|properties| properties.contains_key(key)) {
                    arguments.insert(key.into(), Value::String(from.clone()));
                }
            }
            Ok(Value::Object(arguments))
        }
        MessageDraft::Slack {
            target,
            thread,
            body,
            ..
        } => {
            if target.trim().is_empty() || body.trim().is_empty() {
                return Err(FeatureHostError::Contract(
                    "Slack draft requires a target and message".into(),
                ));
            }
            let properties = properties.ok_or_else(|| {
                FeatureHostError::Contract(
                    "Slack connector did not expose an input schema for its send tool".into(),
                )
            })?;
            let target_key = [
                "channel",
                "channel_id",
                "target",
                "conversation",
                "conversation_id",
            ]
            .into_iter()
            .find(|key| properties.contains_key(*key))
            .ok_or_else(|| {
                FeatureHostError::Contract(
                    "Slack send tool has no supported channel/target parameter".into(),
                )
            })?;
            let body_key = ["text", "message", "body"]
                .into_iter()
                .find(|key| properties.contains_key(*key))
                .ok_or_else(|| {
                    FeatureHostError::Contract(
                        "Slack send tool has no supported message parameter".into(),
                    )
                })?;
            let mut arguments = serde_json::Map::new();
            arguments.insert(target_key.into(), Value::String(target.clone()));
            arguments.insert(body_key.into(), Value::String(body.clone()));
            if let Some(thread) = thread.as_ref().filter(|thread| !thread.trim().is_empty())
                && let Some(thread_key) = ["thread_ts", "thread", "thread_id"]
                    .into_iter()
                    .find(|key| properties.contains_key(*key))
            {
                arguments.insert(thread_key.into(), Value::String(thread.clone()));
            }
            Ok(Value::Object(arguments))
        }
    }
}

#[cfg(feature = "production")]
fn merge_live_connectors(
    mut connectors: Vec<ConnectorSummary>,
    live: &BTreeMap<String, LiveConnectorProjection>,
) -> Vec<ConnectorSummary> {
    for connector in &mut connectors {
        let Some(projection) = live.get(&connector.id) else {
            if connector.id == "git" {
                connector.status = ConnectorStatus::Connected;
                connector.can_add_account = false;
            }
            continue;
        };
        if let Some(status) = projection.status {
            connector.status = status;
        }
        connector.can_add_account = projection.install_url.is_some()
            || projection
                .server_name
                .as_deref()
                .is_some_and(|name| name != "codex_apps");
        if let Some(source) = projection
            .connector_id
            .as_ref()
            .or(projection.server_name.as_ref())
        {
            connector.source = Some(source.clone());
        }
        let enabled_preferences = connector
            .tools
            .iter()
            .map(|tool| (tool.id.clone(), tool.enabled))
            .collect::<BTreeMap<_, _>>();
        if !projection.tools.is_empty() {
            connector.tools = projection
                .tools
                .values()
                .cloned()
                .map(|mut tool| {
                    if let Some(enabled) = enabled_preferences.get(&tool.id) {
                        tool.enabled = *enabled;
                    }
                    tool
                })
                .collect();
        }
        if !projection.accounts.is_empty() {
            let labels = connector
                .accounts
                .iter()
                .map(|account| (account.id.clone(), account.label.clone()))
                .collect::<BTreeMap<_, _>>();
            connector.accounts = projection
                .accounts
                .values()
                .cloned()
                .map(|mut account| {
                    if let Some(label) = labels.get(&account.id) {
                        account.label = label.clone();
                    }
                    account
                })
                .collect();
        } else if connector.status == ConnectorStatus::Connected
            && connector.accounts.is_empty()
            && projection.server_name.as_deref() != Some("codex_apps")
        {
            connector.accounts.push(ConnectorAccountSummary {
                id: format!("mcp:{}", connector.id),
                label: projection
                    .server_name
                    .clone()
                    .unwrap_or_else(|| connector.display_name.clone()),
                status: ConnectorStatus::Connected,
                email: None,
                team_managed: Some(false),
                error: None,
            });
        }
    }
    connectors.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    connectors
}

fn listener_platform_slug(platform: ListenerPlatform) -> &'static str {
    match platform {
        ListenerPlatform::Slack => "slack",
        ListenerPlatform::Github => "github",
        ListenerPlatform::Git => "git",
        ListenerPlatform::Teams => "teams",
        ListenerPlatform::Linear => "linear",
        ListenerPlatform::Sentry => "sentry",
        ListenerPlatform::Pagerduty => "pagerduty",
    }
}

fn listener_platform_display(platform: ListenerPlatform) -> &'static str {
    match platform {
        ListenerPlatform::Slack => "Slack",
        ListenerPlatform::Github => "GitHub",
        ListenerPlatform::Git => "Git",
        ListenerPlatform::Teams => "Microsoft Teams",
        ListenerPlatform::Linear => "Linear",
        ListenerPlatform::Sentry => "Sentry",
        ListenerPlatform::Pagerduty => "PagerDuty",
    }
}

fn normalize_automation_trigger(
    trigger: AutomationTrigger,
) -> Result<AutomationTrigger, FeatureHostError> {
    match trigger {
        AutomationTrigger::Schedule { schedule } => {
            let schedule = normalize_automation_schedule(&schedule)?;
            Ok(AutomationTrigger::Schedule { schedule })
        }
        AutomationTrigger::Event {
            source,
            event,
            filter,
            filters,
        } => Ok(AutomationTrigger::Event {
            source,
            event: required(event, "automation event")?,
            filter: filter.filter(|value| !value.trim().is_empty()),
            filters: filters.filter(|value| !value.is_empty()),
        }),
        AutomationTrigger::Group { listeners } => {
            if listeners.len() < 2 || listeners.len() > 8 {
                return Err(FeatureHostError::Contract(
                    "automation trigger group must contain 2 to 8 listeners".into(),
                ));
            }
            let mut normalized = Vec::with_capacity(listeners.len());
            for listener in listeners {
                if matches!(listener, AutomationTrigger::Group { .. }) {
                    return Err(FeatureHostError::Contract(
                        "nested automation trigger groups are not supported".into(),
                    ));
                }
                normalized.push(normalize_automation_trigger(listener)?);
            }
            Ok(AutomationTrigger::Group {
                listeners: normalized,
            })
        }
    }
}

fn automation_trigger_legacy_schedule(trigger: &AutomationTrigger) -> String {
    match trigger {
        AutomationTrigger::Schedule { schedule } => schedule.clone(),
        AutomationTrigger::Event { source, event, .. } => {
            format!("event:{}:{event}", listener_platform_slug(*source))
        }
        AutomationTrigger::Group { listeners } => listeners
            .iter()
            .find_map(|listener| match listener {
                AutomationTrigger::Schedule { schedule } => Some(schedule.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "event:group".into()),
    }
}

fn automation_trigger_listener_platforms(
    trigger: &AutomationTrigger,
    output: &mut BTreeSet<ListenerPlatform>,
) {
    match trigger {
        AutomationTrigger::Schedule { .. } => {}
        AutomationTrigger::Event { source, .. } => {
            output.insert(*source);
        }
        AutomationTrigger::Group { listeners } => {
            for listener in listeners {
                automation_trigger_listener_platforms(listener, output);
            }
        }
    }
}

fn automation_trigger_event_label(trigger: &AutomationTrigger) -> Option<String> {
    match trigger {
        AutomationTrigger::Schedule { .. } => None,
        AutomationTrigger::Event { event, .. } => Some(event.clone()),
        AutomationTrigger::Group { listeners } => listeners
            .iter()
            .find_map(automation_trigger_event_label)
            .or_else(|| Some("group".into())),
    }
}

fn automation_trigger_first_event(
    trigger: &AutomationTrigger,
) -> Option<(ListenerPlatform, String, Option<String>)> {
    match trigger {
        AutomationTrigger::Schedule { .. } => None,
        AutomationTrigger::Event {
            source,
            event,
            filter,
            ..
        } => Some((*source, event.clone(), filter.clone())),
        AutomationTrigger::Group { listeners } => {
            listeners.iter().find_map(automation_trigger_first_event)
        }
    }
}

fn normalized_event_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn event_value_for_filter(event: &EventCard, key: &str) -> Option<String> {
    let normalized = normalized_event_key(key);
    match normalized.as_str() {
        "event" => return Some(event.event.clone()),
        "actor" | "user" | "username" => return event.actor.clone(),
        "url" => return event.url.clone(),
        "title" => return Some(event.title.clone()),
        "summary" | "message" | "messagecontains" => return Some(event.summary.clone()),
        _ => {}
    }
    let aliases: &[&str] = match normalized.as_str() {
        "repo" => &["repo", "repository"],
        "channel" => &["channel", "channelid"],
        "tenantid" => &["tenant", "tenantid"],
        "teamid" | "teamids" => &["team", "teamid", "teamids"],
        "channelid" | "channelids" => &["channel", "channelid", "channelids"],
        "projectid" | "projectids" => &["project", "projectid", "projectids"],
        "serviceid" | "serviceids" => &["service", "serviceid", "serviceids"],
        "statusid" | "statusids" => &["status", "statusid", "statusids"],
        "cycleid" | "cycleids" => &["cycle", "cycleid", "cycleids"],
        "emoji" => &["emoji", "reaction"],
        _ => &[normalized.as_str()],
    };
    event.fields.as_ref()?.iter().find_map(|field| {
        let label = normalized_event_key(&field.label);
        aliases.contains(&label.as_str()).then(|| field.value.clone())
    })
}

fn structured_event_filters_match(
    filters: &BTreeMap<String, Value>,
    event: &EventCard,
) -> bool {
    filters.iter().all(|(key, expected)| {
        if key == "events" {
            return expected.as_array().is_some_and(|values| {
                values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|value| value.eq_ignore_ascii_case(&event.event))
                })
            });
        }
        if key == "actorAllowlist" || key == "userAllowlist" {
            let Some(actor) = event.actor.as_deref() else { return false; };
            return expected.as_array().is_some_and(|values| {
                values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|value| value.trim_start_matches('@').eq_ignore_ascii_case(actor.trim_start_matches('@')))
                })
            });
        }
        let Some(actual) = event_value_for_filter(event, key) else {
            return false;
        };
        match expected {
            Value::String(value) => {
                if key == "messageContains" {
                    actual.to_ascii_lowercase().contains(&value.to_ascii_lowercase())
                } else {
                    actual.eq_ignore_ascii_case(value)
                }
            }
            Value::Bool(value) => actual.parse::<bool>().ok() == Some(*value),
            Value::Array(values) => values.iter().any(|value| {
                value.as_str().is_some_and(|value| {
                    actual
                        .split(|ch: char| ch.is_whitespace() || ch == ',')
                        .any(|item| item.eq_ignore_ascii_case(value))
                })
            }),
            _ => false,
        }
    })
}

fn automation_trigger_matches_event(
    trigger: &AutomationTrigger,
    event: &EventCard,
    serialized: &str,
) -> bool {
    match trigger {
        AutomationTrigger::Schedule { .. } => false,
        AutomationTrigger::Event {
            source,
            event: expected_event,
            filter,
            filters,
        } => {
            *source == event.source
                && (expected_event == "*" || expected_event == &event.event)
                && filter.as_ref().is_none_or(|filter| {
                    serialized
                        .to_ascii_lowercase()
                        .contains(&filter.to_ascii_lowercase())
                })
                && filters
                    .as_ref()
                    .is_none_or(|filters| structured_event_filters_match(filters, event))
        }
        AutomationTrigger::Group { listeners } => listeners
            .iter()
            .any(|listener| automation_trigger_matches_event(listener, event, serialized)),
    }
}

fn automation_next_run(
    trigger: &AutomationTrigger,
    schedule: &str,
    enabled: bool,
    after_ms: i64,
) -> Option<i64> {
    if !enabled {
        return None;
    }
    match trigger {
        AutomationTrigger::Schedule { schedule } => next_automation_run(schedule, after_ms),
        AutomationTrigger::Event { .. } => None,
        AutomationTrigger::Group { listeners } => listeners
            .iter()
            .filter_map(|listener| automation_next_run(listener, schedule, true, after_ms))
            .min(),
    }
}

fn connector_tool(id: &str, name: &str, description: &str) -> ConnectorToolSummary {
    ConnectorToolSummary {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        enabled: true,
        requires_approval: Some(true),
    }
}

fn connector_summary(
    id: &str,
    display_name: &str,
    description: &str,
    transport: ConnectorTransport,
    tools: Vec<ConnectorToolSummary>,
) -> ConnectorSummary {
    ConnectorSummary {
        id: id.into(),
        display_name: display_name.into(),
        description: description.into(),
        status: ConnectorStatus::Disconnected,
        is_team: false,
        can_add_account: true,
        transport,
        source: Some("Built in".into()),
        teammate_count: None,
        accounts: Vec::new(),
        tools,
    }
}

fn default_connectors() -> BTreeMap<String, ConnectorSummary> {
    [
        connector_summary(
            "github",
            "GitHub",
            "Repositories, pull requests, issues, comments and CI.",
            ConnectorTransport::Http,
            vec![
                connector_tool(
                    "read_repository",
                    "Read repository",
                    "Read repository files and metadata.",
                ),
                connector_tool(
                    "create_issue",
                    "Create issue",
                    "Create and update GitHub issues.",
                ),
                connector_tool(
                    "comment_pull_request",
                    "Comment on pull request",
                    "Post review comments on pull requests.",
                ),
            ],
        ),
        connector_summary(
            "slack",
            "Slack",
            "Messages, mentions, reactions and approved drafts.",
            ConnectorTransport::Http,
            vec![
                connector_tool(
                    "search_messages",
                    "Search messages",
                    "Search workspace messages and threads.",
                ),
                connector_tool(
                    "post_message",
                    "Post message",
                    "Send an approved message or thread reply.",
                ),
                connector_tool(
                    "add_reaction",
                    "Add reaction",
                    "Add a reaction to a message.",
                ),
            ],
        ),
        connector_summary(
            "teams",
            "Microsoft Teams",
            "Teams messages, mentions, channels and approved drafts.",
            ConnectorTransport::Http,
            vec![
                connector_tool(
                    "search_messages",
                    "Search messages",
                    "Search Teams channels and chats.",
                ),
                connector_tool(
                    "post_message",
                    "Post message",
                    "Send an approved Teams message.",
                ),
            ],
        ),
        connector_summary(
            "linear",
            "Linear",
            "Issues, comments, status changes and projects.",
            ConnectorTransport::Http,
            vec![
                connector_tool(
                    "read_issues",
                    "Read issues",
                    "Read Linear issues and projects.",
                ),
                connector_tool(
                    "update_issue",
                    "Update issue",
                    "Update issue state, assignee and fields.",
                ),
            ],
        ),
        connector_summary(
            "sentry",
            "Sentry",
            "Errors, regressions, releases and issue ownership.",
            ConnectorTransport::Http,
            vec![
                connector_tool(
                    "read_issues",
                    "Read issues",
                    "Read Sentry issues and events.",
                ),
                connector_tool(
                    "resolve_issue",
                    "Resolve issue",
                    "Resolve or assign a Sentry issue.",
                ),
            ],
        ),
        connector_summary(
            "pagerduty",
            "PagerDuty",
            "Incidents, acknowledgements, responders and escalation.",
            ConnectorTransport::Http,
            vec![
                connector_tool(
                    "read_incidents",
                    "Read incidents",
                    "Read incident details and timelines.",
                ),
                connector_tool(
                    "acknowledge_incident",
                    "Acknowledge incident",
                    "Acknowledge an incident after approval.",
                ),
            ],
        ),
        connector_summary(
            "git",
            "Git",
            "Local commits, branches and repository state.",
            ConnectorTransport::Command,
            vec![
                connector_tool(
                    "read_status",
                    "Read status",
                    "Read local repository status.",
                ),
                connector_tool(
                    "read_history",
                    "Read history",
                    "Read commit and branch history.",
                ),
            ],
        ),
    ]
    .into_iter()
    .map(|connector| (connector.id.clone(), connector))
    .collect()
}

fn default_skill_teams() -> Vec<SkillTeamSummary> {
    vec![SkillTeamSummary {
        id: "team-mahayana".into(),
        name: "Mahayana Team".into(),
    }]
}

fn default_skills() -> BTreeMap<String, SkillSummary> {
    [
        SkillSummary {
            id: "skill-research-brief".into(),
            name: "Research brief".into(),
            description: "Turn verified sources into a concise research brief.".into(),
            use_when: "Use when a task needs sourced research and a decision-ready summary.".into(),
            instructions: "Verify sources, distinguish facts from inference, and end with actionable conclusions.".into(),
            source: SkillSource::Private,
            publish_state: SkillPublishState::Local,
            owner_agent_id: Some("mahayana-assistant".into()),
            team_id: None,
            team_name: None,
            read_only: Some(false),
            updated_at_ms: 0,
        },
        SkillSummary {
            id: "skill-incident-response".into(),
            name: "Incident response".into(),
            description: "Coordinate incident triage across monitoring and communication tools.".into(),
            use_when: "Use when an alert or incident needs coordinated triage.".into(),
            instructions: "Establish severity, collect evidence, propose actions, and request approval before external changes.".into(),
            source: SkillSource::Team,
            publish_state: SkillPublishState::Managed,
            owner_agent_id: None,
            team_id: Some("team-mahayana".into()),
            team_name: Some("Mahayana Team".into()),
            read_only: Some(true),
            updated_at_ms: 0,
        },
    ]
    .into_iter()
    .map(|skill| (skill.id.clone(), skill))
    .collect()
}

fn default_bots() -> BTreeMap<String, BotSummary> {
    [
        BotSummary {
            id: "mahayana-assistant".into(),
            name: "大乘助手".into(),
            description: "General-purpose Mahayana assistant.".into(),
            title: String::new(),
            hidden: false,
            avatar: None,
            avatar_shape: None,
            avatar_color: None,
            notifications_enabled: true,
            notify_on_updates: true,
            unread: false,
            conversation_id: Some(MAHAYANA_AI_CONVERSATION_ID.into()),
        },
        BotSummary {
            id: "research-bot".into(),
            name: "Research Bot".into(),
            description: "Source verification and research synthesis.".into(),
            title: String::new(),
            hidden: false,
            avatar: None,
            avatar_shape: None,
            avatar_color: None,
            notifications_enabled: true,
            notify_on_updates: true,
            unread: false,
            conversation_id: Some("codex:agent:research".into()),
        },
        BotSummary {
            id: "incident-bot".into(),
            name: "Incident Bot".into(),
            description: "Incident triage and operational coordination.".into(),
            title: String::new(),
            hidden: true,
            avatar: None,
            avatar_shape: None,
            avatar_color: None,
            notifications_enabled: true,
            notify_on_updates: true,
            unread: false,
            conversation_id: Some("codex:agent:incident".into()),
        },
    ]
    .into_iter()
    .map(|bot| (bot.id.clone(), bot))
    .collect()
}

fn listener_summary(
    platform: ListenerPlatform,
    display_name: &str,
    blurb: &str,
) -> ListenerIntegrationSummary {
    ListenerIntegrationSummary {
        platform,
        display_name: display_name.into(),
        blurb: blurb.into(),
        is_connected: false,
        account_label: None,
        error: None,
    }
}

fn default_listeners() -> BTreeMap<ListenerPlatform, ListenerIntegrationSummary> {
    [
        listener_summary(
            ListenerPlatform::Github,
            "GitHub",
            "Let automations watch a repo's PRs, comments, issues, and CI.",
        ),
        listener_summary(
            ListenerPlatform::Git,
            "Git",
            "Wake automations on local commits, branches, tags, and repository changes.",
        ),
        listener_summary(
            ListenerPlatform::Slack,
            "Slack",
            "Wake automations on Slack messages, mentions, and reactions.",
        ),
        listener_summary(
            ListenerPlatform::Teams,
            "Microsoft Teams",
            "Wake automations on Teams messages, mentions, and reactions.",
        ),
        listener_summary(
            ListenerPlatform::Linear,
            "Linear",
            "Wake automations on issues, comments, status changes, and assignments.",
        ),
        listener_summary(
            ListenerPlatform::Sentry,
            "Sentry",
            "Wake automations on new, regressed, assigned, and resolved issues.",
        ),
        listener_summary(
            ListenerPlatform::Pagerduty,
            "PagerDuty",
            "Wake automations when incidents are triggered, acknowledged, escalated, or resolved.",
        ),
    ]
    .into_iter()
    .map(|integration| (integration.platform, integration))
    .collect()
}

fn listener_platform_for_connector(connector_id: &str) -> Option<ListenerPlatform> {
    match connector_id {
        "github" => Some(ListenerPlatform::Github),
        "git" => Some(ListenerPlatform::Git),
        "slack" => Some(ListenerPlatform::Slack),
        "teams" => Some(ListenerPlatform::Teams),
        "linear" => Some(ListenerPlatform::Linear),
        "sentry" => Some(ListenerPlatform::Sentry),
        "pagerduty" => Some(ListenerPlatform::Pagerduty),
        _ => None,
    }
}

fn connector_for_listener_platform(platform: ListenerPlatform) -> Option<&'static str> {
    match platform {
        ListenerPlatform::Github => Some("github"),
        ListenerPlatform::Git => Some("git"),
        ListenerPlatform::Slack => Some("slack"),
        ListenerPlatform::Teams => Some("teams"),
        ListenerPlatform::Linear => Some("linear"),
        ListenerPlatform::Sentry => Some("sentry"),
        ListenerPlatform::Pagerduty => Some("pagerduty"),
    }
}

fn validate_draft(draft: &MessageDraft) -> Result<(), FeatureHostError> {
    match draft {
        MessageDraft::Email {
            to, subject, body, ..
        } => {
            if to.is_empty()
                || to
                    .iter()
                    .any(|recipient| !recipient.contains('@') || recipient.trim().is_empty())
            {
                return Err(FeatureHostError::Contract(
                    "email draft requires valid recipients".into(),
                ));
            }
            required(subject.clone(), "email subject")?;
            required(body.clone(), "email body")?;
        }
        MessageDraft::Slack { target, body, .. } => {
            required(target.clone(), "Slack target")?;
            required(body.clone(), "Slack body")?;
        }
    }
    Ok(())
}

impl Drop for FeatureHostController {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(feature = "production")]
fn product_surface_method(command: &FeatureCommand) -> &'static str {
    match command {
        FeatureCommand::ConnectorList { .. } => "mahayana.connector.list",
        FeatureCommand::ConnectorConnect { .. } => "mahayana.connector.connect",
        FeatureCommand::ConnectorRenameAccount { .. } => "mahayana.connector.account.rename",
        FeatureCommand::ConnectorRemoveAccount { .. } => "mahayana.connector.account.remove",
        FeatureCommand::ConnectorSetToolEnabled { .. } => "mahayana.connector.tool.setEnabled",
        FeatureCommand::SkillList { .. } => "mahayana.skill.list",
        FeatureCommand::SkillUpsert { .. } => "mahayana.skill.upsert",
        FeatureCommand::SkillDelete { .. } => "mahayana.skill.delete",
        FeatureCommand::SkillPublish { .. } => "mahayana.skill.publish",
        FeatureCommand::SkillUnpublish { .. } => "mahayana.skill.unpublish",
        FeatureCommand::SkillSync { .. } => "mahayana.skill.sync",
        FeatureCommand::BotList { .. } => "mahayana.bot.list",
        FeatureCommand::BotSetHidden { .. } => "mahayana.bot.setHidden",
        FeatureCommand::DraftResolve { .. } => "mahayana.draft.resolve",
        FeatureCommand::SecretProvide { .. } => "mahayana.secret.provide",
        FeatureCommand::ListenerList { .. } => "mahayana.listener.list",
        FeatureCommand::ListenerConnect { .. } => "mahayana.listener.connect",
        FeatureCommand::ListenerDisconnect { .. } => "mahayana.listener.disconnect",
        FeatureCommand::UpdateStatus { .. } => "mahayana.update.status",
        FeatureCommand::UpdateCheck { .. } => "mahayana.update.check",
        FeatureCommand::UpdateInstall { .. } => "mahayana.update.install",
        _ => unreachable!("non-product command has no product method"),
    }
}

#[cfg(feature = "production")]
fn decode_product_value<T: DeserializeOwned>(
    value: Value,
    method: &str,
) -> Result<T, FeatureHostError> {
    serde_json::from_value(value)
        .map_err(|error| FeatureHostError::Contract(format!("decode {method} response: {error}")))
}

#[cfg(feature = "production")]
fn decode_product_field<T: DeserializeOwned>(
    value: Value,
    field: &str,
    method: &str,
) -> Result<T, FeatureHostError> {
    let value = value.get(field).cloned().unwrap_or(value);
    decode_product_value(value, method)
}

fn ensure_open(state: &FeatureState) -> Result<(), FeatureHostError> {
    if state.closed {
        Err(FeatureHostError::Closed)
    } else {
        Ok(())
    }
}

fn next_id(state: &mut FeatureState, prefix: &str) -> String {
    state.sequence += 1;
    format!("{prefix}-{}", state.sequence)
}

const MAX_TRAYS: usize = 20;

fn push_error_tray(
    state: &mut FeatureState,
    agent_id: String,
    title: String,
    detail: Option<String>,
    request_id: Option<String>,
    dedupe_key: Option<String>,
) -> ErrorTray {
    let now = now_millis();
    if let Some(key) = dedupe_key.as_deref() {
        if let Some(index) = state
            .trays
            .iter()
            .position(|tray| tray.kind == "error" && tray.dedupe_key.as_deref() == Some(key))
        {
            let mut updated = state.trays[index].clone();
            updated.agent_id = agent_id;
            updated.title = title;
            updated.detail = detail;
            updated.request_id = request_id;
            updated.count = Some(updated.count.unwrap_or(1).saturating_add(1));
            updated.created_at = now;
            updated.error_kind = None;
            updated.raw_detail = None;
            updated.actions = None;
            state.trays[index] = updated.clone();
            state.events.push_back(HostEvent::TrayChanged {
                timestamp: timestamp(),
                action: "pushed".into(),
                tray: Some(updated.clone()),
                id: None,
            });
            return updated;
        }
    }
    let tray = ErrorTray {
        kind: "error".into(),
        id: next_id(state, "tray"),
        agent_id,
        title,
        detail,
        request_id,
        created_at: now,
        error_kind: None,
        raw_detail: None,
        actions: None,
        dedupe_key,
        count: None,
    };
    let mut tray = tray;
    if tray.dedupe_key.is_some() {
        tray.count = Some(1);
    }
    state.trays.push(tray.clone());
    state.events.push_back(HostEvent::TrayChanged {
        timestamp: timestamp(),
        action: "pushed".into(),
        tray: Some(tray.clone()),
        id: None,
    });
    if state.trays.len() > MAX_TRAYS {
        let overflow = state.trays.len() - MAX_TRAYS;
        let dropped = state.trays.drain(0..overflow).collect::<Vec<_>>();
        for dropped in dropped {
            state.events.push_back(HostEvent::TrayChanged {
                timestamp: timestamp(),
                action: "dismissed".into(),
                tray: None,
                id: Some(dropped.id),
            });
        }
    }
    tray
}

fn validate_config(config: &HostConfig) -> Result<(), FeatureHostError> {
    if config.profile_id.trim().is_empty() {
        Err(FeatureHostError::Contract(
            "profileId must not be empty".into(),
        ))
    } else {
        Ok(())
    }
}

#[cfg(feature = "production")]
fn unexpected_response(command: &str, response: RuntimeResponse) -> FeatureHostError {
    FeatureHostError::Contract(format!(
        "unexpected Runtime response for {command}: {response:?}"
    ))
}

fn required(value: String, name: &str) -> Result<String, FeatureHostError> {
    let value = value.trim();
    if value.is_empty() {
        Err(FeatureHostError::Contract(format!(
            "{name} must not be empty"
        )))
    } else {
        Ok(value.to_string())
    }
}

// Shared Fabushi text-shaping and profile semantics.
fn clamp_line(raw: &str, max_length: usize) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_length)
        .collect()
}

fn clamp_block(raw: &str, max_length: usize) -> String {
    raw.trim().chars().take(max_length).collect()
}

fn attachment_byte_limit_for_name(name: &str) -> u64 {
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "mp4" | "mov" | "m4v" | "webm" | "mkv" | "avi" | "mpg" | "mpeg"
    ) {
        VIDEO_BYTE_LIMIT
    } else {
        ATTACHMENT_BYTE_LIMIT
    }
}

fn resolve_agent_attachment_path(
    agent_root: &Path,
    agent_id: &str,
    raw_path: &str,
) -> Result<PathBuf, FeatureHostError> {
    if !is_safe_memory_agent_id(agent_id) {
        return Err(FeatureHostError::Contract(
            "invalid attachment owner".into(),
        ));
    }
    let base = agent_root.join(agent_id).join("attachments");
    let base = std::fs::canonicalize(&base).map_err(|error| {
        FeatureHostError::Contract(format!("attachment directory unavailable: {error}"))
    })?;
    let candidate = {
        let path = PathBuf::from(raw_path);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    };
    let candidate = std::fs::canonicalize(&candidate).map_err(|error| {
        FeatureHostError::Contract(format!("attachment path unavailable: {error}"))
    })?;
    if !candidate.starts_with(&base) {
        return Err(FeatureHostError::Contract(
            "attachment path escapes the agent attachment directory".into(),
        ));
    }
    Ok(candidate)
}

fn read_file_prefix(path: &Path, max_bytes: usize) -> Result<Vec<u8>, FeatureHostError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| FeatureHostError::Contract(format!("open attachment: {error}")))?;
    let mut buffer = vec![0u8; max_bytes];
    let bytes_read = file
        .read(&mut buffer)
        .map_err(|error| FeatureHostError::Contract(format!("read attachment: {error}")))?;
    buffer.truncate(bytes_read);
    Ok(buffer)
}

fn read_file_range(path: &Path, offset: u64, length: usize) -> Result<Vec<u8>, FeatureHostError> {
    if length == 0 {
        return Ok(Vec::new());
    }
    let mut file = std::fs::File::open(path)
        .map_err(|error| FeatureHostError::Contract(format!("open attachment: {error}")))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| FeatureHostError::Contract(format!("seek attachment: {error}")))?;
    let mut buffer = vec![0u8; length];
    let bytes_read = file
        .read(&mut buffer)
        .map_err(|error| FeatureHostError::Contract(format!("read attachment range: {error}")))?;
    buffer.truncate(bytes_read);
    Ok(buffer)
}

fn is_text_previewable_name(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "txt"
            | "md"
            | "markdown"
            | "mdc"
            | "csv"
            | "tsv"
            | "json"
            | "jsonl"
            | "yaml"
            | "yml"
            | "toml"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "py"
            | "rs"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "c"
            | "h"
            | "cpp"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "log"
            | "sql"
            | "ini"
            | "conf"
    )
}

fn looks_like_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    if bytes.contains(&0) {
        return true;
    }
    let control = bytes
        .iter()
        .filter(|byte| **byte < 0x09 || (**byte > 0x0d && **byte < 0x20))
        .count();
    control * 100 / bytes.len() > 5
}

fn image_dimensions(bytes: &[u8], _mime: &str) -> (Option<u32>, Option<u32>) {
    crate::selected_image_inputs::read_image_dimensions(bytes)
        .map(|dimensions| (Some(dimensions.width), Some(dimensions.height)))
        .unwrap_or((None, None))
}

fn build_content_snippet(text: &str, normalized_query: &str) -> Option<String> {
    if normalized_query.is_empty() {
        return None;
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_ascii_lowercase();
    let query = normalized_query.to_ascii_lowercase();
    let byte_index = lower.find(&query)?;
    let match_start = flat[..byte_index].chars().count();
    let match_len = flat[byte_index..byte_index + query.len()].chars().count();
    let chars = flat.chars().collect::<Vec<_>>();
    let start = match_start.saturating_sub(SEARCH_SNIPPET_LEAD);
    let end = (match_start + match_len + SEARCH_SNIPPET_TRAIL).min(chars.len());
    let core = chars[start..end].iter().collect::<String>();
    Some(format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        core,
        if end < chars.len() { "…" } else { "" }
    ))
}

fn collect_agent_media_matches(
    root: &Path,
    agent_id: &str,
    agent_name: &str,
    normalized_query: &str,
    out: &mut Vec<SearchMediaMatch>,
) {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 3 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push((path, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !normalized_query.is_empty() && !name.to_ascii_lowercase().contains(normalized_query)
            {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let timestamp_ms = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            out.push(SearchMediaMatch {
                agent_id: agent_id.to_string(),
                agent_name: agent_name.to_string(),
                path: path.to_string_lossy().to_string(),
                mime_type: media_mime_type(&name).map(str::to_string),
                name,
                size_bytes: metadata.len(),
                timestamp_ms,
            });
        }
    }
}

fn media_mime_type(name: &str) -> Option<&'static str> {
    let extension = Path::new(name)
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "avif" => Some("image/avif"),
        "heic" => Some("image/heic"),
        "heif" => Some("image/heif"),
        "svg" => Some("image/svg+xml"),
        "pdf" => Some("application/pdf"),
        "txt" | "md" | "markdown" | "log" => Some("text/plain"),
        "json" => Some("application/json"),
        "csv" => Some("text/csv"),
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        _ => None,
    }
}

fn clean_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn sanitize_avatar_data_url(value: Option<String>) -> Result<Option<String>, FeatureHostError> {
    const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let (header, payload) = value.split_once(',').ok_or_else(|| {
        FeatureHostError::Contract("avatar must be a base64 image data URL".into())
    })?;
    if !matches!(
        header,
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/webp;base64"
            | "data:image/gif;base64"
    ) {
        return Err(FeatureHostError::Contract(
            "avatar format must be PNG, JPEG, WebP, or GIF".into(),
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| FeatureHostError::Contract("avatar base64 payload is invalid".into()))?;
    if bytes.is_empty() || bytes.len() > MAX_AVATAR_BYTES {
        return Err(FeatureHostError::Contract(format!(
            "avatar must be between 1 byte and {MAX_AVATAR_BYTES} bytes"
        )));
    }
    Ok(Some(value.to_string()))
}

fn clone_agent_display_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "copy".into()
    } else {
        format!("{trimmed} copy")
    }
}

fn selected_input_data_url(
    input: crate::selected_image_inputs::SelectedImageInput,
) -> Option<String> {
    let mime_type = input.mime_type?;
    Some(format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(input.data)
    ))
}

fn load_agent_inbound_image_data_urls(images: &[AgentMessageImage]) -> Vec<String> {
    let paths = images
        .iter()
        .filter_map(|image| {
            let url = url::Url::parse(&image.url).ok()?;
            (url.scheme() == "file")
                .then(|| url.to_file_path().ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    let path_strings = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    crate::selected_image_inputs::load_selected_image_inputs(&path_strings)
        .into_iter()
        .filter_map(selected_input_data_url)
        .collect()
}

fn selected_image_data_urls(attachments: &[AttachmentContext]) -> Vec<String> {
    attachments
        .iter()
        .filter_map(|attachment| {
            let path = attachment.path.as_deref()?;
            let mut selected =
                crate::selected_image_inputs::load_selected_image_inputs([path.to_string()]);
            if let Some(data_url) = selected.pop().and_then(selected_input_data_url) {
                return Some(data_url);
            }

            // iOS-native pickers can supply an explicit image MIME for formats
            // whose Desktop extension classifier intentionally does not claim
            // as send-channel inputs. Preserve that declared product effect
            // without moving ownership to SwiftUI.
            let mime = attachment
                .mime_type
                .as_deref()
                .filter(|mime| mime.starts_with("image/"))?;
            let bytes = std::fs::read(path).ok()?;
            Some(format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ))
        })
        .collect()
}

fn render_native_turn_identity_context(
    auth_user: Option<&Value>,
    bot: Option<&BotSummary>,
) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(user) = auth_user {
        let display_name = ["displayName", "fullName", "nickname", "name"]
            .into_iter()
            .find_map(|key| {
                user.get(key)
                    .and_then(Value::as_str)
                    .map(|value| clamp_line(value, 120))
                    .filter(|value| !value.is_empty())
            });
        if let Some(display_name) = display_name {
            lines.push(format!(
                "Authenticated user display name: {display_name}. Address and represent this user consistently when acting through their authenticated Fabushi account."
            ));
        }
    }
    if let Some(bot) = bot {
        let configured_name = clamp_line(&bot.name, 120);
        let configured_title = clamp_line(&bot.title, 120);
        let description = clamp_block(&bot.description, 1200);
        let display_name = if bot.id == "mahayana-assistant"
            && (configured_name.is_empty()
                || configured_name.eq_ignore_ascii_case("grok")
                || configured_name.eq_ignore_ascii_case("grok bot")
                || configured_name == "大乘助手")
        {
            "Fabushi".to_string()
        } else if !configured_title.is_empty() {
            configured_title
        } else {
            configured_name
        };
        if !display_name.is_empty() {
            lines.push(format!(
                "Current Agent profile name: {display_name}. Use this configured Agent identity when the user asks who this Agent is."
            ));
        }
        if !description.is_empty() {
            lines.push(format!("Current Agent profile description: {description}"));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(format!(
            "[MAHAYANA_HIDDEN_CONTEXT]\n[Authenticated product identity]\n{}",
            lines.join("\n")
        ))
    }
}

fn compose_agent_input(
    text: &str,
    mode: AgentMode,
    mode_statement: Option<&str>,
    attachments: &[AttachmentContext],
) -> String {
    if mode == AgentMode::Agent && attachments.is_empty() {
        return text.to_string();
    }
    let mode_instruction = match mode {
        AgentMode::Agent => "请自主使用可用工具完成任务，并明确报告结果。",
        AgentMode::Ask => {
            "请只分析并回答问题；未经用户明确要求，不要修改文件或执行有副作用的操作。"
        }
        AgentMode::Plan => "请先形成可执行计划，列出依赖、风险和验证方式；暂不执行有副作用的操作。",
    };
    let mut input = format!(
        "[Agent 模式]\n{}\n{mode_instruction}\n\n[用户请求]\n{text}",
        mode_statement.unwrap_or("")
    );
    for attachment in attachments {
        input.push_str("\n\n[附件: ");
        input.push_str(&attachment.name);
        input.push_str("]\n");
        if let Some(path) = attachment.path.as_deref() {
            input.push_str("持久文件路径：");
            input.push_str(path);
            if let Some(size_bytes) = attachment.size_bytes {
                input.push_str(&format!("\n文件大小：{size_bytes} bytes"));
            }
            if let Some(mime_type) = attachment.mime_type.as_deref() {
                input.push_str("\nMIME：");
                input.push_str(mime_type);
            }
            input.push_str("\n需要完整内容时，请直接读取上述本地文件路径。\n");
        }
        if let Some(text) = attachment.text.as_deref() {
            input.push_str("文本预览（最多 64 KiB）：\n");
            input.push_str(text);
        } else if attachment.path.is_none() {
            input.push_str("（仅提供文件元数据）");
        }
    }
    input
}

fn is_safe_automation_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn render_mcp_instruction_context(
    instructions: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let mut entries = instructions
        .iter()
        .filter_map(|(server, instruction)| {
            let server = clamp_line(server, 200);
            let instruction = clamp_block(instruction, 4_000);
            (!server.is_empty() && !instruction.is_empty()).then_some((server, instruction))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries.truncate(16);
    if entries.is_empty() {
        return None;
    }
    let mut context = String::from(
        "[MCP connector operating instructions]
These are user-configured rules for the named connector. Apply them whenever using tools from that connector. They are runtime context, not user message text.
",
    );
    for (server, instruction) in entries {
        let remaining = 14_000usize.saturating_sub(context.chars().count());
        if remaining == 0 {
            break;
        }
        let block = format!(
            "
Connector: {server}
{instruction}
"
        );
        context.push_str(&clamp_block(&block, remaining));
    }
    Some(context)
}

fn cloud_task_resource_id(metadata: Option<&Value>) -> Option<String> {
    let object = metadata?.as_object()?;
    for key in ["bcId", "runId", "run_id", "cloudRunId", "cloud_run_id"] {
        if let Some(value) = object.get(key).and_then(Value::as_str) {
            let value = value.trim();
            if !value.is_empty() && value.len() <= 240 {
                return Some(value.to_string());
            }
        }
    }
    for key in ["run", "cloud", "metadata"] {
        if let Some(nested) = object.get(key) {
            if let Some(value) = cloud_task_resource_id(Some(nested)) {
                return Some(value);
            }
        }
    }
    None
}

fn ensure_automation_agent_scope(
    automation: &AutomationSummary,
    agent_id: Option<&str>,
) -> Result<(), FeatureHostError> {
    let Some(agent_id) = agent_id else {
        return Ok(());
    };
    if automation.agent_id.as_deref() == Some(agent_id) {
        return Ok(());
    }
    Err(FeatureHostError::Contract(format!(
        "automation {} does not belong to agent {agent_id}",
        automation.id
    )))
}

const MEMORY_PROFILE_HEADER: &str = "# About the user\n\n<!-- Enduring facts: who the user is, how to address them, lasting preferences.\n     Kept in mind every turn. Safe to read, grep, and edit.\n     One fact per line, as \"- (YYYY-MM-DD) <fact>\". -->\n";
const MEMORY_LOG_HEADER: &str = "# Memory log\n\n<!-- Dated facts, one per line as \"- (YYYY-MM-DD) <fact>\". Safe to read, grep, and edit. -->\n";
const MEMORY_MAX_CONTENT_LENGTH: usize = 500;
const MEMORY_PROFILE_PROMPT_LIMIT: usize = 100;
const MEMORY_RECENT_PROMPT_LIMIT: usize = 30;
const MEMORY_RECENT_PROMPT_CHAR_BUDGET: usize = 4000;
const MEMORY_DECAY_HALF_LIFE_DAYS: f64 = 30.0;

#[derive(Clone)]
struct ParsedMemoryFact {
    record: MemoryRecord,
    path: PathBuf,
    line_index: usize,
    order: usize,
}

fn is_safe_memory_agent_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xefcdab89;
    let mut h2: u32 = 0x98badcfe;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xc3d2e1f0;
    let bit_len = (input.len() as u64) * 8;
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in padded.as_chunks::<64>().0 {
        let mut words = [0u32; 80];
        for (index, word) in words[..16].iter_mut().enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;
        for (index, word) in words.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }
    let mut output = [0u8; 20];
    for (index, word) in [h0, h1, h2, h3, h4].into_iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

fn normalize_memory_content(raw: &str) -> String {
    clamp_line(raw, MEMORY_MAX_CONTENT_LENGTH)
}

fn memory_dedupe_key(content: &str) -> String {
    normalize_memory_content(content).to_lowercase()
}

fn memory_id_for(content: &str) -> String {
    sha1_digest(memory_dedupe_key(content).as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn format_memory_date(created_at_ms: i64) -> String {
    if created_at_ms <= 0 {
        return "unknown date".into();
    }
    Utc.timestamp_millis_opt(created_at_ms)
        .single()
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown date".into())
}

fn memory_log_files(memory_dir: &Path) -> Vec<PathBuf> {
    let log_dir = memory_dir.join("log");
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_memory_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn write_memory_atomic(path: &Path, content: &str) -> Result<(), FeatureHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create memory directory: {error}"))
        })?;
    }
    let temp = path.with_extension("md.tmp");
    std::fs::write(&temp, content)
        .map_err(|error| FeatureHostError::Contract(format!("write memory: {error}")))?;
    std::fs::rename(&temp, path)
        .map_err(|error| FeatureHostError::Contract(format!("commit memory: {error}")))?;
    Ok(())
}

fn parse_memory_facts(
    raw: &str,
    kind: MemoryKind,
    base: usize,
    path: &Path,
) -> Vec<ParsedMemoryFact> {
    let mut facts = Vec::new();
    let mut order = base;
    for (line_index, line) in raw.lines().enumerate() {
        let line = line.trim_end();
        if !line.starts_with("- (") {
            continue;
        }
        let Some(close) = line[3..].find(')') else {
            continue;
        };
        let close = close + 3;
        let date = &line[3..close];
        if date.len() != 10 || !line[close + 1..].starts_with(' ') {
            continue;
        }
        let content = normalize_memory_content(line[close + 2..].trim());
        if content.is_empty() {
            continue;
        }
        let created_at = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|date| date.and_utc().timestamp_millis())
            .unwrap_or(0);
        facts.push(ParsedMemoryFact {
            record: MemoryRecord {
                id: memory_id_for(&content),
                content,
                created_at,
                kind,
            },
            path: path.to_path_buf(),
            line_index,
            order,
        });
        order += 1;
    }
    facts
}

fn all_memory_facts(memory_dir: &Path) -> Vec<ParsedMemoryFact> {
    let profile = memory_dir.join("profile.md");
    let mut facts = parse_memory_facts(
        &read_memory_text(&profile),
        MemoryKind::Profile,
        0,
        &profile,
    );
    for path in memory_log_files(memory_dir) {
        let base = facts.len();
        facts.extend(parse_memory_facts(
            &read_memory_text(&path),
            MemoryKind::Log,
            base,
            &path,
        ));
    }
    facts
}

fn sort_memories_most_recent(facts: &mut [ParsedMemoryFact]) {
    facts.sort_by(|a, b| {
        b.record
            .created_at
            .cmp(&a.record.created_at)
            .then_with(|| b.order.cmp(&a.order))
    });
}

fn list_memories(memory_dir: &Path, limit: usize) -> Result<Vec<MemoryRecord>, FeatureHostError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut profile = all_memory_facts(memory_dir)
        .into_iter()
        .filter(|fact| fact.record.kind == MemoryKind::Profile)
        .collect::<Vec<_>>();
    let mut logs = all_memory_facts(memory_dir)
        .into_iter()
        .filter(|fact| fact.record.kind == MemoryKind::Log)
        .collect::<Vec<_>>();
    sort_memories_most_recent(&mut profile);
    sort_memories_most_recent(&mut logs);
    Ok(profile
        .into_iter()
        .chain(logs)
        .take(limit)
        .map(|fact| fact.record)
        .collect())
}

fn count_memories(memory_dir: &Path) -> Result<usize, FeatureHostError> {
    Ok(all_memory_facts(memory_dir).len())
}

fn add_memory(
    memory_dir: &Path,
    content: &str,
    created_at: i64,
    kind: MemoryKind,
) -> Result<Option<MemoryRecord>, FeatureHostError> {
    let content = normalize_memory_content(content);
    if content.is_empty() {
        return Ok(None);
    }
    let key = memory_dedupe_key(&content);
    if all_memory_facts(memory_dir)
        .iter()
        .any(|fact| memory_dedupe_key(&fact.record.content) == key)
    {
        return Ok(None);
    }
    let path = match kind {
        MemoryKind::Profile => memory_dir.join("profile.md"),
        MemoryKind::Log => {
            let bucket = format_memory_date(created_at)
                .chars()
                .take(7)
                .collect::<String>();
            memory_dir.join("log").join(format!("{bucket}.md"))
        }
    };
    let header = match kind {
        MemoryKind::Profile => MEMORY_PROFILE_HEADER,
        MemoryKind::Log => MEMORY_LOG_HEADER,
    };
    let raw = read_memory_text(&path);
    let base = if raw.is_empty() {
        header.to_string()
    } else {
        raw
    };
    let separator = if base.ends_with('\n') || base.is_empty() {
        ""
    } else {
        "\n"
    };
    let line = format!("- ({}) {}", format_memory_date(created_at), content);
    write_memory_atomic(&path, &format!("{base}{separator}{line}\n"))?;
    Ok(Some(MemoryRecord {
        id: memory_id_for(&content),
        content,
        created_at,
        kind,
    }))
}

fn remove_memory(memory_dir: &Path, id: &str) -> Result<bool, FeatureHostError> {
    let mut paths = vec![memory_dir.join("profile.md")];
    paths.extend(memory_log_files(memory_dir));
    for path in paths {
        let raw = read_memory_text(&path);
        if raw.is_empty() {
            continue;
        }
        let kind = if path.file_name().is_some_and(|name| name == "profile.md") {
            MemoryKind::Profile
        } else {
            MemoryKind::Log
        };
        let Some(fact) = parse_memory_facts(&raw, kind, 0, &path)
            .into_iter()
            .find(|fact| fact.record.id == id)
        else {
            continue;
        };
        let mut lines = raw.split('\n').map(str::to_string).collect::<Vec<_>>();
        if fact.line_index < lines.len() {
            lines.remove(fact.line_index);
            write_memory_atomic(&fact.path, &lines.join("\n"))?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn memory_importance(content: &str) -> f64 {
    if content.starts_with("[episode] ") {
        1.5
    } else if content.starts_with("[note] ") {
        0.5
    } else {
        1.0
    }
}

fn memory_recall_rank(memory: &MemoryRecord) -> f64 {
    memory_importance(&memory.content).log2()
        + memory.created_at as f64 / (MEMORY_DECAY_HALF_LIFE_DAYS * 86_400_000.0)
}

fn render_memory_system_prompt(memory_dir: &Path) -> String {
    let facts = all_memory_facts(memory_dir);
    let mut profile = facts
        .iter()
        .filter(|fact| fact.record.kind == MemoryKind::Profile)
        .cloned()
        .collect::<Vec<_>>();
    sort_memories_most_recent(&mut profile);
    profile.truncate(MEMORY_PROFILE_PROMPT_LIMIT);
    let mut recent = facts
        .into_iter()
        .filter(|fact| fact.record.kind == MemoryKind::Log)
        .collect::<Vec<_>>();
    recent.sort_by(|a, b| {
        memory_recall_rank(&b.record)
            .partial_cmp(&memory_recall_rank(&a.record))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.record.created_at.cmp(&a.record.created_at))
            .then_with(|| b.order.cmp(&a.order))
    });
    recent.truncate(MEMORY_RECENT_PROMPT_LIMIT);
    if profile.is_empty() && recent.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "Memory: durable facts you have learned about the user and their world.".to_string(),
        "These persist across every conversation with this agent, even after the chat is cleared. Rely on them so you stay consistent and avoid re-asking what you already know.".to_string(),
        format!(
            "Your memory lives in a folder at {}: profile.md holds who the user is and log/ holds dated history.",
            memory_dir.to_string_lossy()
        ),
    ];
    if !profile.is_empty() {
        lines.push("About the user:".into());
        for fact in profile {
            lines.push(format!(
                "- (learned {}) {}",
                format_memory_date(fact.record.created_at),
                fact.record.content
            ));
        }
    }
    if !recent.is_empty() {
        lines.push("Recently:".into());
        let mut budget = MEMORY_RECENT_PROMPT_CHAR_BUDGET;
        for fact in recent {
            let line = format!(
                "- (learned {}) {}",
                format_memory_date(fact.record.created_at),
                fact.record.content
            );
            if line.len() > budget {
                break;
            }
            budget -= line.len();
            lines.push(line);
        }
    }
    lines.join("\n")
}

const WORKFLOW_FILENAME: &str = "SKILL.md";
const LEGACY_WORKFLOW_FILENAME: &str = "workflow.md";
const WORKFLOW_MAX_NAME_LENGTH: usize = 80;
const WORKFLOW_MAX_DESCRIPTION_LENGTH: usize = 1536;
const WORKFLOW_MAX_BODY_LENGTH: usize = 100_000;
const WORKFLOW_UI_LIMIT: usize = 100;
const WORKFLOW_INJECTED_BODY_LIMIT: usize = 8_000;
const ATTACHMENT_BYTE_LIMIT: u64 = 25 * 1024 * 1024;
const VIDEO_BYTE_LIMIT: u64 = 200 * 1024 * 1024;
const ATTACHMENT_CHUNK_MAX_BYTES: usize = 8 * 1024 * 1024;
const ATTACHMENT_TEXT_PREVIEW_BYTE_CAP: usize = 64 * 1024;
const AGENT_CONTENT_SEARCH_MAX_MATCHES_PER_AGENT: usize = 5;
const AGENT_CONTENT_SEARCH_MAX_RESULTS: usize = 50;
const SEARCH_SNIPPET_LEAD: usize = 30;
const SEARCH_SNIPPET_TRAIL: usize = 60;
const WORKFLOW_MAX_PER_AGENT: usize = 100;
const WORKFLOW_ENABLEMENT_FILENAME: &str = "enabled-workflows.json";

#[derive(Clone)]
struct ParsedWorkflowFile {
    name: String,
    description: String,
    trigger: Option<WorkflowTrigger>,
    body: String,
    source_ref: Option<String>,
    data: serde_yaml::Mapping,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct WorkflowEnablementFile {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    enabled: Vec<String>,
}

fn clamp_workflow_name(name: &str) -> String {
    clamp_line(name, WORKFLOW_MAX_NAME_LENGTH)
}

fn clamp_workflow_description(description: &str) -> String {
    clamp_line(description, WORKFLOW_MAX_DESCRIPTION_LENGTH)
}

fn clamp_workflow_body(body: &str) -> String {
    clamp_block(body, WORKFLOW_MAX_BODY_LENGTH)
}

fn slugify_workflow_name(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for character in name.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            if out.len() < 48 {
                out.push(character);
            }
        } else {
            pending_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        format!("workflow-{}", now_millis())
    } else {
        out
    }
}

fn yaml_key(name: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(name.to_string())
}

fn yaml_string(data: &serde_yaml::Mapping, name: &str) -> Option<String> {
    data.get(yaml_key(name))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_string)
}

fn read_workflow_trigger(data: &serde_yaml::Mapping) -> Option<WorkflowTrigger> {
    let trigger = data.get(yaml_key("trigger"))?.as_mapping()?;
    let raw_schedule = trigger.get(yaml_key("schedule"))?.as_str()?;
    let schedule = normalize_automation_schedule(raw_schedule).ok()?;
    if schedule.is_empty() {
        return None;
    }
    let is_enabled = trigger
        .get(yaml_key("enabled"))
        .and_then(serde_yaml::Value::as_bool)
        .unwrap_or(true);
    Some(WorkflowTrigger {
        schedule,
        is_enabled,
    })
}

fn read_workflow_source_ref(data: &serde_yaml::Mapping) -> Option<String> {
    let nested = data
        .get(yaml_key("metadata"))
        .and_then(serde_yaml::Value::as_mapping)
        .and_then(|metadata| metadata.get(yaml_key("source")))
        .and_then(serde_yaml::Value::as_str);
    let raw = nested.or_else(|| {
        data.get(yaml_key("source"))
            .and_then(serde_yaml::Value::as_str)
    })?;
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn split_workflow_frontmatter(raw: &str) -> (serde_yaml::Mapping, String) {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let frontmatter = &rest[..end];
            let after = &rest[end + 4..];
            let content = after.strip_prefix('\n').unwrap_or(after).to_string();
            if let Ok(serde_yaml::Value::Mapping(mapping)) =
                serde_yaml::from_str::<serde_yaml::Value>(frontmatter)
            {
                return (mapping, content);
            }
        }
    }
    (serde_yaml::Mapping::new(), raw.to_string())
}

fn parse_workflow_file(raw: &str) -> Option<ParsedWorkflowFile> {
    let (data, content) = split_workflow_frontmatter(raw);
    let body = clamp_workflow_body(&content);
    if body.is_empty() && data.is_empty() {
        return None;
    }
    Some(ParsedWorkflowFile {
        name: clamp_workflow_name(&yaml_string(&data, "name").unwrap_or_default()),
        description: clamp_workflow_description(
            &yaml_string(&data, "description").unwrap_or_default(),
        ),
        trigger: read_workflow_trigger(&data),
        body,
        source_ref: read_workflow_source_ref(&data),
        data,
    })
}

fn serialize_workflow_file(
    name: &str,
    description: &str,
    body: &str,
    trigger: Option<&WorkflowTrigger>,
    source_ref: Option<&str>,
    existing_data: Option<&serde_yaml::Mapping>,
) -> Result<String, FeatureHostError> {
    let mut data = existing_data.cloned().unwrap_or_default();
    data.insert(
        yaml_key("name"),
        serde_yaml::Value::String(name.to_string()),
    );
    if description.is_empty() {
        data.remove(yaml_key("description"));
    } else {
        data.insert(
            yaml_key("description"),
            serde_yaml::Value::String(description.to_string()),
        );
    }
    let legacy_source = data.remove(yaml_key("source"));
    let mut metadata = data
        .get(yaml_key("metadata"))
        .and_then(serde_yaml::Value::as_mapping)
        .cloned()
        .unwrap_or_default();
    let next_source = source_ref
        .map(str::to_string)
        .or_else(|| {
            metadata
                .get(yaml_key("source"))
                .and_then(serde_yaml::Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| legacy_source.and_then(|value| value.as_str().map(str::to_string)));
    if let Some(source) = next_source.filter(|source| !source.is_empty()) {
        metadata.insert(yaml_key("source"), serde_yaml::Value::String(source));
    } else {
        metadata.remove(yaml_key("source"));
    }
    if metadata.is_empty() {
        data.remove(yaml_key("metadata"));
    } else {
        data.insert(yaml_key("metadata"), serde_yaml::Value::Mapping(metadata));
    }
    if let Some(trigger) = trigger {
        let mut trigger_data = serde_yaml::Mapping::new();
        trigger_data.insert(
            yaml_key("schedule"),
            serde_yaml::Value::String(trigger.schedule.clone()),
        );
        trigger_data.insert(
            yaml_key("enabled"),
            serde_yaml::Value::Bool(trigger.is_enabled),
        );
        data.insert(
            yaml_key("trigger"),
            serde_yaml::Value::Mapping(trigger_data),
        );
    }
    let mut yaml = serde_yaml::to_string(&data).map_err(|error| {
        FeatureHostError::Contract(format!("serialize workflow frontmatter: {error}"))
    })?;
    if let Some(stripped) = yaml.strip_prefix("---\n") {
        yaml = stripped.to_string();
    }
    Ok(format!("---\n{}---\n{}\n", yaml.trim_end(), body.trim()))
}

fn derive_workflow_name_from_markdown(body: &str) -> Option<String> {
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let text = if let Some(index) = line.find(char::is_whitespace) {
            if line[..index].chars().all(|character| character == '#') {
                &line[index..]
            } else {
                line
            }
        } else {
            line
        };
        let cleaned = text
            .chars()
            .filter(|character| !matches!(character, '*' | '_' | '`' | '#' | '>'))
            .collect::<String>();
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            return Some(clamp_workflow_name(cleaned));
        }
    }
    None
}

fn derive_workflow_name_from_source(source: &str) -> String {
    let raw = source
        .split('/')
        .rfind(|segment| !segment.is_empty())
        .unwrap_or("Imported skill");
    let raw = [".markdown", ".mdc", ".md", ".txt"]
        .iter()
        .find_map(|suffix| raw.strip_suffix(suffix))
        .unwrap_or(raw);
    let name = raw.replace(['-', '_'], " ");
    let name = clamp_workflow_name(name.trim());
    if name.is_empty() {
        "Imported skill".into()
    } else {
        name
    }
}

fn build_live_source_pointer_body(source: &str) -> String {
    format!(
        "This workflow is a live reference to the skill at `{source}`.\nRead that source now with your file or fetch tools and follow it as written. Do not assume its contents from this note; the source is the source of truth and may have changed since this workflow was created."
    )
}

fn build_live_source_description(name: &str, source: &str) -> String {
    clamp_workflow_description(&format!(
        "Use when the \"{name}\" skill applies; it is a live reference to {source}."
    ))
}

fn workflow_enablement_path(agent_root: &Path, agent_id: &str) -> PathBuf {
    agent_root.join(agent_id).join(WORKFLOW_ENABLEMENT_FILENAME)
}

fn read_workflow_enablement(agent_root: &Path, agent_id: &str) -> WorkflowEnablementFile {
    let path = workflow_enablement_path(agent_root, agent_id);
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<WorkflowEnablementFile>(&bytes).ok())
        .unwrap_or_default()
}

fn is_workflow_enabled(agent_root: &Path, agent_id: &str, id: &str) -> bool {
    !read_workflow_enablement(agent_root, agent_id)
        .disabled
        .iter()
        .any(|disabled| disabled == id)
}

fn write_workflow_enablement(
    agent_root: &Path,
    agent_id: &str,
    mut file: WorkflowEnablementFile,
) -> Result<(), FeatureHostError> {
    file.disabled.sort();
    file.disabled.dedup();
    file.enabled.sort();
    file.enabled.dedup();
    let path = workflow_enablement_path(agent_root, agent_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create workflow enablement directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let body = serde_json::to_vec_pretty(&file).map_err(|error| {
        FeatureHostError::Contract(format!("serialize workflow enablement: {error}"))
    })?;
    std::fs::write(&temp, [body.as_slice(), b"\n"].concat()).map_err(|error| {
        FeatureHostError::Contract(format!("write workflow enablement: {error}"))
    })?;
    std::fs::rename(&temp, &path).map_err(|error| {
        FeatureHostError::Contract(format!("commit workflow enablement: {error}"))
    })?;
    Ok(())
}

fn set_workflow_enabled(
    agent_root: &Path,
    agent_id: &str,
    id: &str,
    enabled: bool,
) -> Result<(), FeatureHostError> {
    let mut file = read_workflow_enablement(agent_root, agent_id);
    let had = file.disabled.iter().any(|item| item == id);
    if enabled {
        if !had {
            return Ok(());
        }
        file.disabled.retain(|item| item != id);
    } else {
        if had {
            return Ok(());
        }
        file.disabled.push(id.to_string());
    }
    write_workflow_enablement(agent_root, agent_id, file)
}

fn forget_workflow_enablement(
    agent_root: &Path,
    agent_id: &str,
    id: &str,
) -> Result<(), FeatureHostError> {
    let mut file = read_workflow_enablement(agent_root, agent_id);
    let before = file.disabled.len();
    file.disabled.retain(|item| item != id);
    if before == file.disabled.len() {
        return Ok(());
    }
    write_workflow_enablement(agent_root, agent_id, file)
}

fn workflow_helper_scripts(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, current: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.is_file() {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                let name = relative.to_string_lossy();
                if name != WORKFLOW_FILENAME
                    && name != LEGACY_WORKFLOW_FILENAME
                    && name != "runs.json"
                {
                    out.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn workflow_created_at(path: &Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.created().or_else(|_| metadata.modified()).ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_else(now_millis)
}

fn load_workflow_summary(
    workflow_root: &Path,
    agent_root: &Path,
    agent_id: &str,
    id: &str,
) -> Option<WorkflowSummary> {
    let dir = workflow_root.join(id);
    let file_path = dir.join(WORKFLOW_FILENAME);
    let legacy_path = dir.join(LEGACY_WORKFLOW_FILENAME);
    if !file_path.exists() && legacy_path.exists() {
        let _ = std::fs::rename(&legacy_path, &file_path);
    }
    let raw = std::fs::read_to_string(&file_path).ok()?;
    let parsed = parse_workflow_file(&raw)?;
    let name = if parsed.name.is_empty() {
        clamp_workflow_name(id)
    } else {
        parsed.name
    };
    let disable_model_invocation = parsed
        .data
        .get(yaml_key("disable-model-invocation"))
        .and_then(serde_yaml::Value::as_bool);
    let next_run_at = parsed
        .trigger
        .as_ref()
        .filter(|trigger| trigger.is_enabled)
        .and_then(|trigger| next_automation_run(&trigger.schedule, now_millis()));
    Some(WorkflowSummary {
        id: id.to_string(),
        name,
        description: parsed.description,
        body: parsed.body,
        trigger: parsed.trigger.clone(),
        source_ref: parsed.source_ref,
        source: WorkflowSource::Workflow,
        plugin_id: None,
        published_by_current_user: false,
        is_enabled_for_agent: is_workflow_enabled(agent_root, agent_id, id),
        disable_model_invocation,
        schedule_description: parsed
            .trigger
            .as_ref()
            .map(|trigger| trigger.schedule.clone()),
        created_at: workflow_created_at(&file_path),
        last_run_at: None,
        next_run_at,
        helper_scripts: workflow_helper_scripts(&dir),
        file_path: file_path.to_string_lossy().into_owned(),
    })
}

fn list_workflow_summaries(
    workflow_root: &Path,
    agent_root: &Path,
    agent_id: &str,
) -> Vec<WorkflowSummary> {
    let Ok(entries) = std::fs::read_dir(workflow_root) else {
        return Vec::new();
    };
    let mut workflows = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().to_string();
            load_workflow_summary(workflow_root, agent_root, agent_id, &id)
        })
        .collect::<Vec<_>>();
    workflows.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    workflows.truncate(WORKFLOW_UI_LIMIT);
    workflows
}

fn write_workflow(
    workflow_root: &Path,
    agent_root: &Path,
    agent_id: &str,
    id: Option<&str>,
    name: &str,
    description: &str,
    body: &str,
    trigger: Option<&WorkflowTrigger>,
    source_ref: Option<&str>,
) -> Result<WorkflowSummary, FeatureHostError> {
    let name = clamp_workflow_name(name);
    let description = clamp_workflow_description(description);
    let body = clamp_workflow_body(body);
    if name.is_empty() || body.is_empty() {
        return Err(FeatureHostError::Contract(
            "workflow name and body must not be empty".into(),
        ));
    }
    std::fs::create_dir_all(workflow_root)
        .map_err(|error| FeatureHostError::Contract(format!("create workflow root: {error}")))?;
    let id = id
        .map(str::to_string)
        .unwrap_or_else(|| slugify_workflow_name(&name));
    if !is_safe_memory_agent_id(&id) {
        return Err(FeatureHostError::Contract(format!(
            "unsafe workflow id: {id}"
        )));
    }
    let existing_count = std::fs::read_dir(workflow_root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .count()
        })
        .unwrap_or(0);
    if !workflow_root.join(&id).exists() && existing_count >= WORKFLOW_MAX_PER_AGENT {
        return Err(FeatureHostError::Contract(format!(
            "workflow library is limited to {WORKFLOW_MAX_PER_AGENT} user workflows"
        )));
    }
    let dir = workflow_root.join(&id);
    let path = dir.join(WORKFLOW_FILENAME);
    let existing_data = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| parse_workflow_file(&raw))
        .map(|parsed| parsed.data);
    let raw = serialize_workflow_file(
        &name,
        &description,
        &body,
        trigger,
        source_ref,
        existing_data.as_ref(),
    )?;
    std::fs::create_dir_all(&dir).map_err(|error| {
        FeatureHostError::Contract(format!("create workflow directory: {error}"))
    })?;
    let temp = path.with_extension("md.tmp");
    std::fs::write(&temp, raw)
        .map_err(|error| FeatureHostError::Contract(format!("write workflow: {error}")))?;
    std::fs::rename(&temp, &path)
        .map_err(|error| FeatureHostError::Contract(format!("commit workflow: {error}")))?;
    load_workflow_summary(workflow_root, agent_root, agent_id, &id)
        .ok_or_else(|| FeatureHostError::Contract("workflow could not be reloaded".into()))
}

const PUBLISHED_WORKFLOW_CACHE_DIR: &str = ".published-plugin-skills";
const PUBLISHED_WORKFLOW_CACHE_METADATA: &str = "metadata.json";
const PUBLISHED_WORKFLOW_RECOVERY_MARKER: &str = ".fabushi-publish-recovery.json";

fn is_exact_skill_publish_version(value: &str) -> bool {
    let value = value.trim();
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn published_workflow_cache_root(
    workflow_root: &Path,
    plugin_id: &str,
) -> Result<PathBuf, FeatureHostError> {
    let safe = !plugin_id.is_empty()
        && plugin_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if !safe {
        return Err(FeatureHostError::Contract(
            "Published plugin id cannot be mapped to local storage.".into(),
        ));
    }
    Ok(workflow_root.join(PUBLISHED_WORKFLOW_CACHE_DIR).join(plugin_id))
}

fn read_published_workflow_cache(
    cache_root: &Path,
) -> Option<PublishedWorkflowCacheMetadata> {
    std::fs::read(cache_root.join(PUBLISHED_WORKFLOW_CACHE_METADATA))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn write_published_workflow_cache(
    cache_root: &Path,
    metadata: &PublishedWorkflowCacheMetadata,
) -> Result<(), FeatureHostError> {
    std::fs::create_dir_all(cache_root).map_err(|error| {
        FeatureHostError::Contract(format!("create published skill cache: {error}"))
    })?;
    let body = serde_json::to_vec_pretty(metadata).map_err(|error| {
        FeatureHostError::Contract(format!("serialize published skill cache: {error}"))
    })?;
    std::fs::write(
        cache_root.join(PUBLISHED_WORKFLOW_CACHE_METADATA),
        [body.as_slice(), b"\n"].concat(),
    )
    .map_err(|error| {
        FeatureHostError::Contract(format!("write published skill cache: {error}"))
    })
}

fn write_unpublish_recovery_marker(
    workflow_dir: &Path,
    metadata: &PublishedWorkflowCacheMetadata,
) -> Result<(), FeatureHostError> {
    let body = serde_json::to_vec_pretty(&json!({
        "pluginId": metadata.plugin_id,
        "originalWorkflowId": metadata.original_workflow_id,
    }))
    .map_err(|error| {
        FeatureHostError::Contract(format!("serialize unpublish recovery marker: {error}"))
    })?;
    std::fs::write(
        workflow_dir.join(PUBLISHED_WORKFLOW_RECOVERY_MARKER),
        [body.as_slice(), b"\n"].concat(),
    )
    .map_err(|error| {
        FeatureHostError::Contract(format!("write unpublish recovery marker: {error}"))
    })
}

fn unpublish_recovery_marker_matches(
    workflow_dir: &Path,
    metadata: &PublishedWorkflowCacheMetadata,
) -> bool {
    let Ok(bytes) = std::fs::read(workflow_dir.join(PUBLISHED_WORKFLOW_RECOVERY_MARKER)) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    value.get("pluginId").and_then(Value::as_str) == Some(metadata.plugin_id.as_str())
        && value.get("originalWorkflowId").and_then(Value::as_str)
            == Some(metadata.original_workflow_id.as_str())
}

fn copy_workflow_tree(source: &Path, target: &Path) -> Result<(), FeatureHostError> {
    let metadata = std::fs::symlink_metadata(source).map_err(|error| {
        FeatureHostError::Contract(format!("inspect skill source: {error}"))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(FeatureHostError::Contract(
            "Skill source must be a real directory and may not be a symlink.".into(),
        ));
    }
    std::fs::create_dir_all(target).map_err(|error| {
        FeatureHostError::Contract(format!("create skill copy target: {error}"))
    })?;
    for entry in std::fs::read_dir(source)
        .map_err(|error| FeatureHostError::Contract(format!("read skill source: {error}")))?
    {
        let entry = entry.map_err(|error| {
            FeatureHostError::Contract(format!("read skill source entry: {error}"))
        })?;
        let entry_type = entry.file_type().map_err(|error| {
            FeatureHostError::Contract(format!("inspect skill source entry: {error}"))
        })?;
        if entry_type.is_symlink() {
            return Err(FeatureHostError::Contract(
                "Published skills may not contain symlinks.".into(),
            ));
        }
        let destination = target.join(entry.file_name());
        if entry_type.is_dir() {
            copy_workflow_tree(&entry.path(), &destination)?;
        } else if entry_type.is_file() {
            std::fs::copy(entry.path(), destination).map_err(|error| {
                FeatureHostError::Contract(format!("copy skill source entry: {error}"))
            })?;
        }
    }
    Ok(())
}

fn find_published_workflow_cache(
    workflow_root: &Path,
    agent_id: &str,
    workflow_id: &str,
    facts: &[PublishedPluginSnapshot],
) -> Option<(PublishedPluginSnapshot, PublishedWorkflowCacheMetadata, PathBuf)> {
    for fact in facts {
        let Ok(cache_root) = published_workflow_cache_root(workflow_root, &fact.plugin_id) else {
            continue;
        };
        let Some(metadata) = read_published_workflow_cache(&cache_root) else {
            continue;
        };
        if metadata.agent_id == agent_id
            && metadata.promoted_workflow_id == workflow_id
            && metadata.plugin_id == fact.plugin_id
            && cache_root.join("skill").join(WORKFLOW_FILENAME).is_file()
        {
            return Some((fact.clone(), metadata, cache_root));
        }
    }
    None
}

fn published_workflow_summaries(
    workflow_root: &Path,
    agent_root: &Path,
    agent_id: &str,
    facts: &[PublishedPluginSnapshot],
) -> Vec<WorkflowSummary> {
    let mut workflows = Vec::new();
    for fact in facts {
        let Ok(cache_root) = published_workflow_cache_root(workflow_root, &fact.plugin_id) else {
            continue;
        };
        let Some(metadata) = read_published_workflow_cache(&cache_root) else {
            continue;
        };
        if metadata.agent_id != agent_id || metadata.plugin_id != fact.plugin_id {
            continue;
        }
        let Some(mut summary) = load_workflow_summary(&cache_root, &agent_root, agent_id, "skill")
        else {
            continue;
        };
        summary.id = metadata.promoted_workflow_id;
        summary.source = WorkflowSource::Plugin;
        summary.plugin_id = Some(fact.plugin_id.clone());
        summary.published_by_current_user = fact.published_by_current_user;
        summary.is_enabled_for_agent = fact.is_enabled_for_agent;
        workflows.push(summary);
    }
    workflows.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    workflows
}

fn normalize_marketplace_plugin_name(value: &str) -> String {
    let normalized = value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let mut compact = String::new();
    for character in normalized.chars() {
        if character == '-' && compact.ends_with('-') {
            continue;
        }
        compact.push(character);
    }
    compact.trim_matches('-').to_string()
}

fn append_workflow_publish_tree(
    builder: &mut TarBuilder<GzEncoder<Vec<u8>>>,
    workflow_dir: &Path,
    current: &Path,
    archive_root: &Path,
) -> Result<(), FeatureHostError> {
    let mut entries = std::fs::read_dir(current)
        .map_err(|error| FeatureHostError::Contract(format!(
            "read workflow publish directory: {error}"
        )))?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(workflow_dir).map_err(|error| {
            FeatureHostError::Contract(format!("resolve workflow publish path: {error}"))
        })?;
        if relative.components().count() == 1 {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == LEGACY_WORKFLOW_FILENAME || name == "runs.json" {
                continue;
            }
        }
        let archive_path = archive_root.join(relative);
        let file_type = entry.file_type().map_err(|error| {
            FeatureHostError::Contract(format!("inspect workflow publish entry: {error}"))
        })?;
        if file_type.is_symlink() {
            return Err(FeatureHostError::Contract(
                "workflow publish package must not contain symlinks".into(),
            ));
        }
        if file_type.is_dir() {
            builder.append_dir(&archive_path, &path).map_err(|error| {
                FeatureHostError::Contract(format!("append workflow publish directory: {error}"))
            })?;
            append_workflow_publish_tree(builder, workflow_dir, &path, &archive_path)?;
        } else if file_type.is_file() {
            builder.append_path_with_name(&path, &archive_path).map_err(|error| {
                FeatureHostError::Contract(format!("append workflow publish file: {error}"))
            })?;
        }
    }
    Ok(())
}

fn pack_workflow_plugin_artifact(
    workflow_dir: &Path,
    workflow_id: &str,
    display_name: &str,
) -> Result<Vec<u8>, FeatureHostError> {
    let plugin_name = normalize_marketplace_plugin_name(workflow_id);
    if plugin_name.is_empty() {
        return Err(FeatureHostError::Contract(
            "workflow id has no usable marketplace plugin name".into(),
        ));
    }
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = TarBuilder::new(encoder);
    builder.mode(HeaderMode::Deterministic);
    builder.follow_symlinks(false);

    let manifest = serde_json::to_vec_pretty(&json!({
        "name": plugin_name,
        "displayName": display_name,
        "skills": [format!("skills/{workflow_id}")],
    }))
    .map_err(|error| FeatureHostError::Contract(format!(
        "serialize workflow plugin manifest: {error}"
    )))?;
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    builder
        .append_data(&mut header, "plugin.json", manifest.as_slice())
        .map_err(|error| FeatureHostError::Contract(format!(
            "append workflow plugin manifest: {error}"
        )))?;

    append_workflow_publish_tree(
        &mut builder,
        workflow_dir,
        workflow_dir,
        &PathBuf::from("skills").join(workflow_id),
    )?;
    let encoder = builder.into_inner().map_err(|error| {
        FeatureHostError::Contract(format!("finish workflow publish tar: {error}"))
    })?;
    encoder.finish().map_err(|error| {
        FeatureHostError::Contract(format!("finish workflow publish gzip: {error}"))
    })
}

fn workflow_from_automation(automation: &AutomationSummary) -> WorkflowSummary {
    WorkflowSummary {
        id: automation.id.clone(),
        name: automation.name.clone(),
        description: String::new(),
        body: automation.prompt.clone(),
        trigger: Some(WorkflowTrigger {
            schedule: automation.schedule.clone(),
            is_enabled: automation.enabled,
        }),
        source_ref: None,
        source: WorkflowSource::Automation,
        plugin_id: None,
        published_by_current_user: false,
        is_enabled_for_agent: true,
        disable_model_invocation: None,
        schedule_description: Some(automation.schedule.clone()),
        created_at: automation.created_at_ms,
        last_run_at: automation.last_run_at_ms,
        next_run_at: automation.next_run_at_ms,
        helper_scripts: Vec::new(),
        file_path: String::new(),
    }
}

fn render_workflow_catalog(workflow_root: &Path, agent_root: &Path, agent_id: &str) -> String {
    let workflows = list_workflow_summaries(workflow_root, agent_root, agent_id)
        .into_iter()
        .filter(|workflow| workflow.trigger.is_none())
        .filter(|workflow| workflow.is_enabled_for_agent)
        .filter(|workflow| workflow.disable_model_invocation != Some(true))
        .collect::<Vec<_>>();
    if workflows.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "Workflows: reusable recipes available to this agent. Read the referenced SKILL.md when one applies, then follow it as written.".to_string(),
    ];
    for workflow in workflows {
        lines.push(format!(
            "- {} — {} (file: {})",
            workflow.name,
            if workflow.description.is_empty() {
                "No description"
            } else {
                workflow.description.as_str()
            },
            workflow.file_path
        ));
    }
    lines.join("\n")
}

fn group_member_handles(name: &str) -> Vec<String> {
    let lower = name.trim().to_lowercase();
    if lower.is_empty() {
        return Vec::new();
    }
    let mut handles = Vec::new();
    handles.push(lower.clone());
    let compact = lower.split_whitespace().collect::<String>();
    if !compact.is_empty() && compact != lower {
        handles.push(compact);
    }
    if let Some(first) = lower.split_whitespace().next() {
        if !first.is_empty() && !handles.iter().any(|handle| handle == first) {
            handles.push(first.to_string());
        }
    }
    handles
}

fn is_group_word_char(character: Option<char>) -> bool {
    character.is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

fn has_group_mention_at(lower: &str, handle: &str) -> bool {
    let needle = format!("@{handle}");
    let mut search_from = 0usize;
    while let Some(relative) = lower[search_from..].find(&needle) {
        let index = search_from + relative;
        let before = lower[..index].chars().next_back();
        let after_index = index + needle.len();
        let after = lower[after_index..].chars().next();
        if !is_group_word_char(before) && !is_group_word_char(after) {
            return true;
        }
        search_from = index + 1;
        if search_from >= lower.len() {
            break;
        }
    }
    false
}

fn has_everyone_group_mention(lower: &str) -> bool {
    has_group_mention_at(lower, "everyone") || has_group_mention_at(lower, "all")
}

fn resolve_group_responders(
    group: &GroupSummary,
    bots: &BTreeMap<String, BotSummary>,
) -> Vec<String> {
    let members = group
        .member_ids
        .iter()
        .filter_map(|id| bots.get(id).map(|bot| (id.clone(), bot.name.clone())))
        .collect::<Vec<_>>();
    if members.is_empty() {
        return Vec::new();
    }
    let start = group
        .messages
        .iter()
        .rposition(|message| matches!(message.speaker, GroupSpeaker::User { .. }))
        .unwrap_or(0);
    let mut is_everyone = false;
    let mut mentioned = BTreeSet::new();
    for message in group.messages.iter().skip(start) {
        let lower = message.content.to_lowercase();
        if has_everyone_group_mention(&lower) {
            is_everyone = true;
        }
        for (id, name) in &members {
            if mentioned.contains(id) {
                continue;
            }
            if group_member_handles(name)
                .iter()
                .any(|handle| has_group_mention_at(&lower, handle))
            {
                mentioned.insert(id.clone());
            }
        }
    }
    if is_everyone || mentioned.is_empty() {
        return members.into_iter().map(|(id, _)| id).collect();
    }
    members
        .into_iter()
        .filter_map(|(id, _)| mentioned.contains(&id).then_some(id))
        .collect()
}

fn order_round_speakers(member_ids: &[String], round: usize) -> Vec<String> {
    if member_ids.is_empty() {
        return Vec::new();
    }
    let offset = round % member_ids.len();
    member_ids[offset..]
        .iter()
        .chain(member_ids[..offset].iter())
        .cloned()
        .collect()
}

fn is_group_pass_content(content: &str) -> bool {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return true;
    }
    let mut normalized = trimmed.to_ascii_lowercase();
    if normalized.ends_with('.') {
        normalized.pop();
    }
    let normalized = normalized.trim();
    let normalized = normalized
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(normalized)
        .trim();
    normalized.eq_ignore_ascii_case("pass")
}

fn group_messages_since_member_last_spoke<'a>(
    history: &'a [GroupMessage],
    member_id: &str,
) -> &'a [GroupMessage] {
    if let Some(index) = history.iter().rposition(
        |message| matches!(&message.speaker, GroupSpeaker::Member { id, .. } if id == member_id),
    ) {
        &history[index + 1..]
    } else {
        history
    }
}

fn format_group_message_line(message: &GroupMessage, viewer_id: &str) -> String {
    match &message.speaker {
        GroupSpeaker::User { name } => name
            .as_ref()
            .filter(|name| !name.is_empty())
            .map(|name| format!("{name} (user): {}", message.content))
            .unwrap_or_else(|| format!("User: {}", message.content)),
        GroupSpeaker::Member { id, name } => {
            let suffix = if id == viewer_id { " (you)" } else { "" };
            format!("{name}{suffix}: {}", message.content)
        }
    }
}

fn format_group_history(history: &[GroupMessage], viewer_id: &str) -> String {
    let start = history.len().saturating_sub(GROUP_PROMPT_HISTORY_LIMIT);
    let recent = &history[start..];
    if recent.is_empty() {
        return "(no messages yet)".into();
    }
    recent
        .iter()
        .map(|message| format_group_message_line(message, viewer_id))
        .collect::<Vec<_>>()
        .join("\n")
}

fn group_display_name(group: &GroupSummary) -> &str {
    let name = group.name.trim();
    if name.is_empty() { "the group" } else { name }
}

fn build_group_member_system_prompt(
    member: &BotSummary,
    group: &GroupSummary,
    peers: &[BotSummary],
) -> String {
    let description = group.description.trim();
    let group_label = if description.is_empty() {
        format!("\"{}\"", group_display_name(group))
    } else {
        format!("\"{}\" — {description}", group_display_name(group))
    };
    let mut lines = vec![format!(
        "You are {}, one participant in a group chat ({}).",
        member.name, group_label
    )];
    if !member.description.trim().is_empty() {
        lines.push(format!("Your persona: {}", member.description.trim()));
    }
    if !peers.is_empty() {
        lines.push(String::new());
        lines.push("Other participants in the room:".into());
        for peer in peers {
            let peer_description = if peer.description.trim().is_empty() {
                String::new()
            } else {
                format!(" ({})", peer.description.trim())
            };
            lines.push(format!("- {}{peer_description}", peer.name));
        }
    }
    lines.push(String::new());
    lines.push(if peers.is_empty() {
        "Right now you are speaking in this group chat.".into()
    } else {
        format!(
            "Right now you are speaking in this group chat, with {}.",
            peers
                .iter()
                .map(|peer| peer.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    lines.extend([
        String::new(),
        "Several distinct participants share this room. Stay fully in character as yourself. Never speak or write as another participant or as the user, and never narrate the conversation from the outside.".into(),
        String::new(),
        "How you talk in the room:".into(),
        "- Keep each message short and conversational — usually one to three sentences, the way people actually chat. Do not monologue or summarize the whole thread.".into(),
        "- React to what was just said: build on it, agree, disagree, or ask a pointed question. Address others by name when it helps.".into(),
        "- Mentions: write @Name to direct your message at a specific teammate, or @everyone for the whole room. If you are @-mentioned you are being asked to weigh in, so respond; to pull a specific teammate into the conversation, @-mention them.".into(),
        "- Do not repeat points already made, and do not restate other people's messages back to them.".into(),
        "- If you have nothing new worth adding right now, send exactly \"(pass)\". Staying quiet is good — it lets the conversation settle instead of spinning forever.".into(),
        "- Say your piece in one turn, then stop. Never role-play other participants' replies.".into(),
        String::new(),
        "Conversations are private to the people in them: what you and the user discuss in your one-on-one chat stays there. Never quote, summarize, or reveal it in this room.".into(),
    ]);
    lines.join("\n")
}

fn build_group_turn_prompt(
    member: &BotSummary,
    group: &GroupSummary,
    peers: &[BotSummary],
    new_messages: &[GroupMessage],
) -> String {
    let with_clause = if peers.is_empty() {
        String::new()
    } else {
        format!(
            " - with {}",
            peers
                .iter()
                .map(|peer| peer.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut lines = vec![format!(
        "[Group chat: \"{}\"{with_clause}]",
        group_display_name(group)
    )];
    if new_messages.is_empty() {
        lines.push("No new messages in the room since your last turn.".into());
    } else {
        lines.push("New messages in the room (oldest first):".into());
        lines.push(format_group_history(new_messages, &member.id));
    }
    lines.extend([
        String::new(),
        format!(
            "It's your turn, {}. Reply in character with one short room message if you have something worth adding, or reply exactly \"(pass)\" if you don't.",
            member.name
        ),
    ]);
    lines.join("\n")
}

fn validate_group_members(
    state: &FeatureState,
    member_ids: Vec<String>,
) -> Result<Vec<String>, FeatureHostError> {
    let mut seen = BTreeSet::new();
    let mut members = Vec::new();
    for id in member_ids {
        let id = id.trim().to_string();
        if id.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        if state.groups.contains_key(&id) {
            return Err(FeatureHostError::Contract(
                "a group chat can only contain individual agents, not other group chats".into(),
            ));
        }
        if !state.bots.contains_key(&id) {
            return Err(FeatureHostError::Contract(format!(
                "unknown group member: {id}"
            )));
        }
        members.push(id);
    }
    if members.is_empty() {
        return Err(FeatureHostError::Contract(
            "group chat must contain at least one agent".into(),
        ));
    }
    if members.len() > GROUP_MAX_MEMBERS {
        return Err(FeatureHostError::Contract(format!(
            "group chat can contain at most {GROUP_MAX_MEMBERS} agents"
        )));
    }
    Ok(members)
}

fn build_agent_inbound_wake_prompt(sender: &BotSummary, text: &str, priority: bool) -> String {
    let priority_line = if priority {
        "This is a priority instruction from another assistant. It may supersede non-user background work."
    } else {
        "This is another assistant reaching out asynchronously, not the user typing in this chat."
    };
    format!(
        "[agent] A message arrived from {} (id: {}).\n{}\n\n{}: {}\n\nHandle any useful request or action. If a reply is needed, send it back asynchronously through the agent messaging capability; do not create acknowledgement loops.",
        sender.name,
        sender.id,
        priority_line,
        sender.name,
        clamp_block(text, 8000)
    )
}

fn build_admin_broadcast_wake_prompt(message: &str) -> String {
    format!(
        "[broadcast] A direct message from the user who owns and runs this agent was broadcast to their agents.\nTreat it as a user directive, not as another agent or a scheduled routine.\n\nThe user says: {}\n\nAct on it as appropriate. Do not rebroadcast it to other agents; the user already reached them separately.",
        clamp_block(message, 8000)
    )
}

#[cfg(feature = "production")]
fn activity_parent_agent_id(state: &FeatureState, operation_id: &str) -> String {
    state
        .group_operations
        .get(operation_id)
        .map(|context| context.member_id.clone())
        .or_else(|| {
            state
                .background_operations
                .get(operation_id)
                .map(|context| context.agent_id.clone())
        })
        .or_else(|| state.operation_agents.get(operation_id).cloned())
        .unwrap_or_else(|| "mahayana-assistant".into())
}

#[cfg(feature = "production")]
fn subagent_title(prompt: Option<&str>, fallback: &str) -> String {
    prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
        .and_then(|prompt| prompt.lines().find(|line| !line.trim().is_empty()))
        .map(|line| clamp_line(line, 120))
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| clamp_line(fallback, 120))
}

#[cfg(feature = "production")]
fn subagent_status_from_agent_state(value: Option<&Value>) -> Option<SubagentStatus> {
    let status = value?.get("status").and_then(Value::as_str).unwrap_or("");
    match status {
        "pendingInit" | "running" => Some(SubagentStatus::Running),
        "completed" | "shutdown" => Some(SubagentStatus::Done),
        "errored" | "notFound" => Some(SubagentStatus::Error),
        "interrupted" => Some(SubagentStatus::Aborted),
        _ => None,
    }
}

#[cfg(feature = "production")]
fn update_subagents_from_activity(
    state: &mut FeatureState,
    parent_agent_id: &str,
    operation_id: &str,
    title: &str,
    detail: Option<&str>,
    runtime_status: RuntimeActivityStatus,
    metadata: Option<&Value>,
) -> Vec<SubagentSummary> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let event_type = metadata.get("type").and_then(Value::as_str).unwrap_or("");
    let now = now_millis();
    let mut changed = Vec::new();

    if event_type == "collabAgentToolCall" {
        let tool = metadata.get("tool").and_then(Value::as_str).unwrap_or("");
        let prompt = metadata.get("prompt").and_then(Value::as_str);
        let model = metadata
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| !model.trim().is_empty());
        let receivers = metadata
            .get("receiverThreadIds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .collect::<Vec<_>>();
        let agent_states = metadata.get("agentsStates").and_then(Value::as_object);
        for receiver in receivers {
            let existing = state.subagents.get(receiver).cloned();
            let state_value = agent_states.and_then(|states| states.get(receiver));
            let inferred_status =
                subagent_status_from_agent_state(state_value).unwrap_or_else(|| {
                    if runtime_status == RuntimeActivityStatus::Failed {
                        SubagentStatus::Error
                    } else if tool == "closeAgent"
                        && runtime_status == RuntimeActivityStatus::Completed
                    {
                        SubagentStatus::Done
                    } else {
                        existing
                            .as_ref()
                            .map(|subagent| subagent.status)
                            .unwrap_or(SubagentStatus::Running)
                    }
                });
            let state_message = state_value
                .and_then(|state| state.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let subagent = SubagentSummary {
                id: receiver.to_string(),
                parent_agent_id: parent_agent_id.to_string(),
                subagent_type: model
                    .map(str::to_string)
                    .or_else(|| {
                        existing
                            .as_ref()
                            .map(|subagent| subagent.subagent_type.clone())
                    })
                    .unwrap_or_else(|| "codex".into()),
                title: subagent_title(
                    prompt,
                    existing
                        .as_ref()
                        .map(|subagent| subagent.title.as_str())
                        .unwrap_or(title),
                ),
                status: inferred_status,
                started_at_ms: existing
                    .as_ref()
                    .map(|subagent| subagent.started_at_ms)
                    .unwrap_or(now),
                updated_at_ms: now,
                detail: state_message
                    .or_else(|| detail.map(str::to_string))
                    .or_else(|| prompt.map(|prompt| clamp_block(prompt, 1000))),
            };
            state
                .subagents
                .insert(receiver.to_string(), subagent.clone());
            changed.push(subagent);
        }
    } else if event_type == "subAgentActivity" {
        let Some(receiver) = metadata
            .get("agentThreadId")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
        else {
            return Vec::new();
        };
        let activity_kind = metadata.get("kind").and_then(Value::as_str).unwrap_or("");
        let path = metadata
            .get("agentPath")
            .and_then(Value::as_str)
            .unwrap_or("");
        let existing = state.subagents.get(receiver).cloned();
        let status = match activity_kind {
            "interrupted" => SubagentStatus::Aborted,
            "started" | "interacted" => SubagentStatus::Running,
            _ if runtime_status == RuntimeActivityStatus::Failed => SubagentStatus::Error,
            _ => existing
                .as_ref()
                .map(|subagent| subagent.status)
                .unwrap_or(SubagentStatus::Running),
        };
        let fallback_title = path
            .rsplit('/')
            .find(|part| !part.trim().is_empty())
            .unwrap_or(title);
        let subagent = SubagentSummary {
            id: receiver.to_string(),
            parent_agent_id: parent_agent_id.to_string(),
            subagent_type: existing
                .as_ref()
                .map(|subagent| subagent.subagent_type.clone())
                .unwrap_or_else(|| "codex".into()),
            title: existing
                .as_ref()
                .map(|subagent| subagent.title.clone())
                .unwrap_or_else(|| subagent_title(None, fallback_title)),
            status,
            started_at_ms: existing
                .as_ref()
                .map(|subagent| subagent.started_at_ms)
                .unwrap_or(now),
            updated_at_ms: now,
            detail: detail
                .map(str::to_string)
                .or_else(|| (!path.is_empty()).then(|| path.to_string())),
        };
        state
            .subagents
            .insert(receiver.to_string(), subagent.clone());
        changed.push(subagent);
    }

    for subagent in &changed {
        if subagent.status == SubagentStatus::Running {
            state.async_tasks.insert(
                async_task_key(&subagent.parent_agent_id, AsyncTaskKind::Subagent, &subagent.id),
                AsyncTaskSummary {
                    kind: AsyncTaskKind::Subagent,
                    id: subagent.id.clone(),
                    parent_agent_id: subagent.parent_agent_id.clone(),
                    label: subagent.title.clone(),
                    status: AsyncTaskStatus::Running,
                    started_at_ms: subagent.started_at_ms,
                    detail: subagent.detail.clone(),
                    subagent_type: Some(subagent.subagent_type.clone()),
                    resource_id: None,
                },
            );
        } else {
            state
                .async_tasks
                .remove(&async_task_key(&subagent.parent_agent_id, AsyncTaskKind::Subagent, &subagent.id));
        }
    }

    // A generic provider may report a subagent activity without a receiver id.
    // Do not fabricate a durable identity from the operation id; the agent roster
    // only contains actual subagent ids.
    let _ = operation_id;
    changed
}

fn derive_teach_workflow_name(markdown: &str) -> String {
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let line = line.trim_start_matches('#').trim();
        let cleaned = line
            .chars()
            .filter(|character| !matches!(character, '*' | '_' | '`' | '>' | '[' | ']'))
            .collect::<String>();
        let name = clamp_line(&cleaned, 80);
        if !name.is_empty() {
            return name;
        }
    }
    "Taught workflow".into()
}

fn slugify_teach_workflow_name(name: &str) -> String {
    let mut slug = String::new();
    let mut pending_dash = false;
    for character in name.to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
        } else if !slug.is_empty() {
            pending_dash = true;
        }
        if slug.len() >= 60 {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("taught-workflow-{}", now_millis())
    } else {
        slug.to_string()
    }
}

fn teach_recording_status(active: Option<&TeachCaptureProcess>) -> TeachRecordingStatus {
    match active {
        Some(active) => TeachRecordingStatus {
            state: "recording".into(),
            agent_id: Some(active.agent_id.clone()),
            started_at_ms: Some(active.started_at_ms),
            max_duration_ms: TEACH_MAX_DURATION_MS,
            capture_path: if cfg!(target_os = "ios") {
                Some(active.video_path.to_string_lossy().to_string())
            } else {
                None
            },
        },
        None => TeachRecordingStatus::default(),
    }
}

fn find_ffmpeg_binary() -> Result<PathBuf, FeatureHostError> {
    for candidate in [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "ffmpeg",
    ] {
        let ok = std::process::Command::new(candidate)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if ok {
            return Ok(PathBuf::from(candidate));
        }
    }
    Err(FeatureHostError::Contract(
        "Teach Recording requires ffmpeg on this computer".into(),
    ))
}

#[cfg(target_os = "macos")]
fn avfoundation_screen_index(ffmpeg: &Path) -> Result<String, FeatureHostError> {
    let output = std::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-f",
            "avfoundation",
            "-list_devices",
            "true",
            "-i",
            "",
        ])
        .output()
        .map_err(|error| {
            FeatureHostError::Contract(format!("list screen capture devices: {error}"))
        })?;
    let text = String::from_utf8_lossy(&output.stderr);
    for line in text.lines() {
        if !line.contains("Capture screen") {
            continue;
        }
        let Some(open) = line.rfind('[') else {
            continue;
        };
        let Some(close_rel) = line[open + 1..].find(']') else {
            continue;
        };
        let index = &line[open + 1..open + 1 + close_rel];
        if !index.is_empty() && index.chars().all(|character| character.is_ascii_digit()) {
            return Ok(index.to_string());
        }
    }
    Err(FeatureHostError::Contract(
        "ffmpeg could not find a macOS screen capture device; grant Screen Recording permission and retry".into(),
    ))
}

fn spawn_teach_capture(video_path: &Path) -> Result<std::process::Child, FeatureHostError> {
    let ffmpeg = find_ffmpeg_binary()?;
    let mut command = std::process::Command::new(&ffmpeg);
    command
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(target_os = "macos")]
    {
        let screen = avfoundation_screen_index(&ffmpeg)?;
        command.args([
            "-f",
            "avfoundation",
            "-framerate",
            "15",
            "-capture_cursor",
            "1",
            "-i",
            &format!("{screen}:none"),
        ]);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0.0".into());
        command.args(["-f", "x11grab", "-framerate", "15", "-i", &display]);
    }
    #[cfg(target_os = "windows")]
    {
        command.args(["-f", "gdigrab", "-framerate", "15", "-i", "desktop"]);
    }

    command.args([
        "-t",
        "600",
        "-an",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        video_path.to_string_lossy().as_ref(),
    ]);
    command
        .spawn()
        .map_err(|error| FeatureHostError::Contract(format!("start teach recording: {error}")))
}

fn stop_teach_capture(child: &mut std::process::Child) -> Result<(), FeatureHostError> {
    if child
        .try_wait()
        .map_err(|error| FeatureHostError::Contract(format!("poll teach recorder: {error}")))?
        .is_some()
    {
        return Ok(());
    }
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(b"q\n");
        let _ = stdin.flush();
    }
    for _ in 0..40 {
        if child
            .try_wait()
            .map_err(|error| {
                FeatureHostError::Contract(format!("wait for teach recorder: {error}"))
            })?
            .is_some()
        {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    child
        .kill()
        .map_err(|error| FeatureHostError::Contract(format!("stop teach recorder: {error}")))?;
    let _ = child.wait();
    Ok(())
}

fn extract_teach_frames(video_path: &Path, frames_dir: &Path) -> Result<(), FeatureHostError> {
    let ffmpeg = find_ffmpeg_binary()?;
    std::fs::create_dir_all(frames_dir).map_err(|error| {
        FeatureHostError::Contract(format!("create teach frames directory: {error}"))
    })?;
    let pattern = frames_dir.join("frame-%03d.jpg");
    let status = std::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-i",
            video_path.to_string_lossy().as_ref(),
            "-vf",
            "fps=0.5,scale=1280:-2:force_original_aspect_ratio=decrease",
            "-frames:v",
            "120",
            pattern.to_string_lossy().as_ref(),
        ])
        .status()
        .map_err(|error| FeatureHostError::Contract(format!("extract teach frames: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(FeatureHostError::Contract(
            "ffmpeg failed to extract teach frames".into(),
        ))
    }
}

fn sanitize_auto_review_rules(rules: Vec<AutoReviewRule>) -> Vec<AutoReviewRule> {
    let mut seen = BTreeSet::new();
    let mut sanitized = Vec::new();
    for mut rule in rules.into_iter().take(200) {
        rule.id = clamp_line(&rule.id, 96);
        rule.text = clamp_line(&rule.text, 2000);
        if rule.text.is_empty() {
            continue;
        }
        if rule.id.is_empty() {
            rule.id = format!("rule-{}", sanitized.len() + 1);
        }
        if seen.insert(rule.id.clone()) {
            sanitized.push(rule);
        }
    }
    sanitized
}

fn load_product_host_settings(path: &Path) -> ProductHostSettings {
    let Ok(bytes) = std::fs::read(path) else {
        return ProductHostSettings::default();
    };
    let Ok(mut settings) = serde_json::from_slice::<ProductHostSettings>(&bytes) else {
        return ProductHostSettings::default();
    };
    settings.auto_review_rules = sanitize_auto_review_rules(settings.auto_review_rules);
    settings
}

fn persist_product_host_settings(
    path: &Path,
    settings: &ProductHostSettings,
) -> Result<(), FeatureHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create settings directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| FeatureHostError::Contract(format!("serialize host settings: {error}")))?;
    std::fs::write(&temp, bytes)
        .map_err(|error| FeatureHostError::Contract(format!("write host settings: {error}")))?;
    std::fs::rename(&temp, path)
        .map_err(|error| FeatureHostError::Contract(format!("commit host settings: {error}")))
}

fn valid_remote_computer_device_secret(device_id: &str, secret: &str) -> bool {
    is_safe_memory_agent_id(device_id)
        && secret.len() >= 48
        && secret.len() <= 256
        && secret.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn load_remote_computer_device_secrets(path: &Path) -> BTreeMap<String, String> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return BTreeMap::new();
    };
    if metadata.len() > REMOTE_DEVICE_SECRET_MAX_BYTES {
        return BTreeMap::new();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    if bytes.len() as u64 > REMOTE_DEVICE_SECRET_MAX_BYTES {
        return BTreeMap::new();
    }
    let Ok(secrets) = serde_json::from_slice::<BTreeMap<String, String>>(&bytes) else {
        return BTreeMap::new();
    };
    if secrets.len() > REMOTE_DEVICE_SECRET_MAX_ENTRIES {
        return BTreeMap::new();
    }
    secrets
        .into_iter()
        .filter(|(device_id, secret)| valid_remote_computer_device_secret(device_id, secret))
        .collect()
}

fn persist_remote_computer_device_secrets(
    path: &Path,
    secrets: &BTreeMap<String, String>,
) -> Result<(), FeatureHostError> {
    if secrets.len() > REMOTE_DEVICE_SECRET_MAX_ENTRIES
        || secrets
            .iter()
            .any(|(device_id, secret)| !valid_remote_computer_device_secret(device_id, secret))
    {
        return Err(FeatureHostError::Contract(
            "remote device secret state exceeds its safe entry limit or contains invalid data"
                .into(),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create remote device directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(secrets).map_err(|error| {
        FeatureHostError::Contract(format!("serialize remote device secret: {error}"))
    })?;
    if bytes.len() as u64 > REMOTE_DEVICE_SECRET_MAX_BYTES {
        return Err(FeatureHostError::Contract(
            "remote device secret state exceeds its safe byte limit".into(),
        ));
    }
    std::fs::write(&temp, bytes).map_err(|error| {
        FeatureHostError::Contract(format!("write remote device secret: {error}"))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| FeatureHostError::Contract(format!("protect remote device secret: {error}")),
        )?;
    }
    std::fs::rename(&temp, path).map_err(|error| {
        FeatureHostError::Contract(format!("commit remote device secret: {error}"))
    })
}

#[cfg(test)]
mod approval_presentation_redaction_tests {
    use super::*;

    #[test]
    fn approval_details_redact_nested_secrets_before_presentation() {
        let details = json!({
            "command": "curl https://example.test -H Authorization:Bearer super-secret-token",
            "headers": {
                "authorization": "Bearer top-secret-value",
                "x-api-key": "0123456789abcdefghijklmnopqrstuvwxyz"
            },
            "nested": [{"password": "hunter2", "safe": "repository main"}],
            "safe": "run release checks"
        });
        let rendered = approval_presentation_safe_details(&details);
        assert!(!rendered.contains("super-secret-token"));
        assert!(!rendered.contains("top-secret-value"));
        assert!(!rendered.contains("0123456789abcdefghijklmnopqrstuvwxyz"));
        assert!(!rendered.contains("hunter2"));
        assert!(rendered.contains("run release checks"));
    }

    #[test]
    fn approval_visible_strings_fail_closed_when_they_look_sensitive() {
        assert_eq!(
            approval_presentation_safe_text(
                "TOKEN=0123456789abcdefghijklmnopqrstuvwxyz",
                500
            ),
            "…"
        );
        assert_eq!(
            approval_presentation_safe_text("Bearer abcdefghijklmnopqrstuvwxyz", 500),
            "…"
        );
        assert_eq!(
            approval_presentation_safe_text("Run release checks in the repository", 500),
            "Run release checks in the repository"
        );
    }

    #[test]
    fn approval_redaction_does_not_mutate_raw_matching_details() {
        let details = json!({
            "kind": "local-tool",
            "command": "deploy --token raw-secret-value",
            "reason": "Deploy the current release"
        });
        let safe = approval_presentation_safe_value(&details, None, 0);
        assert_eq!(
            details.get("command").and_then(Value::as_str),
            Some("deploy --token raw-secret-value")
        );
        assert_ne!(safe.get("command"), details.get("command"));
        assert_eq!(
            safe.get("reason").and_then(Value::as_str),
            Some("Deploy the current release")
        );
    }
}

#[cfg(test)]
mod remote_device_secret_tests {
    use super::*;

    #[test]
    fn remote_device_secret_round_trip_is_private_and_never_contains_control_data() {
        let path = std::env::temp_dir().join(format!(
            "fabushi-remote-device-secret-test-{}-{}.json",
            std::process::id(),
            now_millis()
        ));
        let device_id = "fabushi-mac-test".to_string();
        let secret = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string();
        let secrets = BTreeMap::from([(device_id.clone(), secret.clone())]);
        persist_remote_computer_device_secrets(&path, &secrets).expect("persist device secret");
        let restored = load_remote_computer_device_secrets(&path);
        assert_eq!(restored.get(&device_id), Some(&secret));
        let raw = std::fs::read_to_string(&path).expect("read device secret file");
        assert!(!raw.contains("screenshot"));
        assert!(!raw.contains("computer.action"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("device secret metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn remote_device_secret_state_limits_fail_closed() {
        let path = std::env::temp_dir().join(format!(
            "fabushi-remote-device-secret-limit-test-{}-{}.json",
            std::process::id(),
            now_millis()
        ));
        std::fs::write(
            &path,
            vec![b'x'; REMOTE_DEVICE_SECRET_MAX_BYTES as usize + 1],
        )
        .expect("write oversized device secret state");
        assert!(load_remote_computer_device_secrets(&path).is_empty());

        let secret = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string();
        let too_many = (0..=REMOTE_DEVICE_SECRET_MAX_ENTRIES)
            .map(|index| (format!("fabushi-device-{index}"), secret.clone()))
            .collect::<BTreeMap<_, _>>();
        assert!(persist_remote_computer_device_secrets(&path, &too_many).is_err());
        std::fs::write(
            &path,
            serde_json::to_vec(&too_many).expect("serialize oversized map"),
        )
        .expect("write oversized entry map");
        assert!(load_remote_computer_device_secrets(&path).is_empty());
        let _ = std::fs::remove_file(path);
    }
}

fn read_action_audit(path: &Path, limit: usize) -> Vec<Value> {
    if limit == 0 {
        return Vec::new();
    }
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn auto_review_rule_matches(rule: &AutoReviewRule, title: &str, details: &Value) -> bool {
    let needle = rule.text.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    let proposed_rule = details
        .get("proposedRule")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !proposed_rule.is_empty() && (proposed_rule == needle || proposed_rule.contains(&needle)) {
        return true;
    }
    let subject = details
        .get("subject")
        .or_else(|| details.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !subject.is_empty() && (subject == needle || subject.contains(&needle)) {
        return true;
    }
    let capability = details
        .get("capability")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !capability.is_empty() && (capability == needle || capability.contains(&needle)) {
        return true;
    }
    title.trim().to_lowercase().contains(&needle)
}

fn load_peer_messages(path: &Path) -> Vec<AgentPeerMessage> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(mut messages) = serde_json::from_slice::<Vec<AgentPeerMessage>>(&bytes) else {
        return Vec::new();
    };
    messages.retain(|message| {
        !message.id.trim().is_empty()
            && !message.from_agent_id.trim().is_empty()
            && !message.target_id.trim().is_empty()
            && !clamp_block(&message.text, 8000).is_empty()
    });
    for message in &mut messages {
        message.text = clamp_block(&message.text, 8000);
        message.from_agent_name = clamp_line(&message.from_agent_name, 72);
        message.target_name = clamp_line(&message.target_name, 72);
    }
    if messages.len() > 5000 {
        messages.drain(0..messages.len() - 5000);
    }
    messages
}

fn persist_peer_messages(
    path: &Path,
    messages: &[AgentPeerMessage],
) -> Result<(), FeatureHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create peer message directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(messages)
        .map_err(|error| FeatureHostError::Contract(format!("serialize peer messages: {error}")))?;
    std::fs::write(&temp, data).map_err(|error| {
        FeatureHostError::Contract(format!("write peer message store: {error}"))
    })?;
    std::fs::rename(&temp, path).map_err(|error| {
        FeatureHostError::Contract(format!("commit peer message store: {error}"))
    })?;
    Ok(())
}

fn load_groups(path: &Path) -> BTreeMap<String, GroupSummary> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    let Ok(items) = serde_json::from_slice::<Vec<GroupSummary>>(&bytes) else {
        return BTreeMap::new();
    };
    items
        .into_iter()
        .filter_map(|mut group| {
            group.name = clamp_line(&group.name, 72);
            group.description = clamp_block(&group.description, 2000);
            let mut seen = BTreeSet::new();
            group
                .member_ids
                .retain(|id| !id.trim().is_empty() && seen.insert(id.clone()));
            if group.id.trim().is_empty()
                || group.name.is_empty()
                || group.member_ids.is_empty()
                || group.member_ids.len() > GROUP_MAX_MEMBERS
            {
                return None;
            }
            Some((group.id.clone(), group))
        })
        .collect()
}

fn persist_groups(
    path: &Path,
    groups: &BTreeMap<String, GroupSummary>,
) -> Result<(), FeatureHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create group directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(&groups.values().cloned().collect::<Vec<_>>())
        .map_err(|error| FeatureHostError::Contract(format!("serialize groups: {error}")))?;
    std::fs::write(&temp, data)
        .map_err(|error| FeatureHostError::Contract(format!("write group store: {error}")))?;
    std::fs::rename(&temp, path)
        .map_err(|error| FeatureHostError::Contract(format!("commit group store: {error}")))?;
    Ok(())
}

fn load_test_auth_user(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    let stored = serde_json::from_slice::<Value>(&bytes).ok()?;
    if stored.get("version").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    stored.get("user").filter(|user| user.is_object()).cloned()
}

fn persist_test_auth_user(path: &Path, user: Option<&Value>) -> Result<(), FeatureHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create test auth directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    if let Some(user) = user {
        let data = serde_json::to_vec_pretty(&json!({
            "version": 1,
            "user": user,
        }))
        .map_err(|error| {
            FeatureHostError::Contract(format!("serialize test auth state: {error}"))
        })?;
        std::fs::write(&temp, data).map_err(|error| {
            FeatureHostError::Contract(format!("write test auth state: {error}"))
        })?;
        std::fs::rename(&temp, path).map_err(|error| {
            FeatureHostError::Contract(format!("commit test auth state: {error}"))
        })?;
        return Ok(());
    }

    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(FeatureHostError::Contract(format!(
                "clear test auth state: {error}"
            )));
        }
    }
    let _ = std::fs::remove_file(temp);
    Ok(())
}

fn load_bots(path: &Path) -> BTreeMap<String, BotSummary> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    let Ok(items) = serde_json::from_slice::<Vec<BotSummary>>(&bytes) else {
        return BTreeMap::new();
    };
    items
        .into_iter()
        .filter_map(|mut bot| {
            bot.name = clamp_line(&bot.name, 72);
            bot.description = clamp_block(&bot.description, 2000);
            bot.title = bot.title.trim().to_string();
            bot.avatar_shape = clean_optional_string(bot.avatar_shape);
            bot.avatar_color = clean_optional_string(bot.avatar_color);
            if bot.id.trim().is_empty() || bot.name.is_empty() {
                return None;
            }
            Some((bot.id.clone(), bot))
        })
        .collect()
}

fn persist_bots(path: &Path, bots: &BTreeMap<String, BotSummary>) -> Result<(), FeatureHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create bot directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(&bots.values().cloned().collect::<Vec<_>>())
        .map_err(|error| FeatureHostError::Contract(format!("serialize bots: {error}")))?;
    std::fs::write(&temp, data)
        .map_err(|error| FeatureHostError::Contract(format!("write bot store: {error}")))?;
    std::fs::rename(&temp, path)
        .map_err(|error| FeatureHostError::Contract(format!("commit bot store: {error}")))?;
    Ok(())
}

fn load_automations(path: &Path) -> BTreeMap<String, AutomationSummary> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeMap::new();
    };
    let Ok(items) = serde_json::from_slice::<Vec<AutomationSummary>>(&bytes) else {
        return BTreeMap::new();
    };
    let now = now_millis();
    items
        .into_iter()
        .filter_map(|mut item| {
            if !is_safe_automation_id(&item.id)
                || item.name.trim().is_empty()
                || item.prompt.trim().is_empty()
            {
                return None;
            }
            let trigger = item.trigger.clone().unwrap_or_else(|| AutomationTrigger::Schedule {
                schedule: item.schedule.clone(),
            });
            let Ok(trigger) = normalize_automation_trigger(trigger) else {
                return None;
            };
            item.schedule = automation_trigger_legacy_schedule(&trigger);
            item.trigger = Some(trigger.clone());
            item.next_run_at_ms = automation_next_run(
                &trigger,
                &item.schedule,
                item.enabled,
                now,
            );
            Some((item.id.clone(), item))
        })
        .collect()
}

fn persist_automations(
    path: &Path,
    automations: &BTreeMap<String, AutomationSummary>,
) -> Result<(), FeatureHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            FeatureHostError::Contract(format!("create automation directory: {error}"))
        })?;
    }
    let temp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(&automations.values().cloned().collect::<Vec<_>>())
        .map_err(|error| FeatureHostError::Contract(format!("serialize automations: {error}")))?;
    std::fs::write(&temp, data)
        .map_err(|error| FeatureHostError::Contract(format!("write automation store: {error}")))?;
    std::fs::rename(&temp, path)
        .map_err(|error| FeatureHostError::Contract(format!("commit automation store: {error}")))?;
    Ok(())
}

fn normalize_automation_schedule(raw: &str) -> Result<String, FeatureHostError> {
    let schedule = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if schedule.is_empty() || next_automation_run(&schedule, now_millis()).is_none() {
        return Err(FeatureHostError::Contract(
            "invalid automation schedule; use a 5-field cron, @hourly/@daily/@weekly/@monthly/@yearly, or @every <n><s|m|h|d>"
                .into(),
        ));
    }
    Ok(schedule)
}

fn parse_every_interval_ms(schedule: &str) -> Option<i64> {
    let rest = schedule.strip_prefix("@every ")?.trim();
    let split = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    let amount = rest[..split].parse::<i64>().ok()?;
    let unit = rest[split..].trim().to_ascii_lowercase();
    let unit_ms = match unit.as_str() {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    amount.checked_mul(unit_ms).filter(|value| *value > 0)
}

fn expand_cron_alias(schedule: &str) -> &str {
    match schedule.to_ascii_lowercase().as_str() {
        "@hourly" => "0 * * * *",
        "@daily" | "@midnight" => "0 0 * * *",
        "@weekly" => "0 0 * * 0",
        "@monthly" => "0 0 1 * *",
        "@yearly" | "@annually" => "0 0 1 1 *",
        _ => schedule,
    }
}

fn parse_cron_field(field: &str, min: u32, max: u32) -> Option<BTreeSet<u32>> {
    let mut values = BTreeSet::new();
    for part in field.split(',') {
        let mut step_parts = part.split('/');
        let range = step_parts.next()?;
        let step: u32 = step_parts
            .next()
            .map_or(Some(1), |value| value.parse().ok())?;
        if step_parts.next().is_some() || step == 0 {
            return None;
        }
        let (start, end) = if range == "*" || range.is_empty() {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (start.parse().ok()?, end.parse().ok()?)
        } else {
            let start = range.parse().ok()?;
            (start, if part.contains('/') { max } else { start })
        };
        if start < min || end > max || start > end {
            return None;
        }
        for value in (start..=end).step_by(step as usize) {
            values.insert(if max == 7 && value == 7 { 0 } else { value });
        }
    }
    (!values.is_empty()).then_some(values)
}

fn next_automation_run(schedule: &str, after_ms: i64) -> Option<i64> {
    if let Some(interval) = parse_every_interval_ms(schedule) {
        return after_ms.checked_add(interval);
    }
    let expression = expand_cron_alias(schedule);
    let fields = expression.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return None;
    }
    let minute = parse_cron_field(fields[0], 0, 59)?;
    let hour = parse_cron_field(fields[1], 0, 23)?;
    let day_of_month = parse_cron_field(fields[2], 1, 31)?;
    let month = parse_cron_field(fields[3], 1, 12)?;
    let day_of_week = parse_cron_field(fields[4], 0, 7)?;
    let dom_restricted = fields[2] != "*";
    let dow_restricted = fields[4] != "*";
    let mut candidate = after_ms.div_euclid(60_000) * 60_000 + 60_000;
    for _ in 0..(366 * 24 * 60) {
        let date = chrono::DateTime::<Utc>::from_timestamp_millis(candidate)?;
        let dom_ok = day_of_month.contains(&date.day());
        let dow_ok = day_of_week.contains(&date.weekday().num_days_from_sunday());
        let day_ok = if dom_restricted && dow_restricted {
            dom_ok || dow_ok
        } else {
            (!dom_restricted || dom_ok) && (!dow_restricted || dow_ok)
        };
        if minute.contains(&date.minute())
            && hour.contains(&date.hour())
            && month.contains(&date.month())
            && day_ok
        {
            return Some(candidate);
        }
        candidate = candidate.checked_add(60_000)?;
    }
    None
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn timestamp() -> String {
    now_millis().to_string()
}

fn computer_origin_label(origin: ComputerControlOrigin) -> &'static str {
    match origin {
        ComputerControlOrigin::LocalUi => "local-ui",
        ComputerControlOrigin::RemoteMobile => "remote-mobile",
        ComputerControlOrigin::Ai => "ai",
    }
}

fn sync_computer_control_policy(settings: &ProductHostSettings) {
    mahayana_computer::set_control_policy(mahayana_computer::ComputerControlPolicy {
        local_execution_enabled: settings.local_execution,
        remote_control_enabled: settings.remote_control_enabled,
        ai_control_enabled: settings.ai_computer_control_enabled,
        local_tool_permission: settings.local_tool_permission,
    });
}

fn test_computer_snapshot() -> ComputerSnapshot {
    ComputerSnapshot {
        captured_at_ms: now_millis(),
        // Deterministic 1x1 transparent PNG. Test mode must never capture or
        // mutate the developer's real desktop.
        data_url: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=".into(),
        width: Some(1),
        height: Some(1),
    }
}

fn stable_identity_component(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn browser_login_platform(platform: SurfacePlatform) -> &'static str {
    match platform {
        SurfacePlatform::Ios | SurfacePlatform::Android => "mobile",
        SurfacePlatform::Wasm => "web",
        SurfacePlatform::Mock | SurfacePlatform::Electron => "desktop",
    }
}

fn stable_authenticated_account_id(auth: &Value) -> Option<String> {
    const ID_KEYS: [&str; 8] = [
        "principalId",
        "principal_id",
        "id",
        "userId",
        "user_id",
        "userNo",
        "user_no",
        "username",
    ];

    auth.get("user")
        .and_then(Value::as_object)
        .and_then(|user| {
            ID_KEYS
                .iter()
                .find_map(|key| stable_identity_component(user.get(*key)))
        })
        .or_else(|| {
            ID_KEYS
                .iter()
                .find_map(|key| stable_identity_component(auth.get(*key)))
        })
}

#[cfg(feature = "production")]
fn auth_payload(response: &Value) -> &Value {
    response
        .get("auth")
        .filter(|value| value.is_object())
        .unwrap_or(response)
}

#[cfg(feature = "production")]
fn auth_account_id(auth: &Value) -> Option<String> {
    stable_authenticated_account_id(auth)
}

#[cfg(feature = "production")]
fn account_fingerprint(account_id: &str) -> String {
    Sha256::digest(account_id.as_bytes())[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(feature = "production")]
fn account_scoped_path(base: &Path, account_id: &str) -> PathBuf {
    base.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("accounts")
        .join(account_fingerprint(account_id))
        .join(
            base.file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("state.json")),
        )
}

#[cfg(feature = "production")]
fn actor_id_for_account_id(account_id: &str) -> ActorId {
    ActorId::new(format!("human:account:{}", account_fingerprint(account_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_persisted_account_preserves_runtime_checkpoint_but_later_replacement_resets() {
        let none = None;
        let account_a = Some("account-a".to_string());
        let account_b = Some("account-b".to_string());

        assert!(!account_boundary_requires_runtime_reset(false, &none, &account_a));
        assert!(!account_boundary_requires_runtime_reset(false, &none, &none));
        assert!(!account_boundary_requires_runtime_reset(true, &account_a, &account_a));
        assert!(account_boundary_requires_runtime_reset(true, &account_a, &account_b));
        assert!(account_boundary_requires_runtime_reset(true, &account_a, &none));
        assert!(account_boundary_requires_runtime_reset(true, &none, &account_a));
    }
    use mahayana_host_protocol::ApprovalDecision;

    #[cfg(feature = "production")]
    fn isolated_host_config(profile: &str) -> HostCreateConfig {
        let root = std::env::temp_dir().join(format!(
            "fabushi-feature-host-{profile}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create isolated Host root");
        HostCreateConfig {
            runtime: mahayana_core::RuntimeConfig {
                data_dir: Some(root.join("runtime")),
                ..Default::default()
            },
            product_session_path: Some(root.join("product-session.json")),
            inherit_installed_plugins: Some(false),
            ..Default::default()
        }
    }

    fn controller() -> FeatureHostController {
        FeatureHostController::create(
            HostConfig {
                profile_id: "fast-e2e".into(),
                mode: HostMode::Test,
            },
            SurfacePlatform::Electron,
        )
        .expect("create feature Host")
    }

    fn async_task_fixture(
        id: &str,
        agent_id: &str,
        kind: AsyncTaskKind,
        started_at_ms: i64,
    ) -> AsyncTaskSummary {
        AsyncTaskSummary {
            kind,
            id: id.into(),
            parent_agent_id: agent_id.into(),
            label: format!("task {id}"),
            status: AsyncTaskStatus::Running,
            started_at_ms,
            detail: None,
            subagent_type: (kind == AsyncTaskKind::Subagent).then(|| "task".into()),
            resource_id: None,
        }
    }

    #[test]
    fn native_turn_identity_context_uses_authenticated_user_and_bot_profile_without_email() {
        let user = json!({
            "id": "human-1",
            "email": "private@example.test",
            "nickname": "莲友"
        });
        let bot = BotSummary {
            id: "agent-research".into(),
            name: "Research Bot".into(),
            description: "Verify sources before conclusions.".into(),
            title: "研究助手".into(),
            hidden: false,
            avatar: None,
            avatar_shape: None,
            avatar_color: None,
            notifications_enabled: true,
            notify_on_updates: true,
            unread: false,
            conversation_id: Some("codex:agent:research".into()),
        };
        let context = render_native_turn_identity_context(Some(&user), Some(&bot))
            .expect("identity context");
        assert!(context.contains("莲友"));
        assert!(context.contains("研究助手"));
        assert!(context.contains("Verify sources before conclusions."));
        assert!(!context.contains("private@example.test"));
    }

    #[test]
    fn canonical_assistant_profile_projects_fabushi_identity() {
        let mut bots = default_bots();
        let bot = bots
            .remove("mahayana-assistant")
            .expect("canonical assistant");
        let context = render_native_turn_identity_context(None, Some(&bot))
            .expect("assistant identity context");
        assert!(context.contains("Current Agent profile name: Fabushi."));
        assert!(!context.contains("Grok"));
    }

    #[test]
    fn async_task_durability_rearms_recent_rows_and_prunes_rows_older_than_48h() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-async-task-durability-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(&root).expect("create async task test root");
        let path = root.join("pending.json");
        let now = 100_000_000_i64;
        let mut tasks = BTreeMap::new();
        tasks.insert(
            async_task_key("agent-a", AsyncTaskKind::Shell, "recent-shell"),
            async_task_fixture(
                "recent-shell",
                "agent-a",
                AsyncTaskKind::Shell,
                now - ASYNC_TASK_STALE_MAX_AGE_MS,
            ),
        );
        tasks.insert(
            async_task_key("agent-a", AsyncTaskKind::CloudAgent, "stale-cloud"),
            async_task_fixture(
                "stale-cloud",
                "agent-a",
                AsyncTaskKind::CloudAgent,
                now - ASYNC_TASK_STALE_MAX_AGE_MS - 1,
            ),
        );
        let operation_ids = BTreeMap::from([
            (
                async_task_key("agent-a", AsyncTaskKind::Shell, "recent-shell"),
                "operation-a".to_string(),
            ),
            (
                async_task_key("agent-a", AsyncTaskKind::CloudAgent, "stale-cloud"),
                "operation-b".to_string(),
            ),
        ]);
        persist_pending_async_tasks(Some(&path), &tasks, &operation_ids)
            .expect("persist pending tasks");

        let (restored, restored_operations) = load_pending_async_tasks(&path, now);
        assert_eq!(restored.len(), 1);
        assert_eq!(
            restored_operations
                .get(&async_task_key("agent-a", AsyncTaskKind::Shell, "recent-shell"))
                .map(String::as_str),
            Some("operation-a")
        );
        let shell = restored
            .get(&async_task_key("agent-a", AsyncTaskKind::Shell, "recent-shell"))
            .expect("recent shell restored");
        assert_eq!(shell.parent_agent_id, "agent-a");
        assert!(
            shell
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("reattached after a host restart"))
        );

        persist_pending_async_tasks(Some(&path), &restored, &restored_operations)
            .expect("prune durable store");
        let raw = std::fs::read_to_string(&path).expect("read pruned pending store");
        assert!(!raw.contains("stale-cloud"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn async_task_durable_identity_includes_parent_agent() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-async-task-agent-scope-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(&root).expect("create async task agent scope root");
        let path = root.join("pending.json");
        let shared_work_id = "shared-work";
        let key_a = async_task_key("agent-a", AsyncTaskKind::CloudAgent, shared_work_id);
        let key_b = async_task_key("agent-b", AsyncTaskKind::CloudAgent, shared_work_id);
        assert_ne!(key_a, key_b);

        let tasks = BTreeMap::from([
            (
                key_a.clone(),
                async_task_fixture(shared_work_id, "agent-a", AsyncTaskKind::CloudAgent, 10),
            ),
            (
                key_b.clone(),
                async_task_fixture(shared_work_id, "agent-b", AsyncTaskKind::CloudAgent, 20),
            ),
        ]);
        let operation_ids = BTreeMap::from([
            (key_a.clone(), "operation-a".to_string()),
            (key_b.clone(), "operation-b".to_string()),
        ]);
        persist_pending_async_tasks(Some(&path), &tasks, &operation_ids)
            .expect("persist agent-scoped tasks");

        let (restored, restored_operations) = load_pending_async_tasks(&path, 30);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.get(&key_a).map(|task| task.parent_agent_id.as_str()), Some("agent-a"));
        assert_eq!(restored.get(&key_b).map(|task| task.parent_agent_id.as_str()), Some("agent-b"));
        assert_eq!(restored_operations.get(&key_a).map(String::as_str), Some("operation-a"));
        assert_eq!(restored_operations.get(&key_b).map(String::as_str), Some("operation-b"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn async_task_settlement_removes_durable_store_when_last_task_finishes() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-async-task-settlement-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(&root).expect("create async task settlement root");
        let path = root.join("pending.json");
        let mut tasks = BTreeMap::new();
        let task_key = async_task_key("agent-a", AsyncTaskKind::Subagent, "subagent-1");
        tasks.insert(
            task_key.clone(),
            async_task_fixture("subagent-1", "agent-a", AsyncTaskKind::Subagent, 10),
        );
        let mut operation_ids =
            BTreeMap::from([(task_key.clone(), "operation-a".to_string())]);
        persist_pending_async_tasks(Some(&path), &tasks, &operation_ids)
            .expect("persist one task");
        assert!(path.is_file());

        tasks.remove(&task_key);
        operation_ids.remove(&task_key);
        persist_pending_async_tasks(Some(&path), &tasks, &operation_ids)
            .expect("settle final task");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn async_task_projection_is_agent_scoped_sorted_and_single_owner() {
        let controller = controller();
        {
            let mut state = controller.state().expect("state");
            state.async_tasks.insert(
                "later".into(),
                async_task_fixture("later", "agent-a", AsyncTaskKind::CloudAgent, 20),
            );
            state.async_tasks.insert(
                "earlier".into(),
                async_task_fixture("earlier", "agent-a", AsyncTaskKind::Shell, 10),
            );
            state.async_tasks.insert(
                "other".into(),
                async_task_fixture("other", "agent-b", AsyncTaskKind::Subagent, 5),
            );
        }

        let projected = controller
            .async_tasks_for_agent("agent-a")
            .expect("project agent tasks");
        assert_eq!(
            projected.iter().map(|task| task.id.as_str()).collect::<Vec<_>>(),
            vec!["earlier", "later"]
        );
        assert!(
            projected
                .iter()
                .all(|task| task.parent_agent_id == "agent-a")
        );
    }

    fn drain(controller: &FeatureHostController) -> Vec<HostEvent> {
        let mut events = Vec::new();
        while let Some(event) = controller.receive().expect("receive event") {
            events.push(event);
        }
        events
    }

    #[test]
    fn awaited_operation_terminal_is_consumed_without_stealing_event_state() {
        let controller = controller();
        let operation_id = "operation-await-terminal".to_string();
        {
            let mut state = controller.state().expect("state");
            state.operations.insert(operation_id.clone());
            state
                .operation_agents
                .insert(operation_id.clone(), "mahayana-assistant".into());
        }

        controller
            .register_awaited_operation(&operation_id)
            .expect("register awaited operation");
        {
            let mut state = controller.state().expect("state");
            state
                .operation_terminals
                .insert(operation_id.clone(), json!({"status": "completed"}));
        }

        let terminal = controller
            .await_operation_step(&operation_id, Duration::ZERO)
            .expect("consume terminal settlement");
        assert_eq!(terminal["status"], "completed");
        let state = controller.state().expect("state");
        assert!(!state.awaited_operations.contains(&operation_id));
        assert!(!state.operation_terminals.contains_key(&operation_id));
    }

    #[test]
    fn awaited_operation_rejects_unknown_or_unregistered_identity() {
        let controller = controller();
        let error = controller
            .register_awaited_operation("missing-operation")
            .expect_err("unknown operation must fail closed");
        assert!(error.to_string().contains("operation is not active"));

        {
            let mut state = controller.state().expect("state");
            state.operations.insert("active-operation".into());
        }
        let error = controller
            .await_operation_step("active-operation", Duration::ZERO)
            .expect_err("unregistered operation must fail closed");
        assert!(error.to_string().contains("not registered"));
    }

    #[test]
    fn close_settles_process_local_host_lifecycle_without_erasing_durable_state() {
        let controller = controller();
        {
            let mut state = controller.state().expect("state");
            state
                .conversation_session
                .schedule_deferred_activation("agent-a", Some("message-1"));
            state.conversation_session.active_conversation_id = Some("agent-a".into());
            state.pending_approvals.insert(
                "approval-1".into(),
                PendingApproval {
                    mini_app_id: "mini-app".into(),
                    capability: "camera".into(),
                    runtime_approval_id: None,
                },
            );
            state.operations.insert("operation-1".into());
            state
                .operation_agents
                .insert("operation-1".into(), "agent-a".into());
            state.awaited_operations.insert("operation-1".into());
            state
                .operation_terminals
                .insert("operation-1".into(), json!({"status": "pending"}));
            state.background_operations.insert(
                "operation-1".into(),
                BackgroundOperationContext {
                    agent_id: "agent-a".into(),
                    agent_name: "Agent A".into(),
                    source: "workflow".into(),
                    teach_artifact: None,
                },
            );
            state.group_operations.insert(
                "operation-1".into(),
                GroupOperationContext {
                    run_id: "group-run-1".into(),
                    group_id: "group-a".into(),
                    member_id: "agent-a".into(),
                    member_name: "Agent A".into(),
                },
            );
            state.automations.insert(
                "durable-automation".into(),
                AutomationSummary {
                    id: "durable-automation".into(),
                    agent_id: Some("agent-a".into()),
                    name: "Durable".into(),
                    prompt: "persist me".into(),
                    schedule: "".into(),
                    trigger: None,
                    enabled: true,
                    created_at_ms: 1,
                    runs: Vec::new(),
                    last_run_at_ms: None,
                    next_run_at_ms: None,
                },
            );
        }

        controller.close().expect("close");
        controller.close().expect("idempotent close");

        let state = controller.state().expect("state");
        assert!(state.closed);
        assert!(state.conversation_session.pending_activation.is_none());
        assert!(state.conversation_session.active_conversation_id.is_none());
        assert!(!state.conversation_session.scene_active);
        assert!(state.pending_approvals.is_empty());
        assert!(state.operations.is_empty());
        assert!(state.operation_agents.is_empty());
        assert!(state.automation_operations.is_empty());
        assert!(state.awaited_operations.is_empty());
        assert!(state.operation_terminals.is_empty());
        assert!(state.background_operations.is_empty());
        assert!(state.group_operations.is_empty());
        assert!(state.automations.contains_key("durable-automation"));
        assert_eq!(
            state
                .events
                .iter()
                .filter(|event| event.kind() == "host.closed")
                .count(),
            1
        );
    }

    #[test]
    fn close_does_not_reclassify_suspended_routine_as_interrupt_candidate() {
        let mut state = FeatureState::default();
        state.operations.insert("routine-operation".into());
        state.operations.insert("interactive-operation".into());
        state.routine_executions.insert(
            "routine-run-close".into(),
            RoutineExecution {
                request_id: "close-test".into(),
                run_id: "routine-run-close".into(),
                automation_id: "daily".into(),
                agent_id: "mahayana-assistant".into(),
                conversation_id: "mahayana-ai:agent:assistant".into(),
                account_key: Some("test-account".into()),
                epoch: state.routine_epoch,
                trigger: RoutineTrigger::Schedule,
                name: "Daily".into(),
                prompt: "continue".into(),
                admitted_at_ms: 1,
                phase: RoutinePhase::Suspended,
                operation_id: Some("routine-operation".into()),
                terminal: None,
            },
        );
        let suspended = state
            .routine_executions
            .values()
            .filter(|execution| execution.phase == RoutinePhase::Suspended)
            .filter_map(|execution| execution.operation_id.as_ref())
            .cloned()
            .collect::<BTreeSet<_>>();
        let interrupt_candidates = state
            .operations
            .iter()
            .filter(|operation_id| !suspended.contains(*operation_id))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(interrupt_candidates, vec!["interactive-operation".to_string()]);
    }

    #[test]
    fn browser_login_platform_maps_native_surfaces_to_mobile() {
        assert_eq!(browser_login_platform(SurfacePlatform::Ios), "mobile");
        assert_eq!(browser_login_platform(SurfacePlatform::Android), "mobile");
        assert_eq!(browser_login_platform(SurfacePlatform::Wasm), "web");
        assert_eq!(browser_login_platform(SurfacePlatform::Electron), "desktop");
    }

    #[test]
    fn deterministic_usage_status_is_ui_safe_and_server_shaped() {
        let controller = controller();
        let usage = controller.usage_status().expect("usage status");
        assert_eq!(usage["tokenLimit"], 100_000);
        assert_eq!(usage["usedTokens"], 25_000);
        assert_eq!(usage["reservedTokens"], 5_000);
        assert_eq!(usage["remainingTokens"], 70_000);
        assert_eq!(usage["unlimited"], false);
        let raw = serde_json::to_string(&usage).expect("serialize usage");
        assert!(!raw.contains("accessToken"));
        assert!(!raw.contains("refreshToken"));
        assert!(!raw.contains("sessionToken"));
    }

    #[test]
    fn remote_computer_target_rejects_stale_generation_and_wrong_device() {
        let controller = controller();
        {
            let mut state = controller.state().expect("state");
            state.remote_computer_sessions.insert(
                "session-1".into(),
                RemoteComputerLocalSession {
                    device_id: "desktop-a".into(),
                    client_id: "phone-a".into(),
                    expires_at_seconds: now_millis() / 1_000 + 60,
                    generation: 7,
                },
            );
        }
        let settings = ProductHostSettings {
            remote_control_enabled: true,
            ..Default::default()
        };
        let stale = ComputerControlTarget::remote_desktop("desktop-a", 6);
        assert!(matches!(
            controller.ensure_computer_origin_allowed(
                ComputerControlOrigin::RemoteMobile,
                Some("session-1"),
                &stale,
                &settings,
            ),
            Err(FeatureHostError::Contract(message)) if message.contains("generation")
        ));
        let wrong = ComputerControlTarget::remote_desktop("desktop-b", 7);
        assert!(matches!(
            controller.ensure_computer_origin_allowed(
                ComputerControlOrigin::RemoteMobile,
                Some("session-1"),
                &wrong,
                &settings,
            ),
            Err(FeatureHostError::Contract(message)) if message.contains("device")
        ));
        let current = ComputerControlTarget::remote_desktop("desktop-a", 7);
        controller
            .ensure_computer_origin_allowed(
                ComputerControlOrigin::RemoteMobile,
                Some("session-1"),
                &current,
                &settings,
            )
            .expect("current target accepted");
    }

    #[test]
    fn feature_host_is_single_owner_for_session_group_and_permission_state() {
        let controller = controller();
        drain(&controller);

        controller
            .set_scene_active(true)
            .expect("activate canonical scene state");

        let mut settings = ProductHostSettings::default();
        settings.local_tool_permission = LocalToolPermission::Always;
        controller
            .execute(FeatureCommand::SettingsUpdate {
                request_id: "single-owner-settings".into(),
                settings,
            })
            .expect("update Host-owned permission settings");

        controller
            .execute(FeatureCommand::GroupCreate {
                request_id: "single-owner-group".into(),
                name: "Single owner room".into(),
                description: "Transcript owner contract".into(),
                avatar: None,
                avatar_shape: None,
                avatar_color: None,
                member_ids: vec!["mahayana-assistant".into(), "research-bot".into()],
            })
            .expect("create group through canonical Host");

        let state = controller.state().expect("canonical FeatureHost state");
        assert!(state.conversation_session.scene_active);
        assert_eq!(
            state.settings.local_tool_permission,
            LocalToolPermission::Always
        );
        assert!(
            state
                .groups
                .values()
                .any(|group| group.name == "Single owner room")
        );
    }

    #[test]
    fn group_chat_handles_mentions_round_order_and_pass_rules() {
        let bots = BTreeMap::from([
            (
                "research-bot".into(),
                BotSummary {
                    id: "research-bot".into(),
                    name: "Research Bot".into(),
                    description: String::new(),
                    title: String::new(),
                    hidden: false,
                    avatar: None,
                    avatar_shape: None,
                    avatar_color: None,
                    notifications_enabled: true,
                    notify_on_updates: true,
                    unread: false,
                    conversation_id: Some("codex:agent:research".into()),
                },
            ),
            (
                "incident-bot".into(),
                BotSummary {
                    id: "incident-bot".into(),
                    name: "Incident Bot".into(),
                    description: String::new(),
                    title: String::new(),
                    hidden: false,
                    avatar: None,
                    avatar_shape: None,
                    avatar_color: None,
                    notifications_enabled: true,
                    notify_on_updates: true,
                    unread: false,
                    conversation_id: Some("codex:agent:incident".into()),
                },
            ),
        ]);
        let mut group = GroupSummary {
            id: "group-1".into(),
            name: "Ops Room".into(),
            description: String::new(),
            avatar: None,
            avatar_shape: None,
            avatar_color: None,
            member_ids: vec!["research-bot".into(), "incident-bot".into()],
            messages: vec![GroupMessage {
                id: "message-1".into(),
                speaker: GroupSpeaker::User { name: None },
                content: "@Research please verify the source".into(),
                created_at_ms: 1,
            }],
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        assert_eq!(
            resolve_group_responders(&group, &bots),
            vec!["research-bot"]
        );
        group.messages.push(GroupMessage {
            id: "message-2".into(),
            speaker: GroupSpeaker::User { name: None },
            content: "@everyone weigh in".into(),
            created_at_ms: 2,
        });
        assert_eq!(
            resolve_group_responders(&group, &bots),
            vec!["research-bot", "incident-bot"]
        );
        assert_eq!(
            order_round_speakers(&["a".into(), "b".into(), "c".into()], 1),
            vec!["b", "c", "a"]
        );
        assert!(is_group_pass_content("(pass)."));
        assert!(is_group_pass_content(" PASS "));
        assert!(!is_group_pass_content("pass this to the next agent"));
    }

    #[test]
    fn group_crud_uses_real_host_state_and_rejects_nested_groups() {
        let controller = controller();
        drain(&controller);
        controller
            .execute(FeatureCommand::GroupCreate {
                request_id: "group-create".into(),
                name: "Research room".into(),
                description: "Cross-check sources".into(),
                avatar: None,
                avatar_shape: None,
                avatar_color: None,
                member_ids: vec!["mahayana-assistant".into(), "research-bot".into()],
            })
            .expect("create group");
        let group = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::GroupChanged { action, group, .. } if action == "created" => Some(group),
                _ => None,
            })
            .expect("created group event");
        controller
            .execute(FeatureCommand::GroupSend {
                request_id: "group-send".into(),
                id: group.id.clone(),
                text: "@Research check this".into(),
            })
            .expect("send group message");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::GroupChanged { action, group: changed, .. }
                if action == "message" && changed.messages.len() == 1
        )));
        let nested = controller.execute(FeatureCommand::GroupCreate {
            request_id: "nested-group".into(),
            name: "Nested".into(),
            description: String::new(),
            avatar: None,
            avatar_shape: None,
            avatar_color: None,
            member_ids: vec![group.id.clone()],
        });
        assert!(nested.is_err());

        {
            let mut state = controller.state().expect("state");
            let template = state.bots.values().next().expect("default bot").clone();
            for index in 0..7 {
                let id = format!("capacity-bot-{index}");
                let mut bot = template.clone();
                bot.id = id.clone();
                bot.name = format!("Capacity {index}");
                state.bots.insert(id, bot);
            }
        }
        let over_capacity = controller.execute(FeatureCommand::GroupCreate {
            request_id: "over-capacity-group".into(),
            name: "Too many".into(),
            description: String::new(),
            avatar: None,
            avatar_shape: None,
            avatar_color: None,
            member_ids: (0..7).map(|index| format!("capacity-bot-{index}")).collect(),
        });
        assert!(matches!(
            over_capacity,
            Err(FeatureHostError::Contract(message)) if message.contains("at most 6")
        ));

        controller
            .execute(FeatureCommand::GroupDelete {
                request_id: "group-delete".into(),
                id: group.id,
            })
            .expect("delete group");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::GroupChanged { action, .. } if action == "deleted"
        )));
    }

    #[test]
    fn group_avatar_update_uses_same_canonical_host_sanitizer() {
        let controller = controller();
        drain(&controller);
        controller
            .execute(FeatureCommand::GroupCreate {
                request_id: "group-avatar-create".into(),
                name: "Avatar room".into(),
                description: String::new(),
                avatar: None,
                avatar_shape: None,
                avatar_color: None,
                member_ids: vec!["mahayana-assistant".into(), "research-bot".into()],
            })
            .expect("create group");
        let group = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::GroupChanged { action, group, .. } if action == "created" => Some(group),
                _ => None,
            })
            .expect("created group");
        controller
            .execute(FeatureCommand::GroupUpdate {
                request_id: "group-avatar-update".into(),
                id: group.id.clone(),
                name: None,
                description: None,
                avatar: Some("data:image/png;base64,iVBORw0KGgo=".into()),
                avatar_shape: None,
                avatar_color: None,
                member_ids: None,
            })
            .expect("update group avatar");
        let updated = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::GroupChanged { action, group, .. } if action == "updated" => Some(group),
                _ => None,
            })
            .expect("updated group");
        assert!(updated.avatar.as_deref().is_some_and(|value| value.starts_with("data:image/png;base64,")));
        controller
            .execute(FeatureCommand::GroupUpdate {
                request_id: "group-avatar-clear".into(),
                id: group.id,
                name: None,
                description: None,
                avatar: Some(String::new()),
                avatar_shape: None,
                avatar_color: None,
                member_ids: None,
            })
            .expect("clear group avatar");
        let cleared = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::GroupChanged { action, group, .. } if action == "updated" => Some(group),
                _ => None,
            })
            .expect("cleared group");
        assert!(cleared.avatar.is_none());
    }

    #[test]
    fn trays_preserve_dedupe_count_and_twenty_item_cap() {
        let controller = controller();
        drain(&controller);
        {
            let mut state = controller.state().expect("state");
            push_error_tray(
                &mut state,
                "research-bot".into(),
                "Provider busy".into(),
                Some("retry later".into()),
                None,
                Some("provider:busy".into()),
            );
            push_error_tray(
                &mut state,
                "research-bot".into(),
                "Provider still busy".into(),
                Some("retry later".into()),
                None,
                Some("provider:busy".into()),
            );
            assert_eq!(state.trays.len(), 1);
            assert_eq!(state.trays[0].count, Some(2));
            assert_eq!(state.trays[0].title, "Provider still busy");
            for index in 0..25 {
                push_error_tray(
                    &mut state,
                    "research-bot".into(),
                    format!("Error {index}"),
                    None,
                    None,
                    Some(format!("error:{index}")),
                );
            }
            assert_eq!(state.trays.len(), MAX_TRAYS);
            assert!(
                state
                    .trays
                    .iter()
                    .all(|tray| tray.id != state.trays[0].dedupe_key.clone().unwrap_or_default())
            );
        }
        controller
            .execute(FeatureCommand::TrayList {
                request_id: "tray-list".into(),
            })
            .expect("list trays");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::TrayListed { ref trays, .. } if trays.len() == MAX_TRAYS
        )));
        controller
            .execute(FeatureCommand::TrayClear {
                request_id: "tray-clear".into(),
            })
            .expect("clear trays");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::TrayChanged { action, .. } if action == "cleared"
        )));
    }

    #[test]
    fn memory_store_preserves_id_dedupe_and_markdown_layout() {
        assert_eq!(memory_id_for("hello"), "aaf4c61ddcc5e8a2");
        let controller = controller();
        drain(&controller);
        controller
            .execute(FeatureCommand::MemoryClear {
                request_id: "memory-clear-initial".into(),
                agent_id: "mahayana-assistant".into(),
            })
            .expect("clear initial memory");
        drain(&controller);
        controller
            .execute(FeatureCommand::MemoryAdd {
                request_id: "memory-add".into(),
                agent_id: "mahayana-assistant".into(),
                content: "  Likes    tea  ".into(),
                kind: MemoryKind::Profile,
            })
            .expect("add profile memory");
        let added = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::MemoryChanged { action, memory, .. } if action == "added" => memory,
                _ => None,
            })
            .expect("added memory event");
        assert_eq!(added.content, "Likes tea");
        assert_eq!(added.id, memory_id_for("likes tea"));
        controller
            .execute(FeatureCommand::MemoryAdd {
                request_id: "memory-duplicate".into(),
                agent_id: "mahayana-assistant".into(),
                content: "likes tea".into(),
                kind: MemoryKind::Log,
            })
            .expect("dedupe memory");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::MemoryChanged { action, memory: None, .. } if action == "duplicate"
        )));
        controller
            .execute(FeatureCommand::MemoryList {
                request_id: "memory-list".into(),
                agent_id: "mahayana-assistant".into(),
                limit: 1000,
            })
            .expect("list memory");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::MemoryListed { count: 1, ref memories, .. }
                if memories.len() == 1 && memories[0].content == "Likes tea"
        )));
        let profile = controller
            .memory_root_path
            .as_ref()
            .expect("memory root")
            .join("mahayana-assistant/memory/profile.md");
        let raw = std::fs::read_to_string(profile).expect("profile markdown");
        assert!(raw.starts_with(MEMORY_PROFILE_HEADER));
        assert!(raw.contains("Likes tea"));
        controller
            .execute(FeatureCommand::MemoryRemove {
                request_id: "memory-remove".into(),
                agent_id: "mahayana-assistant".into(),
                id: added.id,
            })
            .expect("remove memory");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::MemoryChanged { action, .. } if action == "removed"
        )));
    }

    #[test]
    fn automation_schedule_supports_supported_schedule_grammar() {
        let base = 1_750_000_000_000_i64;
        assert_eq!(parse_every_interval_ms("@every 5m"), Some(300_000));
        assert_eq!(next_automation_run("@every 5m", base), Some(base + 300_000));
        assert!(next_automation_run("@daily", base).is_some());
        assert!(next_automation_run("*/15 9-17 * * 1-5", base).is_some());
        assert!(normalize_automation_schedule("not a schedule").is_err());
    }

    #[test]
    fn automation_crud_and_manual_run_use_the_host_event_contract() {
        let controller = controller();
        drain(&controller);
        controller
            .execute(FeatureCommand::AutomationUpsert {
                request_id: "automation-create".into(),
                id: Some("morning-review".into()),
                agent_id: None,
                name: "晨间复盘".into(),
                prompt: "总结昨天的进展并给出今天的三个行动。".into(),
                schedule: "@daily".into(),
                trigger: Some(AutomationTrigger::Schedule {
                    schedule: "@daily".into(),
                }),
                enabled: true,
            })
            .expect("create automation");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AutomationChanged { action, automation, .. }
                if action == "created" && automation.id == "morning-review"
        )));

        controller
            .execute(FeatureCommand::AutomationList {
                request_id: "automation-list".into(),
                agent_id: None,
            })
            .expect("list automations");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AutomationListed { automations, .. }
                if automations.len() == 1 && automations[0].name == "晨间复盘"
        )));

        controller
            .execute(FeatureCommand::AutomationSetEnabled {
                request_id: "automation-pause".into(),
                id: "morning-review".into(),
                agent_id: None,
                enabled: false,
            })
            .expect("pause automation");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AutomationChanged { action, automation, .. }
                if action == "paused" && !automation.enabled
        )));

        controller
            .execute(FeatureCommand::AutomationRun {
                request_id: "automation-run".into(),
                id: "morning-review".into(),
                agent_id: None,
            })
            .expect("run automation");
        let kinds = drain(&controller)
            .into_iter()
            .map(|event| event.kind())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"automation.changed"));
        assert!(kinds.contains(&"chat.message"));

        controller
            .execute(FeatureCommand::AutomationDelete {
                request_id: "automation-delete".into(),
                id: "morning-review".into(),
                agent_id: None,
            })
            .expect("delete automation");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AutomationChanged { action, .. } if action == "deleted"
        )));
    }

    #[test]
    fn automation_agent_scope_filters_routes_and_blocks_cross_agent_mutation() {
        let controller = controller();
        drain(&controller);

        for (id, agent_id, name) in [
            ("research-digest", "research-bot", "Research digest"),
            ("incident-digest", "incident-bot", "Incident digest"),
        ] {
            controller
                .execute(FeatureCommand::AutomationUpsert {
                    request_id: format!("create-{id}"),
                    id: Some(id.into()),
                    agent_id: Some(agent_id.into()),
                    name: name.into(),
                    prompt: format!("Run the {name} task."),
                    schedule: "@daily".into(),
                    trigger: Some(AutomationTrigger::Schedule {
                        schedule: "@daily".into(),
                    }),
                    enabled: true,
                })
                .expect("create agent automation");
            drain(&controller);
        }

        controller
            .execute(FeatureCommand::AutomationList {
                request_id: "research-list".into(),
                agent_id: Some("research-bot".into()),
            })
            .expect("list research automations");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AutomationListed { automations, .. }
                if automations.len() == 1
                    && automations[0].id == "research-digest"
                    && automations[0].agent_id.as_deref() == Some("research-bot")
        )));

        let error = controller
            .execute(FeatureCommand::AutomationSetEnabled {
                request_id: "wrong-owner-pause".into(),
                id: "research-digest".into(),
                agent_id: Some("incident-bot".into()),
                enabled: false,
            })
            .expect_err("cross-agent automation mutation must be rejected");
        assert!(
            error
                .to_string()
                .contains("does not belong to agent incident-bot")
        );

        controller
            .execute(FeatureCommand::AutomationRun {
                request_id: "research-run".into(),
                id: "research-digest".into(),
                agent_id: Some("research-bot".into()),
            })
            .expect("run research automation");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::ChatMessage { role: MessageRole::Assistant, text, .. }
                if text.starts_with("research-bot机器人收到：")
        )));

        let error = controller
            .execute(FeatureCommand::AutomationDelete {
                request_id: "wrong-owner-delete".into(),
                id: "research-digest".into(),
                agent_id: Some("incident-bot".into()),
            })
            .expect_err("cross-agent automation deletion must be rejected");
        assert!(
            error
                .to_string()
                .contains("does not belong to agent incident-bot")
        );
    }

    #[test]
    fn event_automation_write_arms_one_listener_connect_resume_and_consumes_it_once() {
        let controller = controller();
        drain(&controller);

        controller
            .execute(FeatureCommand::AutomationUpsert {
                request_id: "listener-routine-create".into(),
                id: Some("listener-routine".into()),
                agent_id: None,
                name: "Slack listener".into(),
                prompt: "Handle matching messages.".into(),
                schedule: "event:slack:message".into(),
                trigger: Some(AutomationTrigger::Event {
                    source: ListenerPlatform::Slack,
                    event: "message".into(),
                    filter: None,
                    filters: None,
                }),
                enabled: true,
            })
            .expect("create listener automation");
        let first = drain(&controller);
        assert_eq!(
            first
                .iter()
                .filter(|event| matches!(
                    event,
                    HostEvent::TranscriptCard {
                        card: TranscriptCard::ListenerConnect {
                            platform: ListenerPlatform::Slack,
                            connected: false,
                            pending: Some(true),
                            ..
                        },
                        ..
                    }
                ))
                .count(),
            1
        );

        controller
            .execute(FeatureCommand::AutomationUpsert {
                request_id: "listener-routine-update".into(),
                id: Some("listener-routine".into()),
                agent_id: None,
                name: "Slack listener updated".into(),
                prompt: "Handle matching messages carefully.".into(),
                schedule: "event:slack:message".into(),
                trigger: Some(AutomationTrigger::Event {
                    source: ListenerPlatform::Slack,
                    event: "message".into(),
                    filter: None,
                    filters: None,
                }),
                enabled: true,
            })
            .expect("update listener automation");
        assert!(
            drain(&controller).into_iter().all(|event| !matches!(
                event,
                HostEvent::TranscriptCard {
                    card: TranscriptCard::ListenerConnect { .. },
                    ..
                }
            )),
            "same Agent/platform watcher must be deduplicated"
        );

        controller
            .execute(FeatureCommand::ListenerConnect {
                request_id: "listener-connect".into(),
                platform: ListenerPlatform::Slack,
            })
            .expect("connect listener");
        let connected = drain(&controller);
        assert_eq!(
            connected
                .iter()
                .filter(|event| matches!(
                    event,
                    HostEvent::TransportEvent { channel, payload }
                        if channel == "listener-resume"
                            && payload["agentId"] == "mahayana-assistant"
                            && payload["platform"] == "slack"
                            && payload["hidden"] == true
                ))
                .count(),
            1
        );

        controller
            .execute(FeatureCommand::ListenerConnect {
                request_id: "listener-connect-again".into(),
                platform: ListenerPlatform::Slack,
            })
            .expect("repeat connected observation");
        assert!(
            drain(&controller).into_iter().all(|event| !matches!(
                event,
                HostEvent::TransportEvent { channel, .. } if channel == "listener-resume"
            )),
            "consumed resume identity must never wake twice"
        );
    }

    #[test]
    fn listener_resume_watcher_observes_connection_without_manual_list_or_connect_command() {
        let controller = controller();
        drain(&controller);

        controller
            .execute(FeatureCommand::AutomationUpsert {
                request_id: "listener-watch-create".into(),
                id: Some("listener-watch".into()),
                agent_id: None,
                name: "Slack listener".into(),
                prompt: "Handle matching messages.".into(),
                schedule: "event:slack:message".into(),
                trigger: Some(AutomationTrigger::Event {
                    source: ListenerPlatform::Slack,
                    event: "message".into(),
                    filter: None,
                    filters: None,
                }),
                enabled: true,
            })
            .expect("create listener automation");
        let first = drain(&controller);
        assert!(first.into_iter().any(|event| matches!(
            event,
            HostEvent::TranscriptCard {
                card: TranscriptCard::ListenerConnect {
                    platform: ListenerPlatform::Slack,
                    pending: Some(true),
                    ..
                },
                ..
            }
        )));

        controller
            .state()
            .expect("state")
            .listeners
            .get_mut(&ListenerPlatform::Slack)
            .expect("slack listener")
            .is_connected = true;

        let resumed = controller.receive().expect("poll listener watcher");
        assert!(matches!(
            resumed,
            Some(HostEvent::TransportEvent { channel, payload })
                if channel == "listener-resume"
                    && payload["agentId"] == "mahayana-assistant"
                    && payload["platform"] == "slack"
                    && payload["hidden"] == true
        ));
        assert!(
            controller
                .state()
                .expect("state")
                .pending_listener_resumes
                .is_empty(),
            "watcher must consume the pending resume identity before dispatch"
        );
    }

    #[test]
    fn listener_resume_is_pruned_on_disable_delete_disconnect_and_close() {
        let controller = controller();
        drain(&controller);
        let create = |controller: &FeatureHostController, id: &str| {
            controller
                .execute(FeatureCommand::AutomationUpsert {
                    request_id: format!("create-{id}"),
                    id: Some(id.into()),
                    agent_id: None,
                    name: id.into(),
                    prompt: "Handle events.".into(),
                    schedule: "event:github:push".into(),
                    trigger: Some(AutomationTrigger::Event {
                        source: ListenerPlatform::Github,
                        event: "push".into(),
                        filter: None,
                        filters: None,
                    }),
                    enabled: true,
                })
                .expect("create event automation");
            drain(controller);
        };

        create(&controller, "github-a");
        controller
            .execute(FeatureCommand::AutomationSetEnabled {
                request_id: "disable-github-a".into(),
                id: "github-a".into(),
                agent_id: None,
                enabled: false,
            })
            .expect("disable");
        drain(&controller);
        assert!(controller.state().expect("state").pending_listener_resumes.is_empty());

        create(&controller, "github-b");
        controller
            .execute(FeatureCommand::AutomationDelete {
                request_id: "delete-github-b".into(),
                id: "github-b".into(),
                agent_id: None,
            })
            .expect("delete");
        drain(&controller);
        assert!(controller.state().expect("state").pending_listener_resumes.is_empty());

        create(&controller, "github-c");
        controller
            .execute(FeatureCommand::ListenerDisconnect {
                request_id: "disconnect-github".into(),
                platform: ListenerPlatform::Github,
            })
            .expect("disconnect");
        drain(&controller);
        assert!(controller.state().expect("state").pending_listener_resumes.is_empty());

        create(&controller, "github-d");
        controller.close().expect("close");
        assert!(controller.state().expect("state").pending_listener_resumes.is_empty());
    }

    #[test]
    fn cloud_task_resource_id_only_accepts_structured_run_keys() {
        assert_eq!(
            cloud_task_resource_id(Some(&json!({"bcId": "cloud-run-1"}))).as_deref(),
            Some("cloud-run-1")
        );
        assert_eq!(
            cloud_task_resource_id(Some(&json!({"metadata": {"runId": "cloud-run-2"}}))).as_deref(),
            Some("cloud-run-2")
        );
        assert_eq!(
            cloud_task_resource_id(Some(&json!({"id": "generic-step-id"}))),
            None
        );
        assert_eq!(cloud_task_resource_id(Some(&json!({"runId": "   "}))), None);
    }

    #[test]
    fn mcp_settings_use_refresh_event_contract_in_test_mode() {
        let controller = controller();
        drain(&controller);
        controller
            .execute(FeatureCommand::McpSetCustomInstructions {
                request_id: "mcp-instructions-test".into(),
                server: "docs".into(),
                instructions: "Prefer source links.".into(),
            })
            .expect("set MCP custom instructions in test mode");
        controller
            .execute(FeatureCommand::McpSetToolDisabled {
                request_id: "mcp-tool-disable-test".into(),
                server: "docs".into(),
                tool: "delete_page".into(),
                disabled: true,
            })
            .expect("disable MCP tool in test mode");
        let refreshed = drain(&controller)
            .into_iter()
            .filter(|event| matches!(event, HostEvent::McpRefreshed { .. }))
            .count();
        assert_eq!(refreshed, 2);
    }

    #[test]
    fn mcp_instruction_context_is_sorted_bounded_and_hidden() {
        let instructions = std::collections::HashMap::from([
            ("zeta".into(), "Use read-only operations.".into()),
            ("alpha".into(), "Always include source links.".into()),
            ("empty".into(), "   ".into()),
        ]);
        let context = render_mcp_instruction_context(&instructions).expect("MCP context");
        assert!(context.starts_with("[MCP connector operating instructions]"));
        assert!(
            context.find("Connector: alpha").unwrap() < context.find("Connector: zeta").unwrap()
        );
        assert!(!context.contains("Connector: empty"));
        assert!(context.len() < 16_000);
    }

    #[test]
    fn mcp_remove_uses_refresh_event_contract_in_test_mode() {
        let controller = controller();
        drain(&controller);
        controller
            .execute(FeatureCommand::McpRemove {
                request_id: "mcp-remove-test".into(),
                server: "docs".into(),
            })
            .expect("remove MCP server in test mode");
        assert!(
            drain(&controller)
                .into_iter()
                .any(|event| matches!(event, HostEvent::McpRefreshed { .. }))
        );
    }

    #[test]
    fn product_surfaces_emit_stateful_events_without_leaking_secrets() {
        let controller = controller();
        drain(&controller);

        controller
            .execute(FeatureCommand::ConnectorList {
                request_id: "connector-list".into(),
            })
            .expect("list connectors");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::ConnectorListed { connectors, .. }
                if connectors.iter().any(|connector| connector.id == "github")
        )));

        controller
            .execute(FeatureCommand::ConnectorConnect {
                request_id: "connector-connect".into(),
                connector_id: "github".into(),
                account_label: Some("Work".into()),
            })
            .expect("connect GitHub");
        let events = drain(&controller);
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::ConnectorChanged { connector, .. }
                if connector.id == "github"
                    && connector.status == ConnectorStatus::Connected
                    && connector.accounts.iter().any(|account| account.label == "Work")
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::ListenerChanged { integration, .. }
                if integration.platform == ListenerPlatform::Github
                    && integration.is_connected
        )));

        controller
            .execute(FeatureCommand::ConnectorSetToolEnabled {
                request_id: "connector-tool".into(),
                connector_id: "github".into(),
                tool_id: "create_issue".into(),
                enabled: false,
            })
            .expect("disable connector tool");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::ConnectorChanged { action, connector, .. }
                if action == "toolChanged"
                    && connector.tools.iter().any(|tool| tool.id == "create_issue" && !tool.enabled)
        )));

        controller
            .execute(FeatureCommand::SkillUpsert {
                request_id: "skill-create".into(),
                id: Some("skill-release-check".into()),
                name: "Release check".into(),
                description: "Verify a release candidate before publishing.".into(),
                use_when: "Use before a production release.".into(),
                instructions: "Run tests, inspect diffs, and summarize risks.".into(),
                owner_agent_id: Some("mahayana-assistant".into()),
            })
            .expect("create skill");
        drain(&controller);
        controller
            .execute(FeatureCommand::SkillPublish {
                request_id: "skill-publish".into(),
                id: "skill-release-check".into(),
                team_id: "team-mahayana".into(),
            })
            .expect("publish skill");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::SkillChanged { action, skill, .. }
                if action == "published"
                    && skill.publish_state == SkillPublishState::Published
                    && skill.team_id.as_deref() == Some("team-mahayana")
        )));

        controller
            .execute(FeatureCommand::BotSetHidden {
                request_id: "bot-hide".into(),
                id: "research-bot".into(),
                hidden: true,
            })
            .expect("hide bot");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::BotChanged { bot, .. }
                if bot.id == "research-bot" && bot.hidden
        )));

        controller
            .execute(FeatureCommand::DraftResolve {
                request_id: "draft-send".into(),
                draft: MessageDraft::Email {
                    id: "draft-1".into(),
                    from: None,
                    to: vec!["person@example.com".into()],
                    cc: None,
                    subject: "Release ready".into(),
                    body: "The release candidate passed validation.".into(),
                    status: DraftSendState::Editable,
                    error: None,
                },
                action: DraftAction::Send,
            })
            .expect("send draft");
        let events = drain(&controller);
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::DraftChanged {
                status: DraftSendState::Sending,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::DraftChanged {
                status: DraftSendState::Sent,
                ..
            }
        )));

        controller
            .execute(FeatureCommand::SecretProvide {
                request_id: "secret-provide".into(),
                secret_request_id: "deployment-token".into(),
                value: "super-secret-value".into(),
            })
            .expect("provide secret");
        let secret_events = drain(&controller);
        let serialized = serde_json::to_string(&secret_events).expect("serialize events");
        assert!(!serialized.contains("super-secret-value"));
        assert!(secret_events.into_iter().any(|event| matches!(
            event,
            HostEvent::SecretProvided { secret_request_id, .. }
                if secret_request_id == "deployment-token"
        )));

        controller
            .execute(FeatureCommand::AutomationUpsert {
                request_id: "event-routine-create".into(),
                id: Some("regression-triage".into()),
                agent_id: None,
                name: "Regression triage".into(),
                prompt: "Inspect the regression and summarize impact.".into(),
                schedule: "event:sentry:issue.regressed".into(),
                trigger: Some(AutomationTrigger::Event {
                    source: ListenerPlatform::Sentry,
                    event: "issue.regressed".into(),
                    filter: Some("web".into()),
                    filters: None,
                }),
                enabled: true,
            })
            .expect("create event routine");
        drain(&controller);
        assert_eq!(
            controller
                .ingest_listener_event(EventCard {
                    source: ListenerPlatform::Sentry,
                    event: "issue.regressed".into(),
                    title: "Checkout regression".into(),
                    summary: "A production regression was detected.".into(),
                    url: Some("https://sentry.example.invalid/issues/42".into()),
                    actor: Some("sentry".into()),
                    fields: Some(vec![EventField {
                        label: "Project".into(),
                        value: "web".into(),
                    }]),
                    occurred_at_ms: Some(now_millis()),
                })
                .expect("ingest listener event"),
            1
        );
        let event_events = drain(&controller);
        assert!(event_events.iter().any(|event| matches!(
            event,
            HostEvent::TranscriptCard {
                card: TranscriptCard::Event { event },
                ..
            } if event.source == ListenerPlatform::Sentry
                && event.event == "issue.regressed"
                && event.title == "Checkout regression"
        )));
        assert!(event_events.iter().any(|event| matches!(
            event,
            HostEvent::AutomationChanged { action, automation, .. }
                if action == "triggered" && automation.id == "regression-triage"
        )));

        controller
            .execute(FeatureCommand::UpdateCheck {
                request_id: "update-check".into(),
            })
            .expect("check updates");
        let events = drain(&controller);
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::UpdateChanged {
                state: UpdateState::Checking,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::UpdateChanged {
                state: UpdateState::UpToDate { .. },
                ..
            }
        )));
    }

    #[test]
    fn grouped_automation_triggers_validate_schedule_and_match_structured_events() {
        assert!(normalize_automation_trigger(AutomationTrigger::Group {
            listeners: vec![AutomationTrigger::Schedule {
                schedule: "@daily".into(),
            }],
        })
        .is_err());

        let trigger = normalize_automation_trigger(AutomationTrigger::Group {
            listeners: vec![
                AutomationTrigger::Schedule {
                    schedule: "*/15 9-17 * * 1-5".into(),
                },
                AutomationTrigger::Event {
                    source: ListenerPlatform::Sentry,
                    event: "issue.regressed".into(),
                    filter: None,
                    filters: Some(BTreeMap::from([(
                        "projectIds".into(),
                        json!(["web", "api"]),
                    )])),
                },
            ],
        })
        .expect("normalize grouped trigger");

        assert!(automation_next_run(&trigger, "unused", true, 1_750_000_000_000).is_some());

        let event = EventCard {
            source: ListenerPlatform::Sentry,
            event: "issue.regressed".into(),
            title: "Checkout regression".into(),
            summary: "Regression detected".into(),
            url: None,
            actor: None,
            fields: Some(vec![EventField {
                label: "Project".into(),
                value: "web".into(),
            }]),
            occurred_at_ms: Some(1),
        };
        let serialized = serde_json::to_string(&event).expect("serialize event");
        assert!(automation_trigger_matches_event(&trigger, &event, &serialized));

        let nonmatching = EventCard {
            fields: Some(vec![EventField {
                label: "Project".into(),
                value: "mobile".into(),
            }]),
            ..event
        };
        let serialized = serde_json::to_string(&nonmatching).expect("serialize nonmatching event");
        assert!(!automation_trigger_matches_event(
            &trigger,
            &nonmatching,
            &serialized
        ));
    }

    #[test]
    fn event_group_automation_store_survives_reload() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-event-group-store-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let path = root.join("automations.json");
        let trigger = AutomationTrigger::Group {
            listeners: vec![
                AutomationTrigger::Event {
                    source: ListenerPlatform::Github,
                    event: "*".into(),
                    filter: None,
                    filters: Some(BTreeMap::from([
                        ("repo".into(), json!("owner/repo")),
                        ("events".into(), json!(["pr-opened", "ci-failed"])),
                    ])),
                },
                AutomationTrigger::Event {
                    source: ListenerPlatform::Slack,
                    event: "mention".into(),
                    filter: None,
                    filters: Some(BTreeMap::from([(
                        "channel".into(),
                        json!("alerts"),
                    )])),
                },
            ],
        };
        let automation = AutomationSummary {
            id: "event-group".into(),
            agent_id: Some("research-bot".into()),
            name: "Event group".into(),
            prompt: "Handle matching events.".into(),
            schedule: "event:group".into(),
            trigger: Some(trigger),
            enabled: true,
            created_at_ms: 1,
            runs: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        };
        persist_automations(
            &path,
            &BTreeMap::from([("event-group".into(), automation.clone())]),
        )
        .expect("persist event group");

        let loaded = load_automations(&path);
        assert_eq!(loaded.get("event-group"), Some(&automation));
        std::fs::remove_dir_all(&root).expect("remove isolated automation store");
    }

    #[test]
    fn automation_store_round_trips_atomically() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-automation-store-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let path = root.join("automations.json");
        let mut items = BTreeMap::new();
        items.insert(
            "weekly-review".into(),
            AutomationSummary {
                id: "weekly-review".into(),
                agent_id: Some("research-bot".into()),
                name: "每周复盘".into(),
                prompt: "整理本周工作。".into(),
                schedule: "@weekly".into(),
                trigger: Some(AutomationTrigger::Schedule {
                    schedule: "@weekly".into(),
                }),
                enabled: false,
                created_at_ms: 1,
                runs: vec![AutomationRunSummary {
                    id: "weekly-run-1".into(),
                    status: AutomationRunStatus::Error,
                    started_at: 2,
                    detail: Some("network".into()),
                    event: None,
                }],
                last_run_at_ms: Some(2),
                next_run_at_ms: None,
            },
        );
        persist_automations(&path, &items).expect("persist automation store");
        let loaded = load_automations(&path);
        assert_eq!(loaded.get("weekly-review"), items.get("weekly-review"));
        std::fs::remove_dir_all(&root).expect("remove isolated automation store");
    }

    #[test]
    fn automation_run_history_tracks_test_completion_on_the_canonical_summary() {
        let controller = controller();
        drain(&controller);
        controller.execute(FeatureCommand::AutomationUpsert {
            request_id: "create-history".into(),
            id: Some("history-routine".into()),
            agent_id: None,
            name: "History".into(),
            prompt: "Record the run".into(),
            schedule: "@daily".into(),
            trigger: Some(AutomationTrigger::Schedule { schedule: "@daily".into() }),
            enabled: true,
        }).expect("create history routine");
        drain(&controller);
        controller.execute(FeatureCommand::AutomationRun {
            request_id: "run-history".into(),
            id: "history-routine".into(),
            agent_id: None,
        }).expect("run history routine");
        let state = controller.state().expect("state");
        let automation = state.automations.get("history-routine").expect("history routine");
        assert_eq!(automation.runs.len(), 1);
        assert_eq!(automation.runs[0].status, AutomationRunStatus::Ok);
        assert!(automation.runs[0].started_at > 0);
        assert_eq!(automation.last_run_at_ms, Some(automation.runs[0].started_at));
    }

    #[test]
    fn deterministic_rust_backend_executes_every_declared_feature_journey() {
        let controller = controller();
        assert_eq!(drain(&controller)[0].kind(), "host.ready");

        controller
            .execute(FeatureCommand::ChatSend {
                request_id: "chat-1".into(),
                text: "验证极速自动化测试".into(),
                agent_id: None,
                conversation_id: None,
                mode: AgentMode::Agent,
                mode_statement: None,
                model: None,
                attachments: Vec::new(),
                reply_to_message_id: None,
                is_fork: false,
            })
            .expect("chat");
        controller
            .execute(FeatureCommand::MarketplaceInstall {
                request_id: "install-1".into(),
                mini_app_id: "global-dharma".into(),
            })
            .expect("install");
        controller
            .execute(FeatureCommand::MiniAppOpen {
                request_id: "open-1".into(),
                mini_app_id: "global-dharma".into(),
            })
            .expect("open");
        controller
            .execute(FeatureCommand::CapabilityRequest {
                request_id: "capability-1".into(),
                mini_app_id: "global-dharma".into(),
                capability: "camera".into(),
                reason: "scan scripture".into(),
            })
            .expect("capability");
        let approval_id = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::ApprovalRequested { approval_id, .. } => Some(approval_id),
                _ => None,
            })
            .expect("approval id");
        controller
            .resolve_approval(ApprovalResolution {
                approval_id,
                decision: ApprovalDecision::AllowOnce,
            })
            .expect("resolve approval");
        let operation = controller
            .execute(FeatureCommand::RuntimeLongTask {
                request_id: "operation-1".into(),
                label: "index scriptures".into(),
            })
            .expect("long task")
            .operation_id
            .expect("operation id");
        controller.interrupt(&operation).expect("interrupt");
        controller
            .execute(FeatureCommand::SessionClear {
                request_id: "session-1".into(),
            })
            .expect("clear session");
        controller.close().expect("close");

        let kinds = drain(&controller)
            .into_iter()
            .map(|event| event.kind())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"approval.resolved"));
        assert!(kinds.contains(&"operation.started"));
        assert!(kinds.contains(&"operation.interrupted"));
        assert!(kinds.contains(&"session.cleared"));
        assert!(kinds.contains(&"host.closed"));
    }

    #[test]
    fn stable_messaging_account_identity_accepts_numeric_and_legacy_session_ids() {
        assert_eq!(
            stable_authenticated_account_id(&json!({
                "loggedIn": true,
                "user": {"id": 42, "username": "ignored"},
            }))
            .as_deref(),
            Some("42")
        );
        assert_eq!(
            stable_authenticated_account_id(&json!({
                "loggedIn": true,
                "user": {},
                "userId": 77,
                "username": "legacy-user",
            }))
            .as_deref(),
            Some("77")
        );
        assert_eq!(
            stable_authenticated_account_id(&json!({
                "loggedIn": true,
                "user": {"username": "  legacy-name  "},
            }))
            .as_deref(),
            Some("legacy-name")
        );
        assert_eq!(
            stable_authenticated_account_id(&json!({
                "loggedIn": true,
                "user": {"nickname": "No stable identifier"},
            })),
            None
        );
    }

    #[test]
    fn messaging_access_is_issued_only_from_authenticated_account_session() {
        let controller = controller();
        let error = controller
            .issue_messaging_access(
                "desktop:test".into(),
                "account-session:test".into(),
                vec!["messaging".into(), "calls".into()],
                60 * 60 * 1000,
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("authenticated Fabushi account session")
        );
        controller
            .password_login("tester@example.invalid".into(), "secret".into())
            .expect("test account login");
        let issued = controller
            .issue_messaging_access(
                "desktop:test".into(),
                "account-session:test".into(),
                vec!["messaging".into(), "calls".into()],
                60 * 60 * 1000,
            )
            .expect("issue messaging access");
        let token = issued["accessToken"]
            .as_str()
            .expect("one-time access token")
            .to_string();
        let actor_id = ActorId::new(issued["actorId"].as_str().expect("actor id"));
        let root = controller.memory_root_path.as_ref().expect("memory root");
        let access_path = root.join("_messaging").join("access.json");
        let persisted = std::fs::read_to_string(&access_path).expect("read access registry");
        assert!(!persisted.contains(&token));
        let store = FileAccessTokenStore::new(access_path);
        assert!(
            store
                .authorize(
                    token.as_bytes(),
                    &actor_id,
                    "desktop:test",
                    "account-session:test",
                    AccessScope::Messaging,
                    now_millis(),
                )
                .is_ok()
        );
    }

    #[test]
    fn deterministic_browser_login_keeps_credentials_out_of_the_presentation_boundary() {
        let login_controller = controller();
        assert_eq!(login_controller.auth_status().unwrap()["loggedIn"], false);

        let attempt = login_controller
            .browser_login_start()
            .expect("start browser login");
        assert_eq!(attempt["attemptId"], "test-browser-login");
        assert_eq!(
            attempt["loginUrl"],
            "about:blank#fabushi-test-browser-login"
        );
        assert!(attempt.get("accessToken").is_none());
        assert!(attempt.get("refreshToken").is_none());
        assert!(attempt.get("password").is_none());

        let completed = login_controller
            .browser_login_poll(
                attempt["attemptId"]
                    .as_str()
                    .expect("attempt id")
                    .to_string(),
            )
            .expect("complete browser login");
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["auth"]["loggedIn"], true);
        assert!(completed["auth"].get("accessToken").is_none());
        assert!(completed["auth"].get("refreshToken").is_none());
        assert_eq!(login_controller.auth_status().unwrap()["loggedIn"], true);

        let reopened_controller = controller();
        let reopened_attempt = reopened_controller
            .browser_login_start()
            .expect("start reopenable browser login");
        let reopened = reopened_controller
            .browser_login_reopen(
                reopened_attempt["attemptId"]
                    .as_str()
                    .expect("reopen attempt id")
                    .to_string(),
            )
            .expect("reopen browser login");
        assert_eq!(reopened["status"], "pending");
        assert_eq!(reopened["attemptId"], reopened_attempt["attemptId"]);
        assert_eq!(
            reopened["loginUrl"],
            "about:blank#fabushi-test-browser-login"
        );
        assert!(reopened.get("pollSecret").is_none());

        let cancelled_controller = controller();
        let cancelled_attempt = cancelled_controller
            .browser_login_start()
            .expect("start cancellable browser login");
        let cancelled = cancelled_controller
            .browser_login_cancel(
                cancelled_attempt["attemptId"]
                    .as_str()
                    .expect("cancel attempt id")
                    .to_string(),
            )
            .expect("cancel browser login");
        assert_eq!(cancelled["status"], "cancelled");
        assert_eq!(
            cancelled_controller.auth_status().unwrap()["loggedIn"],
            false
        );
    }

    #[cfg(feature = "production")]
    #[test]
    fn test_backend_restores_browser_session_across_controller_restart_and_logout_clears_it() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-feature-host-returning-auth-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create returning auth root");
        let host_config = || HostCreateConfig {
            runtime: mahayana_core::RuntimeConfig {
                data_dir: Some(root.join("runtime")),
                ..Default::default()
            },
            product_session_path: Some(root.join("product-session.json")),
            inherit_installed_plugins: Some(false),
            ..Default::default()
        };
        let config = || HostConfig {
            profile_id: "returning-auth".into(),
            mode: HostMode::Test,
        };

        let first = FeatureHostController::create_with_host_config(
            config(),
            SurfacePlatform::Electron,
            host_config(),
        )
        .expect("create first test Host");
        let attempt = first.browser_login_start().expect("start browser login");
        first
            .browser_login_poll(
                attempt["attemptId"]
                    .as_str()
                    .expect("attempt id")
                    .to_string(),
            )
            .expect("complete browser login");
        assert_eq!(first.auth_status().unwrap()["loggedIn"], true);
        drop(first);

        let reopened = FeatureHostController::create_with_host_config(
            config(),
            SurfacePlatform::Electron,
            host_config(),
        )
        .expect("reopen test Host");
        let restored = reopened.auth_status().expect("restore browser session");
        assert_eq!(restored["loggedIn"], true);
        assert_eq!(restored["user"]["id"], "fast-e2e-browser-user");
        let persisted = std::fs::read_to_string(root.join("runtime/test-auth-session.json"))
            .expect("read persisted test auth state");
        assert!(!persisted.contains("accessToken"));
        assert!(!persisted.contains("refreshToken"));

        reopened.logout().expect("logout returning account");
        drop(reopened);
        let after_logout = FeatureHostController::create_with_host_config(
            config(),
            SurfacePlatform::Electron,
            host_config(),
        )
        .expect("reopen after logout");
        assert_eq!(after_logout.auth_status().unwrap()["loggedIn"], false);
        assert!(!root.join("runtime/test-auth-session.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn deterministic_oauth_journey_matches_the_cross_platform_ui_contract() {
        let controller = controller();
        let providers = controller.auth_providers().expect("OAuth providers");
        assert_eq!(providers.as_array().map(Vec::len), Some(4));
        assert_eq!(providers[0]["id"], "google");

        let attempt = controller
            .oauth_start("google".into())
            .expect("start OAuth");
        assert_eq!(attempt["provider"], "google");
        let completed = controller
            .oauth_poll(
                attempt["attemptId"]
                    .as_str()
                    .expect("attempt id")
                    .to_string(),
            )
            .expect("complete OAuth");
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["auth"]["loggedIn"], true);
        assert_eq!(controller.auth_status().unwrap()["loggedIn"], true);
    }

    #[test]
    fn attachment_store_enforces_limits_content_addressing_and_scoped_reads() {
        let controller = controller();
        drain(&controller);
        let content = b"hello attachment\nsecond line\n";
        controller
            .execute(FeatureCommand::AttachmentUpload {
                request_id: "attachment-upload".into(),
                agent_id: "mahayana-assistant".into(),
                filename: "notes.txt".into(),
                mime_type: Some("text/plain".into()),
                bytes_base64: base64::engine::general_purpose::STANDARD.encode(content),
            })
            .expect("upload attachment");
        let attachment = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::AttachmentStored { attachment, .. } => Some(attachment),
                _ => None,
            })
            .expect("stored attachment event");
        assert_eq!(attachment.size_bytes, content.len() as u64);
        assert_eq!(attachment.hash.len(), 64);
        assert!(attachment.path.ends_with(".txt"));
        assert!(Path::new(&attachment.path).is_file());

        controller
            .execute(FeatureCommand::AttachmentReadText {
                request_id: "attachment-text".into(),
                agent_id: "mahayana-assistant".into(),
                path: attachment.path.clone(),
            })
            .expect("read attachment text");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AttachmentTextRead { result, .. }
                if result.kind == "text"
                    && result.text.as_deref() == Some("hello attachment\nsecond line\n")
                    && !result.truncated
        )));

        controller
            .execute(FeatureCommand::AttachmentReadChunk {
                request_id: "attachment-chunk".into(),
                agent_id: "mahayana-assistant".into(),
                path: attachment.path.clone(),
                offset: 6,
                length: 10,
            })
            .expect("read attachment chunk");
        assert!(drain(&controller).into_iter().any(|event| match event {
            HostEvent::AttachmentChunkRead { result, .. } => {
                base64::engine::general_purpose::STANDARD
                    .decode(result.bytes_base64)
                    .ok()
                    .as_deref()
                    == Some(b"attachment".as_slice())
            }
            _ => false,
        }));

        assert_eq!(attachment_byte_limit_for_name("clip.mp4"), VIDEO_BYTE_LIMIT);
        assert_eq!(
            attachment_byte_limit_for_name("document.pdf"),
            ATTACHMENT_BYTE_LIMIT
        );

        let outside =
            std::env::temp_dir().join(format!("fabushi-attachment-outside-{}", std::process::id()));
        std::fs::write(&outside, b"outside").expect("write outside fixture");
        let escaped = controller.execute(FeatureCommand::AttachmentReadText {
            request_id: "attachment-escape".into(),
            agent_id: "mahayana-assistant".into(),
            path: outside.to_string_lossy().to_string(),
        });
        assert!(
            matches!(escaped, Err(FeatureHostError::Contract(message)) if message.contains("escapes"))
        );
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn agent_messaging_is_async_persistent_and_broadcasts_without_chat_pollution() {
        let controller = controller();
        drain(&controller);
        controller
            .execute(FeatureCommand::BotCreate {
                request_id: "bot-peer".into(),
                name: "Research".into(),
                description: "Research teammate".into(),
                title: "Researcher".into(),
                avatar: None,
                avatar_shape: None,
                avatar_color: None,
            })
            .expect("create peer bot");
        let peer = drain(&controller)
            .into_iter()
            .find_map(|event| match event {
                HostEvent::BotChanged { bot, .. } if bot.name == "Research" => Some(bot),
                _ => None,
            })
            .expect("created peer");

        controller
            .execute(FeatureCommand::AgentSend {
                request_id: "peer-send".into(),
                from_agent_id: "mahayana-assistant".into(),
                target_id: peer.id.clone(),
                text: "Summarize the evidence.".into(),
                images: Vec::new(),
                priority: true,
            })
            .expect("send peer message");
        let events = drain(&controller);
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::AgentPeerMessageChanged { message, .. }
                if message.from_agent_id == "mahayana-assistant"
                    && message.target_id == peer.id
                    && message.priority
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            HostEvent::AgentBackgroundMessage { agent_id, source, .. }
                if agent_id == &peer.id && source == "agent-priority"
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            HostEvent::ChatMessage {
                role: MessageRole::User,
                ..
            }
        )));

        controller
            .execute(FeatureCommand::AgentPeerHistory {
                request_id: "peer-history".into(),
                agent_id: peer.id.clone(),
                limit: 20,
            })
            .expect("load peer history");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AgentPeerHistoryListed { messages, .. }
                if messages.len() == 1 && messages[0].text == "Summarize the evidence."
        )));

        controller
            .execute(FeatureCommand::AgentBroadcast {
                request_id: "broadcast".into(),
                target_ids: None,
                message: "Owner announcement".into(),
            })
            .expect("broadcast");
        assert!(drain(&controller).into_iter().any(|event| matches!(
            event,
            HostEvent::AgentBroadcasted { result, .. }
                if result.total >= 2 && result.scheduled == result.total
        )));
    }

    #[test]
    fn generated_agent_file_image_materializes_for_recipient_media_channel() {
        let path = std::env::temp_dir().join(format!(
            "fabushi-agent-generated-media-{}.png",
            std::process::id()
        ));
        std::fs::write(&path, b"generated-image").expect("write generated image fixture");
        let url = url::Url::from_file_path(&path)
            .expect("file url")
            .to_string();
        let images = vec![AgentMessageImage {
            url,
            alt: Some("generated preview".into()),
        }];
        let data_urls = load_agent_inbound_image_data_urls(&images);
        let _ = std::fs::remove_file(path);
        assert_eq!(data_urls.len(), 1);
        assert_eq!(data_urls[0], "data:image/png;base64,Z2VuZXJhdGVkLWltYWdl");
    }

    #[cfg(not(feature = "production"))]
    #[test]
    fn production_mode_requires_the_explicit_runtime_feature() {
        let error = FeatureHostController::create(
            HostConfig {
                profile_id: "production".into(),
                mode: HostMode::Production,
            },
            SurfacePlatform::Electron,
        )
        .err()
        .expect("production must not fall back to the test backend");
        assert!(matches!(error, FeatureHostError::ProductionUnavailable));
    }

    #[cfg(feature = "production")]
    #[test]
    fn production_uses_the_real_runtime_and_rust_owned_session_store() {
        let controller = FeatureHostController::create_with_host_config(
            HostConfig {
                profile_id: "production".into(),
                mode: HostMode::Production,
            },
            SurfacePlatform::Electron,
            isolated_host_config("production"),
        )
        .expect("create feature Host");
        let ready = drain(&controller);
        assert_eq!(ready[0].kind(), "host.ready");
        assert!(
            controller
                .info()
                .runtime_version
                .starts_with("mahayana-abi-")
        );

        controller
            .execute(FeatureCommand::MarketplaceInstall {
                request_id: "install-1".into(),
                mini_app_id: "global-dharma".into(),
            })
            .expect("verify bundled production MiniApp");
        controller
            .execute(FeatureCommand::SessionClear {
                request_id: "session-1".into(),
            })
            .expect("clear isolated Rust session");

        let kinds = drain(&controller)
            .into_iter()
            .map(|event| event.kind())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"marketplace.installed"));
        assert!(kinds.contains(&"session.cleared"));
    }

    #[cfg(feature = "production")]
    #[test]
    fn transcript_owner_tracks_box_handoff_and_test_settlement_once() {
        let controller = controller();
        drain(&controller);
        let operation_id = OperationId("operation-handoff".into());
        {
            let mut state = controller.state().expect("state");
            state.operations.insert(operation_id.to_string());
            state
                .operation_agents
                .insert(operation_id.to_string(), "mahayana-assistant".into());
        }
        let requested = controller
            .translate_runtime_event(RuntimeEvent::AgentActivity {
                operation_id: operation_id.clone(),
                step_id: "box-handoff:tool-1".into(),
                kind: "box_handoff_request".into(),
                title: "Waiting for user help".into(),
                detail: Some("Complete the sign-in challenge".into()),
                status: RuntimeActivityStatus::Completed,
                metadata: Some(json!({
                    "stepId": "box-handoff:tool-1",
                    "status": "completed",
                    "provider": {
                        "toolCallId": "tool-1",
                        "instruction": "Complete the sign-in challenge",
                        "reason": "auth",
                        "domain": "example.com"
                    }
                })),
            })
            .expect("translate handoff request")
            .expect("handoff request event");
        let handoff_request_id = match requested {
            HostEvent::BoxHandoffRequested {
                request_id,
                agent_id,
                operation_id: source_operation,
                instruction,
                reason,
                ..
            } => {
                assert_eq!(agent_id, "mahayana-assistant");
                assert_eq!(source_operation, operation_id.to_string());
                assert_eq!(instruction, "Complete the sign-in challenge");
                assert_eq!(reason.as_deref(), Some("auth"));
                request_id
            }
            other => panic!("unexpected event: {other:?}"),
        };
        assert_eq!(controller.state().unwrap().pending_box_handoffs.len(), 1);

        let accepted = controller
            .execute(FeatureCommand::BoxHandoffResolve {
                request_id: "resolve-handoff".into(),
                handoff_request_id: handoff_request_id.clone(),
                agent_id: "mahayana-assistant".into(),
                resolution: "completed".into(),
            })
            .expect("settle test handoff");
        assert!(accepted.operation_id.is_none());
        let mut state = controller.state().expect("state");
        assert!(state.pending_box_handoffs.is_empty());
        assert!(matches!(
            state.events.pop_back(),
            Some(HostEvent::BoxHandoffResolved {
                request_id,
                resolution,
                ..
            }) if request_id == handoff_request_id && resolution == "completed"
        ));
        drop(state);
        assert!(
            controller
                .execute(FeatureCommand::BoxHandoffResolve {
                    request_id: "resolve-stale".into(),
                    handoff_request_id,
                    agent_id: "mahayana-assistant".into(),
                    resolution: "completed".into(),
                })
                .is_err()
        );

        controller
            .translate_runtime_event(RuntimeEvent::AgentActivity {
                operation_id,
                step_id: "box-handoff:tool-2".into(),
                kind: "box_handoff_request".into(),
                title: "Waiting for user help".into(),
                detail: Some("Complete a second protected step".into()),
                status: RuntimeActivityStatus::Completed,
                metadata: Some(json!({
                    "stepId": "box-handoff:tool-2",
                    "status": "completed",
                    "provider": {
                        "toolCallId": "tool-2",
                        "instruction": "Complete a second protected step"
                    }
                })),
            })
            .expect("translate second handoff")
            .expect("second handoff event");
        assert_eq!(controller.state().unwrap().pending_box_handoffs.len(), 1);
        controller.close().expect("close host");
        assert!(controller.state().unwrap().pending_box_handoffs.is_empty());
    }

    #[cfg(feature = "production")]
    #[test]
    fn draft_tool_arguments_match_live_gmail_and_slack_schemas() {
        let email = MessageDraft::Email {
            id: "email-1".into(),
            from: Some("sender@example.com".into()),
            to: vec!["one@example.com".into(), "two@example.com".into()],
            cc: Some(vec!["copy@example.com".into()]),
            subject: "Subject".into(),
            body: "Plain text body".into(),
            status: DraftSendState::Editable,
            error: None,
        };
        let gmail_schema = json!({
            "type": "object",
            "properties": {
                "to": {"type": "string"},
                "cc": {"type": "string"},
                "subject": {"type": "string"},
                "from_address": {"type": ["string", "null"]},
                "payload": {"type": "object"}
            }
        });
        let email_args = draft_tool_arguments(&email, Some(&gmail_schema)).expect("email args");
        assert_eq!(email_args["to"], "one@example.com, two@example.com");
        assert_eq!(email_args["cc"], "copy@example.com");
        assert_eq!(email_args["from_address"], "sender@example.com");
        assert_eq!(email_args["payload"]["mime_type"], "text/plain");
        assert_eq!(email_args["payload"]["body"]["content"], "Plain text body");

        let slack = MessageDraft::Slack {
            id: "slack-1".into(),
            workspace: None,
            target: "C012345".into(),
            thread: Some("1234.56".into()),
            body: "hello".into(),
            status: DraftSendState::Editable,
            error: None,
        };
        let slack_schema = json!({
            "type": "object",
            "properties": {
                "channel_id": {"type": "string"},
                "text": {"type": "string"},
                "thread_ts": {"type": "string"}
            }
        });
        let slack_args = draft_tool_arguments(&slack, Some(&slack_schema)).expect("Slack args");
        assert_eq!(slack_args["channel_id"], "C012345");
        assert_eq!(slack_args["text"], "hello");
        assert_eq!(slack_args["thread_ts"], "1234.56");
    }

    #[cfg(feature = "production")]
    #[test]
    fn production_runtime_events_preserve_streaming_and_terminal_states() {
        let controller = FeatureHostController::create_with_host_config(
            HostConfig {
                profile_id: "production-events".into(),
                mode: HostMode::Production,
            },
            SurfacePlatform::Electron,
            isolated_host_config("production-events"),
        )
        .expect("create production event Host");
        let operation_id = OperationId("operation-1".into());
        let conversation_id = ConversationId(MAHAYANA_AI_CONVERSATION_ID.into());

        let delta = controller
            .translate_runtime_event(RuntimeEvent::MessageDelta {
                operation_id: operation_id.clone(),
                conversation_id: conversation_id.clone(),
                delta: "般若".into(),
            })
            .expect("translate delta")
            .expect("delta event");
        assert!(matches!(
            delta,
            HostEvent::ChatDelta {
                operation_id: ref current,
                ref delta,
                ..
            } if current == "operation-1" && delta == "般若"
        ));

        let completed = controller
            .translate_runtime_event(RuntimeEvent::MessageCompleted {
                operation_id: operation_id.clone(),
                message: mahayana_core::Message {
                    id: mahayana_core::MessageId("message-1".into()),
                    conversation_id,
                    role: RuntimeMessageRole::Assistant,
                    text: "般若波罗蜜多".into(),
                    created_at_ms: 1,
                    metadata: serde_json::json!({}),
                },
            })
            .expect("translate completed message")
            .expect("completed message event");
        assert!(matches!(
            completed,
            HostEvent::ChatMessage {
                operation_id: Some(ref current),
                role: MessageRole::Assistant,
                ref text,
                ..
            } if current == "operation-1" && text == "般若波罗蜜多"
        ));

        let terminal = controller
            .translate_runtime_event(RuntimeEvent::OperationCompleted {
                operation_id: operation_id.clone(),
            })
            .expect("translate completion")
            .expect("completion event");
        assert!(matches!(
            terminal,
            HostEvent::OperationCompleted {
                operation_id: ref current,
                ..
            } if current == "operation-1"
        ));

        let interrupted_id = OperationId("operation-interrupted".into());
        {
            let mut state = controller.state().expect("feature state");
            state.operations.insert(interrupted_id.to_string());
            state
                .operation_agents
                .insert(interrupted_id.to_string(), "mahayana-assistant".into());
        }
        let interrupted = controller
            .translate_runtime_event(RuntimeEvent::OperationInterrupted {
                operation_id: interrupted_id.clone(),
                reason: "superseded by a new user message".into(),
            })
            .expect("translate interruption")
            .expect("interruption event");
        assert!(matches!(
            interrupted,
            HostEvent::OperationInterrupted {
                operation_id: ref current,
                reason: Some(ref reason),
                ..
            } if current == "operation-interrupted"
                && reason == "superseded by a new user message"
        ));
        assert!(controller.state().expect("feature state").trays.is_empty());
        assert!(
            controller
                .translate_runtime_event(RuntimeEvent::OperationInterrupted {
                    operation_id: interrupted_id,
                    reason: "duplicate".into(),
                })
                .expect("translate duplicate interruption")
                .is_none()
        );

        let failed = controller
            .translate_runtime_event(RuntimeEvent::OperationFailed {
                operation_id,
                code: "provider_error".into(),
                message: "provider unavailable".into(),
            })
            .expect("translate failure")
            .expect("failure event");
        assert!(matches!(
            failed,
            HostEvent::OperationFailed {
                operation_id: ref current,
                ref code,
                ref message,
                ..
            } if current == "operation-1"
                && code == "provider_error"
                && message == "provider unavailable"
        ));
    }

    #[cfg(feature = "production")]
    #[test]
    fn settled_turn_projects_canonical_automations_only_for_the_still_active_agent() {
        let controller = FeatureHostController::create_with_host_config(
            HostConfig {
                profile_id: "production-automation-projection".into(),
                mode: HostMode::Production,
            },
            SurfacePlatform::Electron,
            isolated_host_config("production-automation-projection"),
        )
        .expect("create production automation projection Host");

        let automation = AutomationSummary {
            id: "research-digest".into(),
            agent_id: Some("research-bot".into()),
            name: "Research digest".into(),
            prompt: "Summarize the current research.".into(),
            schedule: "@daily".into(),
            trigger: Some(AutomationTrigger::Schedule {
                schedule: "@daily".into(),
            }),
            enabled: true,
            created_at_ms: 1,
            runs: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        };

        let completed_id = OperationId("operation-automation-active".into());
        {
            let mut state = controller.state().expect("feature state");
            state.events.clear();
            state.operations.insert(completed_id.to_string());
            state
                .operation_agents
                .insert(completed_id.to_string(), "research-bot".into());
            state
                .conversation_session
                .active_conversation_id = Some("codex:agent:research".into());
            state
                .automations
                .insert(automation.id.clone(), automation.clone());
        }

        let terminal = controller
            .translate_runtime_event(RuntimeEvent::OperationCompleted {
                operation_id: completed_id.clone(),
            })
            .expect("translate successful terminal")
            .expect("successful terminal event");
        assert!(matches!(
            terminal,
            HostEvent::OperationCompleted {
                operation_id,
                ..
            } if operation_id == completed_id.to_string()
        ));

        let projection = controller
            .state()
            .expect("feature state")
            .events
            .pop_front()
            .expect("active Agent automation projection");
        match projection {
            HostEvent::TransportEvent { channel, payload } => {
                assert_eq!(channel, "automations");
                assert_eq!(payload["agentId"], "research-bot");
                assert_eq!(payload["automations"][0]["id"], "research-digest");
                assert_eq!(payload["automations"][0]["agentId"], "research-bot");
            }
            other => panic!("unexpected automation projection: {other:?}"),
        }

        controller
            .translate_runtime_event(RuntimeEvent::OperationCompleted {
                operation_id: completed_id,
            })
            .expect("translate duplicate successful terminal")
            .expect("duplicate terminal remains observable");
        assert!(
            !controller
                .state()
                .expect("feature state")
                .events
                .iter()
                .any(|event| matches!(
                    event,
                    HostEvent::TransportEvent { channel, .. } if channel == "automations"
                )),
            "consumed operation-to-Agent identity must suppress duplicate automation projection"
        );

        let switched_id = OperationId("operation-automation-inactive".into());
        {
            let mut state = controller.state().expect("feature state");
            state.events.clear();
            state.operations.insert(switched_id.to_string());
            state
                .operation_agents
                .insert(switched_id.to_string(), "research-bot".into());
            state
                .conversation_session
                .active_conversation_id = Some("codex:agent:incident".into());
        }
        controller
            .translate_runtime_event(RuntimeEvent::OperationCompleted {
                operation_id: switched_id,
            })
            .expect("translate switched-Agent terminal")
            .expect("switched-Agent terminal event");
        assert!(
            !controller
                .state()
                .expect("feature state")
                .events
                .iter()
                .any(|event| matches!(
                    event,
                    HostEvent::TransportEvent { channel, .. } if channel == "automations"
                )),
            "inactive Agent must not project automations after a Session switch"
        );

        let interrupted_id = OperationId("operation-automation-interrupted".into());
        {
            let mut state = controller.state().expect("feature state");
            state.events.clear();
            state.operations.insert(interrupted_id.to_string());
            state
                .operation_agents
                .insert(interrupted_id.to_string(), "research-bot".into());
            state
                .conversation_session
                .active_conversation_id = Some("codex:agent:research".into());
        }
        controller
            .translate_runtime_event(RuntimeEvent::OperationInterrupted {
                operation_id: interrupted_id,
                reason: "cancelled".into(),
            })
            .expect("translate interrupted terminal")
            .expect("interrupted terminal event");
        assert!(
            !controller
                .state()
                .expect("feature state")
                .events
                .iter()
                .any(|event| matches!(
                    event,
                    HostEvent::TransportEvent { channel, .. } if channel == "automations"
                )),
            "interrupted turn must not project a success automation snapshot"
        );

        let failed_id = OperationId("operation-automation-failed".into());
        {
            let mut state = controller.state().expect("feature state");
            state.events.clear();
            state.operations.insert(failed_id.to_string());
            state
                .operation_agents
                .insert(failed_id.to_string(), "research-bot".into());
        }
        controller
            .translate_runtime_event(RuntimeEvent::OperationFailed {
                operation_id: failed_id,
                code: "provider_error".into(),
                message: "provider unavailable".into(),
            })
            .expect("translate failed terminal")
            .expect("failed terminal event");
        assert!(
            !controller
                .state()
                .expect("feature state")
                .events
                .iter()
                .any(|event| matches!(
                    event,
                    HostEvent::TransportEvent { channel, .. } if channel == "automations"
                )),
            "failed turn must not project a success automation snapshot"
        );
    }

    #[cfg(feature = "production")]
    #[test]
    fn production_tool_activity_uses_host_owned_client_side_tool_v2_producer() {
        let controller = FeatureHostController::create_with_host_config(
            HostConfig {
                profile_id: "production-client-tool-v2".into(),
                mode: HostMode::Production,
            },
            SurfacePlatform::Electron,
            isolated_host_config("production-client-tool-v2"),
        )
        .expect("create production Host");
        let operation_id = OperationId("operation-tool-1".into());
        {
            let mut state = controller.state().expect("feature state");
            state.operations.insert(operation_id.to_string());
            state
                .operation_agents
                .insert(operation_id.to_string(), "agent-tools".into());
            state.events.clear();
        }

        let started = controller
            .translate_runtime_event(RuntimeEvent::AgentActivity {
                operation_id: operation_id.clone(),
                step_id: "tool:call-1".into(),
                kind: "tool".into(),
                title: "Running search".into(),
                detail: None,
                status: RuntimeActivityStatus::Running,
                metadata: Some(json!({"tool": "search", "toolCallId": "call-1"})),
            })
            .expect("translate tool start")
            .expect("tool start event");
        assert!(matches!(started, HostEvent::AgentStep { .. }));

        let first = controller
            .state()
            .expect("feature state")
            .events
            .pop_front()
            .expect("client tool call transport");
        let (epoch, sequence) = match first {
            HostEvent::TransportEvent { channel, payload } => {
                assert_eq!(channel, CLIENT_SIDE_TOOL_V2_FAMILY);
                assert_eq!(payload["kind"], "call");
                assert_eq!(payload["agentId"], "agent-tools");
                assert_eq!(payload["sequence"], 1);
                assert_eq!(
                    payload["message"]["messageType"],
                    "aiserver.v1.ClientSideToolV2Call"
                );
                (
                    payload["epoch"].as_str().expect("epoch").to_string(),
                    payload["sequence"].as_u64().expect("sequence"),
                )
            }
            other => panic!("unexpected event: {other:?}"),
        };
        assert_eq!(sequence, 1);

        let completed = controller
            .translate_runtime_event(RuntimeEvent::AgentActivity {
                operation_id,
                step_id: "tool:call-1".into(),
                kind: "tool".into(),
                title: "Completed search".into(),
                detail: None,
                status: RuntimeActivityStatus::Completed,
                metadata: Some(json!({"tool": "search", "toolCallId": "call-1"})),
            })
            .expect("translate tool completion")
            .expect("tool completion event");
        assert!(matches!(completed, HostEvent::AgentStep { .. }));
        let second = controller
            .state()
            .expect("feature state")
            .events
            .pop_front()
            .expect("client tool result transport");
        match second {
            HostEvent::TransportEvent { channel, payload } => {
                assert_eq!(channel, CLIENT_SIDE_TOOL_V2_FAMILY);
                assert_eq!(payload["kind"], "result");
                assert_eq!(payload["epoch"], epoch);
                assert_eq!(payload["sequence"], 2);
                assert_eq!(
                    payload["message"]["messageType"],
                    "aiserver.v1.ClientSideToolV2Result"
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let before_close_epoch = controller
            .client_side_tool_v2
            .lock()
            .expect("producer")
            .epoch()
            .to_string();
        controller.close().expect("close Host");
        let after_close_epoch = controller
            .client_side_tool_v2
            .lock()
            .expect("producer")
            .epoch()
            .to_string();
        assert_ne!(before_close_epoch, after_close_epoch);
    }

    #[cfg(feature = "production")]
    #[test]
    fn model_reaction_tool_projects_existing_transcript_reaction_transport() {
        let controller = FeatureHostController::create_with_host_config(
            HostConfig {
                profile_id: "production-model-reaction".into(),
                mode: HostMode::Production,
            },
            SurfacePlatform::Electron,
            isolated_host_config("production-model-reaction"),
        )
        .expect("create production Host");
        let operation_id = OperationId("operation-model-reaction".into());
        {
            let mut state = controller.state().expect("feature state");
            state.operations.insert(operation_id.to_string());
            state
                .operation_agents
                .insert(operation_id.to_string(), "agent-reaction".into());
            state.events.clear();
        }
        controller
            .translate_runtime_event(RuntimeEvent::AgentActivity {
                operation_id,
                step_id: "tool:call-react".into(),
                kind: "tool".into(),
                title: "Completed react_to_message".into(),
                detail: None,
                status: RuntimeActivityStatus::Completed,
                metadata: Some(json!({
                    "tool":"react_to_message",
                    "toolCallId":"call-react",
                    "output":{
                        "applied":true,
                        "messageId":"user-reaction-1",
                        "reactions":[{"emoji":"👍","by":"assistant"}],
                        "myReactions":[]
                    }
                })),
            })
            .expect("translate reaction tool")
            .expect("reaction agent step");
        let state = controller.state().expect("feature state");
        assert!(state.events.iter().any(|event| matches!(
            event,
            HostEvent::TransportEvent { channel, payload }
                if channel == "transcript.reaction"
                    && payload["agentId"] == "agent-reaction"
                    && payload["entryId"] == "user-reaction-1"
                    && payload["reactions"] == json!([{"emoji":"👍","by":"assistant"}])
        )));
    }

    #[cfg(feature = "production")]
    #[derive(Default)]
    struct FcmUnreadBackend;

    #[cfg(feature = "production")]
    #[async_trait::async_trait]
    impl mahayana_kernel::EngineBackend for FcmUnreadBackend {
        fn descriptor(&self) -> mahayana_kernel::BackendDescriptor {
            mahayana_kernel::BackendDescriptor {
                id: "fcm-unread-test".into(),
                display_name: "FCM unread deterministic backend".into(),
                native: true,
                capabilities: mahayana_kernel::CapabilitySet::new([
                    mahayana_kernel::Capability::Model,
                ]),
            }
        }

        async fn open_session(
            &self,
            _request: mahayana_kernel::OpenSessionRequest,
        ) -> Result<mahayana_kernel::SessionId, mahayana_kernel::KernelError> {
            Ok(mahayana_kernel::SessionId::new())
        }

        async fn run(
            &self,
            request: mahayana_kernel::RunRequest,
            events: mahayana_kernel::SharedKernelEventSink,
        ) -> Result<(), mahayana_kernel::KernelError> {
            events.emit(mahayana_kernel::KernelEvent::MessageCompleted {
                operation_id: request.operation_id,
                text: "deterministic assistant completion".into(),
            })
        }

        async fn interrupt(
            &self,
            _operation_id: &mahayana_kernel::OperationId,
        ) -> Result<(), mahayana_kernel::KernelError> {
            Ok(())
        }

        async fn resolve_approval(
            &self,
            _resolution: mahayana_kernel::ApprovalResolution,
        ) -> Result<(), mahayana_kernel::KernelError> {
            Ok(())
        }
    }

    #[cfg(feature = "production")]
    fn fcm_unread_production_controller() -> FeatureHostController {
        let profile = format!("fcm-unread-cross-layer-{}", std::process::id());
        let host_config = isolated_host_config(&profile);
        let runtime = MahayanaHost::create_with_engine_backend_for_test(
            host_config,
            std::sync::Arc::new(FcmUnreadBackend),
        )
        .expect("create deterministic production runtime");
        let mut controller = FeatureHostController::create_test_backend(
            HostConfig {
                profile_id: profile,
                mode: HostMode::Test,
            },
            SurfacePlatform::Electron,
            None,
        );
        controller.config.mode = HostMode::Production;
        controller.runtime = Some(runtime);
        controller
    }

    #[cfg(feature = "production")]
    fn fcm_assistant_unread(controller: &FeatureHostController, request_id: &str) -> u32 {
        controller
            .production_list_conversations_from_runtime(request_id.into(), None)
            .expect("authoritative conversation.list");
        let mut state = controller.state().expect("feature state");
        while let Some(event) = state.events.pop_back() {
            if let HostEvent::ConversationListed { conversations, .. } = event {
                return conversations
                    .into_iter()
                    .find(|conversation| conversation.id == MAHAYANA_AI_CONVERSATION_ID)
                    .expect("assistant conversation")
                    .unread_count;
            }
        }
        panic!("conversation.list event missing")
    }

    #[cfg(feature = "production")]
    fn wait_for_fcm_assistant_unread(
        controller: &FeatureHostController,
        expected: u32,
        request_id: &str,
    ) {
        for attempt in 0..100 {
            if fcm_assistant_unread(controller, &format!("{request_id}-{attempt}")) == expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("assistant unread did not become {expected}")
    }

    #[test]
    fn deferred_conversation_activation_is_generation_fenced_and_runs_after_window_delivery() {
        let controller = controller();
        let _ = drain(&controller);

        controller
            .execute(FeatureCommand::ConversationOpenWindowed {
                request_id: "window-a".into(),
                conversation_id: "codex:agent:a".into(),
                before_message_id: None,
                limit: 50,
            })
            .expect("open first bounded conversation");
        controller
            .execute(FeatureCommand::ConversationOpenTail {
                request_id: "window-b".into(),
                conversation_id: "codex:agent:b".into(),
                before_message_id: None,
                limit: 50,
            })
            .expect("supersede bounded conversation");

        let first = controller
            .receive()
            .expect("receive first window")
            .expect("first event");
        let second = controller
            .receive()
            .expect("receive second window")
            .expect("second event");
        assert!(matches!(
            first,
            HostEvent::ConversationWindowOpened {
                ref conversation_id,
                ..
            } if conversation_id == "codex:agent:a"
        ));
        assert!(matches!(
            second,
            HostEvent::ConversationWindowOpened {
                ref conversation_id,
                ..
            } if conversation_id == "codex:agent:b"
        ));
        assert!(
            controller
                .state()
                .expect("feature state")
                .conversation_session
                .active_conversation_id
                .is_none()
        );

        let activated = controller
            .receive()
            .expect("receive activation")
            .expect("activation event");
        assert!(matches!(
            activated,
            HostEvent::ConversationActivated {
                ref conversation_id,
                generation: Some(2),
                ..
            } if conversation_id == "codex:agent:b"
        ));
        assert_eq!(
            controller
                .state()
                .expect("feature state")
                .conversation_session
                .active_conversation_id
                .as_deref(),
            Some("codex:agent:b")
        );
    }

    #[test]
    fn explicit_conversation_switch_invalidates_pending_activation() {
        let controller = controller();
        let _ = drain(&controller);
        controller
            .execute(FeatureCommand::ConversationOpenWindowed {
                request_id: "window-a".into(),
                conversation_id: "codex:agent:a".into(),
                before_message_id: None,
                limit: 50,
            })
            .expect("schedule bounded conversation");
        let _ = controller.receive().expect("receive window");

        controller
            .execute(FeatureCommand::ConversationOpen {
                request_id: "switch-b".into(),
                conversation_id: "codex:agent:b".into(),
            })
            .expect("explicit switch");
        let state = controller.state().expect("feature state");
        assert!(state.conversation_session.pending_activation.is_none());
        assert_eq!(
            state.conversation_session.active_conversation_id.as_deref(),
            Some("codex:agent:b")
        );
    }

    #[test]
    fn conversation_catch_up_is_strictly_after_shipped_anchor_and_missing_anchor_fails_closed() {
        let messages = ["a", "b", "c"]
            .into_iter()
            .map(|id| ConversationMessage {
                id: id.into(),
                role: MessageRole::Assistant,
                text: id.into(),
                created_at_ms: 0,
                reply_to_message_id: None,
                branched: false,
                reactions: Vec::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ConversationSessionState::windowed_catch_up(Some("b"), &messages)
                .into_iter()
                .map(|message| message.id)
                .collect::<Vec<_>>(),
            vec!["c"]
        );
        assert!(ConversationSessionState::windowed_catch_up(Some("missing"), &messages).is_empty());
        assert_eq!(
            ConversationSessionState::windowed_catch_up(None, &messages).len(),
            3
        );
    }

    #[test]
    fn scene_contact_freshness_only_advances_while_active() {
        let mut session = ConversationSessionState::default();
        session.note_contact(10);
        assert_eq!(session.focused_at_ms, None);

        session.set_scene_active(true, 20);
        assert_eq!(session.focused_at_ms, Some(20));
        session.note_contact(30);
        assert_eq!(session.focused_at_ms, Some(30));

        session.set_scene_active(false, 40);
        assert_eq!(session.focused_at_ms, None);
        session.note_contact(50);
        assert_eq!(session.focused_at_ms, None);
    }

    #[cfg(feature = "production")]
    #[test]
    fn fcm_010_13_11_production_adapter_keeps_read_boundary_conversation_scoped() {
        let controller = fcm_unread_production_controller();
        let assistant = ConversationId(MAHAYANA_AI_CONVERSATION_ID.to_string());
        let research = ConversationId("codex:agent:research".to_string());

        assert_eq!(fcm_assistant_unread(&controller, "initial-list"), 0);

        controller
            .runtime()
            .expect("runtime")
            .execute(RuntimeCommand::SendMessage {
                conversation_id: assistant.clone(),
                text: "visible assistant completion".into(),
                display_text: None,
                client_message_id: Some("visible-completion".into()),
                hidden: false,
                show_assistant_output: false,
                recovery_eligible: false,
                reply_to_message_id: None,
                is_fork: false,
                attachment_batch_id: None,
                selected_image_data_urls: Vec::new(),
            })
            .expect("visible production runtime send");
        wait_for_fcm_assistant_unread(&controller, 1, "after-visible");

        controller
            .production_open_conversation_from_runtime("open-research".into(), research.0.clone())
            .expect("explicit unrelated conversation.open");
        assert_eq!(
            fcm_assistant_unread(&controller, "after-unrelated-open"),
            1,
            "opening a shared-provider codex conversation must not clear assistant unread"
        );

        controller
            .runtime()
            .expect("runtime")
            .execute(RuntimeCommand::ConversationHistory {
                conversation_id: assistant.clone(),
                limit: Some(2_000),
            })
            .expect("background history request clamped by Runtime");
        assert_eq!(
            fcm_assistant_unread(&controller, "after-background-history"),
            1,
            "Runtime clamp=500 background history must not acknowledge unread"
        );

        let visible_history_before_hidden = match controller
            .runtime()
            .expect("runtime")
            .execute(RuntimeCommand::ConversationHistory {
                conversation_id: assistant.clone(),
                limit: Some(500),
            })
            .expect("visible history before hidden completion")
        {
            RuntimeResponse::History { data } => data.len(),
            other => panic!("unexpected history response: {other:?}"),
        };
        controller
            .runtime()
            .expect("runtime")
            .execute(RuntimeCommand::SendMessage {
                conversation_id: assistant.clone(),
                text: "hidden background completion".into(),
                display_text: None,
                client_message_id: Some("hidden-completion".into()),
                hidden: true,
                show_assistant_output: false,
                recovery_eligible: false,
                reply_to_message_id: None,
                is_fork: false,
                attachment_batch_id: None,
                selected_image_data_urls: Vec::new(),
            })
            .expect("hidden production runtime send");
        std::thread::sleep(Duration::from_millis(25));
        assert_eq!(fcm_assistant_unread(&controller, "after-hidden"), 1);
        let visible_history_after_hidden = match controller
            .runtime()
            .expect("runtime")
            .execute(RuntimeCommand::ConversationHistory {
                conversation_id: assistant.clone(),
                limit: Some(500),
            })
            .expect("visible history after hidden completion")
        {
            RuntimeResponse::History { data } => data.len(),
            other => panic!("unexpected history response: {other:?}"),
        };
        assert_eq!(visible_history_after_hidden, visible_history_before_hidden);

        controller
            .production_open_conversation_from_runtime("open-assistant".into(), assistant.0.clone())
            .expect("explicit assistant conversation.open");
        assert_eq!(
            fcm_assistant_unread(&controller, "after-assistant-open"),
            0,
            "only explicit assistant open may clear assistant unread"
        );
    }

    #[test]
    fn published_workflow_lifecycle_keeps_private_until_exact_confirmation_and_restores_before_unpublish() {
        let profile = format!("skill-publish-lifecycle-{}", Uuid::new_v4());
        let controller = FeatureHostController::create_test_backend(
            HostConfig {
                profile_id: profile,
                mode: HostMode::Test,
            },
            SurfacePlatform::Ios,
            None,
        );
        let workflow_root = controller
            .workflow_root_path
            .as_deref()
            .expect("workflow root")
            .to_path_buf();
        let agent_root = controller
            .active_account_root(controller.memory_root_path.as_deref())
            .expect("agent root");
        let agent_id = "mahayana-assistant";
        let workflow_id = "release-check";
        let plugin_id = "9001";
        let commit_sha = "abcdef0123456789abcdef0123456789abcdef01";
        let other_sha = "1111111111111111111111111111111111111111";

        let _ = std::fs::remove_dir_all(&workflow_root);
        write_workflow(
            &workflow_root,
            &agent_root,
            agent_id,
            Some(workflow_id),
            "Release check",
            "Verify exact production evidence before shipping.",
            "Inspect the release and its acceptance evidence.",
            None,
            None,
        )
        .expect("create private workflow");

        controller
            .sync_workflow_plugin_facts(
                agent_id,
                json!([{
                    "pluginId": plugin_id,
                    "pluginVersion": other_sha,
                    "name": "release-check",
                    "displayName": "Release check",
                    "publishedByCurrentUser": true,
                    "marketplaceTeamId": 7,
                    "isEnabledForAgent": true
                }]),
            )
            .expect("sync non-matching authoritative plugin facts");
        let unconfirmed = controller
            .confirm_workflow_publish(agent_id, workflow_id, plugin_id, commit_sha)
            .expect("unconfirmed publish remains non-destructive");
        assert_eq!(unconfirmed["confirmed"], false);
        assert!(
            workflow_root.join(workflow_id).join(WORKFLOW_FILENAME).is_file(),
            "private copy must remain before exact pluginId + commit SHA confirmation"
        );

        controller
            .sync_workflow_plugin_facts(
                agent_id,
                json!([{
                    "pluginId": plugin_id,
                    "pluginVersion": commit_sha,
                    "name": "release-check",
                    "displayName": "Release check",
                    "publishedByCurrentUser": true,
                    "marketplaceTeamId": 7,
                    "isEnabledForAgent": true
                }]),
            )
            .expect("sync exact authoritative plugin facts");
        let confirmed = controller
            .confirm_workflow_publish(agent_id, workflow_id, plugin_id, commit_sha)
            .expect("promote confirmed publish");
        assert_eq!(confirmed["confirmed"], true);
        let promoted_id = confirmed["promotedWorkflowId"]
            .as_str()
            .expect("promoted workflow id")
            .to_string();
        assert!(
            !workflow_root.join(workflow_id).exists(),
            "private copy is removed only after authoritative confirmation"
        );
        let cache_root =
            published_workflow_cache_root(&workflow_root, plugin_id).expect("cache root");
        assert!(cache_root.join("skill").join(WORKFLOW_FILENAME).is_file());

        let facts = controller
            .state()
            .expect("state")
            .published_plugins_by_agent
            .get(agent_id)
            .cloned()
            .expect("published facts");
        let projected = published_workflow_summaries(
            &workflow_root,
            &agent_root,
            agent_id,
            &facts,
        );
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, promoted_id);
        assert_eq!(projected[0].source, WorkflowSource::Plugin);
        assert_eq!(projected[0].plugin_id.as_deref(), Some(plugin_id));
        assert!(projected[0].published_by_current_user);

        let next_sha = "2222222222222222222222222222222222222222";
        let not_yet_resynced = controller
            .confirm_published_workflow_resync(agent_id, &promoted_id, plugin_id, next_sha)
            .expect("unconfirmed resync remains pending");
        assert_eq!(not_yet_resynced["confirmed"], false);
        let before_resync = read_published_workflow_cache(&cache_root)
            .expect("published cache before resync");
        assert_eq!(before_resync.plugin_version, commit_sha);

        controller
            .sync_workflow_plugin_facts(
                agent_id,
                json!([{
                    "pluginId": plugin_id,
                    "pluginVersion": next_sha,
                    "name": "release-check",
                    "displayName": "Release check",
                    "publishedByCurrentUser": true,
                    "marketplaceTeamId": 7,
                    "isEnabledForAgent": true
                }]),
            )
            .expect("sync next authoritative plugin version");
        let resynced = controller
            .confirm_published_workflow_resync(agent_id, &promoted_id, plugin_id, next_sha)
            .expect("confirm resync");
        assert_eq!(resynced["confirmed"], true);
        let after_resync = read_published_workflow_cache(&cache_root)
            .expect("published cache after resync");
        assert_eq!(after_resync.plugin_version, next_sha);

        let prepared = controller
            .prepare_workflow_unpublish(agent_id, &promoted_id)
            .expect("restore private copy before remote unpublish");
        assert_eq!(prepared["pluginId"], plugin_id);
        assert_eq!(prepared["teamId"], 7);
        assert_eq!(prepared["restoredWorkflowId"], workflow_id);
        assert!(
            workflow_root.join(workflow_id).join(WORKFLOW_FILENAME).is_file(),
            "private copy must exist before the remote unpublish mutation"
        );
        assert!(
            cache_root.exists(),
            "published cache must remain until the remote unpublish succeeds"
        );

        // A failed remote mutation leaves both the restored private copy and
        // published cache in place. Retrying must reuse that Host-prepared
        // recovery copy rather than dead-ending on its own safety check.
        let retry_prepared = controller
            .prepare_workflow_unpublish(agent_id, &promoted_id)
            .expect("retry unpublish after remote failure");
        assert_eq!(retry_prepared["pluginId"], plugin_id);
        assert_eq!(retry_prepared["restoredWorkflowId"], workflow_id);
        assert_eq!(retry_prepared["restoreReused"], true);
        assert!(workflow_root.join(workflow_id).join(WORKFLOW_FILENAME).is_file());
        assert!(
            workflow_root
                .join(workflow_id)
                .join(PUBLISHED_WORKFLOW_RECOVERY_MARKER)
                .is_file()
        );
        assert!(cache_root.exists());

        // A persisted "restore intended" bit is not enough to prove ownership
        // of an existing private copy. If the Host-prepared copy disappears and
        // an unrelated skill reuses the same id, retry must still fail closed.
        std::fs::remove_dir_all(workflow_root.join(workflow_id))
            .expect("remove prepared recovery copy");
        write_workflow(
            &workflow_root,
            &agent_root,
            agent_id,
            Some(workflow_id),
            "Unrelated local replacement",
            "Must never be mistaken for the Host recovery copy.",
            "Do not overwrite this private skill.",
            None,
            None,
        )
        .expect("create unrelated replacement");
        let collision = controller
            .prepare_workflow_unpublish(agent_id, &promoted_id)
            .expect_err("unrelated replacement must fail closed");
        assert!(collision.to_string().contains("refusing to overwrite"));
        std::fs::remove_dir_all(workflow_root.join(workflow_id))
            .expect("remove unrelated replacement");
        let recreated = controller
            .prepare_workflow_unpublish(agent_id, &promoted_id)
            .expect("recreate missing Host-owned recovery copy");
        assert_eq!(recreated["restoreReused"], false);
        assert!(
            workflow_root
                .join(workflow_id)
                .join(PUBLISHED_WORKFLOW_RECOVERY_MARKER)
                .is_file()
        );

        controller
            .complete_workflow_unpublish(agent_id, plugin_id)
            .expect("complete remote unpublish");
        assert!(!cache_root.exists());
        assert!(
            workflow_root.join(workflow_id).join(WORKFLOW_FILENAME).is_file(),
            "successful unpublish must preserve the restored private copy"
        );
        assert!(
            !workflow_root
                .join(workflow_id)
                .join(PUBLISHED_WORKFLOW_RECOVERY_MARKER)
                .exists(),
            "successful unpublish must remove the internal recovery marker"
        );
        let _ = std::fs::remove_dir_all(&workflow_root);
    }

}
