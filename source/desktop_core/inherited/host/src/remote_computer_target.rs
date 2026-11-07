use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use mahayana_host_protocol::{
    ComputerAction, ComputerActionKind, ComputerControlOrigin, ComputerMouseButton, ComputerPoint,
    ComputerScrollDirection,
};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::extensions::auth::auth_service::{
    FabushiCoreCredential, FabushiCoreCredentialProvider,
};
use crate::network_enrollment_owner::{NetworkEnrollmentError, NetworkWorkerService};
use crate::network_worker_process::{NetworkWorkerStreamRead, OwnedNetworkWorker};
use crate::runner::routed_provider_runtime::RoutedToolBridge;
use crate::sensitive_input_challenge::{
    SensitiveInputBinding, SensitiveInputChallengeOwner, SENSITIVE_INPUT_CHALLENGE_MAX_TTL_MS,
};
use crate::runner::tools::sand_computer_tool::{
    ComputerActionArgs, ComputerActionName, MouseButtonInput, ScrollDirectionInput,
    parse_remote_computer_action,
};

const CORE_REQUEST_TIMEOUT_SECS: u64 = 15;
const MAX_CORE_RESPONSE_BYTES: usize = 1024 * 1024;
const REMOTE_TARGET_STREAM_TIMEOUT_MS: u64 = 15_000;
const REMOTE_TARGET_SERVICE_STREAM_TIMEOUT_MS: u64 = 1_000;
const REMOTE_TARGET_INGRESS_CACHE_TTL: Duration = Duration::from_secs(10);
const REMOTE_TARGET_FRAME_MAX_BYTES: usize = 32 * 1024;
const REMOTE_TARGET_RESULT_MAX_BYTES: usize = 1024 * 1024;
const REMOTE_SENSITIVE_RESULT_MAX_BYTES: usize = 8 * 1024 * 1024;
const REMOTE_SENSITIVE_RESULT_CHUNK_BYTES: usize = 16 * 1024;
const REMOTE_SENSITIVE_RESULT_TTL_SECONDS: i64 = 60;
const REMOTE_SENSITIVE_RESULT_MAX_ENTRIES: usize = 8;
pub const REMOTE_COMPUTER_TARGET_PORT: u16 = 8792;
const REMOTE_NATIVE_TOOL_GRANT_MAX_SECONDS: i64 = 5 * 60;

fn canonical_json(value: &Value, output: &mut String) -> Result<(), RemoteComputerTargetError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => output.push_str(
            &serde_json::to_string(value).map_err(|_| RemoteComputerTargetError::InvalidResponse)?,
        ),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|_| RemoteComputerTargetError::InvalidResponse)?,
                );
                output.push(':');
                canonical_json(&values[key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

pub fn operation_request_sha256(
    tool_name: &str,
    arguments: &Value,
    disk_preflight: &Value,
) -> Result<String, RemoteComputerTargetError> {
    let request = json!({
        "arguments": arguments,
        "diskPreflight": disk_preflight,
        "toolName": tool_name,
    });
    let mut canonical = String::new();
    canonical_json(&request, &mut canonical)?;
    let digest = Sha256::digest(canonical.as_bytes());
    let mut encoded = String::with_capacity(64);
    use std::fmt::Write as _;
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").map_err(|_| RemoteComputerTargetError::InvalidResponse)?;
    }
    Ok(encoded)
}

pub trait RemoteComputerDeviceSecretProvider: Send + Sync {
    fn get_device_secret(&self, credential: &FabushiCoreCredential) -> Result<String, RemoteComputerTargetError>;
}

pub const FABUSHI_REMOTE_COMPUTER_DEVICE_CREDENTIAL_FILE_ENV: &str =
    "FABUSHI_REMOTE_COMPUTER_DEVICE_CREDENTIAL_FILE";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteComputerDeviceCredentialFile {
    version: u32,
    user_id: serde_json::Value,
    device_id: String,
    device_secret: String,
}

#[derive(Clone)]
pub struct FileRemoteComputerDeviceSecretProvider {
    path: PathBuf,
}

impl std::fmt::Debug for FileRemoteComputerDeviceSecretProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileRemoteComputerDeviceSecretProvider")
            .field("path", &"<host-private>")
            .finish()
    }
}

impl FileRemoteComputerDeviceSecretProvider {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_environment() -> Result<Self, RemoteComputerTargetError> {
        let path = std::env::var(FABUSHI_REMOTE_COMPUTER_DEVICE_CREDENTIAL_FILE_ENV)
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        if path.trim().is_empty() {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        Ok(Self::new(path))
    }
}

impl RemoteComputerDeviceSecretProvider for FileRemoteComputerDeviceSecretProvider {
    fn get_device_secret(
        &self,
        credential: &FabushiCoreCredential,
    ) -> Result<String, RemoteComputerTargetError> {
        let raw = fs::read_to_string(&self.path)
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        let parsed: RemoteComputerDeviceCredentialFile = serde_json::from_str(&raw)
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        let user_id = match parsed.user_id {
            Value::String(value) if !value.trim().is_empty() => value,
            Value::Number(value) => value.to_string(),
            _ => return Err(RemoteComputerTargetError::AuthorityUnavailable),
        };
        if parsed.version != 1
            || parsed.device_id != credential.device_id
            || user_id != credential.user_id
            || parsed.device_secret.len() < 48
            || parsed.device_secret.len() > 256
        {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        Ok(parsed.device_secret)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RemoteComputerTargetError {
    #[error("Fabushi target authority is unavailable.")]
    AuthorityUnavailable,
    #[error("Fabushi Core target request failed.")]
    RequestFailed,
    #[error("Fabushi Core target response failed identity validation.")]
    IdentityMismatch,
    #[error("Fabushi Core target response is invalid.")]
    InvalidResponse,
    #[error("Fabushi Remote Computer private transport failed.")]
    TransportFailed,
    #[error("Fabushi Remote Computer operation outcome is unknown; do not retry automatically.")]
    OutcomeUnknown,
}



#[derive(Clone, PartialEq, Eq)]
pub struct RemoteComputerSensitiveResult {
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub captured_at_ms: i64,
    pub snapshot_generation: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl std::fmt::Debug for RemoteComputerSensitiveResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteComputerSensitiveResult")
            .field("media_type", &self.media_type)
            .field("bytes", &format_args!("<redacted:{} bytes>", self.bytes.len()))
            .field("captured_at_ms", &self.captured_at_ms)
            .field("snapshot_generation", &self.snapshot_generation)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RemoteComputerNativeOutcome {
    Completed(Value),
    SensitiveCompleted(RemoteComputerSensitiveResult),
    Failed(Value),
    OutcomeUnknown,
}

pub trait RemoteComputerNativeExecutor: Send + Sync {
    fn execute(
        &self,
        operation: &ClaimedRemoteOperation,
        arguments: &Value,
        disk_preflight: &Value,
    ) -> RemoteComputerNativeOutcome;
}

pub trait RemoteComputerToolAuthorizer: Send + Sync {
    fn authorize_tool(
        &self,
        session_id: &str,
        client_id: &str,
        tool_name: &str,
        requested_expires_at: i64,
    ) -> Result<i64, RemoteComputerTargetError>;
}

pub trait RemoteComputerToolCatalog: Send + Sync {
    fn describe_tool(&self, tool_name: &str) -> Result<Option<Value>, RemoteComputerTargetError>;
}

pub struct RoutedRemoteComputerToolCatalog {
    bridge: Arc<dyn RoutedToolBridge>,
}

impl RoutedRemoteComputerToolCatalog {
    pub fn new(bridge: Arc<dyn RoutedToolBridge>) -> Self {
        Self { bridge }
    }
}

impl RemoteComputerToolCatalog for RoutedRemoteComputerToolCatalog {
    fn describe_tool(&self, tool_name: &str) -> Result<Option<Value>, RemoteComputerTargetError> {
        let tools = self
            .bridge
            .list_tools()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        let tool = tools
            .into_iter()
            .find(|tool| tool.name == tool_name || tool.tool_name == tool_name);
        let Some(tool) = tool else {
            return Ok(None);
        };
        let effective_name = if tool.name == tool_name {
            tool.name
        } else {
            tool.tool_name
        };
        let output_schema = matches!(effective_name.as_str(), "Computer" | "Screenshot").then(|| {
            json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["content", "screenshotPath"],
                "properties": {
                    "content": {"type": "string"},
                    "screenshotPath": {"type": ["string", "null"]}
                }
            })
        });
        Ok(Some(json!({
            "name": effective_name,
            "title": effective_name,
            "description": tool.description,
            "inputSchema": tool.input_schema,
            "outputSchema": output_schema,
        })))
    }
}

pub struct NativeDesktopRemoteToolCatalog;

impl RemoteComputerToolCatalog for NativeDesktopRemoteToolCatalog {
    fn describe_tool(&self, tool_name: &str) -> Result<Option<Value>, RemoteComputerTargetError> {
        if tool_name != "Computer" || !mahayana_computer::control_policy().remote_control_enabled {
            return Ok(None);
        }
        Ok(Some(json!({
            "name": "Computer",
            "title": "Computer",
            "description": "Interact with this authorized Fabushi desktop using mouse, keyboard, scrolling, waits, and target-local observation.",
            "inputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["action"],
                "properties": {
                    "action": {"type": "string", "enum": ["screenshot", "click", "doubleClick", "move", "drag", "type", "key", "scroll", "wait"]},
                    "x": {"type": "integer"}, "y": {"type": "integer"},
                    "x2": {"type": "integer"}, "y2": {"type": "integer"},
                    "path": {"type": "array", "items": {"type": "object", "additionalProperties": false, "required": ["x", "y"], "properties": {"x": {"type": "integer"}, "y": {"type": "integer"}}}},
                    "text": {"type": "string"}, "key": {"type": "string"},
                    "button": {"type": "string", "enum": ["left", "right", "middle"]},
                    "count": {"type": "integer", "minimum": 1, "maximum": 3},
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                    "amount": {"type": "integer"},
                    "durationMs": {"type": "integer", "minimum": 0, "maximum": 30000},
                    "snapshotGeneration": {"type": "integer", "minimum": 1},
                    "description": {"type": "string"},
                    "then": {"type": "array", "minItems": 1, "maxItems": 9, "items": {"type": "object"}}
                }
            },
            "outputSchema": {
                "type": "object",
                "additionalProperties": false,
                "required": ["content", "actionsExecuted", "capturedAtMs", "snapshotGeneration", "screenshotTransferred"],
                "properties": {
                    "content": {"type": "string"},
                    "actionsExecuted": {"type": "integer", "minimum": 1},
                    "capturedAtMs": {"type": "integer"},
                    "snapshotGeneration": {"type": "integer", "minimum": 1},
                    "width": {"type": ["integer", "null"]},
                    "height": {"type": ["integer", "null"]},
                    "screenshotTransferred": {"type": "boolean"}
                }
            }
        })))
    }
}

pub struct NativeDesktopRemoteToolAuthorizer;

impl RemoteComputerToolAuthorizer for NativeDesktopRemoteToolAuthorizer {
    fn authorize_tool(
        &self,
        session_id: &str,
        client_id: &str,
        tool_name: &str,
        requested_expires_at: i64,
    ) -> Result<i64, RemoteComputerTargetError> {
        let policy = mahayana_computer::control_policy();
        if session_id.trim().is_empty()
            || client_id.trim().is_empty()
            || tool_name != "Computer"
            || !policy.remote_control_enabled
        {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        let now = unix_seconds()?;
        if requested_expires_at <= now {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        Ok(requested_expires_at.min(now.saturating_add(REMOTE_NATIVE_TOOL_GRANT_MAX_SECONDS)))
    }
}

fn native_i32(value: Option<i64>, label: &str) -> Result<Option<i32>, String> {
    value
        .map(|value| i32::try_from(value).map_err(|_| format!("{label} is out of native desktop range")))
        .transpose()
}

fn native_computer_action(args: &ComputerActionArgs) -> Result<ComputerAction, String> {
    let action = match args.action {
        ComputerActionName::Screenshot => ComputerActionKind::Screenshot,
        ComputerActionName::Click => ComputerActionKind::Click,
        ComputerActionName::DoubleClick => ComputerActionKind::Click,
        ComputerActionName::Move => ComputerActionKind::Move,
        ComputerActionName::Drag => ComputerActionKind::Drag,
        ComputerActionName::Type => ComputerActionKind::Type,
        ComputerActionName::Key => ComputerActionKind::Key,
        ComputerActionName::Scroll => ComputerActionKind::Scroll,
        ComputerActionName::Wait => ComputerActionKind::Wait,
    };
    let path = if args.path.is_empty() {
        None
    } else {
        Some(args.path.iter().map(|point| {
            Ok(ComputerPoint {
                x: i32::try_from(point.x).map_err(|_| "drag path x is out of native desktop range".to_string())?,
                y: i32::try_from(point.y).map_err(|_| "drag path y is out of native desktop range".to_string())?,
            })
        }).collect::<Result<Vec<_>, String>>()?)
    };
    Ok(ComputerAction {
        action,
        x: native_i32(args.x, "x")?, y: native_i32(args.y, "y")?,
        x2: native_i32(args.x2, "x2")?, y2: native_i32(args.y2, "y2")?,
        path,
        text: args.text.clone(), key: args.key.clone(),
        button: args.button.map(|button| match button {
            MouseButtonInput::Left => ComputerMouseButton::Left,
            MouseButtonInput::Right => ComputerMouseButton::Right,
            MouseButtonInput::Middle => ComputerMouseButton::Middle,
        }),
        click_count: if args.action == ComputerActionName::DoubleClick {
            if args.count.is_some() {
                return Err("doubleClick has a fixed click count of 2 and does not accept count".into());
            }
            Some(2)
        } else {
            args.count
        },
        direction: args.direction.map(|direction| match direction {
            ScrollDirectionInput::Up => ComputerScrollDirection::Up,
            ScrollDirectionInput::Down => ComputerScrollDirection::Down,
            ScrollDirectionInput::Left => ComputerScrollDirection::Left,
            ScrollDirectionInput::Right => ComputerScrollDirection::Right,
        }),
        amount: native_i32(args.amount, "amount")?,
        wait_ms: args.duration_ms,
        description: args.description.clone(),
    })
}

fn native_computer_actions(args: &ComputerActionArgs) -> Result<Vec<ComputerAction>, String> {
    let mut actions = Vec::with_capacity(1 + args.then_actions.len());
    actions.push(native_computer_action(args)?);
    for follow_up in &args.then_actions {
        actions.push(native_computer_action(follow_up)?);
    }
    Ok(actions)
}

pub struct NativeDesktopRemoteExecutor;

impl RemoteComputerNativeExecutor for NativeDesktopRemoteExecutor {
    fn execute(
        &self,
        operation: &ClaimedRemoteOperation,
        arguments: &Value,
        _disk_preflight: &Value,
    ) -> RemoteComputerNativeOutcome {
        if operation.tool_name != "Computer" {
            return RemoteComputerNativeOutcome::Failed(json!({"code":"unsupported_tool","message":"Only the product Computer contract is enabled on this target path.","retryable":false}));
        }
        if !mahayana_computer::control_policy().remote_control_enabled {
            return RemoteComputerNativeOutcome::Failed(json!({"code":"remote_control_disabled","message":"Remote Computer is disabled in Fabushi settings.","retryable":false}));
        }
        let parsed = match parse_remote_computer_action(arguments) {
            Ok(action) => action,
            Err(error) => return RemoteComputerNativeOutcome::Failed(json!({"code":"invalid_arguments","message":error.to_string(),"retryable":false})),
        };
        if parsed.then_actions.iter().any(|action| action.action == ComputerActionName::Screenshot) {
            return RemoteComputerNativeOutcome::Failed(json!({"code":"invalid_arguments","message":"Screenshot may only be the primary Computer action.","retryable":false}));
        }
        let sensitive_screenshot = parsed.action == ComputerActionName::Screenshot;
        if sensitive_screenshot && !operation.sensitive_result {
            return RemoteComputerNativeOutcome::Failed(json!({"code":"sensitive_result_required","message":"Remote screenshot requires a sensitive-result operation.","retryable":false}));
        }
        let actions = match native_computer_actions(&parsed) {
            Ok(actions) => actions,
            Err(message) => return RemoteComputerNativeOutcome::Failed(json!({"code":"invalid_arguments","message":message,"retryable":false})),
        };
        let lease = mahayana_computer::ComputerControlLeaseRequest::new(
            format!("official-device-mcp:{}", operation.client_id),
            operation.session_id.clone(),
            "local-desktop",
            ComputerControlOrigin::RemoteMobile,
            "remote-human",
        );
        match mahayana_computer::execute_with_lease_fenced(
            &actions,
            ComputerControlOrigin::RemoteMobile,
            &lease,
            parsed.snapshot_generation,
        ) {
            Ok(observed) => {
                let result = observed.result;
                if sensitive_screenshot {
                    return match decode_sensitive_snapshot(
                        &result.snapshot.data_url,
                        result.snapshot.captured_at_ms,
                        observed.snapshot_generation,
                        result.snapshot.width,
                        result.snapshot.height,
                    ) {
                        Ok(sensitive) => RemoteComputerNativeOutcome::SensitiveCompleted(sensitive),
                        Err(_) => RemoteComputerNativeOutcome::Failed(json!({
                            "code":"computer_capture_invalid",
                            "message":"Target screenshot could not be prepared for the authenticated sensitive-result channel.",
                            "retryable":false
                        })),
                    };
                }
                RemoteComputerNativeOutcome::Completed(json!({
                    "content": "Computer action ran on the authorized target desktop.",
                    "actionsExecuted": result.actions_executed,
                    "capturedAtMs": result.snapshot.captured_at_ms,
                    "snapshotGeneration": observed.snapshot_generation,
                    "width": result.snapshot.width,
                    "height": result.snapshot.height,
                    "screenshotTransferred": false,
                }))
            }
            Err(mahayana_computer::ComputerError::SnapshotStale { expected, current }) => {
                RemoteComputerNativeOutcome::Failed(json!({
                    "code":"snapshot_stale",
                    "message":"The requested desktop observation is stale; observe again before retrying the action.",
                    "expectedSnapshotGeneration": expected,
                    "currentSnapshotGeneration": current,
                    "retryable":false
                }))
            }
            Err(error) => RemoteComputerNativeOutcome::Failed(json!({"code":"computer_action_failed","message":error.to_string(),"retryable":false})),
        }
    }
}

fn decode_sensitive_snapshot(
    data_url: &str,
    captured_at_ms: i64,
    snapshot_generation: u64,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<RemoteComputerSensitiveResult, RemoteComputerTargetError> {
    let Some((header, encoded)) = data_url.split_once(',') else {
        return Err(RemoteComputerTargetError::InvalidResponse);
    };
    let media_type = header
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .filter(|value| matches!(*value, "image/jpeg" | "image/png" | "image/webp"))
        .ok_or(RemoteComputerTargetError::InvalidResponse)?
        .to_string();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| RemoteComputerTargetError::InvalidResponse)?;
    if bytes.is_empty() || bytes.len() > REMOTE_SENSITIVE_RESULT_MAX_BYTES {
        return Err(RemoteComputerTargetError::InvalidResponse);
    }
    Ok(RemoteComputerSensitiveResult {
        media_type,
        bytes,
        captured_at_ms,
        snapshot_generation,
        width,
        height,
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum RemoteTargetPrivateRequest {
    GrantController {
        device_id: String,
        network_generation: i64,
        connection_id: String,
        delegation_id: String,
        request_id: String,
        label: String,
    },
    DescribeTool {
        device_id: String,
        network_generation: i64,
        connection_id: String,
        delegation_id: String,
        request_id: String,
        label: String,
        tool_name: String,
    },
    AuthorizeTool {
        device_id: String,
        network_generation: i64,
        session_id: String,
        client_id: String,
        tool_name: String,
        expires_at: i64,
    },
    Operation {
        device_id: String,
        network_generation: i64,
        session_id: String,
        client_id: String,
        request_id: String,
        tool_name: String,
        request_sha256: String,
        grant_generation: i64,
        arguments: Value,
        disk_preflight: Value,
    },
    SensitiveInputIssue {
        device_id: String,
        network_generation: i64,
        session_id: String,
        client_id: String,
        request_id: String,
        request_sha256: String,
        grant_generation: i64,
        ttl_ms: i64,
    },
    SensitiveInputSubmit {
        device_id: String,
        network_generation: i64,
        session_id: String,
        client_id: String,
        request_id: String,
        request_sha256: String,
        grant_generation: i64,
        challenge_id: String,
        snapshot_generation: u64,
        payload_base64: String,
    },
    SensitiveResultResume {
        device_id: String,
        network_generation: i64,
        session_id: String,
        client_id: String,
        request_id: String,
        request_sha256: String,
        grant_generation: i64,
    },
    SensitiveResultChunk {
        device_id: String,
        network_generation: i64,
        session_id: String,
        client_id: String,
        request_id: String,
        request_sha256: String,
        grant_generation: i64,
        result_ref: String,
        offset: usize,
    },
    SensitiveResultRevoke {
        device_id: String,
        network_generation: i64,
        session_id: String,
        client_id: String,
        request_id: String,
        request_sha256: String,
        grant_generation: i64,
        result_ref: String,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteServiceClientGrant {
    pub device_id: String,
    pub client_id: String,
    client_token: String,
    pub connection_id: String,
    pub delegation_id: String,
    pub request_id: String,
    pub reconciled: bool,
}

impl std::fmt::Debug for RemoteServiceClientGrant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteServiceClientGrant")
            .field("device_id", &self.device_id)
            .field("client_id", &self.client_id)
            .field("client_token", &"<redacted>")
            .field("connection_id", &self.connection_id)
            .field("delegation_id", &self.delegation_id)
            .field("request_id", &self.request_id)
            .field("reconciled", &self.reconciled)
            .finish()
    }
}

impl RemoteServiceClientGrant {
    pub fn client_token(&self) -> &str {
        &self.client_token
    }
}

pub struct ClaimedRemoteOperation {
    pub request_id: String,
    pub session_id: String,
    pub client_id: String,
    pub tool_name: String,
    pub request_sha256: String,
    pub grant_generation: i64,
    pub sensitive_result: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteNetworkEnrollmentGrant {
    pub grant_id: String,
    grant_secret: String,
    pub cell_id: String,
    pub generation: i64,
    pub expires_at: i64,
    pub request_id: String,
    pub reconciled: bool,
}

impl std::fmt::Debug for RemoteNetworkEnrollmentGrant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteNetworkEnrollmentGrant")
            .field("grant_id", &self.grant_id)
            .field("grant_secret", &"<redacted>")
            .field("cell_id", &self.cell_id)
            .field("generation", &self.generation)
            .field("expires_at", &self.expires_at)
            .field("request_id", &self.request_id)
            .field("reconciled", &self.reconciled)
            .finish()
    }
}

impl RemoteNetworkEnrollmentGrant {
    pub fn grant_secret(&self) -> &str {
        &self.grant_secret
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteNetworkEnrollmentStarted {
    pub grant_id: String,
    auth_key: String,
    pub device_id: String,
    pub cell_id: String,
    pub generation: i64,
    pub expires_at: i64,
    pub reconciled: bool,
}

impl std::fmt::Debug for RemoteNetworkEnrollmentStarted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RemoteNetworkEnrollmentStarted")
            .field("grant_id", &self.grant_id)
            .field("auth_key", &"<redacted>")
            .field("device_id", &self.device_id)
            .field("cell_id", &self.cell_id)
            .field("generation", &self.generation)
            .field("expires_at", &self.expires_at)
            .field("reconciled", &self.reconciled)
            .finish()
    }
}

impl RemoteNetworkEnrollmentStarted {
    pub fn auth_key(&self) -> &str {
        &self.auth_key
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNetworkEnrollmentCompleted {
    pub grant_id: String,
    pub device_id: String,
    pub cell_id: String,
    pub node_id: String,
    pub generation: i64,
    pub reconciled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNetworkBinding {
    pub device_id: String,
    pub cell_id: String,
    pub node_id: String,
    pub proof_sha256: String,
    pub generation: i64,
    pub expires_at: i64,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNetworkIngress {
    pub device_id: String,
    pub cell_id: String,
    pub target_generation: i64,
    pub service_id: String,
    pub node_id: String,
    pub ingress_generation: i64,
    pub expires_at: i64,
    pub active: bool,
}

fn is_fabushi_tailnet_peer(value: &str) -> bool {
    match value.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [first, second, _, _] = ip.octets();
            first == 100 && (64..=127).contains(&second)
        }
        Ok(IpAddr::V6(ip)) => {
            let segments = ip.segments();
            segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
        }
        Err(_) => false,
    }
}

#[derive(Clone)]
struct SensitiveResultEntry {
    account_user_id: String,
    device_id: String,
    network_generation: i64,
    session_id: String,
    client_id: String,
    request_id: String,
    request_sha256: String,
    tool_name: String,
    grant_generation: i64,
    result_sha256: String,
    media_type: String,
    bytes: Vec<u8>,
    captured_at_ms: i64,
    snapshot_generation: u64,
    width: Option<u32>,
    height: Option<u32>,
    expires_at: i64,
    next_offset: usize,
}

#[derive(Default)]
struct SensitiveResultVault {
    entries: HashMap<String, SensitiveResultEntry>,
}

impl SensitiveResultVault {
    fn purge_expired(&mut self, now: i64) {
        self.entries.retain(|_, entry| entry.expires_at > now);
    }

    fn insert(&mut self, result_ref: String, entry: SensitiveResultEntry, now: i64) {
        self.purge_expired(now);
        if self.entries.len() >= REMOTE_SENSITIVE_RESULT_MAX_ENTRIES {
            if let Some(oldest) = self.entries.iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(result_ref, entry);
    }
}

fn sensitive_chunk_range(
    entry: &SensitiveResultEntry,
    offset: usize,
) -> Result<(usize, bool), RemoteComputerTargetError> {
    if offset != entry.next_offset || offset >= entry.bytes.len() {
        return Err(RemoteComputerTargetError::IdentityMismatch);
    }
    let end = offset
        .saturating_add(REMOTE_SENSITIVE_RESULT_CHUNK_BYTES)
        .min(entry.bytes.len());
    Ok((end, end == entry.bytes.len()))
}

#[derive(Debug, Clone)]
struct SensitiveResultAccess {
    result_ref: String,
    result_sha256: String,
    media_type: String,
    captured_at_ms: i64,
    snapshot_generation: u64,
    width: Option<u32>,
    height: Option<u32>,
    expires_at: i64,
    next_offset: usize,
}

pub struct FabushiCoreTargetClient {
    base_url: Url,
    credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
    device_secret_provider: Arc<dyn RemoteComputerDeviceSecretProvider>,
    http: Client,
    sensitive_results: Mutex<SensitiveResultVault>,
    sensitive_inputs: SensitiveInputChallengeOwner,
}

impl std::fmt::Debug for FabushiCoreTargetClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FabushiCoreTargetClient")
            .field("base_url", &self.base_url)
            .field("credential_provider", &"<first-party>")
            .field("device_secret_provider", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl FabushiCoreTargetClient {
    pub fn new(
        base_url: &str,
        credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
        device_secret_provider: Arc<dyn RemoteComputerDeviceSecretProvider>,
    ) -> Result<Self, RemoteComputerTargetError> {
        let mut base_url = Url::parse(base_url).map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        let loopback = matches!(base_url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if base_url.scheme() != "https" && !(loopback && base_url.scheme() == "http") {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        base_url.set_path("/");
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(CORE_REQUEST_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        Ok(Self {
            base_url,
            credential_provider,
            device_secret_provider,
            http,
            sensitive_results: Mutex::new(SensitiveResultVault::default()),
            sensitive_inputs: SensitiveInputChallengeOwner::default(),
        })
    }

    fn authority(&self) -> Result<(FabushiCoreCredential, String), RemoteComputerTargetError> {
        let credential = self
            .credential_provider
            .get_fabushi_core_credential()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        credential.authorize_origin(self.base_url.as_str())
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        let device_secret = self.device_secret_provider.get_device_secret(&credential)?;
        if device_secret.len() < 48 || device_secret.len() > 256 {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        Ok((credential, device_secret))
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, RemoteComputerTargetError> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
            path.clear();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }

    fn post_json(
        &self,
        credential: &FabushiCoreCredential,
        url: Url,
        body: Value,
    ) -> Result<Value, RemoteComputerTargetError> {
        let response = self
            .http
            .post(url)
            .bearer_auth(credential.access_token())
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .map_err(|_| RemoteComputerTargetError::RequestFailed)?;
        if !response.status().is_success() {
            return Err(RemoteComputerTargetError::RequestFailed);
        }
        let bytes = response
            .bytes()
            .map_err(|_| RemoteComputerTargetError::RequestFailed)?;
        if bytes.len() > MAX_CORE_RESPONSE_BYTES {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        serde_json::from_slice(&bytes).map_err(|_| RemoteComputerTargetError::InvalidResponse)
    }

    pub fn network_ingress(
        &self,
        service_id: &str,
    ) -> Result<RemoteNetworkIngress, RemoteComputerTargetError> {
        if service_id != "official-device-mcp" {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&["v1", "computers", "network-ingress"])?,
            json!({
                "deviceId": credential.device_id,
                "deviceSecret": device_secret,
                "serviceId": service_id,
            }),
        )?;
        if response.get("deviceId").and_then(Value::as_str)
            != Some(credential.device_id.as_str())
            || response.get("serviceId").and_then(Value::as_str) != Some(service_id)
            || response.get("active").and_then(Value::as_bool) != Some(true)
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let cell_id = required_text(&response, "cellId")?;
        let node_id = required_text(&response, "nodeId")?;
        if !is_fabushi_tailnet_peer(&node_id) {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let target_generation = response
            .get("targetGeneration")
            .and_then(Value::as_i64)
            .filter(|generation| *generation > 0)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        let ingress_generation = response
            .get("ingressGeneration")
            .and_then(Value::as_i64)
            .filter(|generation| *generation > 0)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        let now = unix_seconds()?;
        let expires_at = response
            .get("expiresAt")
            .and_then(Value::as_i64)
            .filter(|expires_at| *expires_at > now)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        Ok(RemoteNetworkIngress {
            device_id: credential.device_id,
            cell_id,
            target_generation,
            service_id: service_id.to_string(),
            node_id,
            ingress_generation,
            expires_at,
            active: true,
        })
    }

    pub fn grant_service_client(
        &self,
        connection_id: &str,
        delegation_id: &str,
        request_id: &str,
        label: &str,
    ) -> Result<RemoteServiceClientGrant, RemoteComputerTargetError> {
        if connection_id.trim().is_empty()
            || delegation_id.trim().is_empty()
            || request_id.trim().is_empty()
            || label.trim().is_empty()
        {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&["v1", "computers", &credential.device_id, "sessions", "grant"])?,
            json!({
                "deviceSecret": device_secret,
                "connectionId": connection_id,
                "delegationId": delegation_id,
                "requestId": request_id,
                "label": label
            }),
        )?;
        if response.get("deviceId").and_then(Value::as_str) != Some(credential.device_id.as_str())
            || response.get("connectionId").and_then(Value::as_str) != Some(connection_id)
            || response.get("delegationId").and_then(Value::as_str) != Some(delegation_id)
            || response.get("requestId").and_then(Value::as_str) != Some(request_id)
            || response.get("serviceBound").and_then(Value::as_bool) != Some(true)
            || response.get("targetGranted").and_then(Value::as_bool) != Some(true)
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let client_id = required_text(&response, "clientId")?;
        let client_token = required_text(&response, "clientToken")?;
        if client_token.len() < 48 || client_token.len() > 256 {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        Ok(RemoteServiceClientGrant {
            device_id: credential.device_id,
            client_id,
            client_token,
            connection_id: connection_id.to_string(),
            delegation_id: delegation_id.to_string(),
            request_id: request_id.to_string(),
            reconciled: response.get("reconciled").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    pub fn activate_session(&self, session_id: &str) -> Result<Value, RemoteComputerTargetError> {
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&["v1", "computers", &credential.device_id, "sessions", session_id, "activate"])?,
            json!({"deviceSecret": device_secret}),
        )?;
        if response.get("sessionId").and_then(Value::as_str) != Some(session_id)
            || response.get("state").and_then(Value::as_str) != Some("active")
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        Ok(response)
    }

    pub fn grant_tool(
        &self,
        session_id: &str,
        client_id: &str,
        tool_name: &str,
        expires_at: i64,
    ) -> Result<i64, RemoteComputerTargetError> {
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&["v1", "computers", &credential.device_id, "sessions", session_id, "tool-grants", "grant"])?,
            json!({
                "deviceSecret": device_secret,
                "clientId": client_id,
                "toolName": tool_name,
                "expiresAt": expires_at
            }),
        )?;
        if response.get("toolName").and_then(Value::as_str) != Some(tool_name)
            || response.get("revoked").and_then(Value::as_bool) == Some(true)
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        response
            .get("generation")
            .and_then(Value::as_i64)
            .filter(|generation| *generation > 0)
            .ok_or(RemoteComputerTargetError::InvalidResponse)
    }

    pub fn revoke_tool(
        &self,
        session_id: &str,
        client_id: &str,
        tool_name: &str,
    ) -> Result<bool, RemoteComputerTargetError> {
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&["v1", "computers", &credential.device_id, "sessions", session_id, "tool-grants", "revoke"])?,
            json!({
                "deviceSecret": device_secret,
                "clientId": client_id,
                "toolName": tool_name
            }),
        )?;
        if response.get("toolName").and_then(Value::as_str) != Some(tool_name) {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        response
            .get("revoked")
            .and_then(Value::as_bool)
            .ok_or(RemoteComputerTargetError::InvalidResponse)
    }

    pub fn validate_sensitive_result_access(
        &self,
        operation: &ClaimedRemoteOperation,
        expected_result_sha256: &str,
    ) -> Result<(), RemoteComputerTargetError> {
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&[
                "v1", "computers", &credential.device_id, "sessions", &operation.session_id,
                "operations", &operation.request_id, "access",
            ])?,
            json!({"deviceSecret": device_secret}),
        )?;
        if response.get("allowed").and_then(Value::as_bool) != Some(true)
            || response.get("deviceId").and_then(Value::as_str) != Some(credential.device_id.as_str())
            || response.get("sessionId").and_then(Value::as_str) != Some(operation.session_id.as_str())
            || response.get("clientId").and_then(Value::as_str) != Some(operation.client_id.as_str())
            || response.get("requestId").and_then(Value::as_str) != Some(operation.request_id.as_str())
            || response.get("toolName").and_then(Value::as_str) != Some(operation.tool_name.as_str())
            || response.get("requestSha256").and_then(Value::as_str) != Some(operation.request_sha256.as_str())
            || response.get("grantGeneration").and_then(Value::as_i64) != Some(operation.grant_generation)
            || response.get("sensitiveResult").and_then(Value::as_bool) != Some(true)
            || response.get("state").and_then(Value::as_str) != Some("completed")
            || response.get("resultSha256").and_then(Value::as_str) != Some(expected_result_sha256)
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        Ok(())
    }

    fn sensitive_entry_matches(
        entry: &SensitiveResultEntry,
        current_account_user_id: &str,
        current_device_id: &str,
        device_id: &str,
        network_generation: i64,
        session_id: &str,
        client_id: &str,
        request_id: &str,
        request_sha256: &str,
        grant_generation: i64,
    ) -> bool {
        entry.account_user_id == current_account_user_id
            && entry.device_id == current_device_id
            && entry.device_id == device_id
            && entry.network_generation == network_generation
            && entry.session_id == session_id
            && entry.client_id == client_id
            && entry.request_id == request_id
            && entry.request_sha256 == request_sha256
            && entry.grant_generation == grant_generation
            && entry.tool_name == "Computer"
    }

    fn validate_sensitive_entry(
        &self,
        entry: &SensitiveResultEntry,
        device_id: &str,
        network_generation: i64,
        session_id: &str,
        client_id: &str,
        request_id: &str,
        request_sha256: &str,
        grant_generation: i64,
    ) -> Result<ClaimedRemoteOperation, RemoteComputerTargetError> {
        let (credential, _) = self.authority()?;
        let credential_bound = entry.account_user_id == credential.user_id
            && entry.device_id == credential.device_id;
        if !credential_bound || !Self::sensitive_entry_matches(
            entry,
            &credential.user_id,
            &credential.device_id,
            device_id,
            network_generation,
            session_id,
            client_id,
            request_id,
            request_sha256,
            grant_generation,
        ) || entry.expires_at <= unix_seconds()?
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        self.require_current_network_binding(device_id, network_generation)?;
        let operation = ClaimedRemoteOperation {
            request_id: entry.request_id.clone(),
            session_id: entry.session_id.clone(),
            client_id: entry.client_id.clone(),
            tool_name: entry.tool_name.clone(),
            request_sha256: entry.request_sha256.clone(),
            grant_generation: entry.grant_generation,
            sensitive_result: true,
        };
        self.validate_sensitive_result_access(&operation, &entry.result_sha256)?;
        Ok(operation)
    }

    pub fn claim_operation(
        &self,
        session_id: &str,
        request_id: &str,
        expected_tool_name: &str,
        expected_request_sha256: &str,
    ) -> Result<ClaimedRemoteOperation, RemoteComputerTargetError> {
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&[
                "v1", "computers", &credential.device_id, "sessions", session_id,
                "operations", request_id, "claim",
            ])?,
            json!({"deviceSecret": device_secret}),
        )?;
        if response.get("requestId").and_then(Value::as_str) != Some(request_id)
            || response.get("sessionId").and_then(Value::as_str) != Some(session_id)
            || response.get("toolName").and_then(Value::as_str) != Some(expected_tool_name)
            || response.get("requestSha256").and_then(Value::as_str) != Some(expected_request_sha256)
            || response.get("state").and_then(Value::as_str) != Some("running")
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let client_id = response
            .get("clientId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        let grant_generation = response
            .get("grantGeneration")
            .and_then(Value::as_i64)
            .filter(|generation| *generation > 0)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        Ok(ClaimedRemoteOperation {
            request_id: request_id.to_string(),
            session_id: session_id.to_string(),
            client_id: client_id.to_string(),
            tool_name: expected_tool_name.to_string(),
            request_sha256: expected_request_sha256.to_string(),
            grant_generation,
            sensitive_result: response.get("sensitiveResult").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    pub fn publish_result(
        &self,
        operation: &ClaimedRemoteOperation,
        status: &str,
        result_sha256: &str,
        result: Option<Value>,
    ) -> Result<Value, RemoteComputerTargetError> {
        if !matches!(status, "completed" | "failed") {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        if operation.sensitive_result && result.is_some() {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&[
                "v1", "computers", &credential.device_id, "sessions", &operation.session_id,
                "operations", &operation.request_id, "result",
            ])?,
            json!({
                "deviceSecret": device_secret,
                "status": status,
                "resultSha256": result_sha256,
                "result": result
            }),
        )?;
        if response.get("requestId").and_then(Value::as_str) != Some(operation.request_id.as_str())
            || response.get("sessionId").and_then(Value::as_str) != Some(operation.session_id.as_str())
            || response.get("requestSha256").and_then(Value::as_str) != Some(operation.request_sha256.as_str())
            || response.get("toolName").and_then(Value::as_str) != Some(operation.tool_name.as_str())
            || response.get("state").and_then(Value::as_str) != Some(status)
            || response.get("resultSha256").and_then(Value::as_str) != Some(result_sha256)
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        Ok(response)
    }



    pub fn serve_private_request(
        &self,
        worker: &mut OwnedNetworkWorker,
        expected_peer: &str,
        local_port: u16,
        catalog: &dyn RemoteComputerToolCatalog,
        authorizer: &dyn RemoteComputerToolAuthorizer,
        executor: &dyn RemoteComputerNativeExecutor,
    ) -> Result<(), RemoteComputerTargetError> {
        if expected_peer.trim().is_empty()
            || !is_fabushi_tailnet_peer(expected_peer)
            || local_port == 0
        {
            return Err(RemoteComputerTargetError::TransportFailed);
        }
        let stream_id = worker
            .stream_listen(expected_peer, local_port, REMOTE_TARGET_STREAM_TIMEOUT_MS)
            .map_err(|_| RemoteComputerTargetError::TransportFailed)?;
        let result = self.serve_private_stream(worker, &stream_id, catalog, authorizer, executor);
        let close_result = worker.stream_close(&stream_id);
        match (result, close_result) {
            (Err(error), _) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(_)) => Err(RemoteComputerTargetError::TransportFailed),
        }
    }

    pub fn serve_private_request_with_timeout(
        &self,
        worker: &mut OwnedNetworkWorker,
        expected_peer: &str,
        local_port: u16,
        timeout_ms: u64,
        catalog: &dyn RemoteComputerToolCatalog,
        authorizer: &dyn RemoteComputerToolAuthorizer,
        executor: &dyn RemoteComputerNativeExecutor,
    ) -> Result<(), RemoteComputerTargetError> {
        if expected_peer.trim().is_empty()
            || !is_fabushi_tailnet_peer(expected_peer)
            || local_port == 0
            || timeout_ms == 0
            || timeout_ms > REMOTE_TARGET_STREAM_TIMEOUT_MS
        {
            return Err(RemoteComputerTargetError::TransportFailed);
        }
        let stream_id = worker
            .stream_listen(expected_peer, local_port, timeout_ms)
            .map_err(|_| RemoteComputerTargetError::TransportFailed)?;
        let result = self.serve_private_stream(worker, &stream_id, catalog, authorizer, executor);
        let close_result = worker.stream_close(&stream_id);
        match (result, close_result) {
            (Err(error), _) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(_)) => Err(RemoteComputerTargetError::TransportFailed),
        }
    }

    fn serve_private_stream(
        &self,
        worker: &mut OwnedNetworkWorker,
        stream_id: &str,
        catalog: &dyn RemoteComputerToolCatalog,
        authorizer: &dyn RemoteComputerToolAuthorizer,
        executor: &dyn RemoteComputerNativeExecutor,
    ) -> Result<(), RemoteComputerTargetError> {
        let frame = read_private_frame(worker, stream_id)?;
        let request: RemoteTargetPrivateRequest =
            serde_json::from_slice(&frame).map_err(|_| RemoteComputerTargetError::InvalidResponse)?;

        match request {
            RemoteTargetPrivateRequest::GrantController {
                device_id,
                network_generation,
                connection_id,
                delegation_id,
                request_id,
                label,
            } => {
                self.require_current_network_binding(&device_id, network_generation)?;
                let grant =
                    self.grant_service_client(&connection_id, &delegation_id, &request_id, &label)?;
                write_private_frame(
                    worker,
                    stream_id,
                    &json!({
                        "type": "controllerGranted",
                        "deviceId": grant.device_id,
                        "clientId": grant.client_id,
                        "clientToken": grant.client_token(),
                        "connectionId": grant.connection_id,
                        "delegationId": grant.delegation_id,
                        "requestId": grant.request_id,
                        "reconciled": grant.reconciled,
                        "networkGeneration": network_generation,
                    }),
                )
            }
            RemoteTargetPrivateRequest::DescribeTool {
                device_id,
                network_generation,
                connection_id,
                delegation_id,
                request_id,
                label,
                tool_name,
            } => {
                self.require_current_network_binding(&device_id, network_generation)?;
                let grant =
                    self.grant_service_client(&connection_id, &delegation_id, &request_id, &label)?;
                let tool = catalog.describe_tool(&tool_name)?;
                if let Some(descriptor) = tool.as_ref() {
                    if descriptor.get("name").and_then(Value::as_str) != Some(tool_name.as_str()) {
                        return Err(RemoteComputerTargetError::IdentityMismatch);
                    }
                }
                let schema_version = tool
                    .as_ref()
                    .map(tool_descriptor_sha256)
                    .transpose()?
                    .unwrap_or_default();
                write_private_frame(
                    worker,
                    stream_id,
                    &json!({
                        "type": "toolDescription",
                        "deviceId": device_id,
                        "clientId": grant.client_id,
                        "connectionId": connection_id,
                        "delegationId": delegation_id,
                        "requestId": request_id,
                        "toolName": tool_name,
                        "available": tool.is_some(),
                        "schemaVersion": schema_version,
                        "tool": tool,
                        "networkGeneration": network_generation,
                    }),
                )
            }
            RemoteTargetPrivateRequest::AuthorizeTool {
                device_id,
                network_generation,
                session_id,
                client_id,
                tool_name,
                expires_at,
            } => {
                self.require_current_network_binding(&device_id, network_generation)?;
                let authorized_expires_at =
                    authorizer.authorize_tool(&session_id, &client_id, &tool_name, expires_at)?;
                let now = unix_seconds()?;
                if authorized_expires_at <= now || authorized_expires_at > expires_at {
                    return Err(RemoteComputerTargetError::AuthorityUnavailable);
                }
                self.activate_session(&session_id)?;
                let generation =
                    self.grant_tool(&session_id, &client_id, &tool_name, authorized_expires_at)?;
                write_private_frame(
                    worker,
                    stream_id,
                    &json!({
                        "type": "toolAuthorized",
                        "deviceId": device_id,
                        "sessionId": session_id,
                        "clientId": client_id,
                        "toolName": tool_name,
                        "expiresAt": authorized_expires_at,
                        "grantGeneration": generation,
                        "networkGeneration": network_generation,
                    }),
                )
            }
            RemoteTargetPrivateRequest::Operation {
                device_id,
                network_generation,
                session_id,
                client_id,
                request_id,
                tool_name,
                request_sha256,
                grant_generation,
                arguments,
                disk_preflight,
            } => {
                self.require_current_network_binding(&device_id, network_generation)?;
                let locally_computed_sha256 =
                    operation_request_sha256(&tool_name, &arguments, &disk_preflight)?;
                if locally_computed_sha256 != request_sha256 {
                    return Err(RemoteComputerTargetError::IdentityMismatch);
                }
                let claimed =
                    self.claim_operation(&session_id, &request_id, &tool_name, &request_sha256)?;
                if claimed.client_id != client_id || claimed.grant_generation != grant_generation {
                    return Err(RemoteComputerTargetError::IdentityMismatch);
                }

                // Fence account switches, network revoke and generation rollover again after
                // the Core claim but before any native side effect is dispatched.
                self.require_current_network_binding(&device_id, network_generation)?;

                match executor.execute(&claimed, &arguments, &disk_preflight) {
                    RemoteComputerNativeOutcome::OutcomeUnknown => write_private_frame(
                        worker,
                        stream_id,
                        &json!({
                            "type": "operationResult",
                            "deviceId": device_id,
                            "sessionId": session_id,
                            "requestId": request_id,
                            "requestSha256": request_sha256,
                            "status": "outcome_unknown",
                            "networkGeneration": network_generation,
                        }),
                    ),
                    RemoteComputerNativeOutcome::SensitiveCompleted(result) => self.finish_sensitive_operation(
                        worker,
                        stream_id,
                        &claimed,
                        &device_id,
                        network_generation,
                        result,
                    ),
                    RemoteComputerNativeOutcome::Completed(result) => self.finish_native_operation(
                        worker,
                        stream_id,
                        &claimed,
                        &device_id,
                        network_generation,
                        "completed",
                        result,
                    ),
                    RemoteComputerNativeOutcome::Failed(result) => self.finish_native_operation(
                        worker,
                        stream_id,
                        &claimed,
                        &device_id,
                        network_generation,
                        "failed",
                        result,
                    ),
                }
            }
            RemoteTargetPrivateRequest::SensitiveInputIssue {
                device_id,
                network_generation,
                session_id,
                client_id,
                request_id,
                request_sha256,
                grant_generation,
                ttl_ms,
            } => {
                if !mahayana_computer::control_policy().remote_control_enabled {
                    return Err(RemoteComputerTargetError::AuthorityUnavailable);
                }
                self.require_current_network_binding(&device_id, network_generation)?;
                if ttl_ms <= 0 || ttl_ms > SENSITIVE_INPUT_CHALLENGE_MAX_TTL_MS {
                    return Err(RemoteComputerTargetError::InvalidResponse);
                }
                let claimed =
                    self.claim_operation(&session_id, &request_id, "Computer", &request_sha256)?;
                if claimed.client_id != client_id || claimed.grant_generation != grant_generation {
                    return Err(RemoteComputerTargetError::IdentityMismatch);
                }
                self.require_current_network_binding(&device_id, network_generation)?;
                let (credential, _) = self.authority()?;
                if credential.device_id != device_id {
                    return Err(RemoteComputerTargetError::IdentityMismatch);
                }
                let binding = SensitiveInputBinding {
                    account_id: credential.user_id,
                    device_id: device_id.clone(),
                    client_id: client_id.clone(),
                    session_id: session_id.clone(),
                    operation_id: request_id.clone(),
                    request_sha256: request_sha256.clone(),
                    network_generation,
                    authorization_generation: grant_generation,
                };
                let challenge = self
                    .sensitive_inputs
                    .issue(binding, unix_millis()?, ttl_ms)
                    .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
                write_private_frame(worker, stream_id, &json!({
                    "type": "sensitiveInputChallenge",
                    "deviceId": device_id,
                    "sessionId": session_id,
                    "clientId": client_id,
                    "requestId": request_id,
                    "requestSha256": request_sha256,
                    "grantGeneration": grant_generation,
                    "challengeId": challenge.challenge_id,
                    "expiresAtMs": challenge.expires_at_ms,
                    "networkGeneration": network_generation,
                }))
            }
            RemoteTargetPrivateRequest::SensitiveInputSubmit {
                device_id,
                network_generation,
                session_id,
                client_id,
                request_id,
                request_sha256,
                grant_generation,
                challenge_id,
                snapshot_generation,
                payload_base64,
            } => {
                if !mahayana_computer::control_policy().remote_control_enabled {
                    return Err(RemoteComputerTargetError::AuthorityUnavailable);
                }
                self.require_current_network_binding(&device_id, network_generation)?;
                let claimed =
                    self.claim_operation(&session_id, &request_id, "Computer", &request_sha256)?;
                if claimed.client_id != client_id || claimed.grant_generation != grant_generation {
                    return Err(RemoteComputerTargetError::IdentityMismatch);
                }
                self.require_current_network_binding(&device_id, network_generation)?;
                let (credential, _) = self.authority()?;
                if credential.device_id != device_id {
                    return Err(RemoteComputerTargetError::IdentityMismatch);
                }
                let binding = SensitiveInputBinding {
                    account_id: credential.user_id,
                    device_id: device_id.clone(),
                    client_id: client_id.clone(),
                    session_id: session_id.clone(),
                    operation_id: request_id.clone(),
                    request_sha256: request_sha256.clone(),
                    network_generation,
                    authorization_generation: grant_generation,
                };
                let mut encoded = payload_base64.into_bytes();
                let decoded = base64::engine::general_purpose::STANDARD.decode(&encoded);
                encoded.fill(0);
                let payload = decoded.map_err(|_| RemoteComputerTargetError::InvalidResponse)?;
                let delivered = self
                    .sensitive_inputs
                    .consume(
                        &challenge_id,
                        &binding,
                        unix_millis()?,
                        payload,
                        |secret| {
                            self.require_current_network_binding(&device_id, network_generation)?;
                            let bytes = std::mem::take(secret);
                            let text = match String::from_utf8(bytes) {
                                Ok(text) => text,
                                Err(error) => {
                                    let mut bytes = error.into_bytes();
                                    bytes.fill(0);
                                    return Err(RemoteComputerTargetError::InvalidResponse);
                                }
                            };
                            let mut action = ComputerAction {
                                action: ComputerActionKind::Type,
                                x: None,
                                y: None,
                                x2: None,
                                y2: None,
                                path: None,
                                text: Some(text),
                                key: None,
                                button: None,
                                click_count: None,
                                direction: None,
                                amount: None,
                                wait_ms: None,
                                description: Some("user-provided sensitive input".into()),
                            };
                            let lease = mahayana_computer::ComputerControlLeaseRequest::new(
                                format!("official-device-mcp:{}", client_id),
                                session_id.clone(),
                                "local-desktop",
                                ComputerControlOrigin::RemoteMobile,
                                "remote-human",
                            );
                            let executed = mahayana_computer::execute_with_lease_fenced(
                                std::slice::from_ref(&action),
                                ComputerControlOrigin::RemoteMobile,
                                &lease,
                                Some(snapshot_generation),
                            );
                            if let Some(text) = action.text.take() {
                                let mut bytes = text.into_bytes();
                                bytes.fill(0);
                            }
                            match executed {
                                Ok(observed) => Ok(json!({
                                    "content": "Sensitive input was delivered to the authorized target desktop.",
                                    "sensitiveInputDelivered": true,
                                    "snapshotGeneration": observed.snapshot_generation,
                                })),
                                Err(mahayana_computer::ComputerError::SnapshotStale { expected, current }) => Ok(json!({
                                    "content": "Sensitive input was not delivered because the target observation is stale.",
                                    "sensitiveInputDelivered": false,
                                    "code": "snapshot_stale",
                                    "expectedSnapshotGeneration": expected,
                                    "currentSnapshotGeneration": current,
                                })),
                                Err(_) => Ok(json!({
                                    "content": "Sensitive input delivery failed on the authorized target desktop.",
                                    "sensitiveInputDelivered": false,
                                    "code": "sensitive_input_delivery_failed",
                                })),
                            }
                        },
                    )
                    .map_err(|_| RemoteComputerTargetError::IdentityMismatch)??;
                let status = if delivered
                    .get("sensitiveInputDelivered")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    "completed"
                } else {
                    "failed"
                };
                self.finish_native_operation(
                    worker,
                    stream_id,
                    &claimed,
                    &device_id,
                    network_generation,
                    status,
                    delivered,
                )
            }
            RemoteTargetPrivateRequest::SensitiveResultResume {
                device_id,
                network_generation,
                session_id,
                client_id,
                request_id,
                request_sha256,
                grant_generation,
            } => {
                let access = self.resume_sensitive_result(
                    &device_id,
                    network_generation,
                    &session_id,
                    &client_id,
                    &request_id,
                    &request_sha256,
                    grant_generation,
                )?;
                write_private_frame(worker, stream_id, &json!({
                    "type": "sensitiveResultReady",
                    "deviceId": device_id,
                    "sessionId": session_id,
                    "clientId": client_id,
                    "requestId": request_id,
                    "requestSha256": request_sha256,
                    "grantGeneration": grant_generation,
                    "resultRef": access.result_ref,
                    "resultSha256": access.result_sha256,
                    "mediaType": access.media_type,
                    "capturedAtMs": access.captured_at_ms,
                    "snapshotGeneration": access.snapshot_generation,
                    "width": access.width,
                    "height": access.height,
                    "expiresAt": access.expires_at,
                    "nextOffset": access.next_offset,
                    "networkGeneration": network_generation,
                }))
            }
            RemoteTargetPrivateRequest::SensitiveResultChunk {
                device_id,
                network_generation,
                session_id,
                client_id,
                request_id,
                request_sha256,
                grant_generation,
                result_ref,
                offset,
            } => {
                let chunk = self.read_sensitive_result_chunk(
                    &device_id,
                    network_generation,
                    &session_id,
                    &client_id,
                    &request_id,
                    &request_sha256,
                    grant_generation,
                    &result_ref,
                    offset,
                )?;
                write_private_frame(worker, stream_id, &chunk)
            }
            RemoteTargetPrivateRequest::SensitiveResultRevoke {
                device_id,
                network_generation,
                session_id,
                client_id,
                request_id,
                request_sha256,
                grant_generation,
                result_ref,
            } => {
                self.revoke_sensitive_result(
                    &device_id,
                    network_generation,
                    &session_id,
                    &client_id,
                    &request_id,
                    &request_sha256,
                    grant_generation,
                    &result_ref,
                )?;
                write_private_frame(worker, stream_id, &json!({
                    "type": "sensitiveResultRevoked",
                    "requestId": request_id,
                    "resultRef": result_ref,
                }))
            }
        }
    }

    fn finish_sensitive_operation(
        &self,
        worker: &mut OwnedNetworkWorker,
        stream_id: &str,
        claimed: &ClaimedRemoteOperation,
        device_id: &str,
        network_generation: i64,
        result: RemoteComputerSensitiveResult,
    ) -> Result<(), RemoteComputerTargetError> {
        if !claimed.sensitive_result || result.bytes.is_empty() || result.bytes.len() > REMOTE_SENSITIVE_RESULT_MAX_BYTES {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let (credential, _) = self.authority()?;
        if credential.device_id != device_id {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        self.require_current_network_binding(device_id, network_generation)?;
        let now = unix_seconds()?;
        let result_sha256 = sha256_hex(&result.bytes);
        let snapshot_generation = result.snapshot_generation;
        let result_ref = format!("sensitive-result-{}", Uuid::new_v4());
        let expires_at = now.saturating_add(REMOTE_SENSITIVE_RESULT_TTL_SECONDS);
        let entry = SensitiveResultEntry {
            account_user_id: credential.user_id,
            device_id: device_id.to_string(),
            network_generation,
            session_id: claimed.session_id.clone(),
            client_id: claimed.client_id.clone(),
            request_id: claimed.request_id.clone(),
            request_sha256: claimed.request_sha256.clone(),
            tool_name: claimed.tool_name.clone(),
            grant_generation: claimed.grant_generation,
            result_sha256: result_sha256.clone(),
            media_type: result.media_type,
            bytes: result.bytes,
            captured_at_ms: result.captured_at_ms,
            snapshot_generation,
            width: result.width,
            height: result.height,
            expires_at,
            next_offset: 0,
        };
        {
            let mut vault = self.sensitive_results
                .lock()
                .map_err(|_| RemoteComputerTargetError::OutcomeUnknown)?;
            vault.insert(result_ref.clone(), entry, now);
        }
        if self.publish_result(claimed, "completed", &result_sha256, None).is_err() {
            if let Ok(mut vault) = self.sensitive_results.lock() {
                vault.entries.remove(&result_ref);
            }
            return Err(RemoteComputerTargetError::OutcomeUnknown);
        }
        write_private_frame(worker, stream_id, &json!({
            "type": "operationResult",
            "deviceId": device_id,
            "sessionId": claimed.session_id,
            "clientId": claimed.client_id,
            "requestId": claimed.request_id,
            "requestSha256": claimed.request_sha256,
            "grantGeneration": claimed.grant_generation,
            "resultSha256": result_sha256,
            "status": "completed",
            "sensitiveResult": true,
            "resultRef": result_ref,
            "snapshotGeneration": snapshot_generation,
            "expiresAt": expires_at,
            "networkGeneration": network_generation,
        }))
    }

    fn resume_sensitive_result(
        &self,
        device_id: &str,
        network_generation: i64,
        session_id: &str,
        client_id: &str,
        request_id: &str,
        request_sha256: &str,
        grant_generation: i64,
    ) -> Result<SensitiveResultAccess, RemoteComputerTargetError> {
        let now = unix_seconds()?;
        let mut vault = self.sensitive_results
            .lock()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        vault.purge_expired(now);
        let (result_ref, entry) = vault.entries.iter()
            .find(|(_, entry)| {
                entry.device_id == device_id
                    && entry.network_generation == network_generation
                    && entry.session_id == session_id
                    && entry.client_id == client_id
                    && entry.request_id == request_id
                    && entry.request_sha256 == request_sha256
                    && entry.grant_generation == grant_generation
            })
            .map(|(result_ref, entry)| (result_ref.clone(), entry.clone()))
            .ok_or(RemoteComputerTargetError::IdentityMismatch)?;
        drop(vault);
        self.validate_sensitive_entry(
            &entry, device_id, network_generation, session_id, client_id,
            request_id, request_sha256, grant_generation,
        )?;
        Ok(SensitiveResultAccess {
            result_ref,
            result_sha256: entry.result_sha256,
            media_type: entry.media_type,
            captured_at_ms: entry.captured_at_ms,
            snapshot_generation: entry.snapshot_generation,
            width: entry.width,
            height: entry.height,
            expires_at: entry.expires_at,
            next_offset: entry.next_offset,
        })
    }

    fn read_sensitive_result_chunk(
        &self,
        device_id: &str,
        network_generation: i64,
        session_id: &str,
        client_id: &str,
        request_id: &str,
        request_sha256: &str,
        grant_generation: i64,
        result_ref: &str,
        offset: usize,
    ) -> Result<Value, RemoteComputerTargetError> {
        let now = unix_seconds()?;
        let entry = {
            let mut vault = self.sensitive_results
                .lock()
                .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
            vault.purge_expired(now);
            vault.entries.get(result_ref).cloned()
                .ok_or(RemoteComputerTargetError::IdentityMismatch)?
        };
        self.validate_sensitive_entry(
            &entry, device_id, network_generation, session_id, client_id,
            request_id, request_sha256, grant_generation,
        )?;
        let (end, done) = sensitive_chunk_range(&entry, offset)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&entry.bytes[offset..end]);
        {
            let mut vault = self.sensitive_results
                .lock()
                .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
            let current = vault.entries.get_mut(result_ref)
                .ok_or(RemoteComputerTargetError::IdentityMismatch)?;
            if current.next_offset != offset {
                return Err(RemoteComputerTargetError::IdentityMismatch);
            }
            if done {
                vault.entries.remove(result_ref);
            } else {
                current.next_offset = end;
            }
        }
        Ok(json!({
            "type": "sensitiveResultChunk",
            "deviceId": device_id,
            "sessionId": session_id,
            "clientId": client_id,
            "requestId": request_id,
            "requestSha256": request_sha256,
            "grantGeneration": grant_generation,
            "resultRef": result_ref,
            "resultSha256": entry.result_sha256,
            "mediaType": entry.media_type,
            "offset": offset,
            "nextOffset": end,
            "done": done,
            "dataBase64": encoded,
            "expiresAt": entry.expires_at,
            "networkGeneration": network_generation,
        }))
    }

    fn revoke_sensitive_result(
        &self,
        device_id: &str,
        network_generation: i64,
        session_id: &str,
        client_id: &str,
        request_id: &str,
        request_sha256: &str,
        grant_generation: i64,
        result_ref: &str,
    ) -> Result<(), RemoteComputerTargetError> {
        let now = unix_seconds()?;
        let entry = {
            let mut vault = self.sensitive_results
                .lock()
                .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
            vault.purge_expired(now);
            vault.entries.get(result_ref).cloned()
                .ok_or(RemoteComputerTargetError::IdentityMismatch)?
        };
        self.validate_sensitive_entry(
            &entry, device_id, network_generation, session_id, client_id,
            request_id, request_sha256, grant_generation,
        )?;
        let mut vault = self.sensitive_results
            .lock()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        vault.entries.remove(result_ref);
        Ok(())
    }

    fn finish_native_operation(
        &self,
        worker: &mut OwnedNetworkWorker,
        stream_id: &str,
        claimed: &ClaimedRemoteOperation,
        device_id: &str,
        network_generation: i64,
        status: &str,
        result: Value,
    ) -> Result<(), RemoteComputerTargetError> {
        let result_bytes =
            serde_json::to_vec(&result).map_err(|_| RemoteComputerTargetError::OutcomeUnknown)?;
        if result_bytes.len() > REMOTE_TARGET_RESULT_MAX_BYTES {
            return Err(RemoteComputerTargetError::OutcomeUnknown);
        }
        let result_sha256 = sha256_hex(&result_bytes);
        let core_result = if claimed.sensitive_result {
            None
        } else {
            Some(result)
        };
        self.publish_result(claimed, status, &result_sha256, core_result)
            .map_err(|_| RemoteComputerTargetError::OutcomeUnknown)?;
        write_private_frame(
            worker,
            stream_id,
            &json!({
                "type": "operationResult",
                "deviceId": device_id,
                "sessionId": claimed.session_id,
                "requestId": claimed.request_id,
                "requestSha256": claimed.request_sha256,
                "resultSha256": result_sha256,
                "status": status,
                "networkGeneration": network_generation,
            }),
        )
    }

    fn require_current_network_binding(
        &self,
        device_id: &str,
        expected_generation: i64,
    ) -> Result<RemoteNetworkBinding, RemoteComputerTargetError> {
        let binding = self.network_binding()?;
        if binding.device_id != device_id
            || binding.generation != expected_generation
            || !binding.active
            || binding.expires_at <= unix_seconds()?
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        Ok(binding)
    }

    pub fn issue_network_enrollment(
        &self,
        request_id: &str,
    ) -> Result<RemoteNetworkEnrollmentGrant, RemoteComputerTargetError> {
        if request_id.trim().is_empty() || request_id.len() > 160 {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&[
                "v1",
                "computers",
                &credential.device_id,
                "network-enrollment-grants",
            ])?,
            json!({
                "deviceSecret": device_secret,
                "requestId": request_id,
            }),
        )?;
        if response.get("requestId").and_then(Value::as_str) != Some(request_id) {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let grant_id = required_text(&response, "grantId")?;
        let grant_secret = required_text(&response, "grantSecret")?;
        let cell_id = required_text(&response, "cellId")?;
        let generation = response
            .get("generation")
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        let expires_at = response
            .get("expiresAt")
            .and_then(Value::as_i64)
            .filter(|value| *value > unix_seconds().unwrap_or_default())
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        if !grant_id.starts_with("network-enroll-")
            || grant_id.len() > 96
            || grant_secret.len() < 80
            || grant_secret.len() > 192
        {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        Ok(RemoteNetworkEnrollmentGrant {
            grant_id,
            grant_secret,
            cell_id,
            generation,
            expires_at,
            request_id: request_id.to_string(),
            reconciled: response.get("reconciled").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    pub fn revoke_network_enrollment(
        &self,
        grant_id: &str,
    ) -> Result<bool, RemoteComputerTargetError> {
        if !grant_id.starts_with("network-enroll-") || grant_id.len() > 96 {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&[
                "v1",
                "computers",
                &credential.device_id,
                "network-enrollment-grants",
                grant_id,
                "revoke",
            ])?,
            json!({"deviceSecret": device_secret}),
        )?;
        if response.get("grantId").and_then(Value::as_str) != Some(grant_id) {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        response
            .get("revoked")
            .and_then(Value::as_bool)
            .ok_or(RemoteComputerTargetError::InvalidResponse)
    }

    pub fn network_binding(&self) -> Result<RemoteNetworkBinding, RemoteComputerTargetError> {
        let (credential, device_secret) = self.authority()?;
        let response = self.post_json(
            &credential,
            self.endpoint(&["v1", "computers", "network-binding"])?,
            json!({
                "deviceId": credential.device_id,
                "deviceSecret": device_secret
            }),
        )?;
        if response.get("deviceId").and_then(Value::as_str) != Some(credential.device_id.as_str()) {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let generation = response
            .get("generation")
            .and_then(Value::as_i64)
            .filter(|generation| *generation > 0)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        let expires_at = response
            .get("expiresAt")
            .and_then(Value::as_i64)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        Ok(RemoteNetworkBinding {
            device_id: credential.device_id,
            cell_id: required_text(&response, "cellId")?,
            node_id: required_text(&response, "nodeId")?,
            proof_sha256: required_text(&response, "proofSha256")?,
            generation,
            expires_at,
            active: response.get("active").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

pub struct FabushiNetworkEnrollmentBrokerClient {
    base_url: Url,
    credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
    http: Client,
}

impl std::fmt::Debug for FabushiNetworkEnrollmentBrokerClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FabushiNetworkEnrollmentBrokerClient")
            .field("base_url", &self.base_url)
            .field("credential_provider", &"<first-party>")
            .finish_non_exhaustive()
    }
}

impl FabushiNetworkEnrollmentBrokerClient {
    pub fn new(
        base_url: &str,
        credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
    ) -> Result<Self, RemoteComputerTargetError> {
        let mut base_url =
            Url::parse(base_url).map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        let loopback = matches!(base_url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if base_url.scheme() != "https" && !(loopback && base_url.scheme() == "http") {
            return Err(RemoteComputerTargetError::AuthorityUnavailable);
        }
        base_url.set_path("/");
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(CORE_REQUEST_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        Ok(Self { base_url, credential_provider, http })
    }

    fn endpoint(&self, action: &str) -> Result<Url, RemoteComputerTargetError> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
            path.clear();
            for segment in ["api", "fdn", "network-enrollment", action] {
                path.push(segment);
            }
        }
        Ok(url)
    }

    fn post_json(&self, action: &str, body: Value) -> Result<(FabushiCoreCredential, Value), RemoteComputerTargetError> {
        let credential = self
            .credential_provider
            .get_fabushi_core_credential()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        credential.authorize_origin(self.base_url.as_str())
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        let response = self
            .http
            .post(self.endpoint(action)?)
            .bearer_auth(credential.access_token())
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .map_err(|_| RemoteComputerTargetError::RequestFailed)?;
        if !response.status().is_success() {
            return Err(RemoteComputerTargetError::RequestFailed);
        }
        let bytes = response
            .bytes()
            .map_err(|_| RemoteComputerTargetError::RequestFailed)?;
        if bytes.len() > MAX_CORE_RESPONSE_BYTES {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let payload: Value =
            serde_json::from_slice(&bytes).map_err(|_| RemoteComputerTargetError::InvalidResponse)?;
        if payload.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(RemoteComputerTargetError::RequestFailed);
        }
        Ok((credential, payload))
    }

    pub fn start(
        &self,
        grant: &RemoteNetworkEnrollmentGrant,
    ) -> Result<RemoteNetworkEnrollmentStarted, RemoteComputerTargetError> {
        let (credential, payload) = self.post_json(
            "start",
            json!({"grantId": grant.grant_id, "grantSecret": grant.grant_secret()}),
        )?;
        let enrollment = payload
            .get("enrollment")
            .and_then(Value::as_object)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        if enrollment.get("grantId").and_then(Value::as_str) != Some(grant.grant_id.as_str())
            || enrollment.get("deviceId").and_then(Value::as_str) != Some(credential.device_id.as_str())
            || enrollment.get("cellId").and_then(Value::as_str) != Some(grant.cell_id.as_str())
            || enrollment.get("generation").and_then(Value::as_i64) != Some(grant.generation)
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let auth_key = required_text(&Value::Object(enrollment.clone()), "authKey")?;
        if auth_key.len() < 48 || auth_key.len() > 256 {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        Ok(RemoteNetworkEnrollmentStarted {
            grant_id: grant.grant_id.clone(),
            auth_key,
            device_id: credential.device_id,
            cell_id: grant.cell_id.clone(),
            generation: grant.generation,
            expires_at: enrollment
                .get("expiresAt")
                .and_then(Value::as_i64)
                .filter(|value| *value > unix_seconds().unwrap_or_default())
                .ok_or(RemoteComputerTargetError::InvalidResponse)?,
            reconciled: enrollment.get("reconciled").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    pub fn complete(
        &self,
        started: &RemoteNetworkEnrollmentStarted,
    ) -> Result<RemoteNetworkEnrollmentCompleted, RemoteComputerTargetError> {
        let (_credential, payload) =
            self.post_json("complete", json!({"grantId": started.grant_id}))?;
        let enrollment = payload
            .get("enrollment")
            .and_then(Value::as_object)
            .ok_or(RemoteComputerTargetError::InvalidResponse)?;
        if enrollment.get("grantId").and_then(Value::as_str) != Some(started.grant_id.as_str())
            || enrollment.get("deviceId").and_then(Value::as_str) != Some(started.device_id.as_str())
            || enrollment.get("cellId").and_then(Value::as_str) != Some(started.cell_id.as_str())
            || enrollment.get("generation").and_then(Value::as_i64) != Some(started.generation)
            || enrollment.get("completed").and_then(Value::as_bool) != Some(true)
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        Ok(RemoteNetworkEnrollmentCompleted {
            grant_id: started.grant_id.clone(),
            device_id: started.device_id.clone(),
            cell_id: started.cell_id.clone(),
            node_id: required_text(&Value::Object(enrollment.clone()), "nodeId")?,
            generation: started.generation,
            reconciled: enrollment.get("reconciled").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}


fn unix_millis() -> Result<i64, RemoteComputerTargetError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?
        .as_millis();
    i64::try_from(millis).map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)
}

fn unix_seconds() -> Result<i64, RemoteComputerTargetError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?
        .as_secs();
    i64::try_from(seconds).map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)
}

pub struct RemoteComputerTargetNetworkService {
    client: Arc<FabushiCoreTargetClient>,
    catalog: Arc<dyn RemoteComputerToolCatalog>,
    authorizer: Arc<dyn RemoteComputerToolAuthorizer>,
    executor: Arc<dyn RemoteComputerNativeExecutor>,
    local_port: u16,
    ingress_cache: Mutex<Option<(RemoteNetworkIngress, Instant)>>,
}

impl RemoteComputerTargetNetworkService {
    pub fn new(
        client: Arc<FabushiCoreTargetClient>,
        catalog: Arc<dyn RemoteComputerToolCatalog>,
        authorizer: Arc<dyn RemoteComputerToolAuthorizer>,
        executor: Arc<dyn RemoteComputerNativeExecutor>,
        local_port: u16,
    ) -> Result<Self, RemoteComputerTargetError> {
        if local_port == 0 {
            return Err(RemoteComputerTargetError::TransportFailed);
        }
        Ok(Self {
            client,
            catalog,
            authorizer,
            executor,
            local_port,
            ingress_cache: Mutex::new(None),
        })
    }

    fn ingress(&self) -> Result<RemoteNetworkIngress, RemoteComputerTargetError> {
        let now = unix_seconds()?;
        {
            let cache = self
                .ingress_cache
                .lock()
                .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
            if let Some((ingress, fetched_at)) = cache.as_ref() {
                if fetched_at.elapsed() <= REMOTE_TARGET_INGRESS_CACHE_TTL
                    && ingress.active
                    && ingress.expires_at > now + 15
                {
                    return Ok(ingress.clone());
                }
            }
        }
        let ingress = self.client.network_ingress("official-device-mcp")?;
        let binding = self.client.network_binding()?;
        if binding.device_id != ingress.device_id
            || binding.cell_id != ingress.cell_id
            || binding.generation != ingress.target_generation
            || !binding.active
            || binding.expires_at <= now
        {
            return Err(RemoteComputerTargetError::IdentityMismatch);
        }
        let mut cache = self
            .ingress_cache
            .lock()
            .map_err(|_| RemoteComputerTargetError::AuthorityUnavailable)?;
        *cache = Some((ingress.clone(), Instant::now()));
        Ok(ingress)
    }
}

pub fn production_remote_computer_target_service(
    base_url: &str,
    credential_provider: Arc<dyn FabushiCoreCredentialProvider>,
    device_secret_provider: Arc<dyn RemoteComputerDeviceSecretProvider>,
) -> Result<Arc<dyn NetworkWorkerService>, RemoteComputerTargetError> {
    let client = Arc::new(FabushiCoreTargetClient::new(base_url, credential_provider, device_secret_provider)?);
    Ok(Arc::new(RemoteComputerTargetNetworkService::new(
        client,
        Arc::new(NativeDesktopRemoteToolCatalog),
        Arc::new(NativeDesktopRemoteToolAuthorizer),
        Arc::new(NativeDesktopRemoteExecutor),
        REMOTE_COMPUTER_TARGET_PORT,
    )?))
}

impl NetworkWorkerService for RemoteComputerTargetNetworkService {
    fn poll(&self, worker: &mut OwnedNetworkWorker) -> Result<(), NetworkEnrollmentError> {
        let ingress = self.ingress().map_err(|error| match error {
            RemoteComputerTargetError::AuthorityUnavailable => NetworkEnrollmentError::AuthorityUnavailable,
            RemoteComputerTargetError::RequestFailed | RemoteComputerTargetError::TransportFailed => {
                NetworkEnrollmentError::RequestFailed
            }
            RemoteComputerTargetError::OutcomeUnknown => NetworkEnrollmentError::OutcomeUnknown,
            RemoteComputerTargetError::IdentityMismatch | RemoteComputerTargetError::InvalidResponse => {
                NetworkEnrollmentError::RequestRejected
            }
        })?;
        self.client
            .serve_private_request_with_timeout(
                worker,
                &ingress.node_id,
                self.local_port,
                REMOTE_TARGET_SERVICE_STREAM_TIMEOUT_MS,
                self.catalog.as_ref(),
                self.authorizer.as_ref(),
                self.executor.as_ref(),
            )
            .map_err(|error| match error {
                RemoteComputerTargetError::AuthorityUnavailable => NetworkEnrollmentError::AuthorityUnavailable,
                RemoteComputerTargetError::RequestFailed | RemoteComputerTargetError::TransportFailed => {
                    NetworkEnrollmentError::RequestFailed
                }
                RemoteComputerTargetError::OutcomeUnknown => NetworkEnrollmentError::OutcomeUnknown,
                RemoteComputerTargetError::IdentityMismatch | RemoteComputerTargetError::InvalidResponse => {
                    NetworkEnrollmentError::RequestRejected
                }
            })
    }
}

fn read_private_frame(
    worker: &mut OwnedNetworkWorker,
    stream_id: &str,
) -> Result<Vec<u8>, RemoteComputerTargetError> {
    read_private_frame_with(
        |max_bytes, timeout_ms| worker.stream_read(stream_id, max_bytes, timeout_ms),
        Instant::now,
    )
}

fn read_private_frame_with<R, N>(
    mut read: R,
    mut now: N,
) -> Result<Vec<u8>, RemoteComputerTargetError>
where
    R: FnMut(usize, u64) -> Result<NetworkWorkerStreamRead, String>,
    N: FnMut() -> Instant,
{
    let deadline = now() + Duration::from_millis(REMOTE_TARGET_STREAM_TIMEOUT_MS);
    let mut frame = Vec::new();
    loop {
        let timeout_ms = deadline.saturating_duration_since(now()).as_millis() as u64;
        if timeout_ms == 0 {
            return Err(RemoteComputerTargetError::TransportFailed);
        }
        let remaining = REMOTE_TARGET_FRAME_MAX_BYTES.saturating_sub(frame.len());
        if remaining == 0 {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
        let read = read(remaining, timeout_ms)
            .map_err(|_| RemoteComputerTargetError::TransportFailed)?;
        if now() >= deadline {
            return Err(RemoteComputerTargetError::TransportFailed);
        }
        frame.extend_from_slice(&read.data);
        if let Some(newline) = frame.iter().position(|byte| *byte == b'\n') {
            if frame[newline + 1..]
                .iter()
                .any(|byte| !byte.is_ascii_whitespace())
            {
                return Err(RemoteComputerTargetError::InvalidResponse);
            }
            frame.truncate(newline);
            if frame.is_empty() {
                return Err(RemoteComputerTargetError::InvalidResponse);
            }
            return Ok(frame);
        }
        if read.eof {
            return Err(RemoteComputerTargetError::InvalidResponse);
        }
    }
}

#[cfg(test)]
#[path = "remote_computer_target/frame_tests.rs"]
mod private_frame_tests;

fn write_private_frame(
    worker: &mut OwnedNetworkWorker,
    stream_id: &str,
    value: &Value,
) -> Result<(), RemoteComputerTargetError> {
    let mut encoded =
        serde_json::to_vec(value).map_err(|_| RemoteComputerTargetError::InvalidResponse)?;
    if encoded.len() >= REMOTE_TARGET_FRAME_MAX_BYTES {
        return Err(RemoteComputerTargetError::InvalidResponse);
    }
    encoded.push(b'\n');
    let written = worker
        .stream_write(stream_id, &encoded, REMOTE_TARGET_STREAM_TIMEOUT_MS)
        .map_err(|_| RemoteComputerTargetError::TransportFailed)?;
    if written != encoded.len() {
        return Err(RemoteComputerTargetError::TransportFailed);
    }
    Ok(())
}

fn tool_descriptor_sha256(value: &Value) -> Result<String, RemoteComputerTargetError> {
    let mut canonical = String::new();
    canonical_json(value, &mut canonical)?;
    Ok(sha256_hex(canonical.as_bytes()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    use std::fmt::Write as _;
    for byte in digest {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

fn required_text(value: &Value, key: &str) -> Result<String, RemoteComputerTargetError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or(RemoteComputerTargetError::InvalidResponse)
}


#[cfg(test)]
mod sensitive_result_tests {
    use super::*;

    fn entry(now: i64) -> SensitiveResultEntry {
        SensitiveResultEntry {
            account_user_id: "account-a".into(),
            device_id: "device-a".into(),
            network_generation: 7,
            session_id: "session-a".into(),
            client_id: "client-a".into(),
            request_id: "request-00000001".into(),
            request_sha256: "a".repeat(64),
            tool_name: "Computer".into(),
            grant_generation: 9,
            result_sha256: "b".repeat(64),
            media_type: "image/png".into(),
            bytes: vec![42; REMOTE_SENSITIVE_RESULT_CHUNK_BYTES + 3],
            captured_at_ms: 123,
            width: Some(100),
            height: Some(80),
            snapshot_generation: 1,
            expires_at: now + 60,
            next_offset: 0,
        }
    }

    #[test]
    fn sensitive_binding_rejects_wrong_account_device_session_digest_and_generation() {
        let item = entry(100);
        assert!(FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-a", "device-a", "device-a", 7, "session-a", "client-a",
            "request-00000001", &"a".repeat(64), 9,
        ));
        assert!(!FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-b", "device-a", "device-a", 7, "session-a", "client-a",
            "request-00000001", &"a".repeat(64), 9,
        ));
        assert!(!FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-a", "device-b", "device-a", 7, "session-a", "client-a",
            "request-00000001", &"a".repeat(64), 9,
        ));
        assert!(!FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-a", "device-a", "device-a", 7, "session-b", "client-a",
            "request-00000001", &"a".repeat(64), 9,
        ));
        assert!(!FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-a", "device-a", "device-a", 7, "session-a", "client-b",
            "request-00000001", &"a".repeat(64), 9,
        ));
        assert!(!FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-a", "device-a", "device-a", 7, "session-a", "client-a",
            "request-00000001", &"c".repeat(64), 9,
        ));
        assert!(!FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-a", "device-a", "device-a", 8, "session-a", "client-a",
            "request-00000001", &"a".repeat(64), 9,
        ));
        assert!(!FabushiCoreTargetClient::sensitive_entry_matches(
            &item, "account-a", "device-a", "device-a", 7, "session-a", "client-a",
            "request-00000001", &"a".repeat(64), 10,
        ));
    }

    #[test]
    fn sensitive_chunk_range_is_sequential_and_replay_rejected() {
        let mut item = entry(100);
        let (end, done) = sensitive_chunk_range(&item, 0).expect("first chunk");
        assert_eq!(end, REMOTE_SENSITIVE_RESULT_CHUNK_BYTES);
        assert!(!done);
        item.next_offset = end;
        assert_eq!(
            sensitive_chunk_range(&item, 0),
            Err(RemoteComputerTargetError::IdentityMismatch),
            "old offset must not replay"
        );
        let (final_end, final_done) = sensitive_chunk_range(&item, end).expect("final chunk");
        assert_eq!(final_end, item.bytes.len());
        assert!(final_done);
    }

    #[test]
    fn sensitive_vault_purges_expired_entries_and_is_bounded() {
        let mut vault = SensitiveResultVault::default();
        vault.insert("expired".into(), entry(0), 0);
        vault.purge_expired(61);
        assert!(vault.entries.is_empty());

        for index in 0..(REMOTE_SENSITIVE_RESULT_MAX_ENTRIES + 2) {
            let mut item = entry(100);
            item.expires_at = 200 + index as i64;
            vault.insert(format!("ref-{index}"), item, 100);
        }
        assert!(vault.entries.len() <= REMOTE_SENSITIVE_RESULT_MAX_ENTRIES);
    }
}
