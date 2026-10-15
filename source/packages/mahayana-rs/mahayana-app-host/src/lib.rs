use base64::Engine as _;
use mahayana_core::{BuildProfile, ModelProviderMode, RuntimeConfig};
use mahayana_feature_host::FeatureHostController;
use mahayana_host::HostCreateConfig;
use mahayana_host_protocol::{
    ApprovalResolution, AutomationSummary, AutomationTrigger, EventCard, FeatureCommand, HostConfig, HostMode,
    ListenerPlatform, SurfacePlatform,
};
use mahayana_js_runtime::{DeepSeekJsHost, scan_package_compatibility};
use mahayana_native_engine::{PlatformExecBroker, ProcessExecution};
use mahayana_plugin_runtime::{
    build_fabapp_package, compute_fabapp_draft_digest, fabapp_signing_payload,
    prepare_local_generated_fabapp_manifest, ExternalReleaseManifest, FabappLocalAuthorBinding,
    FabappManifest, FabappSignature, FabappTrustContext, InstalledPluginPointer,
    LocalGeneratedFabappTrust, PermissionManager, PluginInstaller, PluginState,
};
use mahayana_product::MahayanaProductClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppHostFeatureMode {
    Production,
    Test,
}

const TEST_MARKETPLACE_PLUGINS: &[(&str, &str, &str)] = &[
    ("global-dharma", "全球法布施", "任务、日志与部署"),
    ("faliu-flashcards", "法流记忆卡", "经文牌组与复习"),
    ("platform-publish", "平台发布", "内容发布与自动化"),
    (
        "hermes-installer",
        "Hermes Installer",
        "插件安装与运行时管理",
    ),
    ("bot-father", "Bot Father", "创建和管理机器人"),
    (
        "chatgpt-auto-confirm",
        "ChatGPT Auto Confirm",
        "受控自动确认与任务协作",
    ),
];
const TEST_MARKETPLACE_REPOSITORY: &str = "https://github.com/bhrumom/fabushi";
const TEST_MARKETPLACE_SOURCE_REF: &str = "7b02d8d00e0646e9bf4e90a129cbf203fcff015d";

const SHARING_RPC_METHODS: [&str; 6] = [
    "sharing.state",
    "sharing.createRoomInvite",
    "sharing.respondToRoomJoinRequest",
    "sharing.addOwnAgent",
    "sharing.removeOwnAgent",
    "sharing.leaveRoom",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SharingRpc {
    State,
    CreateRoomInvite,
    RespondToRoomJoinRequest,
    AddOwnAgent,
    RemoveOwnAgent,
    LeaveRoom,
}


impl From<AppHostFeatureMode> for HostMode {
    fn from(value: AppHostFeatureMode) -> Self {
        match value {
            AppHostFeatureMode::Production => HostMode::Production,
            AppHostFeatureMode::Test => HostMode::Test,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppHostError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("host operation failed: {0}")]
    Operation(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRequest {
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResponse {
    pub id: Option<Value>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct PluginAccountContext {
    scope: String,
    generation: String,
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PluginRuntimeAccountBinding {
    account_scope: String,
    account_generation: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginAccountMarker {
    schema_version: u32,
    account_scope: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPluginOwnerProof {
    schema_version: u32,
    account_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginGitAuthBlock {
    plugin_id: String,
    plugin_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marketplace_name: Option<String>,
    release: Value,
    platform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPluginMigrationJournal {
    schema_version: u32,
    account_scope: String,
    state: String,
    entries: Vec<String>,
    completed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalFabappDraftTransaction {
    schema_version: u32,
    account_scope: String,
    account_generation: String,
    transaction_id: String,
    reserved_app_id: String,
    publisher_id: String,
    state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    draft_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalFabappPreparedResource {
    mime: String,
    data_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalFabappPreparedDraft {
    schema_version: u32,
    account_scope: String,
    account_generation: String,
    transaction_id: String,
    draft_digest: String,
    manifest: FabappManifest,
    resources: BTreeMap<String, LocalFabappPreparedResource>,
    total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalFabappHostApproval {
    schema_version: u32,
    account_scope: String,
    account_generation: String,
    transaction_id: String,
    reserved_app_id: String,
    publisher_id: String,
    draft_digest: String,
    user_approval_id: String,
    key_id: String,
    public_key_sha256: String,
    state: String,
}

fn local_fabapp_commit_records_match(
    account: &PluginAccountContext,
    expected_approval: &LocalFabappHostApproval,
    current_approval: Option<&LocalFabappHostApproval>,
    current_transaction: Option<&LocalFabappDraftTransaction>,
) -> bool {
    current_approval.is_some_and(|current| {
        current.schema_version == 1
            && current.state == "approved"
            && current.account_scope == account.scope
            && current.account_generation == account.generation
            && current.user_approval_id == expected_approval.user_approval_id
            && current.transaction_id == expected_approval.transaction_id
            && current.draft_digest == expected_approval.draft_digest
    }) && current_transaction.is_some_and(|current| {
        current.schema_version == 1
            && current.state == "approved"
            && current.account_scope == account.scope
            && current.account_generation == account.generation
            && current.transaction_id == expected_approval.transaction_id
            && current.approval_id.as_deref() == Some(expected_approval.user_approval_id.as_str())
            && current.draft_digest.as_deref() == Some(expected_approval.draft_digest.as_str())
    })
}

pub struct AppHost {
    app_data_dir: PathBuf,
    feature_mode: AppHostFeatureMode,
    product: MahayanaProductClient,
    js: Mutex<DeepSeekJsHost>,
    runtime_accounts: Mutex<BTreeMap<String, PluginRuntimeAccountBinding>>,
    feature: FeatureHostController,
    platform_exec: Option<Arc<PlatformExecBroker>>,
}

impl AppHost {
    pub fn new(app_data_dir: impl Into<PathBuf>) -> Result<Self, AppHostError> {
        let feature_mode = configured_feature_host_mode()?;
        Self::new_with_feature_mode(app_data_dir, feature_mode)
    }

    pub fn new_with_feature_mode(
        app_data_dir: impl Into<PathBuf>,
        feature_mode: AppHostFeatureMode,
    ) -> Result<Self, AppHostError> {
        Self::new_with_feature_mode_and_storage_passphrase(app_data_dir, feature_mode, None)
    }

    pub fn new_with_feature_mode_and_storage_passphrase(
        app_data_dir: impl Into<PathBuf>,
        feature_mode: AppHostFeatureMode,
        storage_passphrase: Option<String>,
    ) -> Result<Self, AppHostError> {
        let app_data_dir = app_data_dir.into();
        std::fs::create_dir_all(&app_data_dir)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let feature_root = feature_host_root(&app_data_dir);
        let (feature, platform_exec) =
            create_feature_host(&app_data_dir, feature_mode, storage_passphrase.clone())?;
        let product = match storage_passphrase {
            Some(passphrase) => {
                MahayanaProductClient::new_with_default_api_base_url_and_storage_passphrase(
                    feature_root.join("account-session.json"),
                    feature_root.join("product-surface.json"),
                    passphrase,
                )
            }
            None => MahayanaProductClient::new_with_default_api_base_url(
                feature_root.join("account-session.json"),
                feature_root.join("product-surface.json"),
            ),
        };
        product
            .bootstrap_ci_test_account_session()
            .map_err(|error| {
                AppHostError::Operation(format!(
                    "GitHub Actions test-account bootstrap failed: {error}"
                ))
            })?;
        Ok(Self {
            app_data_dir,
            feature_mode,
            product,
            js: Mutex::new(
                DeepSeekJsHost::new()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?,
            ),
            runtime_accounts: Mutex::new(BTreeMap::new()),
            feature,
            platform_exec,
        })
    }

    /// Trusted native Human-call transport identity. Credentials remain inside
    /// the Rust product owner; callers receive only the stable user/device IDs
    /// needed for lease and signaling fences.
    pub fn human_call_transport_identity(&self) -> Result<Value, AppHostError> {
        let session = self
            .product
            .device_agent_session()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let user_id = session
            .get("userId")
            .cloned()
            .ok_or_else(|| AppHostError::Operation("account session is missing userId".into()))?;
        let device_id = session
            .get("deviceId")
            .cloned()
            .ok_or_else(|| AppHostError::Operation("account session is missing deviceId".into()))?;
        Ok(json!({
            "userId": user_id,
            "deviceId": device_id,
        }))
    }

    pub fn human_call_ice_servers(&self) -> Result<Value, AppHostError> {
        self.human_call_product_execute("mahayana.calls.ice", json!({}))
    }

    pub fn human_call_remote_list(&self, limit: usize) -> Result<Value, AppHostError> {
        self.human_call_product_execute(
            "mahayana.calls.list",
            json!({"limit": limit.clamp(1, 200)}),
        )
    }

    pub fn human_call_remote_create(
        &self,
        call_id: &str,
        peer_human_id: &str,
    ) -> Result<Value, AppHostError> {
        self.human_call_product_execute(
            "mahayana.calls.create",
            json!({
                "callId": call_id,
                "peerHumanId": peer_human_id,
            }),
        )
    }

    pub fn human_call_remote_get(
        &self,
        call_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<Value, AppHostError> {
        self.human_call_product_execute(
            "mahayana.calls.get",
            json!({
                "callId": call_id,
                "afterSeq": after_seq,
                "limit": limit.clamp(1, 200),
            }),
        )
    }

    pub fn human_call_remote_append_event(
        &self,
        call_id: &str,
        client_event_id: &str,
        generation: u64,
        kind: &str,
        payload: Value,
    ) -> Result<Value, AppHostError> {
        self.human_call_product_execute(
            "mahayana.calls.event.append",
            json!({
                "callId": call_id,
                "clientEventId": client_event_id,
                "generation": generation,
                "kind": kind,
                "payload": payload,
            }),
        )
    }

    /// Resolve an incoming Human call onto the one canonical messaging
    /// conversation owned by FeatureHost. Existing iOS direct conversations are
    /// reused by participant identity; only missing conversations use the
    /// Desktop-compatible deterministic Human-direct id.
    pub fn human_call_ensure_messaging_conversation(
        &self,
        peer_user_id: &str,
        title: &str,
    ) -> Result<Value, AppHostError> {
        let peer_user_id = peer_user_id.trim();
        if peer_user_id.is_empty() {
            return Err(AppHostError::InvalidRequest(
                "Human call peer user id is required".into(),
            ));
        }
        let session = self
            .product
            .device_agent_session()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let local_user_id = session
            .get("userId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppHostError::Operation(
                "account session is missing userId".into(),
            ))?;
        if local_user_id == peer_user_id {
            return Err(AppHostError::InvalidRequest(
                "Human call participants must be distinct".into(),
            ));
        }
        let device_id = session
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppHostError::Operation(
                "account session is missing deviceId".into(),
            ))?;
        let local_actor_id = messaging_actor_id_for_user(local_user_id);
        let peer_actor_id = messaging_peer_actor_id_for_user(peer_user_id);
        let request_id = format!("human-call-sync:{}", uuid::Uuid::new_v4().simple());
        let context = json!({
            "requestId": request_id,
            "deviceId": device_id,
            "actorId": local_actor_id,
            "sessionId": "account-session:human-call-host",
            "sentAtMs": app_host_now_ms(),
        });
        let sync = self.feature_messaging_execute(json!({
            "requestId": request_id,
            "envelope": {
                "protocolVersion": 2,
                "context": context,
                "command": {"type": "sync", "cursor": Value::Null, "limit": 1000},
            },
        }))?;
        if let Some(existing) = find_direct_conversation_for_actors(
            &sync,
            &local_actor_id,
            &peer_actor_id,
        ) {
            return Ok(existing);
        }

        let peer_title = title.trim();
        let peer_title = if peer_title.is_empty() {
            peer_user_id
        } else {
            peer_title
        };
        let local_profile_request = format!(
            "human-call-local-profile:{}",
            uuid::Uuid::new_v4().simple()
        );
        self.feature_messaging_execute(json!({
            "requestId": local_profile_request,
            "envelope": {
                "protocolVersion": 2,
                "context": {
                    "requestId": local_profile_request,
                    "deviceId": device_id,
                    "actorId": local_actor_id,
                    "sessionId": "account-session:human-call-host",
                    "sentAtMs": app_host_now_ms(),
                },
                "command": {
                    "type": "upsertProfile",
                    "actor": human_messaging_actor(&local_actor_id, "当前用户"),
                },
            },
        }))?;

        let peer_profile_request = format!(
            "human-call-peer-profile:{}",
            uuid::Uuid::new_v4().simple()
        );
        self.feature_messaging_execute(json!({
            "requestId": peer_profile_request,
            "envelope": {
                "protocolVersion": 2,
                "context": {
                    "requestId": peer_profile_request,
                    "deviceId": device_id,
                    "actorId": local_actor_id,
                    "sessionId": "account-session:human-call-host",
                    "sentAtMs": app_host_now_ms(),
                },
                "command": {
                    "type": "upsertProfile",
                    "actor": human_messaging_actor(&peer_actor_id, peer_title),
                },
            },
        }))?;

        let conversation_id = deterministic_human_conversation_id(
            local_user_id,
            peer_user_id,
        );
        let now = app_host_now_ms();
        let conversation = json!({
            "id": conversation_id,
            "kind": "direct",
            "title": peer_title,
            "description": Value::Null,
            "avatarUrl": Value::Null,
            "participants": [
                {
                    "actorId": local_actor_id,
                    "role": "owner",
                    "joinedAtMs": now,
                    "mutedUntilMs": Value::Null,
                },
                {
                    "actorId": peer_actor_id,
                    "role": "member",
                    "joinedAtMs": now,
                    "mutedUntilMs": Value::Null,
                },
            ],
            "ownerId": local_actor_id,
            "lastMessageId": Value::Null,
            "lastReadMessageId": Value::Null,
            "unreadCount": 0,
            "mentionCount": 0,
            "pinnedMessageIds": [],
            "notificationSettings": {
                "mutedUntilMs": Value::Null,
                "sound": Value::Null,
                "showPreview": true,
                "notifyMentions": true,
            },
            "permissions": {
                "canSendMessages": true,
                "canSendMedia": true,
                "canSendPolls": true,
                "canAddMembers": true,
                "canPinMessages": true,
                "canManageTopics": true,
                "canManageCalls": true,
            },
            "historyVisibility": "allMembers",
            "topics": [],
            "folderIds": [],
            "archived": false,
            "pinned": false,
            "markedUnread": false,
            "createdAtMs": now,
            "updatedAtMs": now,
        });
        let create_request = format!(
            "human-call-create-conversation:{}",
            uuid::Uuid::new_v4().simple()
        );
        self.feature_messaging_execute(json!({
            "requestId": create_request,
            "envelope": {
                "protocolVersion": 2,
                "context": {
                    "requestId": create_request,
                    "deviceId": device_id,
                    "actorId": local_actor_id,
                    "sessionId": "account-session:human-call-host",
                    "sentAtMs": now,
                },
                "command": {
                    "type": "createConversation",
                    "conversation": conversation,
                },
            },
        }))?;
        Ok(conversation)
    }

    pub fn human_call_peer_for_messaging_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<String, AppHostError> {
        let conversation_id = conversation_id.trim();
        if conversation_id.is_empty() {
            return Err(AppHostError::InvalidRequest(
                "Human call conversation id is required".into(),
            ));
        }
        let session = self
            .product
            .device_agent_session()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let local_user_id = session
            .get("userId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppHostError::Operation(
                "account session is missing userId".into(),
            ))?;
        let device_id = session
            .get("deviceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppHostError::Operation(
                "account session is missing deviceId".into(),
            ))?;
        let local_actor_id = messaging_actor_id_for_user(local_user_id);
        let request_id = format!(
            "human-call-resolve-peer:{}",
            uuid::Uuid::new_v4().simple()
        );
        let sync = self.feature_messaging_execute(json!({
            "requestId": request_id,
            "envelope": {
                "protocolVersion": 2,
                "context": {
                    "requestId": request_id,
                    "deviceId": device_id,
                    "actorId": local_actor_id,
                    "sessionId": "account-session:human-call-host",
                    "sentAtMs": app_host_now_ms(),
                },
                "command": {"type": "sync", "cursor": Value::Null, "limit": 1000},
            },
        }))?;
        let conversation = find_conversation_by_id(&sync, conversation_id)
            .ok_or_else(|| AppHostError::InvalidRequest(
                "Human call scope is not a canonical messaging conversation".into(),
            ))?;
        if conversation.get("kind").and_then(Value::as_str) != Some("direct") {
            return Err(AppHostError::InvalidRequest(
                "shipping Human calls require a direct conversation".into(),
            ));
        }
        let participants = conversation
            .get("participants")
            .and_then(Value::as_array)
            .ok_or_else(|| AppHostError::InvalidRequest(
                "Human call conversation participants are invalid".into(),
            ))?;
        let mut peers = Vec::new();
        let mut contains_local = false;
        for participant in participants {
            let actor_id = participant
                .get("actorId")
                .and_then(Value::as_str)
                .ok_or_else(|| AppHostError::InvalidRequest(
                    "Human call conversation participant actorId is invalid".into(),
                ))?;
            if actor_id == local_actor_id {
                contains_local = true;
                continue;
            }
            if let Some(peer_user_id) = actor_id.strip_prefix("human:platform:") {
                let peer_user_id = peer_user_id.trim();
                if !peer_user_id.is_empty() {
                    peers.push(peer_user_id.to_string());
                }
            } else {
                return Err(AppHostError::InvalidRequest(
                    "Human call peer is not a platform Human identity".into(),
                ));
            }
        }
        peers.sort();
        peers.dedup();
        if !contains_local || peers.len() != 1 {
            return Err(AppHostError::InvalidRequest(
                "shipping Human calls require the authenticated account and exactly one platform Human peer"
                    .into(),
            ));
        }
        Ok(peers.remove(0))
    }

    fn human_call_product_execute(
        &self,
        method: &str,
        mut params: Value,
    ) -> Result<Value, AppHostError> {
        let session = self
            .product
            .device_agent_session()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let access_token = session
            .get("accessToken")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppHostError::Operation("account session is missing accessToken".into()))?;
        let device_id = session
            .get("deviceId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppHostError::Operation("account session is missing deviceId".into()))?;
        let object = params
            .as_object_mut()
            .ok_or_else(|| AppHostError::InvalidRequest("Human-call params must be an object".into()))?;
        object.insert("accessToken".into(), Value::String(access_token.to_string()));
        object.insert("deviceId".into(), Value::String(device_id.to_string()));
        self.product
            .execute(method, &params)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    pub fn dispatch(&self, request: HostRequest) -> HostResponse {
        let id = request.id.clone();
        let mut outcome = self
            .reconcile_plugin_runtime_accounts()
            .and_then(|_| self.handle(&request.method, request.params));
        let post_reconcile = self.reconcile_plugin_runtime_accounts();
        if outcome.is_ok() {
            if let Err(error) = post_reconcile {
                outcome = Err(error);
            }
        }
        match outcome {
            Ok(result) => HostResponse {
                id,
                ok: true,
                result: Some(result),
                error: None,
            },
            Err(error) => HostResponse {
                id,
                ok: false,
                result: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn handle(&self, method: &str, params: Value) -> Result<Value, AppHostError> {
        match method {
            "host.platform" => Ok(json!({"platform": host_platform()})),
            "platformExec.next" => self.platform_exec_next(),
            "platformExec.resolve" => self.platform_exec_resolve(&params),
            "platformExec.reject" => self.platform_exec_reject(&params),
            "platformExec.cancel" => self.platform_exec_cancel(&params),
            method if method.starts_with("feature.") => self.handle_feature(method, params),
            "platform.request" => self
                .product
                .execute("mahayana.platform.request", &params)
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "updateCursorAccountName" => {
                let name = string_param(&params, "name")?;
                self.product
                    .execute("mahayana.auth.profile.update", &json!({"displayName": name}))
                    .map_err(|error| AppHostError::Operation(error.to_string()))
            }
            "submitFeedback" => {
                let message = string_param(&params, "message")?;
                let submission_id = string_param(&params, "submissionId")?;
                self.product
                    .execute(
                        "mahayana.feedback.submit",
                        &json!({
                            "message": message,
                            "submissionId": submission_id,
                        }),
                    )
                    .map_err(|error| AppHostError::Operation(error.to_string()))
            }
            "getForeverBoxStatus" => self
                .product
                .status_agent_box(string_param(&params, "id")?)
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "ensureForeverBox" => self
                .product
                .ensure_agent_box(string_param(&params, "id")?)
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "handBackForeverBox" => self
                .product
                .release_agent_box(
                    string_param(&params, "id")?,
                    string_param(&params, "trigger")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "computer.agentBox.ensure" => self
                .product
                .ensure_agent_box(string_param(&params, "agentId")?)
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "computer.agentBox.release" => self
                .product
                .release_agent_box(
                    string_param(&params, "agentId")?,
                    string_param(&params, "trigger")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            method if method.starts_with("sharing.") => self.handle_sharing(method, params),
            "getLinkMetadata" => self.get_link_metadata(params),
            "reactToMessage" => self
                .feature
                .react_to_message(
                    string_param(&params, "agentId")?,
                    string_param(&params, "entryId")?,
                    string_param(&params, "emoji")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "listAgents" => serde_json::to_value(json!({
                "agents": self.feature.agent_roster_direct()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?
            }))
            .map_err(|error| AppHostError::Operation(error.to_string())),
            "createAgent" => self.create_agent(params),
            "sendPrompt" => self.send_agent_prompt(params),
            "getPluginSyncStatus" => self.plugin_sync_status(),
            "getHostSettings" => {
                let has_seen_onboarding = self
                    .feature
                    .host_settings_onboarding_seen_direct()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
                let user_time_zone = self
                    .feature
                    .host_settings_user_timezone_direct()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
                let feature_flag_overrides = self
                    .feature
                    .host_settings_feature_flag_overrides_direct()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
                Ok(json!({
                    "hasSeenOnboarding": has_seen_onboarding,
                    "userTimeZone": user_time_zone,
                    "featureFlagOverrides": feature_flag_overrides,
                }))
            }
            "setHostSettings" => {
                let object = params
                    .get("settings")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        AppHostError::InvalidRequest(
                            "host settings payload must contain a settings object".into(),
                        )
                    })?;
                let mut touched = false;
                if let Some(raw) = object.get("hasSeenOnboarding") {
                    touched = true;
                    let value = match raw {
                        Value::Bool(value) => Some(*value),
                        Value::Null => None,
                        _ => return Err(AppHostError::InvalidRequest(
                            "hasSeenOnboarding must be boolean or null".into(),
                        )),
                    };
                    self.feature
                        .set_has_seen_onboarding_direct(value)
                        .map_err(|error| AppHostError::Operation(error.to_string()))?;
                }
                if let Some(raw) = object.get("userTimeZone") {
                    touched = true;
                    let value = match raw {
                        Value::String(value) => Some(value.clone()),
                        Value::Null => None,
                        _ => return Err(AppHostError::InvalidRequest(
                            "userTimeZone must be an IANA string or null".into(),
                        )),
                    };
                    self.feature
                        .set_user_timezone_direct(value)
                        .map_err(|error| AppHostError::Operation(error.to_string()))?;
                }
                if let Some(raw) = object.get("featureFlagOverrides") {
                    touched = true;
                    let overrides = serde_json::from_value::<
                        std::collections::BTreeMap<String, bool>
                    >(raw.clone())
                    .map_err(|error| AppHostError::InvalidRequest(format!(
                        "featureFlagOverrides must be a boolean map: {error}"
                    )))?;
                    self.feature
                        .set_feature_flag_overrides_direct(overrides)
                        .map_err(|error| AppHostError::Operation(error.to_string()))?;
                }
                if !touched {
                    return Err(AppHostError::InvalidRequest(
                        "host settings payload must include a supported field".into(),
                    ));
                }
                let has_seen_onboarding = self
                    .feature
                    .host_settings_onboarding_seen_direct()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
                let user_time_zone = self
                    .feature
                    .host_settings_user_timezone_direct()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
                let feature_flag_overrides = self
                    .feature
                    .host_settings_feature_flag_overrides_direct()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
                Ok(json!({
                    "hasSeenOnboarding": has_seen_onboarding,
                    "userTimeZone": user_time_zone,
                    "featureFlagOverrides": feature_flag_overrides,
                }))
            }
            "feature.settings.localToolPermission" => {
                let permission = serde_json::from_value::<mahayana_host_protocol::LocalToolPermission>(
                    params.get("permission").cloned().unwrap_or(Value::Null),
                )
                .map_err(|error| AppHostError::InvalidRequest(format!(
                    "invalid local-tool permission: {error}"
                )))?;
                serde_json::to_value(
                    self.feature
                        .set_local_tool_permission_direct(permission)
                        .map_err(|error| AppHostError::Operation(error.to_string()))?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string()))
            }
            "feature.settings.autoReviewRules" => {
                let rules: Vec<mahayana_host_protocol::AutoReviewRule> = serde_json::from_value(
                    params.get("rules").cloned().unwrap_or_else(|| json!([]))
                )
                .map_err(|error| AppHostError::InvalidRequest(format!(
                    "invalid auto-review rules: {error}"
                )))?;
                serde_json::to_value(
                    self.feature
                        .set_auto_review_rules_direct(rules)
                        .map_err(|error| AppHostError::Operation(error.to_string()))?
                )
                .map_err(|error| AppHostError::Operation(error.to_string()))
            }
            "getAgentChannels" => self
                .feature
                .agent_channels(string_param(&params, "id")?)
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "connectChannel" => self
                .feature
                .connect_agent_channel(
                    string_param(&params, "id")?,
                    string_param(&params, "platform")?,
                    string_param(&params, "token")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "disconnectChannel" => self
                .feature
                .disconnect_agent_channel(
                    string_param(&params, "id")?,
                    string_param(&params, "platform")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "refreshChannel" => self
                .feature
                .refresh_agent_channel(
                    string_param(&params, "id")?,
                    string_param(&params, "platform")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "getAsyncTasks" => {
                let agent_id = string_param(&params, "id")?;
                let tasks = self
                    .feature
                    .async_tasks_for_agent(agent_id)
                    .map_err(|error| AppHostError::Operation(error.to_string()))?
                    .into_iter()
                    .map(|task| {
                        let mut value = serde_json::to_value(task)
                            .map_err(|error| AppHostError::Operation(error.to_string()))?;
                        if let Value::Object(object) = &mut value {
                            object.remove("parentAgentId");
                            object.remove("resourceId");
                        }
                        Ok(value)
                    })
                    .collect::<Result<Vec<_>, AppHostError>>()?;
                Ok(Value::Array(tasks))
            }
            "native.cloudAgent.pendingWakes" => serde_json::to_value(
                self.feature
                    .pending_cloud_agent_wakes()
                    .map_err(|error| AppHostError::Operation(error.to_string()))?,
            )
            .map_err(|error| AppHostError::Operation(error.to_string())),
            "native.cloudAgent.settleWake" => {
                let agent_id = string_param(&params, "agentId")?;
                let work_id = string_param(&params, "workId")?;
                let status = string_param(&params, "status")?;
                let result = params
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let settled = self
                    .feature
                    .settle_cloud_agent_wake(agent_id, work_id, status, result)
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
                Ok(json!({"settled": settled}))
            }
            "listAllAutomations" => self.list_all_automations(),
            "plugin.permissions" => self.plugin_permissions(params),
            "plugin.permission.grant" => self.set_permission(params, true),
            "plugin.permission.revoke" => self.set_permission(params, false),
            "plugin.compatibility" => self.plugin_compatibility(params),
            "runtime.start" => self.start_runtime(params),
            "runtime.stop" => self.stop_runtime(params),
            "runtime.tools" => self.runtime_tools(),
            "runtime.call" => self.runtime_call(params),
            other => Err(AppHostError::InvalidRequest(format!(
                "unknown method {other}"
            ))),
        }
    }


    fn create_agent(&self, params: Value) -> Result<Value, AppHostError> {
        let name = string_param(&params, "name")?.trim().to_string();
        if name.is_empty() {
            return Err(AppHostError::InvalidRequest("Agent name must not be empty".into()));
        }
        let description = params
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let purpose = params
            .get("purpose")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let is_introduction_suppressed = params
            .get("isIntroductionSuppressed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let before = self
            .feature
            .agent_roster_direct()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let before_ids = before
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let request_id = format!("native-create-agent-{}", uuid::Uuid::new_v4().simple());
        self.feature_execute(json!({
            "command": {
                "type": "bot.create",
                "requestId": request_id,
                "name": name,
                "description": description,
                "purpose": purpose,
                "isIntroductionSuppressed": is_introduction_suppressed,
            }
        }))?;
        let after = self
            .feature
            .agent_roster_direct()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let created = after
            .into_iter()
            .find(|agent| !before_ids.contains(&agent.id))
            .ok_or_else(|| AppHostError::Operation("Agent creation did not produce a roster entry".into()))?;
        serde_json::to_value(json!({"agent": created}))
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn send_agent_prompt(&self, params: Value) -> Result<Value, AppHostError> {
        let agent_id = string_param(&params, "agentId")?.trim();
        let prompt = string_param(&params, "prompt")?.trim();
        if agent_id.is_empty() || prompt.is_empty() {
            return Err(AppHostError::InvalidRequest(
                "sendPrompt requires non-empty agentId and prompt".into(),
            ));
        }
        self.feature_execute(json!({
            "command": {
                "type": "chat.send",
                "requestId": format!("native-send-prompt-{}", uuid::Uuid::new_v4().simple()),
                "agentId": agent_id,
                "text": prompt,
            }
        }))
    }

    fn handle_sharing(&self, method: &str, params: Value) -> Result<Value, AppHostError> {
        let rpc = sharing_rpc(method)?;
        validate_sharing_params(rpc, &params)?;
        match rpc {
            SharingRpc::State => self
                .product
                .sharing_state()
                .map_err(|error| AppHostError::Operation(error.to_string())),
            SharingRpc::CreateRoomInvite => self
                .product
                .sharing_create_room_invite(string_param(&params, "roomId")?)
                .map_err(|error| AppHostError::Operation(error.to_string())),
            SharingRpc::RespondToRoomJoinRequest => self
                .product
                .sharing_respond_to_join_request(
                    string_param(&params, "requestId")?,
                    bool_param(&params, "isApproved")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            SharingRpc::AddOwnAgent => self
                .product
                .sharing_add_own_agent(
                    string_param(&params, "roomId")?,
                    string_param(&params, "agentId")?,
                    string_param(&params, "agentName")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            SharingRpc::RemoveOwnAgent => self
                .product
                .sharing_remove_own_agent(
                    string_param(&params, "roomId")?,
                    string_param(&params, "agentId")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            SharingRpc::LeaveRoom => self
                .product
                .sharing_leave_room(
                    string_param(&params, "roomId")?,
                    params
                        .get("targetAuthId")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty()),
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
        }
    }

    fn list_all_automations(&self) -> Result<Value, AppHostError> {
        let rows = self
            .feature
            .list_all_automations()
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .into_iter()
            .filter_map(|automation| routine_projection(automation))
            .collect::<Vec<_>>();
        Ok(Value::Array(rows))
    }

    fn get_link_metadata(&self, params: Value) -> Result<Value, AppHostError> {
        let requested = string_param(&params, "url")?;
        let parsed = validate_public_link_url(requested)?;
        let hostname = parsed
            .host_str()
            .ok_or_else(|| AppHostError::InvalidRequest("link URL hostname is required".into()))?
            .to_string();

        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(8))
            .redirects(0)
            .build();
        let mut current = parsed;
        for redirect_count in 0..=LINK_METADATA_MAX_REDIRECTS {
            let response = match agent
                .get(current.as_str())
                .set("User-Agent", "Fabushi-iOS-LinkMetadata/1.0")
                .set("Accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1")
                .call()
            {
                Ok(response) => response,
                Err(ureq::Error::Status(status, response)) if (300..400).contains(&status) => {
                    if redirect_count == LINK_METADATA_MAX_REDIRECTS {
                        return Err(AppHostError::Operation(
                            "link metadata exceeded redirect limit".into(),
                        ));
                    }
                    let location = response.header("Location").ok_or_else(|| {
                        AppHostError::Operation("link metadata redirect omitted Location".into())
                    })?;
                    let next = current.join(location).map_err(|error| {
                        AppHostError::InvalidRequest(format!("invalid link metadata redirect: {error}"))
                    })?;
                    current = validate_public_link_url(next.as_str())?;
                    continue;
                }
                Err(error) => {
                    return Err(AppHostError::Operation(format!(
                        "link metadata request failed: {error}"
                    )));
                }
            };

            let content_type = response
                .header("Content-Type")
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !content_type.is_empty()
                && !content_type.contains("text/html")
                && !content_type.contains("application/xhtml+xml")
            {
                return Ok(json!({
                    "hostname": hostname,
                    "title": hostname,
                }));
            }

            let mut html = String::new();
            response
                .into_reader()
                .take(LINK_METADATA_HTML_BYTE_LIMIT)
                .read_to_string(&mut html)
                .map_err(|error| AppHostError::Operation(format!("read link metadata: {error}")))?;
            let title = html_tag_text(&html, "title");
            let description = html_meta_content(&html, "description")
                .or_else(|| html_meta_property_content(&html, "og:description"));
            let image = html_meta_property_content(&html, "og:image");
            return Ok(json!({
                "hostname": hostname,
                "title": title.unwrap_or_else(|| hostname.clone()),
                "description": description,
                "imageDataUrl": Value::Null,
                "faviconDataUrl": Value::Null,
                "imageUrl": image,
            }));
        }
        Err(AppHostError::Operation(
            "link metadata exceeded redirect limit".into(),
        ))
    }

    fn handle_feature(&self, method: &str, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .note_scene_contact()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        match method {
            "feature.info" => serde_json::to_value(self.feature.info())
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.execute" => self.feature_execute(params),
            "feature.awaitOperation" => self.feature_await_operation(params),
            "feature.awaitOperation.cancel" => self.feature_cancel_await_operation(params),
            "feature.receive" => self.feature_receive(params),
            "feature.automation.ingestListenerEvent" => self.feature_automation_ingest_listener_event(params),
            "feature.automation.relaySubscription" => self.feature_automation_relay_subscription(),
            "feature.automation.pollListenerReconnects" => self.feature_automation_poll_listener_reconnects(),
            "feature.automation.admitBackendFire" => self.feature_automation_admit_backend_fire(params),
            "feature.automation.backendFireStatus" => self.feature_automation_backend_fire_status(params),
            "feature.automation.backendFirePollState" => self.feature_automation_backend_fire_poll_state(),
            "feature.automation.cloudSyncSnapshot" => self.feature_automation_cloud_sync_snapshot(),
            "feature.automation.cloudSyncPlan" => self.feature_automation_cloud_sync_plan(params),
            "feature.automation.cloudSyncCommit" => self.feature_automation_cloud_sync_commit(params),
            "feature.automation.cloudSyncMarkFailed" => self.feature_automation_cloud_sync_mark_failed(params),
            // Trusted native Host surface. Credentials remain Rust-owned inside
            // MahayanaProductClient and are never accepted from or returned to Swift/UI.
            "feature.agentStore.list" => self.feature_agent_store_list(params),
            "feature.agentStore.read" => self.feature_agent_store_read(params),
            "feature.agentStore.write" => self.feature_agent_store_write(params),
            "feature.agentStore.delete" => self.feature_agent_store_delete(params),
            "feature.agentStore.manifestRead" => self.feature_agent_store_manifest_read(params),
            "feature.agentStore.manifestWrite" => self.feature_agent_store_manifest_write(params),
            "feature.sessionActivity" => self.feature_session_activity(params),
            "feature.approval.resolve" => self.feature_resolve_approval(params),
            "feature.interrupt" => self.feature_interrupt(params),
            "feature.auth.status" => self
                .feature
                .auth_status()
                .map_err(|error| AppHostError::Operation(error.to_string())),
            // Main-process only: this method is intentionally absent from the
            // renderer IPC allowlist. It never returns a refresh credential.
            "feature.auth.deviceAgentSession" => self
                .product
                .device_agent_session()
                .map_err(|error| AppHostError::Operation(error.to_string())),
            // Trusted native Host only. Generic platform.request deliberately redacts
            // bearer credentials before returning data to UI shells.
            "feature.miniapp.delegatedToken" => self.feature_miniapp_delegated_token(params),
            "feature.auth.providers" => self
                .feature
                .auth_providers()
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.auth.passwordLogin" => self.feature_password_login(params),
            "feature.auth.browserStart" => self
                .feature
                .browser_login_start()
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.auth.browserPoll" => self.feature_browser_login_poll(params),
            "feature.auth.browserCancel" => self.feature_browser_login_cancel(params),
            "feature.auth.browserReopen" => self.feature_browser_login_reopen(params),
            "feature.auth.oauthStart" => self.feature_oauth_start(params),
            "feature.auth.oauthPoll" => self.feature_oauth_poll(params),
            "feature.auth.logout" => self
                .feature
                .logout()
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.usage.status" => self
                .feature
                .usage_status()
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.publishPackage" => self
                .feature
                .export_workflow_publish_package(
                    string_param(&params, "agentId")?,
                    string_param(&params, "workflowId")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.pluginFactsSync" => self
                .feature
                .sync_workflow_plugin_facts(
                    string_param(&params, "agentId")?,
                    params.get("plugins").cloned().unwrap_or_else(|| json!([])),
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.pluginSkillsSync" => self
                .feature
                .sync_installed_plugin_skills(
                    string_param(&params, "agentId")?,
                    params.get("skills").cloned().unwrap_or_else(|| json!([])),
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.publishConfirm" => self
                .feature
                .confirm_workflow_publish(
                    string_param(&params, "agentId")?,
                    string_param(&params, "workflowId")?,
                    string_param(&params, "pluginId")?,
                    string_param(&params, "commitSha")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.resyncConfirm" => self
                .feature
                .confirm_published_workflow_resync(
                    string_param(&params, "agentId")?,
                    string_param(&params, "workflowId")?,
                    string_param(&params, "pluginId")?,
                    string_param(&params, "commitSha")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.resyncPackage" => self
                .feature
                .export_published_workflow_publish_package(
                    string_param(&params, "agentId")?,
                    string_param(&params, "workflowId")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.unpublishPrepare" => self
                .feature
                .prepare_workflow_unpublish(
                    string_param(&params, "agentId")?,
                    string_param(&params, "workflowId")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.workflow.unpublishComplete" => self
                .feature
                .complete_workflow_unpublish(
                    string_param(&params, "agentId")?,
                    string_param(&params, "pluginId")?,
                )
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.mcp.servers" => self
                .feature
                .mcp_servers_snapshot()
                .map(|servers| json!({"servers": servers}))
                .map_err(|error| AppHostError::Operation(error.to_string())),
            "feature.mcp.setToolDisabled" => {
                let server = string_param(&params, "server")?.to_string();
                let tool = string_param(&params, "tool")?.to_string();
                let disabled = params
                    .get("disabled")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| AppHostError::InvalidRequest("disabled is required".into()))?;
                self.feature
                    .set_mcp_tool_disabled_direct(server.clone(), tool, disabled)
                    .map(|disabled_tools| json!({
                        "server": server,
                        "disabledTools": disabled_tools,
                    }))
                    .map_err(|error| AppHostError::Operation(error.to_string()))
            }
            "feature.mcp.toolCall" => {
                let server = string_param(&params, "server")?.to_string();
                let tool = string_param(&params, "tool")?.to_string();
                let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                self.feature
                    .call_mcp_tool_direct(server, tool, arguments)
                    .map_err(|error| AppHostError::Operation(error.to_string()))
            }
            "feature.marketplace.browse" => self.marketplace_browse(params),
            "feature.marketplace.release" => self.marketplace_release(params),
            "feature.marketplace.added" => self.marketplace_added(params),
            "feature.marketplace.add" => self.marketplace_add(params),
            "feature.marketplace.remove" => self.marketplace_remove(params),
            "feature.plugin.accountIdentity" => self.plugin_account_identity(),
            "feature.plugin.install" => self.install_plugin(params),
            // Trusted native-shell only. WebMCP and Agent tool surfaces never receive
            // these methods, signing keys, or install authority.
            "feature.plugin.localDraft.begin" => self.begin_local_fabapp_draft(),
            "feature.plugin.localDraft.materializeArtifact" => {
                self.materialize_local_fabapp_artifact(params)
            }
            "feature.plugin.localDraft.prepare" => self.prepare_local_fabapp_draft(params),
            "feature.plugin.localDraft.cancel" => self.cancel_local_fabapp_draft(params),
            "feature.plugin.localDraft.approve" => self.approve_local_fabapp_draft(params),
            "feature.plugin.localDraft.signingPayload" => {
                self.local_fabapp_signing_payload(params)
            }
            "feature.plugin.installLocalFabapp" => self.install_local_fabapp(params),
            "feature.plugin.uninstall" => self.uninstall_plugin(params),
            "feature.plugin.rollback" => self.rollback_plugin(params),
            "feature.plugin.active" => self.active_plugin(params),
            "feature.plugin.listInstalled" => self.list_installed_plugins(params),
            "feature.plugin.uiDocument" => self.plugin_ui_document(params),
            "feature.plugin.uiResource" => self.plugin_ui_resource(params),
            "feature.messaging.execute" => self.feature_messaging_execute(params),
            "feature.messaging.blob.read" => self.feature_messaging_blob_read(params),
            "feature.attachment.uploadDirect" => self.feature_attachment_upload_direct(params),
            "feature.attachment.readTextDirect" => self.feature_attachment_read_text_direct(params),
            "feature.attachment.readChunkDirect" => self.feature_attachment_read_chunk_direct(params),
            "feature.attachment.readImageDirect" => self.feature_attachment_read_image_direct(params),
            "feature.messaging.access.issue" => self.feature_messaging_access_issue(params),
            other => Err(AppHostError::InvalidRequest(format!(
                "unknown feature method {other}"
            ))),
        }
    }

    fn feature_automation_ingest_listener_event(&self, params: Value) -> Result<Value, AppHostError> {
        let event_value = params.get("event").cloned().unwrap_or(params);
        let event: EventCard = serde_json::from_value(event_value)
            .map_err(|error| AppHostError::InvalidRequest(format!("invalid listener event: {error}")))?;
        self.feature
            .ingest_listener_event(event)
            .map(|matched| json!({"matchedAutomationCount": matched}))
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_relay_subscription(&self) -> Result<Value, AppHostError> {
        let automations = self.feature.list_all_automations()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let mut slack_channels = BTreeSet::new();
        let mut github_repos = BTreeSet::new();
        let mut github_kinds = BTreeSet::new();
        for automation in automations.into_iter().filter(|automation| automation.enabled) {
            if let Some(trigger) = automation.trigger.as_ref() {
                collect_automation_relay_subscription(
                    trigger,
                    &mut slack_channels,
                    &mut github_repos,
                    &mut github_kinds,
                );
            }
        }
        Ok(json!({
            "slackChannels": slack_channels.into_iter().collect::<Vec<_>>(),
            "githubRepos": github_repos.into_iter().collect::<Vec<_>>(),
            "githubKinds": github_kinds.into_iter().collect::<Vec<_>>(),
        }))
    }

    fn feature_automation_poll_listener_reconnects(&self) -> Result<Value, AppHostError> {
        self.feature
            .poll_listener_reconnect_watcher()
            .map(|()| json!({"ok": true}))
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_backend_fire_poll_state(&self) -> Result<Value, AppHostError> {
        self.feature
            .backend_automation_fire_poll_state()
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_cloud_sync_snapshot(&self) -> Result<Value, AppHostError> {
        self.feature
            .automation_cloud_sync_snapshot()
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_cloud_sync_plan(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .automation_cloud_sync_plan(
                string_param(&params, "agentId")?,
                string_param(&params, "responseBase64")?,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_cloud_sync_commit(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .automation_cloud_sync_commit(
                string_param(&params, "agentId")?,
                string_param(&params, "responseBase64")?,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_cloud_sync_mark_failed(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .automation_cloud_sync_mark_failed(string_param(&params, "agentId")?)
            .map(|()| json!({"ok": true}))
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_agent_store_list(&self, params: Value) -> Result<Value, AppHostError> {
        let prefix = match params.get("prefix") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if value.is_empty() => None,
            Some(Value::String(value)) => Some(value.as_str()),
            Some(_) => {
                return Err(AppHostError::InvalidRequest(
                    "prefix must be a string when provided".into(),
                ));
            }
        };
        self.feature
            .agent_store_list(string_param(&params, "agentId")?, prefix)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_agent_store_read(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .agent_store_read(
                string_param(&params, "agentId")?,
                agent_store_path_param(&params, "path")?,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_agent_store_write(&self, params: Value) -> Result<Value, AppHostError> {
        let data_base64 = params
            .get("dataBase64")
            .and_then(Value::as_str)
            .ok_or_else(|| AppHostError::InvalidRequest("dataBase64 is required".into()))?;
        let base_etag = params
            .get("baseEtag")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let expect_absent = params
            .get("expectAbsent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.feature
            .agent_store_write(
                string_param(&params, "agentId")?,
                agent_store_path_param(&params, "path")?,
                data_base64,
                base_etag,
                expect_absent,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_agent_store_delete(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .agent_store_delete(
                string_param(&params, "agentId")?,
                agent_store_path_param(&params, "path")?,
                string_param(&params, "baseEtag")?,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_agent_store_manifest_read(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .agent_store_manifest_read(string_param(&params, "agentId")?)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_agent_store_manifest_write(&self, params: Value) -> Result<Value, AppHostError> {
        let entries = params
            .get("entries")
            .ok_or_else(|| AppHostError::InvalidRequest("entries is required".into()))?;
        let writer_window_id = params
            .get("writerWindowId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let fully_hydrated = match params.get("fullyHydrated") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(value)) => Some(*value),
            Some(_) => {
                return Err(AppHostError::InvalidRequest(
                    "fullyHydrated must be boolean or null".into(),
                ));
            }
        };
        let base_etag = params
            .get("baseEtag")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let expect_absent = params
            .get("expectAbsent")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.feature
            .agent_store_manifest_write(
                string_param(&params, "agentId")?,
                entries,
                writer_window_id,
                fully_hydrated,
                base_etag,
                expect_absent,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_admit_backend_fire(&self, params: Value) -> Result<Value, AppHostError> {
        let fire = params.get("fire").unwrap_or(&params);
        self.feature
            .admit_backend_automation_fire(fire)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_automation_backend_fire_status(&self, params: Value) -> Result<Value, AppHostError> {
        let run_uuid = params
            .get("runUuid")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AppHostError::InvalidRequest("runUuid is required".into()))?;
        self.feature
            .backend_automation_fire_status(run_uuid)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_execute(&self, params: Value) -> Result<Value, AppHostError> {
        let await_turn = params.get("awaitTurn").and_then(Value::as_bool) == Some(true);
        let command_value = params
            .get("command")
            .cloned()
            .unwrap_or_else(|| params.clone());
        let command: FeatureCommand = serde_json::from_value(command_value).map_err(|error| {
            AppHostError::InvalidRequest(format!("invalid feature command: {error}"))
        })?;
        let accepted = self
            .feature
            .execute(command)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if await_turn {
            if let Some(operation_id) = accepted.operation_id.as_deref() {
                self.feature
                    .register_awaited_operation(operation_id)
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
            }
        }
        serde_json::to_value(accepted).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_await_operation(&self, params: Value) -> Result<Value, AppHostError> {
        let operation_id = string_param(&params, "operationId")?;
        let timeout_ms = params
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(250)
            .min(1_000);
        self.feature
            .await_operation_step(operation_id, Duration::from_millis(timeout_ms))
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_cancel_await_operation(&self, params: Value) -> Result<Value, AppHostError> {
        let operation_id = string_param(&params, "operationId")?;
        self.feature
            .cancel_awaited_operation(operation_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(Value::Null)
    }

    fn feature_session_activity(&self, params: Value) -> Result<Value, AppHostError> {
        let active = params
            .get("active")
            .and_then(Value::as_bool)
            .ok_or_else(|| AppHostError::InvalidRequest("active is required".into()))?;
        self.feature
            .set_scene_active(active)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_receive(&self, params: Value) -> Result<Value, AppHostError> {
        let timeout_ms = params
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(30_000);
        let event = self
            .feature
            .receive_with_timeout(Duration::from_millis(timeout_ms))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(event).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_resolve_approval(&self, params: Value) -> Result<Value, AppHostError> {
        let resolution_value = params.get("resolution").cloned().unwrap_or(params);
        let resolution: ApprovalResolution =
            serde_json::from_value(resolution_value).map_err(|error| {
                AppHostError::InvalidRequest(format!("invalid approval resolution: {error}"))
            })?;
        self.feature
            .resolve_approval(resolution)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(Value::Null)
    }

    fn feature_interrupt(&self, params: Value) -> Result<Value, AppHostError> {
        let operation_id = string_param(&params, "operationId")?;
        self.feature
            .interrupt(operation_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(Value::Null)
    }

    fn feature_attachment_upload_direct(&self, params: Value) -> Result<Value, AppHostError> {
        let agent_id = string_param(&params, "agentId")?;
        let filename = string_param(&params, "filename")?;
        let bytes_base64 = string_param(&params, "bytesBase64")?;
        let mime_type = params
            .get("mimeType")
            .and_then(Value::as_str)
            .map(str::to_string);
        let attachment = self
            .feature
            .attachment_upload_direct(agent_id, filename, mime_type, bytes_base64)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(attachment).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_attachment_read_text_direct(&self, params: Value) -> Result<Value, AppHostError> {
        let result = self
            .feature
            .attachment_read_text_direct(
                string_param(&params, "agentId")?,
                string_param(&params, "path")?,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(result).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_attachment_read_chunk_direct(&self, params: Value) -> Result<Value, AppHostError> {
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let length = params
            .get("length")
            .and_then(Value::as_u64)
            .unwrap_or(4 * 1024 * 1024)
            .clamp(1, 8 * 1024 * 1024);
        let result = self
            .feature
            .attachment_read_chunk_direct(
                string_param(&params, "agentId")?,
                string_param(&params, "path")?,
                offset,
                length,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(result).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_attachment_read_image_direct(&self, params: Value) -> Result<Value, AppHostError> {
        let result = self
            .feature
            .attachment_read_image_direct(
                string_param(&params, "agentId")?,
                string_param(&params, "path")?,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(result).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_messaging_execute(&self, params: Value) -> Result<Value, AppHostError> {
        let request_id = string_param(&params, "requestId")?.to_string();
        let envelope = params
            .get("envelope")
            .cloned()
            .ok_or_else(|| AppHostError::InvalidRequest("envelope is required".into()))?;
        let envelopes = self
            .feature
            .execute_messaging_sync(request_id, envelope)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(json!({"envelopes": envelopes}))
    }

    fn feature_messaging_blob_read(&self, params: Value) -> Result<Value, AppHostError> {
        let blob_id = string_param(&params, "blobId")?;
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let length = params
            .get("length")
            .and_then(Value::as_u64)
            .unwrap_or(1024 * 1024)
            .clamp(1, 1024 * 1024);
        let (metadata, bytes) = self
            .feature
            .read_messaging_blob_range(blob_id, offset, length)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(json!({
            "metadata": metadata,
            "offset": offset,
            "dataBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }))
    }

    fn feature_messaging_access_issue(&self, params: Value) -> Result<Value, AppHostError> {
        let device_id = string_param(&params, "deviceId")?.to_string();
        let session_id = string_param(&params, "sessionId")?.to_string();
        let scopes = params
            .get("scopes")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        item.as_str().map(str::to_string).ok_or_else(|| {
                            AppHostError::InvalidRequest("scopes must contain strings".into())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let ttl_ms = params
            .get("ttlMs")
            .and_then(Value::as_i64)
            .unwrap_or(24 * 60 * 60 * 1000);
        self.feature
            .issue_messaging_access(device_id, session_id, scopes, ttl_ms)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_password_login(&self, params: Value) -> Result<Value, AppHostError> {
        let username = string_param(&params, "username")?.to_string();
        let password = string_param(&params, "password")?.to_string();
        self.feature
            .password_login(username, password)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_browser_login_reopen(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .browser_login_reopen(string_param(&params, "attemptId")?.to_string())
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_browser_login_cancel(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .browser_login_cancel(string_param(&params, "attemptId")?.to_string())
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_browser_login_poll(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .browser_login_poll(string_param(&params, "attemptId")?.to_string())
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_miniapp_delegated_token(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let device_id = format!("fabushi-{}-miniapp-host", host_platform());
        self.product
            .miniapp_delegated_token(plugin_id, &device_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_oauth_start(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .oauth_start(string_param(&params, "provider")?.to_string())
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn feature_oauth_poll(&self, params: Value) -> Result<Value, AppHostError> {
        self.feature
            .oauth_poll(string_param(&params, "attemptId")?.to_string())
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn marketplace_browse(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let query = params.get("query").and_then(Value::as_str);
        if self.feature_mode == AppHostFeatureMode::Test {
            let term = query.unwrap_or_default().trim().to_lowercase();
            let plugins = TEST_MARKETPLACE_PLUGINS
                .iter()
                .filter_map(|(plugin_id, display_name, description)| {
                    let searchable =
                        format!("{plugin_id} {display_name} {description}").to_lowercase();
                    if !term.is_empty() && !searchable.contains(&term) {
                        return None;
                    }
                    Some(json!({
                        "pluginId": plugin_id,
                        "displayName": display_name,
                        "description": description,
                        "latestVersion": "1.0.0",
                        "platforms": ["desktop"],
                        "releaseStatus": "approved"
                    }))
                })
                .collect::<Vec<_>>();
            return Ok(json!({"plugins": plugins}));
        }
        let requested_platform = params
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or(host_platform());
        let result = self.product
            .marketplace_browse(query, Some(requested_platform))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(result)
    }

    fn marketplace_release(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let version = string_param(&params, "version")?;
        if self.feature_mode == AppHostFeatureMode::Test {
            if !TEST_MARKETPLACE_PLUGINS
                .iter()
                .any(|(candidate, _, _)| *candidate == plugin_id)
            {
                return Err(AppHostError::Operation(format!(
                    "test marketplace plugin {plugin_id} was not found"
                )));
            }
            let artifact_id = format!("{plugin_id}-test-ui");
            let artifact_url = format!(
                "https://raw.githubusercontent.com/bhrumom/fabushi/{TEST_MARKETPLACE_SOURCE_REF}/marketplace/packages/{plugin_id}/1.0.0/app.tar.gz"
            );
            let artifact = json!({
                "id": artifact_id,
                "runtime": "local-web",
                "platforms": ["desktop"],
                "source": {"type": "https", "url": artifact_url},
                "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                "size": 1,
                "format": "tar-gz"
            });
            let install = json!({
                "protocol": "fabushi.marketplace.install.v1",
                "strategy": "github-immutable",
                "pluginId": plugin_id,
                "version": version,
                "source": {
                    "repository": TEST_MARKETPLACE_REPOSITORY,
                    "sourceRef": TEST_MARKETPLACE_SOURCE_REF,
                    "marketplaceHostsPackage": false
                },
                "artifacts": [artifact.clone()],
                "update": {
                    "check": "marketplace-release",
                    "comparison": "version-then-artifact-sha256",
                    "allowDowngrade": false,
                    "rollback": "previous-active"
                }
            });
            return Ok(json!({
                "pluginId": plugin_id,
                "version": version,
                "releaseStatus": "approved",
                "releaseManifest": {
                    "schemaVersion": 1,
                    "protocol": "mahayana.external-release.v1",
                    "pluginId": plugin_id,
                    "version": version,
                    "source": {
                        "repository": TEST_MARKETPLACE_REPOSITORY,
                        "sourceRef": TEST_MARKETPLACE_SOURCE_REF
                    },
                    "permissions": [],
                    "artifacts": [artifact],
                    "install": install.clone()
                },
                "install": install
            }));
        }
        let requested_platform = params
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or(host_platform());
        let result = self.product
            .marketplace_release_metadata(plugin_id, version, Some(requested_platform))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(result)
    }

    fn marketplace_added(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        if self.feature_mode == AppHostFeatureMode::Test {
            let plugins = self
                .list_installed_plugins(params.clone())?
                .get("plugins")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|pointer| {
                    let plugin_id = pointer
                        .get("pluginId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    json!({"pluginId": plugin_id, "id": plugin_id})
                })
                .collect::<Vec<_>>();
            return Ok(json!({"apps": plugins}));
        }
        let result = self.product
            .marketplace_added()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(result)
    }

    fn marketplace_add(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let requested_platform = params
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or(host_platform());
        let marketplace_platform = match requested_platform {
            "ios" | "android" => "mobile",
            other => other,
        };
        if self.feature_mode == AppHostFeatureMode::Test {
            if !TEST_MARKETPLACE_PLUGINS
                .iter()
                .any(|(candidate, _, _)| *candidate == plugin_id)
            {
                return Err(AppHostError::Operation(format!(
                    "test marketplace plugin {plugin_id} was not found"
                )));
            }
            return Ok(json!({
                "added": true,
                "accountSynchronized": true,
                "bot": {
                    "id": format!("{plugin_id}-bot"),
                    "displayName": plugin_id,
                },
            }));
        }
        let result = self.product
            .marketplace_add(plugin_id, marketplace_platform)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(result)
    }

    fn marketplace_remove(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        if self.feature_mode == AppHostFeatureMode::Test {
            if !TEST_MARKETPLACE_PLUGINS
                .iter()
                .any(|(candidate, _, _)| *candidate == plugin_id)
            {
                return Err(AppHostError::Operation(format!(
                    "test marketplace plugin {plugin_id} was not found"
                )));
            }
            return Ok(json!({
                "removed": true,
                "miniAppId": plugin_id,
                "accountSynchronized": true,
            }));
        }
        let result = self.product
            .marketplace_remove(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(result)
    }

    fn current_plugin_account_identity(&self) -> Result<(String, String), AppHostError> {
        let session = self
            .product
            .device_agent_session()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let user_id = session
            .get("userId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppHostError::Operation(
                    "plugin storage requires an authenticated account with userId".into(),
                )
            })?;
        let session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppHostError::Operation(
                    "plugin storage requires an authenticated account generation".into(),
                )
            })?;
        Ok((
            stable_plugin_account_scope(user_id),
            stable_plugin_account_generation(user_id, session_id),
        ))
    }

    fn current_plugin_account_scope(&self) -> Result<String, AppHostError> {
        self.current_plugin_account_identity().map(|(scope, _)| scope)
    }

    fn ensure_plugin_account_current(
        &self,
        expected_scope: &str,
        expected_generation: &str,
    ) -> Result<(), AppHostError> {
        let (scope, generation) = self.current_plugin_account_identity()?;
        if !plugin_account_identity_matches(
            expected_scope,
            expected_generation,
            &scope,
            &generation,
        ) {
            return Err(AppHostError::Operation(
                "account generation changed while plugin operation was in flight".into(),
            ));
        }
        Ok(())
    }

    fn plugin_account_identity(&self) -> Result<Value, AppHostError> {
        let (account_scope, account_generation) = self.current_plugin_account_identity()?;
        Ok(json!({
            "protocol": "fabushi.plugin-account-identity.v1",
            "accountScope": account_scope,
            "accountGeneration": account_generation,
        }))
    }

    fn expected_plugin_account_identity(
        &self,
        params: &Value,
    ) -> Result<Option<(String, String)>, AppHostError> {
        let expected = parse_expected_plugin_account_identity(params)?;
        if let Some((scope, generation)) = expected.as_ref() {
            self.ensure_plugin_account_current(scope, generation)?;
        }
        Ok(expected)
    }

    fn ensure_expected_plugin_account_identity_current(
        &self,
        expected: Option<&(String, String)>,
    ) -> Result<(), AppHostError> {
        if let Some((scope, generation)) = expected {
            self.ensure_plugin_account_current(scope, generation)?;
        }
        Ok(())
    }

    fn ensure_plugin_account_context_matches_expected(
        &self,
        account: &PluginAccountContext,
        expected: Option<&(String, String)>,
    ) -> Result<(), AppHostError> {
        if let Some((scope, generation)) = expected
            && !plugin_account_identity_matches(
                scope,
                generation,
                &account.scope,
                &account.generation,
            )
        {
            return Err(AppHostError::Operation(
                "captured plugin account does not match expected account generation".into(),
            ));
        }
        Ok(())
    }

    fn reconcile_plugin_runtime_accounts(&self) -> Result<(), AppHostError> {
        let mut bindings = self
            .runtime_accounts
            .lock()
            .map_err(|_| AppHostError::Operation("plugin runtime account lock poisoned".into()))?;
        if bindings.is_empty() {
            return Ok(());
        }
        let current = self.current_plugin_account_identity().ok();
        let stale = stale_plugin_runtime_ids(&bindings, current.as_ref());
        if stale.is_empty() {
            return Ok(());
        }
        let mut host = self
            .js
            .lock()
            .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?;
        for plugin_id in &stale {
            let _ = host.disable_plugin(plugin_id);
        }
        for plugin_id in stale {
            bindings.remove(&plugin_id);
        }
        Ok(())
    }

    fn disable_plugin_runtime(
        &self,
        plugin_id: &str,
        require_active: bool,
    ) -> Result<(), AppHostError> {
        let mut bindings = self
            .runtime_accounts
            .lock()
            .map_err(|_| AppHostError::Operation("plugin runtime account lock poisoned".into()))?;
        let mut host = self
            .js
            .lock()
            .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?;
        let disabled = host.disable_plugin(plugin_id);
        bindings.remove(plugin_id);
        match disabled {
            Ok(_) => Ok(()),
            Err(_) if !require_active => Ok(()),
            Err(error) => Err(AppHostError::Operation(error.to_string())),
        }
    }

    fn plugin_account_context(&self) -> Result<PluginAccountContext, AppHostError> {
        let (scope, generation) = self.current_plugin_account_identity()?;
        let base = self.app_data_dir.join("plugins");
        let root = base.join("accounts").join(&scope);
        std::fs::create_dir_all(&root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        ensure_plugin_account_marker(&root, &scope)?;
        migrate_legacy_plugin_root_if_proven(&base, &scope, &root)?;
        Ok(PluginAccountContext {
            scope,
            generation,
            root,
        })
    }

    fn plugin_root(&self) -> Result<PathBuf, AppHostError> {
        Ok(self.plugin_account_context()?.root)
    }

    fn permission_store(&self) -> Result<PathBuf, AppHostError> {
        Ok(self.plugin_root()?.join("permissions.json"))
    }

    fn plugin_git_auth_blocks_path(&self) -> Result<PathBuf, AppHostError> {
        Ok(self.plugin_root()?.join("git-auth-blocked.json"))
    }

    fn plugin_sync_status(&self) -> Result<Value, AppHostError> {
        let path = self.plugin_git_auth_blocks_path()?;
        let blocks = load_plugin_git_auth_blocks(&path);
        for block in blocks {
            let _ = self.install_plugin(json!({
                "release": block.release,
                "platform": block.platform,
            }));
        }
        let blocks = load_plugin_git_auth_blocks(&path);
        Ok(json!({
            "authBlocked": blocks.into_iter().map(|block| json!({
                "pluginId": block.plugin_id,
                "pluginName": block.plugin_name,
                "marketplaceName": block.marketplace_name,
            })).collect::<Vec<_>>()
        }))
    }

    fn record_plugin_git_auth_block(
        &self,
        release: &ExternalReleaseManifest,
        release_value: &Value,
        platform: &str,
    ) -> Result<(), AppHostError> {
        let path = self.plugin_git_auth_blocks_path()?;
        let mut blocks = load_plugin_git_auth_blocks(&path);
        blocks.retain(|block| block.plugin_id != release.plugin_id);
        blocks.push(PluginGitAuthBlock {
            plugin_id: release.plugin_id.clone(),
            plugin_name: release.plugin_id.clone(),
            marketplace_name: None,
            release: release_value.clone(),
            platform: platform.to_string(),
        });
        persist_plugin_git_auth_blocks(&path, &blocks)
    }

    fn clear_plugin_git_auth_block(&self, plugin_id: &str) -> Result<(), AppHostError> {
        let path = self.plugin_git_auth_blocks_path()?;
        let mut blocks = load_plugin_git_auth_blocks(&path);
        let before = blocks.len();
        blocks.retain(|block| block.plugin_id != plugin_id);
        if blocks.len() != before {
            persist_plugin_git_auth_blocks(&path, &blocks)?;
        }
        Ok(())
    }

    fn installer(&self) -> Result<PluginInstaller, AppHostError> {
        PluginInstaller::new(self.plugin_root()?)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn install_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let release_value = params
            .get("release")
            .cloned()
            .ok_or_else(|| AppHostError::InvalidRequest("release is required".into()))?;
        let release: ExternalReleaseManifest = serde_json::from_value(release_value.clone())
            .map_err(|error| AppHostError::InvalidRequest(error.to_string()))?;
        let platform = params
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or(host_platform());
        if self.feature_mode == AppHostFeatureMode::Test {
            if !TEST_MARKETPLACE_PLUGINS
                .iter()
                .any(|(candidate, _, _)| *candidate == release.plugin_id)
            {
                return Err(AppHostError::Operation(format!(
                    "test marketplace plugin {} was not found",
                    release.plugin_id
                )));
            }
            let plugin_root = self.plugin_root()?.join(&release.plugin_id);
            let installed_dir = plugin_root
                .join("versions")
                .join(&release.version)
                .join("test-ui");
            std::fs::create_dir_all(&installed_dir)
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
            let display_name = TEST_MARKETPLACE_PLUGINS
                .iter()
                .find(|(candidate, _, _)| *candidate == release.plugin_id)
                .map(|(_, display_name, _)| *display_name)
                .unwrap_or(release.plugin_id.as_str());
            let html = format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><title>{display_name}</title></head><body><main><h1>{display_name}</h1><p>Installed from the deterministic Mahayana Marketplace test backend.</p></main></body></html>"
            );
            std::fs::write(installed_dir.join("index.html"), html)
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
            let pointer = InstalledPluginPointer {
                plugin_id: release.plugin_id.clone(),
                version: release.version.clone(),
                artifact_id: "test-ui".to_string(),
                artifact_sha256: "0".repeat(64),
                runtime: "local-web".to_string(),
                entry: Some("index.html".to_string()),
                requested_permissions: release.permissions.clone(),
                installed_path: installed_dir.to_string_lossy().into_owned(),
            };
            std::fs::create_dir_all(&plugin_root)
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
            let active = serde_json::to_vec_pretty(&pointer)
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
            std::fs::write(plugin_root.join("active.json"), active)
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
            return serde_json::to_value(pointer)
                .map_err(|error| AppHostError::Operation(error.to_string()));
        }
        let preferred: &[&str] = match platform {
            "ios" | "android" | "mobile" => &[
                "deepseek-js",
                "javascript",
                "cordis-js",
                "web-wasm",
                "userscript",
                "mcp",
                "local-web",
            ],
            _ => &[
                "deepseek-js",
                "javascript",
                "cordis-js",
                "local-web",
                "web-wasm",
                "native",
                "desktop-stdio",
                "mcp",
            ],
        };
        let account = self.plugin_account_context()?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        let installer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        let pointer_result = installer
            .install_with_commit_guard(&release, platform, preferred, || {
                self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                    scope == account.scope && generation == account.generation
                })
            })
            .or_else(|error| {
                if matches!(platform, "ios" | "android") {
                    installer.install_with_commit_guard(&release, "mobile", preferred, || {
                        self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                            scope == account.scope && generation == account.generation
                        })
                    })
                } else {
                    Err(error)
                }
            });
        let pointer = match pointer_result {
            Ok(pointer) => {
                self.clear_plugin_git_auth_block(&release.plugin_id)?;
                pointer
            }
            Err(error) => {
                let message = error.to_string();
                if is_git_authentication_failure(&message) {
                    self.record_plugin_git_auth_block(
                        &release,
                        &release_value,
                        platform,
                    )?;
                }
                return Err(AppHostError::Operation(message));
            }
        };
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        serde_json::to_value(pointer).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn materialize_local_fabapp_artifact(&self, params: Value) -> Result<Value, AppHostError> {
        const LOCAL_GENERATED_ARTIFACT_MAX_BYTES: u64 = 4 * 1024 * 1024;

        let agent_id = string_param(&params, "agentId")?.trim();
        let source_path = validate_local_generated_artifact_source_path(
            string_param(&params, "path")?,
        )?;
        let display_name = string_param(&params, "displayName")?.trim();
        if display_name.is_empty() {
            return Err(AppHostError::InvalidRequest(
                "displayName must not be empty".into(),
            ));
        }

        let account = self.plugin_account_context()?;
        let raw = self
            .feature
            .read_agent_attachment_for_local_miniapp(
                agent_id,
                source_path,
                LOCAL_GENERATED_ARTIFACT_MAX_BYTES,
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        let raw_html = String::from_utf8(raw).map_err(|_| {
            AppHostError::InvalidRequest("Agent Mini App artifact must be UTF-8 HTML".into())
        })?;
        let hardened_html = harden_generated_miniapp_document(&raw_html);
        let hardened_bytes = hardened_html.as_bytes();
        if hardened_bytes.len() as u64 > LOCAL_GENERATED_ARTIFACT_MAX_BYTES {
            return Err(AppHostError::InvalidRequest(format!(
                "hardened Agent Mini App artifact exceeds {LOCAL_GENERATED_ARTIFACT_MAX_BYTES} bytes"
            )));
        }
        let source_sha256 = format!("{:x}", Sha256::digest(raw_html.as_bytes()));
        let preview_sha256 = format!("{:x}", Sha256::digest(hardened_bytes));

        let begin = self.begin_local_fabapp_draft()?;
        if begin.get("accountScope").and_then(Value::as_str) != Some(account.scope.as_str()) {
            return Err(AppHostError::Operation(
                "account changed while beginning Agent Mini App draft".into(),
            ));
        }
        let transaction_id = begin
            .get("transactionId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppHostError::Operation("local Mini App draft transaction missing".into()))?
            .to_string();
        let reserved_app_id = begin
            .get("reservedAppId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppHostError::Operation("local Mini App reserved app id missing".into()))?
            .to_string();
        let publisher_id = begin
            .get("publisherId")
            .and_then(Value::as_str)
            .ok_or_else(|| AppHostError::Operation("local Mini App publisher id missing".into()))?
            .to_string();

        let prepared = match self.prepare_local_fabapp_draft(json!({
            "transactionId": transaction_id,
            "version": "1.0.0",
            "displayName": display_name,
            "entrypoint": "index.html",
            "requestedCapabilities": [],
            "files": {
                "index.html": {
                    "mime": "text/html",
                    "dataBase64": base64::engine::general_purpose::STANDARD.encode(hardened_bytes),
                }
            }
        })) {
            Ok(value) => value,
            Err(error) => {
                let _ = self.cancel_local_fabapp_draft(json!({
                    "transactionId": transaction_id,
                }));
                return Err(error);
            }
        };
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        Ok(json!({
            "protocol": "fabushi.local-miniapp-artifact-draft.v1",
            "accountScope": account.scope,
            "transactionId": transaction_id,
            "reservedAppId": reserved_app_id,
            "publisherId": publisher_id,
            "sourcePath": source_path,
            "sourceSha256": source_sha256,
            "previewSha256": preview_sha256,
            "previewHtmlBase64": base64::engine::general_purpose::STANDARD.encode(hardened_bytes),
            "review": prepared,
        }))
    }

    fn cancel_local_fabapp_draft(&self, params: Value) -> Result<Value, AppHostError> {
        let transaction_id =
            strict_hex_identifier(string_param(&params, "transactionId")?, 32, "transactionId")?;
        let account = self.plugin_account_context()?;
        let transaction_path = account
            .root
            .join(".local-drafts")
            .join(format!("{transaction_id}.json"));
        let mut transaction: LocalFabappDraftTransaction = serde_json::from_slice(
            &std::fs::read(&transaction_path).map_err(|_| {
                AppHostError::Operation("local Mini App draft transaction was not found".into())
            })?,
        )
        .map_err(|error| {
            AppHostError::Operation(format!("invalid local draft transaction: {error}"))
        })?;
        if transaction.schema_version != 1
            || transaction.account_scope != account.scope
            || transaction.account_generation != account.generation
        {
            return Err(AppHostError::Operation(
                "local Mini App draft transaction is stale".into(),
            ));
        }
        if transaction.state == "installed" {
            return Err(AppHostError::Operation(
                "installed local Mini App transaction cannot be cancelled".into(),
            ));
        }
        if transaction.state == "cancelled" {
            return Ok(json!({
                "protocol": "fabushi.local-miniapp-draft-cancelled.v1",
                "accountScope": account.scope,
                "transactionId": transaction_id,
                "cancelled": true,
            }));
        }
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;

        if let Some(approval_id) = transaction.approval_id.as_deref() {
            let approval_path = account
                .root
                .join(".local-authorizations")
                .join(format!("{approval_id}.json"));
            if let Ok(bytes) = std::fs::read(&approval_path)
                && let Ok(mut approval) = serde_json::from_slice::<LocalFabappHostApproval>(&bytes)
                && approval.schema_version == 1
                && approval.account_scope == account.scope
                && approval.account_generation == account.generation
                && approval.transaction_id == transaction_id
                && approval.state == "approved"
            {
                approval.state = "cancelled".into();
                atomic_write_json(&approval_path, &approval)?;
            }
        }

        transaction.state = "cancelled".into();
        atomic_write_json(&transaction_path, &transaction)?;
        let prepared_path = account
            .root
            .join(".local-drafts")
            .join(format!("{transaction_id}.prepared.json"));
        let _ = std::fs::remove_file(prepared_path);
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        Ok(json!({
            "protocol": "fabushi.local-miniapp-draft-cancelled.v1",
            "accountScope": account.scope,
            "transactionId": transaction_id,
            "cancelled": true,
        }))
    }

    fn begin_local_fabapp_draft(&self) -> Result<Value, AppHostError> {
        let account = self.plugin_account_context()?;
        let transaction_id = uuid::Uuid::new_v4().simple().to_string();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let scope_prefix = &account.scope[..16];
        let publisher_id = format!("local.fabushi.a{scope_prefix}");
        let reserved_app_id = format!("{publisher_id}.a{suffix}");
        let transaction = LocalFabappDraftTransaction {
            schema_version: 1,
            account_scope: account.scope.clone(),
            account_generation: account.generation.clone(),
            transaction_id: transaction_id.clone(),
            reserved_app_id: reserved_app_id.clone(),
            publisher_id: publisher_id.clone(),
            state: "draft".into(),
            draft_digest: None,
            approval_id: None,
        };
        let path = account
            .root
            .join(".local-drafts")
            .join(format!("{transaction_id}.json"));
        atomic_write_json(&path, &transaction)?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        Ok(json!({
            "protocol": "fabushi.local-miniapp-draft.v1",
            "accountScope": account.scope,
            "transactionId": transaction_id,
            "reservedAppId": reserved_app_id,
            "publisherId": publisher_id,
        }))
    }

    fn prepare_local_fabapp_draft(&self, params: Value) -> Result<Value, AppHostError> {
        const LOCAL_DRAFT_MAX_BYTES: u64 = 32 * 1024 * 1024;

        let transaction_id =
            strict_hex_identifier(string_param(&params, "transactionId")?, 32, "transactionId")?;
        let version = string_param(&params, "version")?;
        let display_name = string_param(&params, "displayName")?;
        let entrypoint = string_param(&params, "entrypoint")?;
        let capabilities = params
            .get("requestedCapabilities")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                AppHostError::InvalidRequest("requestedCapabilities must be an array".into())
            })?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppHostError::InvalidRequest(
                            "requestedCapabilities must contain strings".into(),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let files = params
            .get("files")
            .and_then(Value::as_object)
            .ok_or_else(|| AppHostError::InvalidRequest("files must be an object".into()))?;
        if files.is_empty() {
            return Err(AppHostError::InvalidRequest(
                "local Mini App draft must contain files".into(),
            ));
        }

        let account = self.plugin_account_context()?;
        let transaction_path = account
            .root
            .join(".local-drafts")
            .join(format!("{transaction_id}.json"));
        let mut transaction: LocalFabappDraftTransaction = serde_json::from_slice(
            &std::fs::read(&transaction_path).map_err(|_| {
                AppHostError::Operation("local Mini App draft transaction was not found".into())
            })?,
        )
        .map_err(|error| {
            AppHostError::Operation(format!("invalid local draft transaction: {error}"))
        })?;
        if transaction.schema_version != 1
            || !matches!(transaction.state.as_str(), "draft" | "prepared")
            || transaction.approval_id.is_some()
            || transaction.account_scope != account.scope
            || transaction.account_generation != account.generation
        {
            return Err(AppHostError::Operation(
                "local Mini App draft transaction is stale or already approved".into(),
            ));
        }
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;

        let mut resources = BTreeMap::<String, (String, Vec<u8>)>::new();
        let mut persisted = BTreeMap::<String, LocalFabappPreparedResource>::new();
        let mut total_bytes = 0_u64;
        for (path, raw) in files {
            let object = raw.as_object().ok_or_else(|| {
                AppHostError::InvalidRequest(format!("file {path} must be an object"))
            })?;
            let mime = object
                .get("mime")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AppHostError::InvalidRequest(format!("file {path} is missing mime"))
                })?
                .to_string();
            let encoded = object
                .get("dataBase64")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    AppHostError::InvalidRequest(format!("file {path} is missing dataBase64"))
                })?;
            let data = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map_err(|_| {
                    AppHostError::InvalidRequest(format!("file {path} dataBase64 is invalid"))
                })?;
            total_bytes = total_bytes
                .checked_add(data.len() as u64)
                .ok_or_else(|| AppHostError::InvalidRequest("draft size overflow".into()))?;
            if total_bytes > LOCAL_DRAFT_MAX_BYTES {
                return Err(AppHostError::InvalidRequest(
                    "local Mini App draft exceeds 32 MiB".into(),
                ));
            }
            persisted.insert(
                path.clone(),
                LocalFabappPreparedResource {
                    mime: mime.clone(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(&data),
                },
            );
            resources.insert(path.clone(), (mime, data));
        }

        let manifest = prepare_local_generated_fabapp_manifest(
            &transaction.reserved_app_id,
            &transaction.publisher_id,
            version,
            display_name,
            entrypoint,
            capabilities,
            &resources,
        )
        .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let draft_digest = compute_fabapp_draft_digest(&manifest)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let prepared = LocalFabappPreparedDraft {
            schema_version: 1,
            account_scope: account.scope.clone(),
            account_generation: account.generation.clone(),
            transaction_id: transaction_id.clone(),
            draft_digest: draft_digest.clone(),
            manifest: manifest.clone(),
            resources: persisted,
            total_bytes,
        };
        let prepared_path = account
            .root
            .join(".local-drafts")
            .join(format!("{transaction_id}.prepared.json"));
        atomic_write_json(&prepared_path, &prepared)?;
        transaction.state = "prepared".into();
        transaction.draft_digest = Some(draft_digest.clone());
        atomic_write_json(&transaction_path, &transaction)?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;

        Ok(json!({
            "protocol": "fabushi.local-miniapp-draft-prepared.v1",
            "accountScope": account.scope,
            "transactionId": transaction_id,
            "reservedAppId": transaction.reserved_app_id,
            "publisherId": transaction.publisher_id,
            "draftDigest": draft_digest,
            "totalBytes": total_bytes,
            "requestedCapabilities": manifest.requested_capabilities,
            "files": manifest.files,
        }))
    }

    fn approve_local_fabapp_draft(&self, params: Value) -> Result<Value, AppHostError> {
        let transaction_id = strict_hex_identifier(string_param(&params, "transactionId")?, 32, "transactionId")?;
        let draft_digest = strict_lower_hex_digest(string_param(&params, "draftDigest")?, "draftDigest")?;
        let key_id = strict_ascii_identifier(string_param(&params, "keyId")?, "keyId")?;
        let public_key = decode_ed25519_public_key(string_param(&params, "publicKeyBase64")?)?;
        let account = self.plugin_account_context()?;
        let transaction_path = account
            .root
            .join(".local-drafts")
            .join(format!("{transaction_id}.json"));
        let mut transaction: LocalFabappDraftTransaction = serde_json::from_slice(
            &std::fs::read(&transaction_path)
                .map_err(|_| AppHostError::Operation("local Mini App draft transaction was not found".into()))?,
        )
        .map_err(|error| AppHostError::Operation(format!("invalid local draft transaction: {error}")))?;
        let prepared_path = account
            .root
            .join(".local-drafts")
            .join(format!("{transaction_id}.prepared.json"));
        let prepared: LocalFabappPreparedDraft = serde_json::from_slice(
            &std::fs::read(&prepared_path).map_err(|_| {
                AppHostError::Operation("local Mini App prepared draft was not found".into())
            })?,
        )
        .map_err(|error| {
            AppHostError::Operation(format!("invalid local prepared draft: {error}"))
        })?;
        if transaction.schema_version != 1
            || transaction.state != "prepared"
            || transaction.account_scope != account.scope
            || transaction.account_generation != account.generation
            || transaction.draft_digest.as_deref() != Some(draft_digest.as_str())
            || prepared.schema_version != 1
            || prepared.account_scope != account.scope
            || prepared.account_generation != account.generation
            || prepared.transaction_id != transaction_id
            || prepared.draft_digest != draft_digest
            || compute_fabapp_draft_digest(&prepared.manifest)
                .map_err(|error| AppHostError::Operation(error.to_string()))?
                != draft_digest
        {
            return Err(AppHostError::Operation(
                "local Mini App draft approval does not match the prepared bytes".into(),
            ));
        }
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;

        let user_approval_id = uuid::Uuid::new_v4().simple().to_string();
        let public_key_sha256 = format!("{:x}", Sha256::digest(public_key));
        let approval = LocalFabappHostApproval {
            schema_version: 1,
            account_scope: account.scope.clone(),
            account_generation: account.generation.clone(),
            transaction_id: transaction_id.clone(),
            reserved_app_id: transaction.reserved_app_id.clone(),
            publisher_id: transaction.publisher_id.clone(),
            draft_digest: draft_digest.clone(),
            user_approval_id: user_approval_id.clone(),
            key_id: key_id.clone(),
            public_key_sha256,
            state: "approved".into(),
        };
        let approval_path = account
            .root
            .join(".local-authorizations")
            .join(format!("{user_approval_id}.json"));
        atomic_write_json(&approval_path, &approval)?;
        transaction.state = "approved".into();
        transaction.approval_id = Some(user_approval_id.clone());
        atomic_write_json(&transaction_path, &transaction)?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;

        Ok(json!({
            "protocol": "fabushi.local-miniapp-approval.v1",
            "accountScope": account.scope,
            "transactionId": transaction_id,
            "reservedAppId": transaction.reserved_app_id,
            "publisherId": transaction.publisher_id,
            "draftDigest": draft_digest,
            "userApprovalId": user_approval_id,
            "keyId": key_id,
        }))
    }

    fn local_fabapp_signing_payload(&self, params: Value) -> Result<Value, AppHostError> {
        let approval_id = strict_hex_identifier(
            string_param(&params, "userApprovalId")?,
            32,
            "userApprovalId",
        )?;
        let account = self.plugin_account_context()?;
        let approval_path = account
            .root
            .join(".local-authorizations")
            .join(format!("{approval_id}.json"));
        let approval: LocalFabappHostApproval = serde_json::from_slice(
            &std::fs::read(&approval_path).map_err(|_| {
                AppHostError::Operation("local Mini App approval was not found".into())
            })?,
        )
        .map_err(|error| {
            AppHostError::Operation(format!("invalid local Mini App approval: {error}"))
        })?;
        if approval.schema_version != 1
            || approval.state != "approved"
            || approval.account_scope != account.scope
            || approval.account_generation != account.generation
            || approval.user_approval_id != approval_id
        {
            return Err(AppHostError::Operation(
                "local Mini App approval is stale or consumed".into(),
            ));
        }
        let prepared_path = account
            .root
            .join(".local-drafts")
            .join(format!("{}.prepared.json", approval.transaction_id));
        let prepared: LocalFabappPreparedDraft = serde_json::from_slice(
            &std::fs::read(&prepared_path).map_err(|_| {
                AppHostError::Operation("local Mini App prepared draft was not found".into())
            })?,
        )
        .map_err(|error| {
            AppHostError::Operation(format!("invalid local prepared draft: {error}"))
        })?;
        if prepared.draft_digest != approval.draft_digest
            || prepared.account_scope != account.scope
            || prepared.account_generation != account.generation
            || prepared.transaction_id != approval.transaction_id
        {
            return Err(AppHostError::Operation(
                "local Mini App prepared draft no longer matches approval".into(),
            ));
        }
        let mut manifest = prepared.manifest;
        manifest.local_author = Some(FabappLocalAuthorBinding {
            account_scope: account.scope.clone(),
            draft_digest: approval.draft_digest.clone(),
            user_approval_id: approval.user_approval_id.clone(),
            creation_transaction_id: approval.transaction_id.clone(),
        });
        manifest.signature = FabappSignature {
            algorithm: "Ed25519".into(),
            key_id: approval.key_id.clone(),
            value: String::new(),
        };
        let payload = fabapp_signing_payload(&manifest)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        Ok(json!({
            "protocol": "fabushi.local-miniapp-signing-payload.v1",
            "accountScope": account.scope,
            "transactionId": approval.transaction_id,
            "userApprovalId": approval.user_approval_id,
            "draftDigest": approval.draft_digest,
            "signingPayloadBase64": base64::engine::general_purpose::STANDARD.encode(payload),
        }))
    }

    fn install_local_fabapp(&self, params: Value) -> Result<Value, AppHostError> {
        let approval_id = strict_hex_identifier(
            string_param(&params, "userApprovalId")?,
            32,
            "userApprovalId",
        )?;
        let signature_value = string_param(&params, "signatureBase64URL")?.trim().to_string();
        let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature_value.as_bytes())
            .map_err(|_| AppHostError::InvalidRequest("signatureBase64URL is invalid".into()))?;
        if signature_bytes.len() != 64 {
            return Err(AppHostError::InvalidRequest(
                "Ed25519 signature must be 64 bytes".into(),
            ));
        }
        let key_id = strict_ascii_identifier(string_param(&params, "keyId")?, "keyId")?;
        let public_key = decode_ed25519_public_key(string_param(&params, "publicKeyBase64")?)?;
        let account = self.plugin_account_context()?;
        let approval_path = account
            .root
            .join(".local-authorizations")
            .join(format!("{approval_id}.json"));
        let mut approval: LocalFabappHostApproval = serde_json::from_slice(
            &std::fs::read(&approval_path).map_err(|_| {
                AppHostError::Operation("local Mini App approval was not found".into())
            })?,
        )
        .map_err(|error| {
            AppHostError::Operation(format!("invalid local Mini App approval: {error}"))
        })?;
        if approval.schema_version != 1
            || approval.state != "approved"
            || approval.account_scope != account.scope
            || approval.account_generation != account.generation
            || approval.user_approval_id != approval_id
            || approval.key_id != key_id
            || approval.public_key_sha256 != format!("{:x}", Sha256::digest(public_key))
        {
            return Err(AppHostError::Operation(
                "local Mini App approval is stale, consumed, or does not match the signing key"
                    .into(),
            ));
        }
        let prepared_path = account
            .root
            .join(".local-drafts")
            .join(format!("{}.prepared.json", approval.transaction_id));
        let prepared: LocalFabappPreparedDraft = serde_json::from_slice(
            &std::fs::read(&prepared_path).map_err(|_| {
                AppHostError::Operation("local Mini App prepared draft was not found".into())
            })?,
        )
        .map_err(|error| {
            AppHostError::Operation(format!("invalid local prepared draft: {error}"))
        })?;
        if prepared.schema_version != 1
            || prepared.account_scope != account.scope
            || prepared.account_generation != account.generation
            || prepared.transaction_id != approval.transaction_id
            || prepared.draft_digest != approval.draft_digest
            || compute_fabapp_draft_digest(&prepared.manifest)
                .map_err(|error| AppHostError::Operation(error.to_string()))?
                != approval.draft_digest
        {
            return Err(AppHostError::Operation(
                "local Mini App approved draft bytes changed before install".into(),
            ));
        }
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;

        let mut resources = BTreeMap::<String, (String, Vec<u8>)>::new();
        for (path, resource) in &prepared.resources {
            let data = base64::engine::general_purpose::STANDARD
                .decode(resource.data_base64.as_bytes())
                .map_err(|_| {
                    AppHostError::Operation(format!(
                        "prepared local Mini App resource {path} is corrupt"
                    ))
                })?;
            resources.insert(path.clone(), (resource.mime.clone(), data));
        }
        let mut manifest = prepared.manifest;
        manifest.local_author = Some(FabappLocalAuthorBinding {
            account_scope: account.scope.clone(),
            draft_digest: approval.draft_digest.clone(),
            user_approval_id: approval.user_approval_id.clone(),
            creation_transaction_id: approval.transaction_id.clone(),
        });
        manifest.signature = FabappSignature {
            algorithm: "Ed25519".into(),
            key_id: approval.key_id.clone(),
            value: signature_value,
        };
        let package = build_fabapp_package(&manifest, &resources)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let trust = FabappTrustContext::LocalGenerated(LocalGeneratedFabappTrust {
            account_scope: account.scope.clone(),
            publisher_id: approval.publisher_id.clone(),
            reserved_app_id: approval.reserved_app_id.clone(),
            approved_draft_digest: approval.draft_digest.clone(),
            user_approval_id: approval.user_approval_id.clone(),
            creation_transaction_id: approval.transaction_id.clone(),
            key_id: approval.key_id.clone(),
            public_key,
        });
        let transaction_path = account
            .root
            .join(".local-drafts")
            .join(format!("{}.json", approval.transaction_id));
        let installer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let pointer = installer
            .install_fabapp_bytes_with_commit_guard(&package, &trust, || {
                if !self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                    scope == account.scope && generation == account.generation
                }) {
                    return false;
                }
                let latest_approval = std::fs::read(&approval_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<LocalFabappHostApproval>(&bytes).ok());
                let latest_transaction = std::fs::read(&transaction_path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<LocalFabappDraftTransaction>(&bytes).ok());
                local_fabapp_commit_records_match(
                    &account,
                    &approval,
                    latest_approval.as_ref(),
                    latest_transaction.as_ref(),
                )
            })
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;

        approval.state = "consumed".into();
        atomic_write_json(&approval_path, &approval)?;
        if let Ok(bytes) = std::fs::read(&transaction_path) {
            if let Ok(mut transaction) =
                serde_json::from_slice::<LocalFabappDraftTransaction>(&bytes)
            {
                transaction.state = "installed".into();
                let _ = atomic_write_json(&transaction_path, &transaction);
            }
        }
        let _ = std::fs::remove_file(&prepared_path);
        Ok(json!({
            "protocol": "fabushi.local-miniapp-install.v1",
            "pointer": pointer,
            "accountScope": account.scope,
            "userApprovalId": approval_id,
        }))
    }

    fn uninstall_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let account = self.plugin_account_context()?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        let installer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.disable_plugin_runtime(plugin_id, false)?;
        let removed = installer
            .uninstall_with_commit_guard(plugin_id, || {
                self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                    scope == account.scope && generation == account.generation
                })
            })
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        let permissions_removed = PermissionManager::load(account.root.join("permissions.json"))
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .remove_plugin_with_commit_guard(plugin_id, || {
                self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                    scope == account.scope && generation == account.generation
                })
            })
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(json!({
            "pluginId": plugin_id,
            "removed": removed,
            "permissionsRemoved": permissions_removed
        }))
    }

    fn active_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let account = self.plugin_account_context()?;
        self.ensure_plugin_account_context_matches_expected(&account, expected_account.as_ref())?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        let pointer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        serde_json::to_value(pointer).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn rollback_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let account = self.plugin_account_context()?;
        let installer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let pointer = installer
            .rollback_with_commit_guard(plugin_id, || {
                self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                    scope == account.scope && generation == account.generation
                })
            })
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        serde_json::to_value(pointer).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn list_installed_plugins(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let account = self.plugin_account_context()?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        let root = account.root.clone();
        let installer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let mut plugins = Vec::new();
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.ensure_plugin_account_current(&account.scope, &account.generation)?;
                self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
                return Ok(json!({"plugins": plugins}));
            }
            Err(error) => return Err(AppHostError::Operation(error.to_string())),
        };
        for entry in entries {
            let entry = entry.map_err(|error| AppHostError::Operation(error.to_string()))?;
            if !entry
                .file_type()
                .map_err(|error| AppHostError::Operation(error.to_string()))?
                .is_dir()
            {
                continue;
            }
            let plugin_id = entry.file_name().to_string_lossy().into_owned();
            if let Some(pointer) = installer
                .active(&plugin_id)
                .map_err(|error| AppHostError::Operation(error.to_string()))?
            {
                plugins.push(pointer);
            }
        }
        plugins.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(json!({"plugins": plugins}))
    }

    fn plugin_permissions(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let account = self.plugin_account_context()?;
        self.ensure_plugin_account_context_matches_expected(&account, expected_account.as_ref())?;
        let pointer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        let mut manager = PermissionManager::load(account.root.join("permissions.json"))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        manager
            .retain_requested_with_commit_guard(
                plugin_id,
                &pointer.requested_permissions,
                || {
                    self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                        scope == account.scope && generation == account.generation
                    })
                },
            )
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        let granted = manager.grants_for(plugin_id);
        let missing = pointer
            .requested_permissions
            .iter()
            .filter(|permission| !granted.contains(permission))
            .cloned()
            .collect::<Vec<_>>();
        Ok(
            json!({"requested": pointer.requested_permissions, "granted": granted, "missing": missing}),
        )
    }

    fn set_permission(&self, params: Value, grant: bool) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let permission = string_param(&params, "permission")?;
        let account = self.plugin_account_context()?;
        self.ensure_plugin_account_context_matches_expected(&account, expected_account.as_ref())?;
        let pointer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        let mut manager = PermissionManager::load(account.root.join("permissions.json"))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if grant {
            manager
                .grant_with_commit_guard(
                    plugin_id,
                    &pointer.requested_permissions,
                    permission,
                    || {
                        self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                            scope == account.scope && generation == account.generation
                        })
                    },
                )
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
        } else {
            manager
                .revoke_with_commit_guard(plugin_id, permission, || {
                    self.current_plugin_account_identity().is_ok_and(|(scope, generation)| {
                        scope == account.scope && generation == account.generation
                    })
                })
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
        }
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        let granted = manager.grants_for(plugin_id);
        let missing = pointer
            .requested_permissions
            .iter()
            .filter(|candidate| !granted.contains(candidate))
            .cloned()
            .collect::<Vec<_>>();
        Ok(json!({
            "requested": pointer.requested_permissions,
            "granted": granted,
            "missing": missing
        }))
    }

    fn plugin_compatibility(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let (account_scope, account_generation, _pointer, root) =
            self.active_plugin_ui_root(plugin_id, expected_account.as_ref())?;
        let report = scan_package_compatibility(&root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account_scope, &account_generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        serde_json::to_value(report).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn active_plugin_ui_root(
        &self,
        plugin_id: &str,
        expected_account: Option<&(String, String)>,
    ) -> Result<(String, String, InstalledPluginPointer, PathBuf), AppHostError> {
        let account = self.plugin_account_context()?;
        self.ensure_plugin_account_context_matches_expected(&account, expected_account)?;
        let installer = PluginInstaller::new(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let pointer = installer
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        let account_root = std::fs::canonicalize(&account.root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let plugin_root = account_root.join(plugin_id);
        let root = std::fs::canonicalize(&pointer.installed_path)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if !root.starts_with(&plugin_root) || !root.is_dir() {
            return Err(AppHostError::Operation(
                "installed plugin root escaped the active account scope".into(),
            ));
        }
        self.ensure_plugin_account_current(&account.scope, &account.generation)?;
        Ok((account.scope, account.generation, pointer, root))
    }

    fn plugin_ui_entry(
        &self,
        plugin_id: &str,
        pointer: &InstalledPluginPointer,
        root: &Path,
    ) -> Result<String, AppHostError> {
        let requested_entry = pointer
            .entry
            .as_deref()
            .map(|value| value.trim_start_matches("./"));
        requested_entry
            .filter(|value| value.to_ascii_lowercase().ends_with(".html"))
            .map(str::to_string)
            .or_else(|| {
                ["ui/index.html", "web/index.html", "index.html"]
                    .into_iter()
                    .find(|candidate| root.join(candidate).is_file())
                    .map(str::to_string)
            })
            .ok_or_else(|| {
                AppHostError::Operation(format!(
                    "installed plugin {plugin_id} does not expose an HTML Mini App entry"
                ))
            })
    }

    fn plugin_ui_document(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let (account_scope, account_generation, pointer, root) =
            self.active_plugin_ui_root(plugin_id, expected_account.as_ref())?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        let entry = self.plugin_ui_entry(plugin_id, &pointer, &root)?;
        let relative = safe_ui_relative_path(&entry)?;
        let document = std::fs::canonicalize(root.join(relative))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if !document.starts_with(&root) || !document.is_file() {
            return Err(AppHostError::InvalidRequest(
                "plugin UI entry escaped installed root".into(),
            ));
        }
        let html = std::fs::read_to_string(document)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        self.ensure_plugin_account_current(&account_scope, &account_generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(json!({
            "pluginId": plugin_id,
            "html": html,
            "entry": entry,
            "version": pointer.version,
            "artifactSha256": pointer.artifact_sha256,
            "installedPath": pointer.installed_path,
        }))
    }

    fn plugin_ui_resource(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?;
        let path = string_param(&params, "path")?;
        let (account_scope, account_generation, pointer, root) =
            self.active_plugin_ui_root(plugin_id, expected_account.as_ref())?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;

        if let Some(expected_version) = params.get("version").and_then(Value::as_str)
            && expected_version != pointer.version
        {
            return Err(AppHostError::Operation(
                "active Mini App version changed before resource read".into(),
            ));
        }
        if let Some(expected_digest) = params.get("artifactSha256").and_then(Value::as_str)
            && !expected_digest.eq_ignore_ascii_case(&pointer.artifact_sha256)
        {
            return Err(AppHostError::Operation(
                "active Mini App artifact changed before resource read".into(),
            ));
        }

        let relative = safe_ui_relative_path(path)?;
        let resource = std::fs::canonicalize(root.join(&relative))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if !resource.starts_with(&root) || !resource.is_file() {
            return Err(AppHostError::InvalidRequest(
                "plugin UI resource escaped installed root".into(),
            ));
        }

        let metadata = std::fs::metadata(&resource)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let total_bytes = metadata.len();
        let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0);
        if offset > total_bytes {
            return Err(AppHostError::InvalidRequest(
                "plugin UI resource offset exceeds file length".into(),
            ));
        }
        let requested_length = params
            .get("length")
            .and_then(Value::as_u64)
            .unwrap_or(256 * 1024)
            .clamp(1, 1024 * 1024);
        let read_length = requested_length.min(total_bytes.saturating_sub(offset)) as usize;

        let mut file = std::fs::File::open(&resource)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let mut bytes = vec![0_u8; read_length];
        file.read_exact(&mut bytes)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;

        let normalized_path = relative.to_string_lossy().replace('\\', "/");
        self.ensure_plugin_account_current(&account_scope, &account_generation)?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        Ok(json!({
            "pluginId": plugin_id,
            "path": normalized_path,
            "version": pointer.version,
            "artifactSha256": pointer.artifact_sha256,
            "offset": offset,
            "totalBytes": total_bytes,
            "eof": offset.saturating_add(read_length as u64) >= total_bytes,
            "mime": miniapp_resource_mime(&relative),
            "dataBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
        }))
    }

    fn start_runtime(&self, params: Value) -> Result<Value, AppHostError> {
        let expected_account = self.expected_plugin_account_identity(&params)?;
        let plugin_id = string_param(&params, "pluginId")?.to_string();
        let (account_scope, account_generation, pointer, root) =
            self.active_plugin_ui_root(&plugin_id, expected_account.as_ref())?;
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        if !matches!(
            pointer.runtime.as_str(),
            "deepseek-js" | "javascript" | "cordis-js"
        ) {
            return Err(AppHostError::Operation(format!(
                "runtime {} is not supported by the portable JS host",
                pointer.runtime
            )));
        }
        let entry = discover_js_entry(&pointer.entry, &root)?;
        let report = scan_package_compatibility(&root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if !report.portable_compatible {
            return Err(AppHostError::Operation(report.blockers().join("; ")));
        }
        let grants = PermissionManager::load(
            self.app_data_dir
                .join("plugins")
                .join("accounts")
                .join(&account_scope)
                .join("permissions.json"),
        )
        .map_err(|error| AppHostError::Operation(error.to_string()))?
        .grants_for(&plugin_id);
        self.ensure_plugin_account_current(&account_scope, &account_generation)?;
        let config = params.get("config").cloned().unwrap_or_else(|| json!({}));
        let mut bindings = self
            .runtime_accounts
            .lock()
            .map_err(|_| AppHostError::Operation("plugin runtime account lock poisoned".into()))?;
        let mut host = self
            .js
            .lock()
            .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?;
        let state = host
            .register_plugin_with_grants(&plugin_id, &root, &entry, &config, &grants)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if self
            .ensure_plugin_account_current(&account_scope, &account_generation)
            .is_err()
            || self
                .ensure_expected_plugin_account_identity_current(expected_account.as_ref())
                .is_err()
        {
            let _ = host.disable_plugin(&plugin_id);
            bindings.remove(&plugin_id);
            return Err(AppHostError::Operation(
                "account generation changed while plugin runtime was starting".into(),
            ));
        }
        bindings.insert(
            plugin_id,
            PluginRuntimeAccountBinding {
                account_scope,
                account_generation,
            },
        );
        self.ensure_expected_plugin_account_identity_current(expected_account.as_ref())?;
        serde_json::to_value(state).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn stop_runtime(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        self.disable_plugin_runtime(plugin_id, true)?;
        Ok(Value::Null)
    }

    fn runtime_tools(&self) -> Result<Value, AppHostError> {
        let tools = self
            .js
            .lock()
            .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?
            .registered_tools()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(tools).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn platform_exec_broker(&self) -> Result<&Arc<PlatformExecBroker>, AppHostError> {
        self.platform_exec.as_ref().ok_or_else(|| {
            AppHostError::Operation("platform execution broker is unavailable".into())
        })
    }

    fn platform_exec_next(&self) -> Result<Value, AppHostError> {
        let request = self
            .platform_exec_broker()?
            .next_request()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(request)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn platform_exec_resolve(&self, params: &Value) -> Result<Value, AppHostError> {
        let request_id = string_param(params, "requestId")?;
        let result = params.get("result").cloned().unwrap_or(Value::Null);
        self.platform_exec_broker()?
            .resolve(request_id, result)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(json!({"settled": true}))
    }

    fn platform_exec_reject(&self, params: &Value) -> Result<Value, AppHostError> {
        let request_id = string_param(params, "requestId")?;
        let code = params
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("platform_execution_failed");
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("platform execution failed");
        self.platform_exec_broker()?
            .reject(request_id, code, message)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(json!({"settled": true}))
    }

    fn platform_exec_cancel(&self, params: &Value) -> Result<Value, AppHostError> {
        let request_id = string_param(params, "requestId")?;
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("platform execution cancelled");
        self.platform_exec_broker()?
            .cancel(request_id, reason)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(json!({"settled": true}))
    }

    /// Deterministically settle the canonical feature Host before the process-local
    /// app-host owner is released. Plugin runtimes are torn down first so no
    /// account-scoped JavaScript task survives the owner that supplied its grants.
    pub fn close(&self) -> Result<(), AppHostError> {
        {
            let mut bindings = self
                .runtime_accounts
                .lock()
                .map_err(|_| AppHostError::Operation("plugin runtime account lock poisoned".into()))?;
            let mut host = self
                .js
                .lock()
                .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?;
            for plugin_id in bindings.keys() {
                let _ = host.disable_plugin(plugin_id);
            }
            bindings.clear();
        }
        if let Some(broker) = &self.platform_exec {
            broker
                .close("app host closed")
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
        }
        self.feature
            .close()
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn runtime_call(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let name = string_param(&params, "name")?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err(AppHostError::InvalidRequest(
                "runtime.call arguments must be an object".into(),
            ));
        }
        if self.feature_mode == AppHostFeatureMode::Test {
            let installed = self
                .plugin_root()?
                .join(plugin_id)
                .join("active.json")
                .is_file();
            if installed
                && TEST_MARKETPLACE_PLUGINS
                    .iter()
                    .any(|(candidate, _, _)| *candidate == plugin_id)
            {
                return deterministic_test_runtime_call(plugin_id, name, &arguments);
            }
        }

        self.reconcile_plugin_runtime_accounts()?;
        let current = self.current_plugin_account_identity()?;
        let needs_restore = {
            let bindings = self.runtime_accounts.lock().map_err(|_| {
                AppHostError::Operation("plugin runtime account lock poisoned".into())
            })?;
            let binding_current = bindings
                .get(plugin_id)
                .is_some_and(|binding| {
                    plugin_runtime_binding_matches_identity(binding, Some(&current))
                });
            let host = self
                .js
                .lock()
                .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?;
            !binding_current || host.plugin_state(plugin_id) != Some(PluginState::Active)
        };
        if needs_restore {
            self.start_runtime(json!({
                "pluginId": plugin_id,
                "config": {},
            }))?;
        }

        let current = self.current_plugin_account_identity()?;
        let bindings = self
            .runtime_accounts
            .lock()
            .map_err(|_| AppHostError::Operation("plugin runtime account lock poisoned".into()))?;
        let binding = bindings.get(plugin_id).ok_or_else(|| {
            AppHostError::Operation(format!(
                "plugin {plugin_id} runtime has no current account binding"
            ))
        })?;
        if !plugin_runtime_binding_matches_identity(binding, Some(&current)) {
            return Err(AppHostError::Operation(
                "plugin runtime belongs to a stale account generation".into(),
            ));
        }
        let host = self
            .js
            .lock()
            .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?;
        if host.plugin_state(plugin_id) != Some(PluginState::Active) {
            return Err(AppHostError::Operation(format!(
                "plugin {plugin_id} runtime is not active"
            )));
        }
        let tools = host
            .registered_tools()
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if !tools.iter().any(|candidate| candidate == name) {
            return Err(AppHostError::Operation(format!(
                "tool {name} is not registered in the active runtime"
            )));
        }
        let result = host
            .call_tool_json(name, &arguments)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        drop(host);
        drop(bindings);
        self.ensure_plugin_account_current(&current.0, &current.1)?;
        Ok(result)
    }
}

fn deterministic_test_runtime_call(
    plugin_id: &str,
    name: &str,
    arguments: &Value,
) -> Result<Value, AppHostError> {
    if plugin_id != "global-dharma" {
        return Ok(json!({
            "content": [{"type": "text", "text": format!("{plugin_id} test tool {name} completed") }],
            "structuredContent": {"testMode": true, "tool": name, "arguments": arguments},
        }));
    }
    let (text, structured) = match name {
        "status" => (
            "已读取全球法布施状态。",
            json!({"running": false, "mode": "home", "testMode": true}),
        ),
        "start" => (
            "本地转经轮已通过宿主权限校验并启动。",
            json!({"running": true, "mode": "local-prayer-wheel", "testMode": true}),
        ),
        "stop" => (
            "全球法布施本地模式已停止。",
            json!({"running": false, "mode": "home", "testMode": true}),
        ),
        "logs" => (
            "已读取全球法布施日志。",
            json!({"entries": ["deterministic GitHub Actions test runtime"], "testMode": true}),
        ),
        "send" => (
            "全球发送测试请求已完成。",
            json!({"sent": 1, "testMode": true}),
        ),
        other => {
            return Err(AppHostError::Operation(format!(
                "test runtime tool {other} is not available for global-dharma"
            )));
        }
    };
    Ok(json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured,
    }))
}

fn configured_feature_host_mode() -> Result<AppHostFeatureMode, AppHostError> {
    match std::env::var("FABUSHI_FEATURE_HOST_MODE") {
        Ok(value) if value.eq_ignore_ascii_case("test") => Ok(AppHostFeatureMode::Test),
        Ok(value) if value.eq_ignore_ascii_case("production") => Ok(AppHostFeatureMode::Production),
        Ok(value) if value.trim().is_empty() => Ok(AppHostFeatureMode::Production),
        Ok(value) => Err(AppHostError::InvalidRequest(format!(
            "unsupported FABUSHI_FEATURE_HOST_MODE {value:?}; expected test or production"
        ))),
        Err(std::env::VarError::NotPresent) => Ok(AppHostFeatureMode::Production),
        Err(error) => Err(AppHostError::InvalidRequest(format!(
            "invalid FABUSHI_FEATURE_HOST_MODE: {error}"
        ))),
    }
}

fn feature_host_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("feature-host")
}

fn effective_inference_provider(
    feature_mode: AppHostFeatureMode,
    configured_provider: &str,
) -> String {
    if feature_mode == AppHostFeatureMode::Production {
        "fabushi".to_string()
    } else {
        configured_provider.to_string()
    }
}

fn normalize_fabushi_responses_url(raw: &str) -> String {
    let base = raw.trim().trim_end_matches('/');
    if base.ends_with("/responses") {
        base.to_string()
    } else if base.ends_with("/codex-deepseek/v1") || base.ends_with("/v1/ai") {
        format!("{base}/responses")
    } else {
        format!("{base}/codex-deepseek/v1/responses")
    }
}

fn configured_fabushi_responses_url() -> String {
    if let Ok(value) = std::env::var("FABUSHI_RESPONSES_URL")
        && !value.trim().is_empty()
    {
        return normalize_fabushi_responses_url(&value);
    }
    let api_base = std::env::var("FABUSHI_API_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("MAHAYANA_API_BASE_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "https://api.ombhrum.com".to_string());
    normalize_fabushi_responses_url(&api_base)
}

fn resolve_fabushi_model(raw: Option<String>) -> String {
    raw.map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "deepseek-chat".to_string())
}

fn configured_fabushi_model() -> String {
    resolve_fabushi_model(std::env::var("FABUSHI_RESPONSES_MODEL").ok())
}

fn runtime_build_profile_for_surface(platform: SurfacePlatform) -> BuildProfile {
    match platform {
        SurfacePlatform::Ios | SurfacePlatform::Android => BuildProfile::MobileEmbedded,
        SurfacePlatform::Wasm => BuildProfile::WebWasm,
        SurfacePlatform::Electron | SurfacePlatform::Mock => BuildProfile::DesktopFull,
    }
}

fn process_execution_for_surface(platform: SurfacePlatform) -> ProcessExecution {
    match platform {
        SurfacePlatform::Ios | SurfacePlatform::Android | SurfacePlatform::Wasm => {
            ProcessExecution::Unavailable
        }
        SurfacePlatform::Electron | SurfacePlatform::Mock => {
            if std::env::var("MAHAYANA_SANDBOX_RUNTIME").as_deref() == Ok("local-docker") {
                ProcessExecution::LocalDocker {
                    docker_path: std::env::var_os("MAHAYANA_DOCKER_BIN")
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from("docker")),
                    image: std::env::var("MAHAYANA_DOCKER_IMAGE").unwrap_or_default(),
                }
            } else {
                ProcessExecution::Host
            }
        }
    }
}

fn create_feature_host(
    app_data_dir: &Path,
    feature_mode: AppHostFeatureMode,
    storage_passphrase: Option<String>,
) -> Result<(FeatureHostController, Option<Arc<PlatformExecBroker>>), AppHostError> {
    let root = feature_host_root(app_data_dir);
    std::fs::create_dir_all(&root).map_err(|error| AppHostError::Operation(error.to_string()))?;
    let platform = surface_platform();
    let configured_provider =
        std::env::var("MAHAYANA_INFERENCE_PROVIDER").unwrap_or_else(|_| "fabushi".into());
    let provider = effective_inference_provider(feature_mode, &configured_provider);
    let mut runtime = RuntimeConfig {
        build_profile: runtime_build_profile_for_surface(platform),
        data_dir: Some(root.join("runtime")),
        ..RuntimeConfig::default()
    };
    if provider == "fabushi" {
        runtime.model.provider = ModelProviderMode::FirstPartyDacheng;
        runtime.model.base_url = Some(configured_fabushi_responses_url());
        runtime.model.model = configured_fabushi_model();
        runtime.model.credential_key = Some("mahayana.account.session".into());
    } else if provider == "openrouter" {
        runtime.model.provider = ModelProviderMode::UserConfiguredRemote;
        runtime.model.base_url = Some("https://openrouter.ai/api/v1".into());
        runtime.model.model =
            std::env::var("MAHAYANA_OPENROUTER_MODEL").unwrap_or_else(|_| "openai/gpt-5.2".into());
        runtime.model.credential_key = Some("inference/openrouter/api-key".into());
    } else if provider == "claude-code" {
        runtime.model.provider = ModelProviderMode::UserConfiguredRemote;
        runtime.model.base_url = Some("https://api.anthropic.com/v1".into());
        runtime.model.model =
            std::env::var("MAHAYANA_CLAUDE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".into());
        runtime.model.credential_key = Some("inference/claude/api-key".into());
    }
    let platform_exec = match platform {
        SurfacePlatform::Ios | SurfacePlatform::Android => {
            Some(Arc::new(PlatformExecBroker::new(64)))
        }
        _ => None,
    };
    let process_execution = platform_exec
        .as_ref()
        .map(|broker| ProcessExecution::Remote(Arc::clone(broker)))
        .unwrap_or_else(|| process_execution_for_surface(platform));
    let host_config = HostCreateConfig {
        runtime,
        product_session_path: Some(root.join("account-session.json")),
        product_surface_state_path: Some(root.join("product-surface.json")),
        automation_path: Some(root.join("automations.json")),
        use_codex_account: std::env::var("MAHAYANA_USE_CODEX_ACCOUNT").as_deref() == Ok("1"),
        codex_home: std::env::var_os("MAHAYANA_CODEX_HOME").map(PathBuf::from),
        product_storage_passphrase: storage_passphrase,
        model_bearer_token: std::env::var("MAHAYANA_MODEL_BEARER_TOKEN")
            .ok()
            .filter(|value| !value.is_empty()),
        model_wire_api: match provider.as_str() {
            "openrouter" => mahayana_model::responses::ResponsesWireApi::ChatCompletions,
            "claude-code" => mahayana_model::responses::ResponsesWireApi::AnthropicMessages,
            _ => mahayana_model::responses::ResponsesWireApi::Responses,
        },
        inherit_installed_plugins: Some(false),
        process_execution,
        ..HostCreateConfig::default()
    };
    let feature = FeatureHostController::create_with_host_config(
        HostConfig {
            profile_id: "default".to_string(),
            mode: feature_mode.into(),
        },
        platform,
        host_config,
    )
    .map_err(|error| {
        AppHostError::Operation(format!("feature host initialization failed: {error}"))
    })?;
    Ok((feature, platform_exec))
}

fn safe_ui_relative_path(value: &str) -> Result<PathBuf, AppHostError> {
    let value = value.trim();
    if value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(AppHostError::InvalidRequest(
            "plugin UI resource path is invalid".into(),
        ));
    }
    let relative = PathBuf::from(value);
    if relative.components().any(|component| {
        !matches!(component, std::path::Component::Normal(_))
    }) {
        return Err(AppHostError::InvalidRequest(
            "plugin UI resource path is invalid".into(),
        ));
    }
    Ok(relative)
}

fn miniapp_resource_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn discover_js_entry(entry: &Option<String>, root: &Path) -> Result<PathBuf, AppHostError> {
    if let Some(entry) = entry.as_deref() {
        let relative = PathBuf::from(entry.trim_start_matches("./"));
        if relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return Err(AppHostError::InvalidRequest(
                "plugin entry escaped installed root".into(),
            ));
        }
        if root.join(&relative).is_file() {
            return Ok(relative);
        }
    }
    for candidate in ["index.mjs", "index.js", "lib/index.js", "dist/index.js"] {
        if root.join(candidate).is_file() {
            return Ok(PathBuf::from(candidate));
        }
    }
    Err(AppHostError::Operation(
        "installed plugin has no runnable JavaScript entry".into(),
    ))
}


fn relay_filter_strings(filters: Option<&BTreeMap<String, Value>>, keys: &[&str]) -> Vec<String> {
    let Some(filters) = filters else { return Vec::new() };
    let mut values = Vec::new();
    for key in keys {
        let Some(value) = filters.get(*key) else { continue };
        match value {
            Value::String(value) if !value.trim().is_empty() => values.push(value.trim().to_string()),
            Value::Array(items) => values.extend(
                items.iter().filter_map(Value::as_str).map(str::trim)
                    .filter(|value| !value.is_empty()).map(str::to_string),
            ),
            _ => {}
        }
    }
    values
}

fn collect_automation_relay_subscription(
    trigger: &AutomationTrigger,
    slack_channels: &mut BTreeSet<String>,
    github_repos: &mut BTreeSet<String>,
    github_kinds: &mut BTreeSet<String>,
) {
    match trigger {
        AutomationTrigger::Schedule { .. } => {}
        AutomationTrigger::Group { listeners } => {
            for listener in listeners {
                collect_automation_relay_subscription(listener, slack_channels, github_repos, github_kinds);
            }
        }
        AutomationTrigger::Event { source, event, filter, filters } => match source {
            ListenerPlatform::Slack => {
                let mut channels = relay_filter_strings(filters.as_ref(), &["channel", "channelId", "channelIds"]);
                if channels.is_empty() {
                    if let Some(filter) = filter.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
                        channels.push(filter.to_string());
                    }
                }
                slack_channels.extend(channels);
            }
            ListenerPlatform::Github => {
                let mut repos = relay_filter_strings(filters.as_ref(), &["repo", "repository", "repos", "repositories"]);
                if repos.is_empty() {
                    if let Some(filter) = filter.as_deref().map(str::trim).filter(|value| value.contains('/')) {
                        repos.push(filter.to_string());
                    }
                }
                github_repos.extend(repos.into_iter().map(|repo| repo.to_ascii_lowercase()));
                github_kinds.extend(relay_filter_strings(filters.as_ref(), &["events", "kinds"]));
                let event = event.trim();
                if !event.is_empty() && event != "*" {
                    github_kinds.insert(event.to_string());
                }
            }
            _ => {}
        },
    }
}

fn routine_projection(automation: AutomationSummary) -> Option<Value> {
    let agent_id = automation.agent_id.as_deref()?.trim();
    if agent_id.is_empty() {
        return None;
    }
    let trigger_description = automation
        .trigger
        .as_ref()
        .map(routine_trigger_description)
        .unwrap_or_else(|| automation.schedule.clone());
    Some(json!({
        "agentId": agent_id,
        "automation": {
            "id": automation.id,
            "name": automation.name,
            "triggerDescription": trigger_description,
            "createdAt": automation.created_at_ms,
            "lastRunAt": automation.last_run_at_ms,
        }
    }))
}

fn routine_trigger_description(trigger: &AutomationTrigger) -> String {
    match trigger {
        AutomationTrigger::Schedule { schedule } => schedule.clone(),
        AutomationTrigger::Event {
            source,
            event,
            filter,
            filters,
        } => {
            let source = serde_json::to_value(source)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "event".into());
            routine_event_description(
                &source,
                event,
                filter.as_deref(),
                filters.as_ref(),
            )
        }
        AutomationTrigger::Group { listeners } => {
            let descriptions = listeners
                .iter()
                .map(routine_trigger_description)
                .filter(|description| !description.trim().is_empty())
                .collect::<Vec<_>>();
            if descriptions.is_empty() {
                "group".into()
            } else {
                descriptions.join(" or ")
            }
        }
    }
}

fn routine_event_description(
    source: &str,
    event: &str,
    legacy_filter: Option<&str>,
    filters: Option<&BTreeMap<String, Value>>,
) -> String {
    let mut segments = vec![source.to_string(), event.to_string()];
    if let Some(filter) = legacy_filter
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        segments.push(filter.to_string());
    }
    if let Some(filters) = filters {
        segments.extend(filters.iter().filter_map(|(key, value)| {
            routine_filter_value(value).map(|value| format!("{key}={value}"))
        }));
    }
    segments.join(" · ")
}

fn routine_filter_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Value::Array(values) => {
            let values = values
                .iter()
                .filter_map(routine_filter_value)
                .collect::<Vec<_>>();
            (!values.is_empty()).then(|| values.join(", "))
        }
        Value::Object(values) if values.is_empty() => None,
        Value::Object(_) | Value::Bool(_) | Value::Number(_) => {
            serde_json::to_string(value).ok()
        }
    }
}

const LINK_METADATA_MAX_URL_BYTES: usize = 2_048;
const LINK_METADATA_MAX_REDIRECTS: usize = 5;
const LINK_METADATA_HTML_BYTE_LIMIT: u64 = 512 * 1024;

fn validate_public_link_url(raw: &str) -> Result<url::Url, AppHostError> {
    if raw.len() > LINK_METADATA_MAX_URL_BYTES {
        return Err(AppHostError::InvalidRequest(
            "link metadata URL is too long".into(),
        ));
    }
    let parsed = url::Url::parse(raw)
        .map_err(|error| AppHostError::InvalidRequest(format!("invalid link URL: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(AppHostError::InvalidRequest(
            "link metadata requires HTTPS".into(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(AppHostError::InvalidRequest(
            "link metadata URL must not contain credentials".into(),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| AppHostError::InvalidRequest("link URL hostname is required".into()))?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err(AppHostError::InvalidRequest(
            "link metadata rejects local network destinations".into(),
        ));
    }
    if parsed.port().is_some() {
        return Err(AppHostError::InvalidRequest(
            "link metadata URL must not use a custom port".into(),
        ));
    }
    let addresses = (host, 443)
        .to_socket_addrs()
        .map_err(|error| AppHostError::Operation(format!("resolve link URL: {error}")))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(AppHostError::InvalidRequest(
            "link metadata rejects local or private network destinations".into(),
        ));
    }
    Ok(parsed)
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && ip.octets()[0] != 0
        }
        IpAddr::V6(ip) => {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && !ip.is_multicast()
        }
    }
}

fn html_tag_text(html: &str, tag: &str) -> Option<String> {
    let lowercase = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let start = lowercase.find(&open)?;
    let content_start = lowercase[start..].find('>')? + start + 1;
    let close = format!("</{tag}>");
    let end = lowercase[content_start..].find(&close)? + content_start;
    clean_html_text(&html[content_start..end])
}

fn html_meta_content(html: &str, name: &str) -> Option<String> {
    html_meta_value(html, "name", name)
}

fn html_meta_property_content(html: &str, property: &str) -> Option<String> {
    html_meta_value(html, "property", property)
}

fn html_meta_value(html: &str, attribute: &str, expected: &str) -> Option<String> {
    let lowercase = html.to_ascii_lowercase();
    let expected_a = format!("{attribute}=\"{}\"", expected.to_ascii_lowercase());
    let expected_b = format!("{attribute}='{}'", expected.to_ascii_lowercase());
    let mut cursor = 0;
    while let Some(relative) = lowercase[cursor..].find("<meta") {
        let start = cursor + relative;
        let end = lowercase[start..].find('>')? + start + 1;
        let lower_tag = &lowercase[start..end];
        if lower_tag.contains(&expected_a) || lower_tag.contains(&expected_b) {
            return html_attribute(&html[start..end], "content");
        }
        cursor = end;
    }
    None
}

fn html_attribute(tag: &str, attribute: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{attribute}=");
    let index = lower.find(&needle)? + needle.len();
    let bytes = tag.as_bytes();
    let quote = *bytes.get(index)?;
    if quote == b'"' || quote == b'\'' {
        let rest = &tag[index + 1..];
        let end = rest.find(quote as char)?;
        clean_html_text(&rest[..end])
    } else {
        let rest = &tag[index..];
        let end = rest.find(|ch: char| ch.is_whitespace() || ch == '>').unwrap_or(rest.len());
        clean_html_text(&rest[..end])
    }
}

fn clean_html_text(value: &str) -> Option<String> {
    let collapsed = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(512).collect())
    }
}


fn strict_hex_identifier(value: &str, length: usize, label: &str) -> Result<String, AppHostError> {
    let normalized = value.trim();
    if normalized.len() != length || !normalized.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        return Err(AppHostError::InvalidRequest(format!("{label} must be {length} lowercase hex characters")));
    }
    Ok(normalized.to_string())
}

fn strict_lower_hex_digest(value: &str, label: &str) -> Result<String, AppHostError> {
    strict_hex_identifier(value, 64, label)
}

fn strict_ascii_identifier(value: &str, label: &str) -> Result<String, AppHostError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.len() > 128
        || !normalized.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AppHostError::InvalidRequest(format!("{label} is invalid")));
    }
    Ok(normalized.to_string())
}

fn decode_ed25519_public_key(value: &str) -> Result<[u8; 32], AppHostError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim().as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value.trim().as_bytes()))
        .map_err(|_| AppHostError::InvalidRequest("publicKeyBase64 is invalid".into()))?;
    bytes
        .try_into()
        .map_err(|_| AppHostError::InvalidRequest("Ed25519 public key must be 32 bytes".into()))
}

fn stable_plugin_account_scope(user_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"fabushi.plugin-account-scope.v1\0");
    digest.update(user_id.trim().as_bytes());
    format!("{:x}", digest.finalize())
}

fn stable_plugin_account_generation(user_id: &str, session_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"fabushi.plugin-account-generation.v1\0");
    digest.update(user_id.trim().as_bytes());
    digest.update([0]);
    digest.update(session_id.trim().as_bytes());
    format!("{:x}", digest.finalize())
}

fn plugin_runtime_binding_matches_identity(
    binding: &PluginRuntimeAccountBinding,
    identity: Option<&(String, String)>,
) -> bool {
    identity.is_some_and(|(scope, generation)| {
        binding.account_scope.as_str() == scope.as_str()
            && binding.account_generation.as_str() == generation.as_str()
    })
}

fn stale_plugin_runtime_ids(
    bindings: &BTreeMap<String, PluginRuntimeAccountBinding>,
    current: Option<&(String, String)>,
) -> Vec<String> {
    bindings
        .iter()
        .filter_map(|(plugin_id, binding)| {
            (!plugin_runtime_binding_matches_identity(binding, current))
                .then_some(plugin_id.clone())
        })
        .collect()
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AppHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| AppHostError::Operation(error.to_string()))?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    std::fs::write(&temp, bytes)
        .map_err(|error| AppHostError::Operation(error.to_string()))?;
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
    }
    std::fs::rename(&temp, path)
        .map_err(|error| AppHostError::Operation(error.to_string()))
}

fn ensure_plugin_account_marker(root: &Path, scope: &str) -> Result<(), AppHostError> {
    let marker_path = root.join("account-scope.json");
    if marker_path.is_file() {
        let marker: PluginAccountMarker = serde_json::from_slice(
            &std::fs::read(&marker_path)
                .map_err(|error| AppHostError::Operation(error.to_string()))?,
        )
        .map_err(|error| AppHostError::Operation(format!("invalid plugin account marker: {error}")))?;
        if marker.schema_version != 1 || marker.account_scope != scope {
            return Err(AppHostError::Operation(
                "plugin account marker does not match the active account".into(),
            ));
        }
        return Ok(());
    }
    atomic_write_json(
        &marker_path,
        &PluginAccountMarker {
            schema_version: 1,
            account_scope: scope.to_string(),
        },
    )
}

fn migrate_legacy_plugin_root_if_proven(
    base: &Path,
    scope: &str,
    account_root: &Path,
) -> Result<(), AppHostError> {
    let proof_path = base.join("legacy-owner.json");
    let proof_source = match std::fs::read(&proof_path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppHostError::Operation(error.to_string())),
    };
    let proof: LegacyPluginOwnerProof = serde_json::from_slice(&proof_source)
        .map_err(|error| AppHostError::Operation(format!("invalid legacy plugin owner proof: {error}")))?;
    if proof.schema_version != 1 || proof.account_scope != scope {
        // Ambiguous or other-account legacy bytes remain invisible under the old root.
        return Ok(());
    }

    let migrations_root = base.join("migrations");
    std::fs::create_dir_all(&migrations_root)
        .map_err(|error| AppHostError::Operation(error.to_string()))?;
    let journal_path = migrations_root.join(format!("{scope}.json"));
    let mut journal = if journal_path.is_file() {
        let parsed: LegacyPluginMigrationJournal = serde_json::from_slice(
            &std::fs::read(&journal_path)
                .map_err(|error| AppHostError::Operation(error.to_string()))?,
        )
        .map_err(|error| AppHostError::Operation(format!("invalid legacy plugin migration journal: {error}")))?;
        if parsed.schema_version != 1 || parsed.account_scope != scope {
            return Err(AppHostError::Operation(
                "legacy plugin migration journal scope mismatch".into(),
            ));
        }
        parsed
    } else {
        let mut entries = Vec::new();
        if base.is_dir() {
            for entry in std::fs::read_dir(base)
                .map_err(|error| AppHostError::Operation(error.to_string()))?
            {
                let entry = entry.map_err(|error| AppHostError::Operation(error.to_string()))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if matches!(
                    name.as_str(),
                    "accounts" | "migrations" | "legacy-owner.json"
                ) {
                    continue;
                }
                if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
                    return Err(AppHostError::Operation(
                        "legacy plugin root contains an unsafe entry name".into(),
                    ));
                }
                entries.push(name);
            }
        }
        entries.sort();
        let journal = LegacyPluginMigrationJournal {
            schema_version: 1,
            account_scope: scope.to_string(),
            state: "prepared".into(),
            entries,
            completed: Vec::new(),
        };
        atomic_write_json(&journal_path, &journal)?;
        journal
    };

    if journal.state == "completed" {
        return Ok(());
    }

    for name in journal.entries.clone() {
        if journal.completed.iter().any(|done| done == &name) {
            continue;
        }
        let source = base.join(&name);
        let target = account_root.join(&name);
        match (source.exists(), target.exists()) {
            (false, true) => {}
            (false, false) => {
                return Err(AppHostError::Operation(format!(
                    "legacy plugin migration lost entry {name}"
                )));
            }
            (true, true) => {
                return Err(AppHostError::Operation(format!(
                    "legacy plugin migration conflict for {name}"
                )));
            }
            (true, false) => {
                std::fs::rename(&source, &target)
                    .map_err(|error| AppHostError::Operation(error.to_string()))?;
            }
        }
        journal.completed.push(name);
        journal.state = "migrating".into();
        atomic_write_json(&journal_path, &journal)?;
    }
    journal.state = "completed".into();
    atomic_write_json(&journal_path, &journal)
}

fn load_plugin_git_auth_blocks(path: &Path) -> Vec<PluginGitAuthBlock> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    serde_json::from_slice::<Vec<PluginGitAuthBlock>>(&bytes).unwrap_or_default()
}

fn persist_plugin_git_auth_blocks(
    path: &Path,
    blocks: &[PluginGitAuthBlock],
) -> Result<(), AppHostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
    }
    let temp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(blocks)
        .map_err(|error| AppHostError::Operation(error.to_string()))?;
    std::fs::write(&temp, bytes)
        .map_err(|error| AppHostError::Operation(error.to_string()))?;
    std::fs::rename(&temp, path)
        .map_err(|error| AppHostError::Operation(error.to_string()))
}

fn is_git_authentication_failure(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    [
        "authentication failed",
        "could not read username",
        "permission denied",
        "repository not found",
        "authorization failed",
        "authentication required",
        "http 401",
        "http 403",
        "status 401",
        "status 403",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn agent_store_path_param<'a>(
    params: &'a Value,
    name: &str,
) -> Result<&'a str, AppHostError> {
    match params.get(name) {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(AppHostError::InvalidRequest(format!("{name} is required"))),
    }
}

fn string_param<'a>(params: &'a Value, name: &str) -> Result<&'a str, AppHostError> {
    params
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppHostError::InvalidRequest(format!("{name} is required")))
}

fn bool_param(params: &Value, name: &str) -> Result<bool, AppHostError> {
    params
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            AppHostError::InvalidRequest(format!("{name} is required and must be boolean"))
        })
}

fn sharing_rpc(method: &str) -> Result<SharingRpc, AppHostError> {
    match method {
        "sharing.state" => Ok(SharingRpc::State),
        "sharing.createRoomInvite" => Ok(SharingRpc::CreateRoomInvite),
        "sharing.respondToRoomJoinRequest" => Ok(SharingRpc::RespondToRoomJoinRequest),
        "sharing.addOwnAgent" => Ok(SharingRpc::AddOwnAgent),
        "sharing.removeOwnAgent" => Ok(SharingRpc::RemoveOwnAgent),
        "sharing.leaveRoom" => Ok(SharingRpc::LeaveRoom),
        other => Err(AppHostError::InvalidRequest(format!(
            "unknown sharing method {other}"
        ))),
    }
}

fn validate_sharing_params(rpc: SharingRpc, params: &Value) -> Result<(), AppHostError> {
    match rpc {
        SharingRpc::State => Ok(()),
        SharingRpc::CreateRoomInvite => {
            string_param(params, "roomId")?;
            Ok(())
        }
        SharingRpc::RespondToRoomJoinRequest => {
            string_param(params, "requestId")?;
            bool_param(params, "isApproved")?;
            Ok(())
        }
        SharingRpc::AddOwnAgent => {
            string_param(params, "roomId")?;
            string_param(params, "agentId")?;
            string_param(params, "agentName")?;
            Ok(())
        }
        SharingRpc::RemoveOwnAgent => {
            string_param(params, "roomId")?;
            string_param(params, "agentId")?;
            Ok(())
        }
        SharingRpc::LeaveRoom => {
            string_param(params, "roomId")?;
            Ok(())
        }
    }
}

fn surface_platform() -> SurfacePlatform {
    if cfg!(target_os = "ios") {
        SurfacePlatform::Ios
    } else if cfg!(target_os = "android") {
        SurfacePlatform::Android
    } else {
        SurfacePlatform::Electron
    }
}

pub fn messaging_actor_id_for_user(user_id: &str) -> String {
    let digest = Sha256::digest(user_id.trim().as_bytes());
    let fingerprint = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("human:account:{fingerprint}")
}

fn messaging_peer_actor_id_for_user(user_id: &str) -> String {
    format!("human:platform:{}", user_id.trim())
}

fn deterministic_human_conversation_id(local_user_id: &str, peer_user_id: &str) -> String {
    let mut participant_ids = [
        local_user_id.trim().to_string(),
        peer_user_id.trim().to_string(),
    ];
    participant_ids.sort();
    let mut digest = Sha256::new();
    for participant_id in participant_ids {
        digest.update((participant_id.len() as u64).to_be_bytes());
        digest.update(participant_id.as_bytes());
    }
    format!("human-direct-{:x}", digest.finalize())
}

fn human_messaging_actor(actor_id: &str, display_name: &str) -> Value {
    json!({
        "id": actor_id,
        "kind": "human",
        "displayName": display_name,
        "username": Value::Null,
        "avatarUrl": Value::Null,
        "bio": Value::Null,
        "capabilities": ["messages", "groups", "channels", "calls", "payments", "miniApps"],
        "presence": {
            "status": "online",
            "lastSeenAtMs": app_host_now_ms(),
            "statusText": Value::Null,
        },
        "verified": false,
    })
}

fn find_conversation_by_id(sync_result: &Value, conversation_id: &str) -> Option<Value> {
    let envelopes = sync_result.get("envelopes")?.as_array()?;
    for envelope in envelopes {
        let event = envelope.get("event")?;
        if event.get("type")?.as_str()? != "syncBatch" {
            continue;
        }
        for conversation in event.get("conversations")?.as_array()? {
            if conversation.get("id").and_then(Value::as_str) == Some(conversation_id) {
                return Some(conversation.clone());
            }
        }
    }
    None
}

fn find_direct_conversation_for_actors(
    sync_result: &Value,
    local_actor_id: &str,
    peer_actor_id: &str,
) -> Option<Value> {
    let envelopes = sync_result.get("envelopes")?.as_array()?;
    for envelope in envelopes {
        let event = envelope.get("event")?;
        if event.get("type")?.as_str()? != "syncBatch" {
            continue;
        }
        let conversations = event.get("conversations")?.as_array()?;
        for conversation in conversations {
            if conversation.get("kind").and_then(Value::as_str) != Some("direct") {
                continue;
            }
            let participants = conversation.get("participants")?.as_array()?;
            let mut ids = participants
                .iter()
                .filter_map(|participant| participant.get("actorId").and_then(Value::as_str))
                .collect::<Vec<_>>();
            ids.sort_unstable();
            ids.dedup();
            let mut expected = vec![local_actor_id, peer_actor_id];
            expected.sort_unstable();
            expected.dedup();
            if ids == expected {
                return Some(conversation.clone());
            }
        }
    }
    None
}

fn app_host_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn host_platform() -> &'static str {
    if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "desktop"
    }
}

pub fn default_app_data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("FABUSHI_APP_DATA") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/com.ombhrum.fabushi");
    }
    #[cfg(target_os = "windows")]
    if let Some(data) = std::env::var_os("APPDATA") {
        return PathBuf::from(data).join("Fabushi");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".fabushi")
}

pub fn dispatch_json(host: &AppHost, input: &str) -> String {
    let response = match serde_json::from_str::<HostRequest>(input) {
        Ok(request) => host.dispatch(request),
        Err(error) => HostResponse {
            id: None,
            ok: false,
            result: None,
            error: Some(format!("invalid JSON request: {error}")),
        },
    };
    serde_json::to_string(&response).unwrap_or_else(|error| {
        format!("{{\"ok\":false,\"error\":\"serialization failed: {error}\"}}")
    })
}


fn validate_local_generated_artifact_source_path(raw: &str) -> Result<&str, AppHostError> {
    let path = raw.trim();
    let lowered = path.to_ascii_lowercase();
    if path.is_empty()
        || path.contains("://")
        || lowered.starts_with("file:")
        || lowered.starts_with("http:")
        || lowered.starts_with("https:")
    {
        return Err(AppHostError::InvalidRequest(
            "Agent Mini App source must be a local canonical attachment path".into(),
        ));
    }
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "html" | "htm") {
        return Err(AppHostError::InvalidRequest(
            "Agent Mini App source must be an HTML attachment".into(),
        ));
    }
    Ok(path)
}

fn plugin_account_identity_matches(
    expected_scope: &str,
    expected_generation: &str,
    current_scope: &str,
    current_generation: &str,
) -> bool {
    expected_scope == current_scope && expected_generation == current_generation
}

fn strict_plugin_account_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_expected_plugin_account_identity(
    params: &Value,
) -> Result<Option<(String, String)>, AppHostError> {
    let scope = params.get("expectedAccountScope").and_then(Value::as_str);
    let generation = params
        .get("expectedAccountGeneration")
        .and_then(Value::as_str);
    match (scope, generation) {
        (None, None) => Ok(None),
        (Some(scope), Some(generation))
            if strict_plugin_account_digest(scope)
                && strict_plugin_account_digest(generation) =>
        {
            Ok(Some((scope.to_string(), generation.to_string())))
        }
        (Some(_), Some(_)) => Err(AppHostError::InvalidRequest(
            "expected plugin account identity must contain lowercase 64-byte hex digests".into(),
        )),
        _ => Err(AppHostError::InvalidRequest(
            "expectedAccountScope and expectedAccountGeneration must be supplied together".into(),
        )),
    }
}

fn harden_generated_miniapp_document(html: &str) -> String {
    const POLICY: &str = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data:; font-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'none'; media-src data:; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'\">";
    let lower = html.to_ascii_lowercase();
    if let Some(index) = lower.find("<head>") {
        let insertion = index + "<head>".len();
        let mut result = html.to_string();
        result.insert_str(insertion, POLICY);
        return result;
    }
    if let Some(index) = lower.find("<html>") {
        let insertion = index + "<html>".len();
        let mut result = html.to_string();
        result.insert_str(insertion, &format!("<head>{POLICY}</head>"));
        return result;
    }
    format!("<!doctype html><html><head>{POLICY}</head><body>{html}</body></html>")
}

#[cfg(test)]
mod fabushi_shipping_inference_tests {
    use super::*;

    #[test]
    fn direct_attachment_host_methods_are_owner_scoped_and_bounded() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-app-host-direct-attachment-{}-{}",
            std::process::id(),
            app_host_now_ms()
        ));
        let host = AppHost::new_with_feature_mode(&root, AppHostFeatureMode::Test)
            .expect("create test app host");

        let upload = host.dispatch(HostRequest {
            id: Some(json!(1)),
            method: "feature.attachment.uploadDirect".into(),
            params: json!({
                "agentId": "mahayana-assistant",
                "filename": "notes.txt",
                "mimeType": "text/plain",
                "bytesBase64": base64::engine::general_purpose::STANDARD.encode(b"hello canonical host"),
            }),
        });
        assert!(upload.ok, "{:?}", upload.error);
        let attachment = upload.result.expect("upload result");
        let path = attachment["path"].as_str().expect("attachment path");
        assert!(Path::new(path).is_file());

        let chunk = host.dispatch(HostRequest {
            id: Some(json!(2)),
            method: "feature.attachment.readChunkDirect".into(),
            params: json!({
                "agentId": "mahayana-assistant",
                "path": path,
                "offset": 6,
                "length": 1024 * 1024 * 64_u64,
            }),
        });
        assert!(chunk.ok, "{:?}", chunk.error);
        let chunk = chunk.result.expect("chunk result");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(chunk["bytesBase64"].as_str().expect("chunk base64"))
                .expect("decode chunk"),
            b"canonical host"
        );

        let escaped = host.dispatch(HostRequest {
            id: Some(json!(3)),
            method: "feature.attachment.readTextDirect".into(),
            params: json!({
                "agentId": "mahayana-assistant",
                "path": root.join("outside.txt").to_string_lossy(),
            }),
        });
        assert!(!escaped.ok);

        let wrong_owner = host.dispatch(HostRequest {
            id: Some(json!(4)),
            method: "feature.attachment.readTextDirect".into(),
            params: json!({
                "agentId": "unknown-agent",
                "path": path,
            }),
        });
        assert!(!wrong_owner.ok);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn native_agent_create_and_list_preserve_plugin_auth_purpose() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-app-host-plugin-auth-agent-{}-{}",
            std::process::id(),
            app_host_now_ms()
        ));
        let host = AppHost::new_with_feature_mode(&root, AppHostFeatureMode::Test)
            .expect("create test app host");
        let created = host.dispatch(HostRequest {
            id: Some(json!(1)),
            method: "createAgent".into(),
            params: json!({
                "name": "Plugin Setup",
                "description": "Sets up git credentials.",
                "purpose": "plugin-auth",
                "isIntroductionSuppressed": true,
            }),
        });
        assert!(created.ok, "{:?}", created.error);
        let created = created.result.expect("create result");
        let agent = created.get("agent").expect("created agent");
        assert_eq!(agent["purpose"], "plugin-auth");
        assert_eq!(agent["isIntroductionSuppressed"], true);

        let listed = host.dispatch(HostRequest {
            id: Some(json!(2)),
            method: "listAgents".into(),
            params: json!({}),
        });
        assert!(listed.ok, "{:?}", listed.error);
        let agents = listed.result
            .and_then(|value| value.get("agents").cloned())
            .and_then(|value| value.as_array().cloned())
            .expect("agent roster");
        assert!(agents.iter().any(|candidate| {
            candidate["id"] == agent["id"]
                && candidate["purpose"] == "plugin-auth"
                && candidate["isIntroductionSuppressed"] == true
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn git_authentication_failure_classifier_is_fail_closed() {
        assert!(is_git_authentication_failure(
            "fatal: Authentication failed for 'https://github.com/private/repo'"
        ));
        assert!(is_git_authentication_failure(
            "fatal: could not read Username for 'https://github.com'"
        ));
        assert!(!is_git_authentication_failure(
            "connection reset while downloading public release artifact"
        ));
        assert!(!is_git_authentication_failure("plugin archive checksum mismatch"));
    }

    #[test]
    fn plugin_account_scope_is_stable_irreversible_and_disjoint() {
        let a1 = stable_plugin_account_scope("account-a");
        let a2 = stable_plugin_account_scope(" account-a ");
        let b = stable_plugin_account_scope("account-b");
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        assert_eq!(a1.len(), 64);
        assert!(!a1.contains("account-a"));
    }

    #[test]
    fn generated_miniapp_artifact_source_is_local_html_only() {
        assert_eq!(
            validate_local_generated_artifact_source_path("/tmp/agent/attachments/generated.html")
                .expect("canonical html"),
            "/tmp/agent/attachments/generated.html"
        );
        assert!(validate_local_generated_artifact_source_path("https://evil.invalid/app.html").is_err());
        assert!(validate_local_generated_artifact_source_path("file:///tmp/app.html").is_err());
        assert!(validate_local_generated_artifact_source_path("/tmp/agent/attachments/app.js").is_err());
    }

    #[test]
    fn generated_miniapp_artifact_hardening_matches_preview_security_contract() {
        let raw = "<html><head><title>Agent</title></head><body><script>fetch('https://evil.invalid')</script></body></html>";
        let hardened = harden_generated_miniapp_document(raw);
        assert!(hardened.contains("default-src 'none'"));
        assert!(hardened.contains("connect-src 'none'"));
        assert!(hardened.contains("frame-src 'none'"));
        assert_eq!(hardened.matches("Content-Security-Policy").count(), 1);
        assert!(hardened.contains("fetch('https://evil.invalid')"));
    }

    #[test]
    fn local_fabapp_authority_identifiers_fail_closed() {
        assert!(strict_hex_identifier(&"a".repeat(32), 32, "approval").is_ok());
        assert!(strict_hex_identifier("../escape", 32, "approval").is_err());
        assert!(strict_lower_hex_digest(&"b".repeat(64), "digest").is_ok());
        assert!(strict_lower_hex_digest(&"B".repeat(64), "digest").is_err());
        assert!(strict_ascii_identifier("local-author.v1", "keyId").is_ok());
        assert!(strict_ascii_identifier("../../key", "keyId").is_err());
    }

    #[test]
    fn local_fabapp_commit_records_fail_closed_after_cancel_or_stale_generation() {
        let account = PluginAccountContext {
            scope: "a".repeat(64),
            generation: "b".repeat(64),
            root: PathBuf::new(),
        };
        let approval = LocalFabappHostApproval {
            schema_version: 1,
            account_scope: account.scope.clone(),
            account_generation: account.generation.clone(),
            transaction_id: "transaction-1".into(),
            reserved_app_id: "private.fabushi.generated-test".into(),
            publisher_id: "local.fabushi".into(),
            draft_digest: "c".repeat(64),
            user_approval_id: "approval-1".into(),
            key_id: "local-author.v1".into(),
            public_key_sha256: "d".repeat(64),
            state: "approved".into(),
        };
        let transaction = LocalFabappDraftTransaction {
            schema_version: 1,
            account_scope: account.scope.clone(),
            account_generation: account.generation.clone(),
            transaction_id: approval.transaction_id.clone(),
            reserved_app_id: approval.reserved_app_id.clone(),
            publisher_id: approval.publisher_id.clone(),
            state: "approved".into(),
            draft_digest: Some(approval.draft_digest.clone()),
            approval_id: Some(approval.user_approval_id.clone()),
        };

        assert!(local_fabapp_commit_records_match(
            &account,
            &approval,
            Some(&approval),
            Some(&transaction),
        ));

        let mut cancelled_approval = approval.clone();
        cancelled_approval.state = "cancelled".into();
        assert!(!local_fabapp_commit_records_match(
            &account,
            &approval,
            Some(&cancelled_approval),
            Some(&transaction),
        ));

        let mut cancelled_transaction = transaction.clone();
        cancelled_transaction.state = "cancelled".into();
        assert!(!local_fabapp_commit_records_match(
            &account,
            &approval,
            Some(&approval),
            Some(&cancelled_transaction),
        ));

        let stale_account = PluginAccountContext {
            generation: "e".repeat(64),
            ..account.clone()
        };
        assert!(!local_fabapp_commit_records_match(
            &stale_account,
            &approval,
            Some(&approval),
            Some(&transaction),
        ));

        let mut stale_digest = transaction.clone();
        stale_digest.draft_digest = Some("f".repeat(64));
        assert!(!local_fabapp_commit_records_match(
            &account,
            &approval,
            Some(&approval),
            Some(&stale_digest),
        ));
    }

    #[test]
    fn expected_plugin_account_identity_is_paired_strict_and_generation_bound() {
        assert_eq!(
            parse_expected_plugin_account_identity(&json!({})).unwrap(),
            None
        );
        assert!(parse_expected_plugin_account_identity(&json!({
            "expectedAccountScope": "a".repeat(64)
        }))
        .is_err());
        assert!(parse_expected_plugin_account_identity(&json!({
            "expectedAccountScope": "A".repeat(64),
            "expectedAccountGeneration": "b".repeat(64),
        }))
        .is_err());

        let expected = parse_expected_plugin_account_identity(&json!({
            "expectedAccountScope": "a".repeat(64),
            "expectedAccountGeneration": "b".repeat(64),
        }))
        .unwrap()
        .expect("expected account identity");
        assert!(plugin_account_identity_matches(
            &expected.0,
            &expected.1,
            &"a".repeat(64),
            &"b".repeat(64),
        ));
        assert!(!plugin_account_identity_matches(
            &expected.0,
            &expected.1,
            &"a".repeat(64),
            &"c".repeat(64),
        ));
        assert!(!plugin_account_identity_matches(
            &expected.0,
            &expected.1,
            &"d".repeat(64),
            &"b".repeat(64),
        ));
    }

    #[test]
    fn captured_plugin_account_must_match_expected_generation_before_reads() {
        let account = PluginAccountContext {
            scope: "a".repeat(64),
            generation: "b".repeat(64),
            root: PathBuf::from("/tmp/unused"),
        };
        let expected = ("a".repeat(64), "b".repeat(64));
        assert!(plugin_account_identity_matches(
            &expected.0,
            &expected.1,
            &account.scope,
            &account.generation,
        ));
        assert!(!plugin_account_identity_matches(
            &expected.0,
            &"c".repeat(64),
            &account.scope,
            &account.generation,
        ));
    }

    #[test]
    fn plugin_account_generation_fences_same_account_reauthentication() {
        let scope_a = stable_plugin_account_scope("account-a");
        assert_eq!(scope_a, stable_plugin_account_scope("account-a"));
        let generation_1 = stable_plugin_account_generation("account-a", "session-1");
        let generation_2 = stable_plugin_account_generation("account-a", "session-2");
        let other_account = stable_plugin_account_generation("account-b", "session-1");
        assert_ne!(generation_1, generation_2);
        assert_ne!(generation_1, other_account);
        assert_eq!(generation_1.len(), 64);
        assert!(!generation_1.contains("session-1"));
    }

    #[test]
    fn plugin_runtime_binding_prunes_account_and_generation_changes() {
        let scope_a = stable_plugin_account_scope("account-a");
        let scope_b = stable_plugin_account_scope("account-b");
        let generation_1 = stable_plugin_account_generation("account-a", "session-1");
        let generation_2 = stable_plugin_account_generation("account-a", "session-2");
        let mut bindings = BTreeMap::new();
        bindings.insert(
            "plugin-a".into(),
            PluginRuntimeAccountBinding {
                account_scope: scope_a.clone(),
                account_generation: generation_1.clone(),
            },
        );
        assert!(stale_plugin_runtime_ids(
            &bindings,
            Some(&(scope_a.clone(), generation_1.clone()))
        ).is_empty());
        assert_eq!(
            stale_plugin_runtime_ids(
                &bindings,
                Some(&(scope_a.clone(), generation_2))
            ),
            vec!["plugin-a".to_string()]
        );
        assert_eq!(
            stale_plugin_runtime_ids(
                &bindings,
                Some(&(scope_b, generation_1))
            ),
            vec!["plugin-a".to_string()]
        );
        assert_eq!(
            stale_plugin_runtime_ids(&bindings, None),
            vec!["plugin-a".to_string()]
        );
    }

    #[test]
    fn legacy_plugin_root_migrates_only_with_matching_owner_proof_and_journal() {
        let base = std::env::temp_dir().join(format!(
            "fabushi-plugin-migration-{}",
            uuid::Uuid::new_v4()
        ));
        let account_a = stable_plugin_account_scope("account-a");
        let account_b = stable_plugin_account_scope("account-b");
        let root_a = base.join("accounts").join(&account_a);
        std::fs::create_dir_all(base.join("legacy-plugin")).unwrap();
        std::fs::write(base.join("permissions.json"), b"{}").unwrap();
        std::fs::create_dir_all(&root_a).unwrap();

        migrate_legacy_plugin_root_if_proven(&base, &account_a, &root_a).unwrap();
        assert!(base.join("legacy-plugin").is_dir());
        assert!(!root_a.join("legacy-plugin").exists());

        atomic_write_json(
            &base.join("legacy-owner.json"),
            &LegacyPluginOwnerProof {
                schema_version: 1,
                account_scope: account_b,
            },
        )
        .unwrap();
        migrate_legacy_plugin_root_if_proven(&base, &account_a, &root_a).unwrap();
        assert!(base.join("legacy-plugin").is_dir());

        atomic_write_json(
            &base.join("legacy-owner.json"),
            &LegacyPluginOwnerProof {
                schema_version: 1,
                account_scope: account_a.clone(),
            },
        )
        .unwrap();
        migrate_legacy_plugin_root_if_proven(&base, &account_a, &root_a).unwrap();
        assert!(root_a.join("legacy-plugin").is_dir());
        assert!(root_a.join("permissions.json").is_file());
        let journal: LegacyPluginMigrationJournal = serde_json::from_slice(
            &std::fs::read(base.join("migrations").join(format!("{account_a}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(journal.state, "completed");
        migrate_legacy_plugin_root_if_proven(&base, &account_a, &root_a).unwrap();

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn mobile_surfaces_use_embedded_runtime_and_never_spawn_local_processes() {
        for platform in [SurfacePlatform::Ios, SurfacePlatform::Android] {
            assert_eq!(
                runtime_build_profile_for_surface(platform),
                BuildProfile::MobileEmbedded
            );
            assert!(matches!(
                process_execution_for_surface(platform),
                ProcessExecution::Unavailable
            ));
        }
        assert_eq!(
            runtime_build_profile_for_surface(SurfacePlatform::Wasm),
            BuildProfile::WebWasm
        );
        assert!(matches!(
            process_execution_for_surface(SurfacePlatform::Wasm),
            ProcessExecution::Unavailable
        ));
        assert_eq!(
            runtime_build_profile_for_surface(SurfacePlatform::Electron),
            BuildProfile::DesktopFull
        );
    }

    #[test]
    fn production_feature_host_forces_fabushi_inference_owner() {
        assert_eq!(
            effective_inference_provider(AppHostFeatureMode::Production, "openrouter"),
            "fabushi"
        );
        assert_eq!(
            effective_inference_provider(AppHostFeatureMode::Production, "claude-code"),
            "fabushi"
        );
    }

    #[test]
    fn test_feature_host_keeps_explicit_provider_for_contract_coverage() {
        assert_eq!(
            effective_inference_provider(AppHostFeatureMode::Test, "openrouter"),
            "openrouter"
        );
    }

    #[test]
    fn fabushi_model_uses_product_specific_default_and_override() {
        assert_eq!(resolve_fabushi_model(None), "deepseek-chat");
        assert_eq!(
            resolve_fabushi_model(Some("  custom-fabushi-model  ".into())),
            "custom-fabushi-model"
        );
    }

    #[test]
    fn routine_projection_uses_host_owned_agent_scope_and_desktop_shape() {
        let row = routine_projection(AutomationSummary {
            id: "daily".into(),
            agent_id: Some("research".into()),
            name: "Daily research".into(),
            prompt: "Summarize".into(),
            schedule: "@daily".into(),
            trigger: Some(AutomationTrigger::Schedule { schedule: "@daily".into() }),
            enabled: true,
            created_at_ms: 10,
            runs: Vec::new(),
            raised_notices: Vec::new(),
            last_run_at_ms: Some(20),
            next_run_at_ms: Some(30),
        })
        .expect("agent-scoped automation should project");
        assert_eq!(row["agentId"], "research");
        assert_eq!(row["automation"]["id"], "daily");
        assert_eq!(row["automation"]["triggerDescription"], "@daily");
        assert_eq!(row["automation"]["createdAt"], 10);
        assert_eq!(row["automation"]["lastRunAt"], 20);
    }

    #[test]
    fn routine_projection_event_preserves_legacy_and_structured_filters() {
        let row = routine_projection(AutomationSummary {
            id: "regression-triage".into(),
            agent_id: Some("research".into()),
            name: "Regression triage".into(),
            prompt: "Inspect regressions".into(),
            schedule: "event:sentry:issue.regressed".into(),
            trigger: Some(AutomationTrigger::Event {
                source: mahayana_host_protocol::ListenerPlatform::Sentry,
                event: "issue.regressed".into(),
                filter: Some("legacy-web".into()),
                filters: Some(BTreeMap::from([
                    ("projectIds".into(), json!(["web", "api"])),
                    ("resolved".into(), json!(false)),
                ])),
            }),
            enabled: true,
            created_at_ms: 10,
            runs: Vec::new(),
            raised_notices: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        })
        .expect("event automation should project");
        assert_eq!(
            row["automation"]["triggerDescription"],
            "sentry · issue.regressed · legacy-web · projectIds=web, api · resolved=false"
        );
    }

    #[test]
    fn routine_projection_event_preserves_structured_filter_readback_without_legacy_filter() {
        let row = routine_projection(AutomationSummary {
            id: "github-watch".into(),
            agent_id: Some("research".into()),
            name: "GitHub watch".into(),
            prompt: "Inspect matching repository events".into(),
            schedule: "event:github:*".into(),
            trigger: Some(AutomationTrigger::Event {
                source: mahayana_host_protocol::ListenerPlatform::Github,
                event: "*".into(),
                filter: None,
                filters: Some(BTreeMap::from([
                    ("events".into(), json!(["pr-opened", "ci-failed"])),
                    ("repo".into(), json!("owner/repo")),
                ])),
            }),
            enabled: true,
            created_at_ms: 10,
            runs: Vec::new(),
            raised_notices: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        })
        .expect("structured event automation should project");
        assert_eq!(
            row["automation"]["triggerDescription"],
            "github · * · events=pr-opened, ci-failed · repo=owner/repo"
        );
    }

    #[test]
    fn routine_projection_event_omits_empty_filter_values_and_handles_absent_filters() {
        let empty = routine_projection(AutomationSummary {
            id: "slack-empty".into(),
            agent_id: Some("research".into()),
            name: "Slack empty".into(),
            prompt: "Inspect messages".into(),
            schedule: "event:slack:mention".into(),
            trigger: Some(AutomationTrigger::Event {
                source: mahayana_host_protocol::ListenerPlatform::Slack,
                event: "mention".into(),
                filter: Some("   ".into()),
                filters: Some(BTreeMap::from([
                    ("blank".into(), json!("   ")),
                    ("emptyArray".into(), json!([])),
                    ("emptyObject".into(), json!({})),
                    ("nullValue".into(), Value::Null),
                ])),
            }),
            enabled: true,
            created_at_ms: 10,
            runs: Vec::new(),
            raised_notices: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        })
        .expect("empty-filter event automation should project");
        assert_eq!(empty["automation"]["triggerDescription"], "slack · mention");

        let absent = routine_projection(AutomationSummary {
            id: "github-plain".into(),
            agent_id: Some("research".into()),
            name: "GitHub plain".into(),
            prompt: "Inspect events".into(),
            schedule: "event:github:pr-opened".into(),
            trigger: Some(AutomationTrigger::Event {
                source: mahayana_host_protocol::ListenerPlatform::Github,
                event: "pr-opened".into(),
                filter: None,
                filters: None,
            }),
            enabled: true,
            created_at_ms: 10,
            runs: Vec::new(),
            raised_notices: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        })
        .expect("plain event automation should project");
        assert_eq!(
            absent["automation"]["triggerDescription"],
            "github · pr-opened"
        );
    }

    #[test]
    fn routine_projection_group_describes_each_rich_trigger_member() {
        let row = routine_projection(AutomationSummary {
            id: "group".into(),
            agent_id: Some("research".into()),
            name: "Group".into(),
            prompt: "Handle grouped triggers".into(),
            schedule: "event:group".into(),
            trigger: Some(AutomationTrigger::Group {
                listeners: vec![
                    AutomationTrigger::Schedule {
                        schedule: "@daily".into(),
                    },
                    AutomationTrigger::Event {
                        source: mahayana_host_protocol::ListenerPlatform::Slack,
                        event: "mention".into(),
                        filter: None,
                        filters: Some(BTreeMap::from([(
                            "channel".into(),
                            json!("alerts"),
                        )])),
                    },
                ],
            }),
            enabled: true,
            created_at_ms: 10,
            runs: Vec::new(),
            raised_notices: Vec::new(),
            last_run_at_ms: None,
            next_run_at_ms: None,
        })
        .expect("group automation should project");
        assert_eq!(
            row["automation"]["triggerDescription"],
            "@daily or slack · mention · channel=alerts"
        );
    }

    #[test]
    #[test]
    fn link_metadata_url_validation_matches_host_security_contract() {
        assert!(validate_public_link_url("http://127.0.0.1/").is_err());
        assert!(validate_public_link_url("https://127.0.0.1:444/").is_err());
        assert!(validate_public_link_url("https://127.0.0.1/").is_err());
        let oversized = format!("https://127.0.0.1/{}", "a".repeat(LINK_METADATA_MAX_URL_BYTES));
        assert!(validate_public_link_url(&oversized).is_err());
        assert_eq!(LINK_METADATA_MAX_REDIRECTS, 5);
        assert_eq!(LINK_METADATA_HTML_BYTE_LIMIT, 512 * 1024);
    }

    fn link_metadata_html_parser_extracts_title_and_description() {
        let html = r#"<html><head><title> Example &amp; Docs </title><meta name="description" content="A useful page"></head></html>"#;
        assert_eq!(html_tag_text(html, "title").as_deref(), Some("Example & Docs"));
        assert_eq!(html_meta_content(html, "description").as_deref(), Some("A useful page"));
    }

    #[test]
    fn miniapp_ui_resource_paths_and_mime_fail_closed() {
        assert_eq!(
            safe_ui_relative_path("assets/app.js").unwrap(),
            PathBuf::from("assets/app.js")
        );
        for invalid in ["", "/index.html", "../index.html", "./index.html", "a/../b", r"a\\b"] {
            assert!(safe_ui_relative_path(invalid).is_err(), "{invalid:?}");
        }
        assert_eq!(
            miniapp_resource_mime(Path::new("assets/app.wasm")),
            "application/wasm"
        );
        assert_eq!(
            miniapp_resource_mime(Path::new("assets/data.bin")),
            "application/octet-stream"
        );
    }

    #[test]
    fn fabushi_responses_url_matches_desktop_shipping_contract() {
        assert_eq!(
            normalize_fabushi_responses_url("https://api.ombhrum.com"),
            "https://api.ombhrum.com/codex-deepseek/v1/responses"
        );
        assert_eq!(
            normalize_fabushi_responses_url("https://api.ombhrum.com/codex-deepseek/v1"),
            "https://api.ombhrum.com/codex-deepseek/v1/responses"
        );
        assert_eq!(
            normalize_fabushi_responses_url("https://api.ombhrum.com/codex-deepseek/v1/responses"),
            "https://api.ombhrum.com/codex-deepseek/v1/responses"
        );
    }

    fn assert_required_string_rejected(rpc: SharingRpc, base: Value, field: &str) {
        let mut missing = base.clone();
        missing
            .as_object_mut()
            .expect("sharing test params must be an object")
            .remove(field);
        assert!(validate_sharing_params(rpc, &missing).is_err());

        for invalid in [Value::Null, json!(7), json!("   ")] {
            let mut params = base.clone();
            params
                .as_object_mut()
                .expect("sharing test params must be an object")
                .insert(field.to_string(), invalid);
            assert!(validate_sharing_params(rpc, &params).is_err());
        }
    }

    #[test]
    fn agent_box_rpc_required_identity_fields_fail_closed() {
        assert!(string_param(&json!({}), "agentId").is_err());
        assert!(string_param(&json!({"agentId": "agent-a"}), "agentId").is_ok());
        assert!(string_param(&json!({"agentId": "agent-a"}), "trigger").is_err());
        assert!(
            string_param(
                &json!({"agentId": "agent-a", "trigger": "scope-changed"}),
                "trigger"
            )
            .is_ok()
        );
    }

    #[test]
    fn shared_room_rpc_surface_is_closed_to_six_shipping_methods() {
        assert_eq!(
            SHARING_RPC_METHODS,
            [
                "sharing.state",
                "sharing.createRoomInvite",
                "sharing.respondToRoomJoinRequest",
                "sharing.addOwnAgent",
                "sharing.removeOwnAgent",
                "sharing.leaveRoom",
            ]
        );
        for method in SHARING_RPC_METHODS {
            assert!(sharing_rpc(method).is_ok(), "{method} must remain routable");
        }
        for method in [
            "sharing.getSharingState",
            "sharing.addOwnAgentToSharedRoom",
            "sharing.removeOwnAgentFromSharedRoom",
            "sharing.leaveSharedRoom",
            "sharing.deleteRoom",
        ] {
            assert!(sharing_rpc(method).is_err(), "{method} must fail closed");
        }
    }

    #[test]
    fn shared_room_required_identities_and_reply_fail_closed() {
        assert_required_string_rejected(
            SharingRpc::CreateRoomInvite,
            json!({"roomId": "room-1"}),
            "roomId",
        );
        assert_required_string_rejected(
            SharingRpc::RespondToRoomJoinRequest,
            json!({"requestId": "req-1", "isApproved": true}),
            "requestId",
        );
        for field in ["roomId", "agentId", "agentName"] {
            assert_required_string_rejected(
                SharingRpc::AddOwnAgent,
                json!({"roomId": "room-1", "agentId": "agent-1", "agentName": "Agent"}),
                field,
            );
        }
        assert_required_string_rejected(
            SharingRpc::RemoveOwnAgent,
            json!({"roomId": "room-1", "agentId": "agent-1"}),
            "agentId",
        );
        assert_required_string_rejected(
            SharingRpc::LeaveRoom,
            json!({"roomId": "room-1"}),
            "roomId",
        );

        for params in [
            json!({"requestId": "req-1"}),
            json!({"requestId": "req-1", "isApproved": "true"}),
            json!({"requestId": "req-1", "isApproved": null}),
        ] {
            assert!(validate_sharing_params(
                SharingRpc::RespondToRoomJoinRequest,
                &params
            )
            .is_err());
        }
        assert!(validate_sharing_params(
            SharingRpc::RespondToRoomJoinRequest,
            &json!({"requestId": "req-1", "isApproved": false})
        )
        .is_ok());
    }

    #[test]
    fn human_call_identity_projection_is_stable_and_order_independent() {
        let alice_actor = messaging_actor_id_for_user("alice");
        assert_eq!(alice_actor, messaging_actor_id_for_user(" alice "));
        assert!(alice_actor.starts_with("human:account:"));
        assert_eq!(alice_actor.len(), "human:account:".len() + 32);

        assert_eq!(
            messaging_peer_actor_id_for_user(" 42 "),
            "human:platform:42"
        );

        let forward = deterministic_human_conversation_id("alice", "bob");
        let reverse = deterministic_human_conversation_id("bob", "alice");
        assert_eq!(forward, reverse);
        assert!(forward.starts_with("human-direct-"));
        assert_eq!(forward.len(), "human-direct-".len() + 64);
    }

    #[test]
    fn human_call_conversation_lookup_reuses_exact_existing_direct_participants() {
        let local = messaging_actor_id_for_user("alice");
        let peer = messaging_peer_actor_id_for_user("bob");
        let other = messaging_peer_actor_id_for_user("carol");
        let sync = json!({
            "envelopes": [{
                "event": {
                    "type": "syncBatch",
                    "conversations": [
                        {
                            "id": "wrong",
                            "kind": "direct",
                            "participants": [
                                {"actorId": local},
                                {"actorId": other}
                            ]
                        },
                        {
                            "id": "existing",
                            "kind": "direct",
                            "participants": [
                                {"actorId": peer},
                                {"actorId": local}
                            ]
                        }
                    ]
                }
            }]
        });
        let found = find_direct_conversation_for_actors(&sync, &local, &peer)
            .expect("existing direct conversation");
        assert_eq!(found["id"], "existing");
        assert!(find_direct_conversation_for_actors(&sync, &local, "missing").is_none());
    }

}
