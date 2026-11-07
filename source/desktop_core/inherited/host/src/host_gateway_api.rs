use std::collections::{HashMap, VecDeque};

use serde_json::Value;

pub const HOST_CAPABILITIES: [&str; 2] = ["orderedReplicasV1", "sendAcceptanceV1"];
pub const CREATE_AGENT_NONCE_LEDGER_CAP: usize = 64;
pub const DISABLE_SEND_ACCEPT_RETURN_ENV: &str = "SAND_DISABLE_SEND_ACCEPT_RETURN";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostGatewayOwner {
    TranscriptManager,
    AutoReview,
    LocalToolPermission,
    AgentLifecycle,
    Session,
    HostControl,
    FeatureGate,
    CrossUserSharing,
    Automations,
    Workflow,
    ManagedSetup,
    Mcp,
    ForeverBox,
    CloudAgents,
    CloudWorkers,
    BoxStore,
    HostUpgrade,
    TeachRecording,
    Trays,
    Attachments,
    Settings,
    WebAuthn,
    Secrets,
    DevicePairing,
}

pub const OPENCLAW_GATEWAY_METHODS: &[&str] = &[
    "device.pair.request",
    "device.pair.status",
    "device.pair.approve",
    "device.pair.reject",
    "device.pair.list",
    "device.pair.revoke",
    "device.pair.rotateToken",
    "device.pair.setupCode",
    "node.register",
    "node.invoke",
    "node.status",
    "node.list",
    "chat.send",
    "chat.abort",
    "sessions.list",
    "sessions.get",
    "sessions.create",
    "sessions.patch",
    "sessions.delete",
    "sessions.providerReview.continue",
    "cron.list",
    "cron.trigger",
    "config.get",
    "config.set",
    "system.info",
];

pub fn openclaw_gateway_owner(method: &str) -> Option<HostGatewayOwner> {
    match method {
        "device.pair.request"
        | "device.pair.status"
        | "device.pair.approve"
        | "device.pair.reject"
        | "device.pair.list"
        | "device.pair.revoke"
        | "device.pair.rotateToken"
        | "device.pair.setupCode"
        | "node.register"
        | "node.status"
        | "node.list" => Some(HostGatewayOwner::DevicePairing),
        "node.invoke" | "system.info" => Some(HostGatewayOwner::HostControl),
        "chat.send" | "chat.abort" | "sessions.list" | "sessions.get" => {
            Some(HostGatewayOwner::TranscriptManager)
        }
        "sessions.create" | "sessions.delete" => Some(HostGatewayOwner::AgentLifecycle),
        "sessions.patch" | "sessions.providerReview.continue" => {
            Some(HostGatewayOwner::TranscriptManager)
        }
        "cron.list" | "cron.trigger" => Some(HostGatewayOwner::Automations),
        "config.get" | "config.set" => Some(HostGatewayOwner::Settings),
        _ => None,
    }
}

macro_rules! define_host_gateway_registry {
    (
        $(($method:literal, $owner:ident)),+ $(,)?;
        native $(($native:literal, $native_owner:ident)),+ $(,)?;
        cloud_workers $(($cw:literal, $cw_owner:ident)),+ $(,)?
    ) => {
        /// Frozen Grok 0.18 public Host gateway surface. Keep this list as the
        /// single public-method registry; protocol exposure and ownership
        /// contracts consume it instead of maintaining parallel string tables.
        pub const FROZEN_HOST_GATEWAY_METHODS: &[&str] = &[
            $($method),+
        ];
        pub const FABUSHI_NATIVE_MINIAPP_GATEWAY_METHODS: &[&str] = &[
            $($native),+
        ];
        pub const FABUSHI_CLOUD_WORKERS_GATEWAY_METHODS: &[&str] = &[
            $($cw),+
        ];

        pub fn host_gateway_owner(method: &str) -> Option<HostGatewayOwner> {
            if let Some(owner) = openclaw_gateway_owner(method) {
                return Some(owner);
            }
            match method {
                $($method => Some(HostGatewayOwner::$owner),)+
                $($native => Some(HostGatewayOwner::$native_owner),)+
                $($cw => Some(HostGatewayOwner::$cw_owner),)+
                _ => None,
            }
        }
    };
}

define_host_gateway_registry!(
    ("getTranscript", TranscriptManager),
    ("getAgentTranscript", TranscriptManager),
    ("getAgentTranscriptPage", TranscriptManager),
    ("getAgentTranscriptWindow", TranscriptManager),
    ("getAgentTranscriptTail", TranscriptManager),
    ("getAgentThread", TranscriptManager),
    ("sendPrompt", TranscriptManager),
    ("promptAcceptanceStatus", TranscriptManager),
    ("respondToWidget", TranscriptManager),
    ("resolveAutoReviewApproval", AutoReview),
    ("resolveLocalToolPermission", LocalToolPermission),
    ("dismissWidget", TranscriptManager),
    ("submitSecret", TranscriptManager),
    ("reactToMessage", TranscriptManager),
    ("appendConnectorCard", TranscriptManager),
    ("listAgents", TranscriptManager),
    ("countAgents", TranscriptManager),
    ("searchAgents", TranscriptManager),
    ("searchMedia", TranscriptManager),
    ("createAgent", AgentLifecycle),
    ("kickstartAgent", AgentLifecycle),
    ("requestDiskSaverAudit", HostControl),
    ("createGroup", AgentLifecycle),
    ("setGroupMembers", AgentLifecycle),
    ("updateAgent", AgentLifecycle),
    ("deleteAgent", AgentLifecycle),
    ("deleteAgents", AgentLifecycle),
    ("duplicateAgent", AgentLifecycle),
    ("setAgentUnread", Session),
    ("setAgentNotificationsEnabled", Session),
    ("setAgentNotifyOnUpdates", Session),
    ("setAgentHiddenFromSidebar", Session),
    ("openAgent", Session),
    ("openAgentWindowed", Session),
    ("openAgentTail", Session),
    ("setWindowFocused", Session),
    ("getAgentMemories", TranscriptManager),
    ("deleteAgentMemory", TranscriptManager),
    ("clearAgentMemories", TranscriptManager),
    ("getAgentAutomations", Automations),
    ("listAllAutomations", Automations),
    ("isAgentNetworkEnabled", FeatureGate),
    ("isGlobalSearchEnabled", FeatureGate),
    ("isEgressTunnelAvailable", FeatureGate),
    ("getSharingState", CrossUserSharing),
    ("createRoomFromAgent", CrossUserSharing),
    ("createRoomInvite", CrossUserSharing),
    ("joinSharedRoom", CrossUserSharing),
    ("respondToRoomJoinRequest", CrossUserSharing),
    ("createSharedRoom", CrossUserSharing),
    ("addOwnAgentToSharedRoom", CrossUserSharing),
    ("removeOwnAgentFromSharedRoom", CrossUserSharing),
    ("setSharedRoomTyping", CrossUserSharing),
    ("leaveSharedRoom", CrossUserSharing),
    ("setAgentAutomationEnabled", Automations),
    ("createAgentAutomation", Automations),
    ("updateAgentAutomation", Automations),
    ("deleteAgentAutomation", Automations),
    ("runAgentAutomationNow", Automations),
    ("broadcastToAgents", HostControl),
    ("getAgentWorkflows", Workflow),
    ("createAgentWorkflow", Workflow),
    ("updateAgentWorkflow", Workflow),
    ("setAgentWorkflowEnabled", Workflow),
    ("deleteAgentWorkflow", Workflow),
    ("runAgentWorkflowNow", Workflow),
    ("importAgentWorkflowText", Workflow),
    ("importAgentWorkflowUrl", Workflow),
    ("portAgentLocalSkills", Workflow),
    ("getConversationOutline", TranscriptManager),
    ("skillsCatalog", ManagedSetup),
    ("syncPluginSkills", Mcp),
    ("getPluginSyncStatus", Mcp),
    ("getSkillPublishTargets", Mcp),
    ("publishSkill", Mcp),
    ("resyncPublishedSkill", Mcp),
    ("unpublishSkill", Mcp),
    ("getAgentChannels", Automations),
    ("connectChannel", Automations),
    ("disconnectChannel", Automations),
    ("refreshChannel", Automations),
    ("getListenerIntegrations", Automations),
    ("getListenerConnectUrl", Automations),
    ("getSubagents", TranscriptManager),
    ("getAsyncTasks", TranscriptManager),
    ("setAgentAvatarBytes", Session),
    ("getAgentAvatar", Session),
    ("getForeverBoxStatus", ForeverBox),
    ("getCloudAgentInfo", CloudAgents),
    ("ensureForeverBox", ForeverBox),
    ("resetForeverBox", ForeverBox),
    ("updateForeverBox", ForeverBox),
    ("autoUpdateBoxNow", ForeverBox),
    ("snapshotBoxStoreNow", BoxStore),
    ("getBoxStoreStatus", BoxStore),
    ("clearBoxStoreNow", BoxStore),
    ("updateHostNow", HostUpgrade),
    ("getHostStatus", HostUpgrade),
    ("setBoxMigrating", ForeverBox),
    ("prepareBoxForRecreate", HostControl),
    ("resumeBoxAfterRecreate", HostControl),
    ("handBackForeverBox", Session),
    ("startTeachRecording", TeachRecording),
    ("stopTeachRecording", TeachRecording),
    ("getTeachRecordingStatus", TeachRecording),
    ("getTrays", Trays),
    ("dismissTray", Trays),
    ("clearTrays", Trays),
    ("uploadAttachment", Attachments),
    ("readAttachmentImage", Attachments),
    ("readAttachmentText", Attachments),
    ("readAttachmentChunk", Attachments),
    ("getHostSettings", Settings),
    ("setHostSettings", Settings),
    ("refreshMcp", Mcp),
    ("listRoutedMcpTools", Mcp),
    ("executeRoutedMcpTool", Mcp),
    ("listBoxMcpServers", Mcp),
    ("completeMcpOAuth", Mcp),
    ("requestWebAuthnCeremony", WebAuthn),
    ("setBoxSecrets", Secrets),
    ("getBoxSecretsStatus", Secrets),
    ; native
    // Fabushi-owned extensions stay outside the frozen upstream inventory.
    // Coordinator restricts these to MainData; Host owns identity and bytes.
    ("listApprovedMiniApps", Mcp),
    ("listInstalledMiniApps", Mcp),
    ("installApprovedMiniApp", Mcp),
    ("getInstalledMiniAppResources", Mcp),
    ("uninstallMiniApp", Mcp),
    ; cloud_workers
    ("cloudWorkers.status", CloudWorkers),
    ("cloudWorkers.capabilities", CloudWorkers),
    ("cloudWorkers.doctor", CloudWorkers),
);

pub fn is_sand_agent_purpose(value: &str) -> bool {
    matches!(value, "disk-saver" | "plugin-auth")
}

pub fn sanitize_template_id(value: &str) -> Option<&str> {
    let valid_len = (1..=64).contains(&value.len());
    let valid_chars = value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    (valid_len && valid_chars).then_some(value)
}

pub fn sanitize_create_agent_args(args: &Value) -> Value {
    let mut projected = args.clone();
    let Some(object) = projected.as_object_mut() else {
        return projected;
    };
    if object
        .get("purpose")
        .and_then(Value::as_str)
        .is_some_and(|purpose| !is_sand_agent_purpose(purpose))
    {
        object.remove("purpose");
    }
    if object
        .get("templateId")
        .and_then(Value::as_str)
        .is_some_and(|template_id| sanitize_template_id(template_id).is_none())
    {
        object.remove("templateId");
    }
    projected
}

#[derive(Debug, Default)]
pub struct CreateAgentNonceLedger<T> {
    order: VecDeque<String>,
    values: HashMap<String, T>,
}

impl<T> CreateAgentNonceLedger<T> {
    pub fn insert(&mut self, nonce: impl Into<String>, value: T) {
        let nonce = nonce.into();
        if !self.values.contains_key(&nonce) {
            self.order.push_back(nonce.clone());
        }
        self.values.insert(nonce, value);
        while self.order.len() > CREATE_AGENT_NONCE_LEDGER_CAP {
            if let Some(expired) = self.order.pop_front() {
                self.values.remove(&expired);
            }
        }
    }

    pub fn get(&self, nonce: &str) -> Option<&T> {
        self.values.get(nonce)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}
