use base64::Engine as _;
use mahayana_core::{ModelProviderMode, RuntimeConfig};
use mahayana_feature_host::FeatureHostController;
use mahayana_host::HostCreateConfig;
use mahayana_host_protocol::{
    ApprovalResolution, AutomationSummary, AutomationTrigger, FeatureCommand, HostConfig, HostMode,
    SurfacePlatform,
};
use mahayana_js_runtime::{DeepSeekJsHost, scan_package_compatibility};
use mahayana_native_engine::ProcessExecution;
use mahayana_plugin_runtime::{
    ExternalReleaseManifest, InstalledPluginPointer, PermissionManager, PluginInstaller,
    PluginState,
};
use mahayana_product::MahayanaProductClient;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read as _;
use std::net::{IpAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

pub struct AppHost {
    app_data_dir: PathBuf,
    feature_mode: AppHostFeatureMode,
    product: MahayanaProductClient,
    js: Mutex<DeepSeekJsHost>,
    feature: FeatureHostController,
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
        let feature = create_feature_host(&app_data_dir, feature_mode, storage_passphrase.clone())?;
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
            feature,
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
        match self.handle(&request.method, request.params) {
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
        for _ in 0..=3 {
            let response = match agent
                .get(current.as_str())
                .set("User-Agent", "Fabushi-iOS-LinkMetadata/1.0")
                .set("Accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1")
                .call()
            {
                Ok(response) => response,
                Err(ureq::Error::Status(status, response)) if (300..400).contains(&status) => {
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
                .take(1_048_576)
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
            "feature.marketplace.add" => self.marketplace_add(params),
            "feature.plugin.install" => self.install_plugin(params),
            "feature.plugin.uninstall" => self.uninstall_plugin(params),
            "feature.plugin.rollback" => self.rollback_plugin(params),
            "feature.plugin.active" => self.active_plugin(params),
            "feature.plugin.listInstalled" => self.list_installed_plugins(),
            "feature.plugin.uiDocument" => self.plugin_ui_document(params),
            "feature.messaging.execute" => self.feature_messaging_execute(params),
            "feature.messaging.blob.read" => self.feature_messaging_blob_read(params),
            "feature.messaging.access.issue" => self.feature_messaging_access_issue(params),
            other => Err(AppHostError::InvalidRequest(format!(
                "unknown feature method {other}"
            ))),
        }
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
        let marketplace_platform = match requested_platform {
            "ios" | "android" => "mobile",
            other => other,
        };
        self.product
            .marketplace_browse(query, Some(marketplace_platform))
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn marketplace_release(&self, params: Value) -> Result<Value, AppHostError> {
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
        self.product
            .marketplace_release_metadata(plugin_id, version)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn marketplace_add(&self, params: Value) -> Result<Value, AppHostError> {
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
        self.product
            .marketplace_add(plugin_id, marketplace_platform)
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn plugin_root(&self) -> PathBuf {
        self.app_data_dir.join("plugins")
    }

    fn permission_store(&self) -> PathBuf {
        self.plugin_root().join("permissions.json")
    }

    fn installer(&self) -> Result<PluginInstaller, AppHostError> {
        PluginInstaller::new(self.plugin_root())
            .map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn install_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let release_value = params
            .get("release")
            .cloned()
            .ok_or_else(|| AppHostError::InvalidRequest("release is required".into()))?;
        let release: ExternalReleaseManifest = serde_json::from_value(release_value)
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
            let plugin_root = self.plugin_root().join(&release.plugin_id);
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
        let installer = self.installer()?;
        let pointer = installer
            .install(&release, platform, preferred)
            .or_else(|error| {
                if matches!(platform, "ios" | "android") {
                    installer.install(&release, "mobile", preferred)
                } else {
                    Err(error)
                }
            })
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(pointer).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn uninstall_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        if let Ok(mut host) = self.js.lock() {
            let _ = host.disable_plugin(plugin_id);
        }
        let removed = self
            .installer()?
            .uninstall(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let permissions_removed = PermissionManager::load(self.permission_store())
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .remove_plugin(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(json!({
            "pluginId": plugin_id,
            "removed": removed,
            "permissionsRemoved": permissions_removed
        }))
    }

    fn active_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let pointer = self
            .installer()?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(pointer).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn rollback_plugin(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let pointer = self
            .installer()?
            .rollback(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(pointer).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn list_installed_plugins(&self) -> Result<Value, AppHostError> {
        let root = self.plugin_root();
        let installer = self.installer()?;
        let mut plugins = Vec::new();
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
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
        Ok(json!({"plugins": plugins}))
    }

    fn plugin_permissions(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let pointer = self
            .installer()?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        let mut manager = PermissionManager::load(self.permission_store())
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        manager
            .retain_requested(plugin_id, &pointer.requested_permissions)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
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
        let plugin_id = string_param(&params, "pluginId")?;
        let permission = string_param(&params, "permission")?;
        let pointer = self
            .installer()?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        let mut manager = PermissionManager::load(self.permission_store())
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if grant {
            manager
                .grant(plugin_id, &pointer.requested_permissions, permission)
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
        } else {
            manager
                .revoke(plugin_id, permission)
                .map_err(|error| AppHostError::Operation(error.to_string()))?;
        }
        self.plugin_permissions(json!({"pluginId": plugin_id}))
    }

    fn plugin_compatibility(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let pointer = self
            .installer()?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        let report = scan_package_compatibility(Path::new(&pointer.installed_path))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(report).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn plugin_ui_document(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        let pointer = self
            .installer()?
            .active(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        let root = std::fs::canonicalize(&pointer.installed_path)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        let requested_entry = pointer
            .entry
            .as_deref()
            .map(|value| value.trim_start_matches("./"));
        let entry = requested_entry
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
            })?;
        let relative = PathBuf::from(&entry);
        if relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        }) {
            return Err(AppHostError::InvalidRequest(
                "plugin UI entry escaped installed root".into(),
            ));
        }
        let document = std::fs::canonicalize(root.join(relative))
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if !document.starts_with(&root) {
            return Err(AppHostError::InvalidRequest(
                "plugin UI entry escaped installed root".into(),
            ));
        }
        let html = std::fs::read_to_string(document)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        Ok(json!({"pluginId": plugin_id, "html": html}))
    }

    fn start_runtime(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?.to_string();
        let pointer = self
            .installer()?
            .active(&plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .ok_or_else(|| {
                AppHostError::Operation(format!("plugin {plugin_id} is not installed"))
            })?;
        if !matches!(
            pointer.runtime.as_str(),
            "deepseek-js" | "javascript" | "cordis-js"
        ) {
            return Err(AppHostError::Operation(format!(
                "runtime {} is not supported by the portable JS host",
                pointer.runtime
            )));
        }
        let root = PathBuf::from(&pointer.installed_path);
        let entry = discover_js_entry(&pointer.entry, &root)?;
        let report = scan_package_compatibility(&root)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        if !report.portable_compatible {
            return Err(AppHostError::Operation(report.blockers().join("; ")));
        }
        let grants = PermissionManager::load(self.permission_store())
            .map_err(|error| AppHostError::Operation(error.to_string()))?
            .grants_for(&plugin_id);
        let config = params.get("config").cloned().unwrap_or_else(|| json!({}));
        let mut host = self
            .js
            .lock()
            .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?;
        let state = host
            .register_plugin_with_grants(&plugin_id, &root, &entry, &config, &grants)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
        serde_json::to_value(state).map_err(|error| AppHostError::Operation(error.to_string()))
    }

    fn stop_runtime(&self, params: Value) -> Result<Value, AppHostError> {
        let plugin_id = string_param(&params, "pluginId")?;
        self.js
            .lock()
            .map_err(|_| AppHostError::Operation("JavaScript host lock poisoned".into()))?
            .disable_plugin(plugin_id)
            .map_err(|error| AppHostError::Operation(error.to_string()))?;
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

    /// Deterministically settle the canonical feature Host before the process-local
    /// app-host owner is released. FeatureHostController::close owns operation
    /// interruption and pending approval/session cleanup; this layer only exposes
    /// that existing owner to composition roots that need ordered shutdown.
    pub fn close(&self) -> Result<(), AppHostError> {
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
                .plugin_root()
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
        host.call_tool_json(name, &arguments)
            .map_err(|error| AppHostError::Operation(error.to_string()))
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

fn create_feature_host(
    app_data_dir: &Path,
    feature_mode: AppHostFeatureMode,
    storage_passphrase: Option<String>,
) -> Result<FeatureHostController, AppHostError> {
    let root = feature_host_root(app_data_dir);
    std::fs::create_dir_all(&root).map_err(|error| AppHostError::Operation(error.to_string()))?;
    let configured_provider =
        std::env::var("MAHAYANA_INFERENCE_PROVIDER").unwrap_or_else(|_| "fabushi".into());
    let provider = effective_inference_provider(feature_mode, &configured_provider);
    let mut runtime = RuntimeConfig {
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
        process_execution: if std::env::var("MAHAYANA_SANDBOX_RUNTIME").as_deref()
            == Ok("local-docker")
        {
            ProcessExecution::LocalDocker {
                docker_path: std::env::var_os("MAHAYANA_DOCKER_BIN")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("docker")),
                image: std::env::var("MAHAYANA_DOCKER_IMAGE").unwrap_or_default(),
            }
        } else {
            ProcessExecution::Host
        },
        ..HostCreateConfig::default()
    };
    FeatureHostController::create_with_host_config(
        HostConfig {
            profile_id: "default".to_string(),
            mode: feature_mode.into(),
        },
        surface_platform(),
        host_config,
    )
    .map_err(|error| {
        AppHostError::Operation(format!("feature host initialization failed: {error}"))
    })
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

fn validate_public_link_url(raw: &str) -> Result<url::Url, AppHostError> {
    let parsed = url::Url::parse(raw)
        .map_err(|error| AppHostError::InvalidRequest(format!("invalid link URL: {error}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppHostError::InvalidRequest(
            "link metadata only supports HTTP(S) URLs".into(),
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
    let port = parsed.port_or_known_default().ok_or_else(|| {
        AppHostError::InvalidRequest("link URL has no usable port".into())
    })?;
    let addresses = (host, port)
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


#[cfg(test)]
mod fabushi_shipping_inference_tests {
    use super::*;

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
    fn link_metadata_html_parser_extracts_title_and_description() {
        let html = r#"<html><head><title> Example &amp; Docs </title><meta name="description" content="A useful page"></head></html>"#;
        assert_eq!(html_tag_text(html, "title").as_deref(), Some("Example & Docs"));
        assert_eq!(html_meta_content(html, "description").as_deref(), Some("A useful page"));
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
