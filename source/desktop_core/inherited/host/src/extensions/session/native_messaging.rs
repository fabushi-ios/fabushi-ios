use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

const DEFAULT_FABUSHI_API_BASE_URL: &str = "https://api.ombhrum.com";
const CI_ISOLATED_FABUSHI_AUTH_BASE_URL: &str = "https://fabushi-ci-isolated-auth.bhrumom.workers.dev";
const ISOLATED_AUTH_ID_DOMAIN: &str = "fabushi-native-auth-id-v1";
const FABUSHI_API_BASE_URL_ENV: &str = "FABUSHI_API_BASE_URL";
const FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV: &str = "FABUSHI_HOST_ACCESS_CREDENTIAL_FILE";
// Current Native message-resource service limit, shared with forward snapshots.
// Shared Agent video and downloaded-resource budgets retain their own limits.
pub(super) const MAX_HUMAN_RESOURCE_UPLOAD_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabushiNativeIdentity {
    pub user_id: String,
    pub device_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FabushiRemoteHumanContact {
    pub id: Value,
    pub user_id: Value,
    pub username: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FabushiRemoteHumanAttachment {
    pub resource_id: String,
    pub name: String,
    pub content_type: String,
    pub size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FabushiRemoteHumanReaction {
    pub emoji: String,
    pub count: u64,
    pub reacted_by_me: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FabushiRemoteHumanMessage {
    pub id: Value,
    pub sender_user_id: Value,
    pub sender_username: Option<String>,
    pub recipient_user_id: Value,
    pub recipient_username: Option<String>,
    pub text: String,
    pub client_request_id: Option<String>,
    pub created_at: String,
    pub read_at: Option<String>,
    #[serde(default)]
    pub is_outgoing: bool,
    pub reply_to_message_id: Option<Value>,
    #[serde(default)]
    pub attachments: Vec<FabushiRemoteHumanAttachment>,
    // Missing means a partial snapshot. An explicit [] is an authoritative clear.
    #[serde(default)]
    pub reactions: Option<Vec<FabushiRemoteHumanReaction>>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FabushiRemoteHumanCall {
    pub call_id: String,
    pub creator_user_id: Value,
    pub peer_user_id: Value,
    #[serde(default)]
    pub creator_device_id: Option<String>,
    #[serde(default)]
    pub peer_device_id: Option<String>,
    pub state: String,
    pub generation: u64,
    pub event_seq: u64,
    pub terminal_state: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FabushiRemoteHumanCallEvent {
    pub call_id: String,
    pub seq: u64,
    pub generation: u64,
    pub user_id: Value,
    pub device_id: String,
    pub client_event_id: String,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
struct CredentialFile {
    #[serde(default)]
    issuer: Option<String>,
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "deviceId")]
    device_id: String,
    #[serde(rename = "userId")]
    user_id: Value,
    #[serde(rename = "sessionId", default)]
    session_id: String,
    #[serde(rename = "expiresAtMs", default)]
    expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FabushiNativeCredentialClaim {
    pub user_id: String,
    pub issuer: String,
    pub device_id: String,
    /// SHA-256 of the opaque Native sessionId; the identifier itself never leaves this owner.
    pub session_binding_hash: String,
    pub expires_at_ms: u64,
}

impl FabushiNativeCredentialClaim {
    /// Matches Electron's account-slot hash of the issuer-qualified Native authId.
    pub fn account_scope_hash(&self) -> String {
        let auth_id = if self.issuer == DEFAULT_FABUSHI_API_BASE_URL {
            self.user_id.clone()
        } else if self.issuer == CI_ISOLATED_FABUSHI_AUTH_BASE_URL {
            format!("fabushi-ci-isolated:v1:{}", sha256_hex(
                format!("{ISOLATED_AUTH_ID_DOMAIN}\0{}\0{}", self.issuer, self.user_id).as_bytes()
            ))
        } else {
            self.user_id.clone()
        };
        sha256_hex(auth_id.as_bytes())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FabushiIdentityVerificationError {
    Unauthenticated,
    Network,
    Rejected,
    IdentityMismatch,
}

#[derive(Debug, Deserialize)]
struct SendEnvelope {
    success: bool,
    message: Option<FabushiRemoteHumanMessage>,
    error: Option<String>,
}

// Only an ambiguous transport/read failure may be reconciled to an independently
// persisted acknowledgement. Invalid or rejected service responses stay errors.
pub(super) enum FabushiHumanSendError {
    Transport(String),
    Rejected(String),
}

impl FabushiHumanSendError {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::Transport(message) | Self::Rejected(message) => message,
        }
    }
}

impl From<String> for FabushiHumanSendError {
    fn from(message: String) -> Self {
        Self::Rejected(message)
    }
}

// Session must revalidate its original binding and durable target before this
// transport-level refusal can permit continued recovery.
pub(super) enum FabushiHumanReactionError {
    Failure(String),
    RemoteRefusal { status: u16, message: String },
}

impl FabushiHumanReactionError {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::Failure(message) | Self::RemoteRefusal { message, .. } => message,
        }
    }
}

impl From<String> for FabushiHumanReactionError {
    fn from(message: String) -> Self {
        Self::Failure(message)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListData {
    messages: Vec<FabushiRemoteHumanMessage>,
    #[serde(default)]
    history_cursor: Option<HistoryCursorEcho>,
    #[serde(default)]
    next_after_id: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryCursorEcho {
    mode: String,
    after_id: Value,
}

#[derive(Debug, Deserialize)]
struct ResourceEnvelope {
    success: bool,
    resource: Option<FabushiRemoteHumanAttachment>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FriendData {
    friends: Vec<FabushiRemoteHumanContact>,
}

#[derive(Debug, Deserialize)]
struct FriendEnvelope {
    success: bool,
    data: Option<FriendData>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListEnvelope {
    success: bool,
    data: Option<ListData>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendAttachmentRequest<'a> {
    resource_id: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendRequest<'a> {
    target_user_id: &'a str,
    text: &'a str,
    client_request_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<SendAttachmentRequest<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReactionRequest<'a> {
    emoji: &'a str,
    active: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReactionEnvelope {
    success: bool,
    message_id: Option<Value>,
    // Missing is not an authoritative empty summary; a mutation intent must
    // remain retryable until the backend explicitly reports its result.
    reactions: Option<Vec<FabushiRemoteHumanReaction>>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateCallRequest<'a> {
    call_id: &'a str,
    target_user_id: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppendCallEventRequest<'a> {
    client_event_id: &'a str,
    generation: u64,
    kind: &'a str,
    payload: &'a Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallEnvelope {
    success: bool,
    call: Option<FabushiRemoteHumanCall>,
    #[serde(default)]
    calls: Vec<FabushiRemoteHumanCall>,
    #[serde(default)]
    events: Vec<FabushiRemoteHumanCallEvent>,
    event: Option<FabushiRemoteHumanCallEvent>,
    next_after_seq: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FabushiCallIceServer {
    pub urls: Value,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallIceEnvelope {
    success: bool,
    #[serde(default)]
    ice_servers: Vec<FabushiCallIceServer>,
    ttl_seconds: Option<u64>,
    error: Option<String>,
}

fn valid_call_ice_urls(value: &Value) -> bool {
    let valid = |url: &str| {
        let trimmed = url.trim();
        !trimmed.is_empty()
            && trimmed.len() <= 1024
            && !trimmed.chars().any(char::is_whitespace)
            && (trimmed.starts_with("stun:")
                || trimmed.starts_with("turn:")
                || trimmed.starts_with("turns:"))
    };
    match value {
        Value::String(url) => valid(url),
        Value::Array(urls) => {
            !urls.is_empty()
                && urls.len() <= 16
                && urls.iter().all(|url| url.as_str().is_some_and(valid))
        }
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct FabushiNativeMessagingClient {
    base_url: Url,
    credential_path: PathBuf,
    client: Client,
}

impl FabushiNativeMessagingClient {
    pub fn from_env() -> Result<Self, String> {
        let base_url = env::var(FABUSHI_API_BASE_URL_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_FABUSHI_API_BASE_URL.to_string());
        let credential_path = env::var_os(FABUSHI_HOST_ACCESS_CREDENTIAL_FILE_ENV)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                "Fabushi native messaging requires the account-bound Host credential file.".to_string()
            })?;
        Self::new(&base_url, credential_path)
    }

    pub fn new(base_url: &str, credential_path: impl Into<PathBuf>) -> Result<Self, String> {
        let base_url = validate_base_url(base_url)?;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("could not create Fabushi messaging HTTP client: {error}"))?;
        Ok(Self {
            base_url,
            credential_path: credential_path.into(),
            client,
        })
    }

    pub fn identity(&self) -> Result<FabushiNativeIdentity, String> {
        let credentials = self.read_bound_credentials()?;
        Ok(FabushiNativeIdentity {
            user_id: fabushi_identity_text(&credentials.user_id)?,
            device_id: required_trimmed(&credentials.device_id, "deviceId")?.to_string(),
        })
    }

    /// A durable forward request binds to the Native account and issuer, not
    /// the bearer token or rotating device session. Legacy credential files
    /// remain supported through the same private service-origin checks.
    pub(super) fn forward_account_scope(&self) -> Result<(String, String), String> {
        let credentials = self.read_bound_credentials()?;
        Ok((
            fabushi_identity_text(&credentials.user_id)?,
            credential_issuer(credentials.issuer.as_deref())?,
        ))
    }

    /// Read only the existing private credential owner and produce a bounded
    /// local claim. The bearer token and raw sessionId remain private here.
    pub fn credential_claim(&self) -> Result<FabushiNativeCredentialClaim, String> {
        let credentials = self.read_bound_credentials()?;
        credential_claim_from(&credentials)
    }

    /// Confirm the private credential's userId with Fabushi's authenticated
    /// identity endpoint; never trust a caller-provided account identifier.
    pub fn verify_account_identity(&self) -> Result<FabushiNativeCredentialClaim, FabushiIdentityVerificationError> {
        let credentials = read_credentials(&self.credential_path)
            .map_err(|_| FabushiIdentityVerificationError::Unauthenticated)?;
        let claim = credential_claim_from(&credentials)
            .map_err(|_| FabushiIdentityVerificationError::Unauthenticated)?;
        if !issuer_matches_base(&claim.issuer, &self.base_url) {
            return Err(FabushiIdentityVerificationError::Rejected);
        }
        let response = self.client.get(self.endpoint("/api/auth/user-info")
                .map_err(|_| FabushiIdentityVerificationError::Rejected)?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")
                .map_err(|_| FabushiIdentityVerificationError::Unauthenticated)?)
            .header("accept", "application/json")
            .send()
            .map_err(|_| FabushiIdentityVerificationError::Network)?;
        classify_identity_http_status(response.status())?;
        let mut limited = response.take(1024 * 1024 + 1);
        let mut bytes = Vec::new();
        limited.read_to_end(&mut bytes).map_err(|_| FabushiIdentityVerificationError::Network)?;
        if bytes.len() > 1024 * 1024 { return Err(FabushiIdentityVerificationError::Rejected); }
        let payload: Value = serde_json::from_slice(&bytes).map_err(|_| FabushiIdentityVerificationError::Rejected)?;
        let data = payload.get("data").unwrap_or(&payload);
        let user = data.get("user");
        let remote_id = [data.get("userId"), data.get("user_id"), data.get("userNo"), data.get("user_no"), data.get("id"),
            user.and_then(|value| value.get("userId")), user.and_then(|value| value.get("user_id")),
            user.and_then(|value| value.get("userNo")), user.and_then(|value| value.get("user_no")),
            user.and_then(|value| value.get("id"))]
            .into_iter().flatten().find_map(|value| fabushi_identity_text(value).ok());
        if remote_id.as_deref() != Some(claim.user_id.as_str()) {
            return Err(FabushiIdentityVerificationError::IdentityMismatch);
        }
        Ok(claim)
    }

    pub fn upload_direct_message_resource(
        &self,
        path: &Path,
        name: &str,
    ) -> Result<FabushiRemoteHumanAttachment, String> {
        self.upload_direct_message_resource_bound(path, name, None)
    }

    pub(super) fn upload_direct_message_resource_for_user(
        &self,
        path: &Path,
        name: &str,
        expected_user_id: &str,
    ) -> Result<FabushiRemoteHumanAttachment, String> {
        self.upload_direct_message_resource_bound(path, name, Some(expected_user_id))
    }

    fn upload_direct_message_resource_bound(
        &self,
        path: &Path,
        name: &str,
        expected_user_id: Option<&str>,
    ) -> Result<FabushiRemoteHumanAttachment, String> {
        let name = required_trimmed(name, "attachment name")?;
        let bytes = read_human_resource_upload_file(path)?;
        self.upload_direct_message_resource_bytes_bound(bytes, name, expected_user_id)
    }

    pub(super) fn upload_direct_message_resource_bytes_for_user(
        &self,
        bytes: Vec<u8>,
        name: &str,
        expected_user_id: &str,
    ) -> Result<FabushiRemoteHumanAttachment, String> {
        self.upload_direct_message_resource_bytes_bound(bytes, name, Some(expected_user_id))
    }

    fn upload_direct_message_resource_bytes_bound(
        &self,
        bytes: Vec<u8>,
        name: &str,
        expected_user_id: Option<&str>,
    ) -> Result<FabushiRemoteHumanAttachment, String> {
        let name = required_trimmed(name, "attachment name")?;
        validate_human_resource_upload_size(bytes.len() as u64)?;
        let credentials = self.read_bound_credentials()?;
        Self::ensure_bound_user(&credentials, expected_user_id)?;
        let part = reqwest::blocking::multipart::Part::bytes(bytes)
            .file_name(name.to_string())
            .mime_str("application/octet-stream")
            .map_err(|error| format!("Fabushi Human attachment MIME metadata was invalid: {error}"))?;
        let response = self
            .client
            .post(self.endpoint("/api/social/message-resources")?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .multipart(reqwest::blocking::multipart::Form::new().part("file", part))
            .send()
            .map_err(|error| format!("Fabushi Human attachment upload failed: {error}"))?;
        self.ensure_request_credential_stable(&credentials)?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human attachment response could not be read: {error}"))?;
        self.ensure_request_credential_stable(&credentials)?;
        let envelope: ResourceEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human attachment response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human attachment upload was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        envelope
            .resource
            .ok_or_else(|| "Fabushi Human attachment response omitted the persisted resource.".to_string())
    }

    pub fn download_direct_message_resource(
        &self,
        resource_id: &str,
        expected_size: u64,
    ) -> Result<Vec<u8>, String> {
        self.download_direct_message_resource_bound(resource_id, expected_size, None)
    }

    pub fn download_direct_message_resource_for_user(
        &self,
        resource_id: &str,
        expected_size: u64,
        expected_user_id: &str,
    ) -> Result<Vec<u8>, String> {
        self.download_direct_message_resource_bound(resource_id, expected_size, Some(expected_user_id))
    }

    fn download_direct_message_resource_bound(
        &self,
        resource_id: &str,
        expected_size: u64,
        expected_user_id: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        const MAX_RESOURCE_BYTES: u64 = 32 * 1024 * 1024;
        let resource_id = required_trimmed(resource_id, "resource id")?;
        if resource_id.len() > 128 || resource_id.contains('/') || resource_id.contains('\\') {
            return Err("Fabushi Human attachment resource id is invalid".into());
        }
        if expected_size == 0 || expected_size > MAX_RESOURCE_BYTES {
            return Err("Fabushi Human attachment size exceeds the backend limit".into());
        }
        let credentials = self.read_bound_credentials()?;
        Self::ensure_bound_user(&credentials, expected_user_id)?;
        let encoded_resource_id: String =
            url::form_urlencoded::byte_serialize(resource_id.as_bytes()).collect();
        let response = self
            .client
            .get(self.endpoint(&format!(
                "/api/social/message-resources/{encoded_resource_id}"
            ))?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/octet-stream")
            .send()
            .map_err(|error| format!("Fabushi Human attachment download failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(format!(
                "Fabushi Human attachment download was rejected (HTTP {}): {}",
                status.as_u16(),
                body.chars().take(512).collect::<String>()
            ));
        }
        if let Some(length) = response.content_length() {
            if length != expected_size || length > MAX_RESOURCE_BYTES {
                return Err(format!(
                    "Fabushi Human attachment download length mismatch: expected {expected_size}, received {length}"
                ));
            }
        }
        let mut bytes = Vec::with_capacity(expected_size as usize);
        response
            .take(MAX_RESOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Fabushi Human attachment body could not be read: {error}"))?;
        if bytes.len() as u64 != expected_size || bytes.len() as u64 > MAX_RESOURCE_BYTES {
            return Err(format!(
                "Fabushi Human attachment body length mismatch: expected {expected_size}, received {}",
                bytes.len()
            ));
        }
        self.ensure_request_credential_stable(&credentials)?;
        Ok(bytes)
    }

    pub fn send_direct_message(
        &self,
        peer_human_id: &str,
        text: &str,
        client_request_id: &str,
        reply_to_message_id: Option<&str>,
        attachments: &[FabushiRemoteHumanAttachment],
    ) -> Result<FabushiRemoteHumanMessage, String> {
        let expected_user_id = self.identity()?.user_id;
        self.send_direct_message_for_settlement(
            peer_human_id, text, client_request_id, reply_to_message_id, attachments,
            &expected_user_id,
        ).map_err(FabushiHumanSendError::into_message)
    }

    pub(super) fn send_direct_message_for_settlement(
        &self,
        peer_human_id: &str,
        text: &str,
        client_request_id: &str,
        reply_to_message_id: Option<&str>,
        attachments: &[FabushiRemoteHumanAttachment],
        expected_user_id: &str,
    ) -> Result<FabushiRemoteHumanMessage, FabushiHumanSendError> {
        let peer_human_id = required_trimmed(peer_human_id, "peer Human id")?;
        let text = text.trim();
        let client_request_id = required_trimmed(client_request_id, "client request id")?;
        let reply_to_message_id = reply_to_message_id
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if text.is_empty() && attachments.is_empty() {
            return Err(FabushiHumanSendError::Rejected("Fabushi Human message requires text or attachments".into()));
        }
        let credentials = self.read_bound_credentials()?;
        Self::ensure_bound_user(&credentials, Some(expected_user_id))?;
        let response = self
            .client
            .post(self.endpoint("/api/social/messages")?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .json(&SendRequest {
                target_user_id: peer_human_id,
                text,
                client_request_id,
                reply_to_message_id,
                attachments: attachments
                    .iter()
                    .map(|attachment| SendAttachmentRequest {
                        resource_id: attachment.resource_id.as_str(),
                    })
                    .collect(),
            })
            .send();
        self.ensure_request_credential_stable(&credentials)?;
        let response = response.map_err(|error| {
            let message = format!("Fabushi Human message send failed: {error}");
            if error.is_builder() {
                FabushiHumanSendError::Rejected(message)
            } else {
                FabushiHumanSendError::Transport(message)
            }
        })?;
        let status = response.status();
        let body = response.text();
        self.ensure_request_credential_stable(&credentials)?;
        let body = body.map_err(|error| {
                let message = format!("Fabushi Human message response could not be read: {error}");
                if status.is_success() {
                    FabushiHumanSendError::Transport(message)
                } else {
                    // A known HTTP denial cannot become successful transport
                    // recovery merely because its error body was truncated.
                    FabushiHumanSendError::Rejected(message)
                }
        })?;
        let envelope: SendEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human message response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(FabushiHumanSendError::Rejected(format!(
                "Fabushi Human message send was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            )));
        }
        envelope
            .message
            .ok_or_else(|| FabushiHumanSendError::Rejected("Fabushi Human message response omitted the persisted message.".to_string()))
    }

    pub fn set_direct_message_reaction(
        &self,
        message_id: &str,
        emoji: &str,
        active: bool,
    ) -> Result<Vec<FabushiRemoteHumanReaction>, String> {
        self.set_direct_message_reaction_bound(message_id, emoji, active, None)
            .map_err(FabushiHumanReactionError::into_message)
    }

    pub fn set_direct_message_reaction_for_user(
        &self,
        message_id: &str,
        emoji: &str,
        active: bool,
        expected_user_id: &str,
    ) -> Result<Vec<FabushiRemoteHumanReaction>, String> {
        self.set_direct_message_reaction_for_user_classified(
            message_id, emoji, active, expected_user_id,
        ).map_err(FabushiHumanReactionError::into_message)
    }

    pub(super) fn set_direct_message_reaction_for_user_classified(
        &self,
        message_id: &str,
        emoji: &str,
        active: bool,
        expected_user_id: &str,
    ) -> Result<Vec<FabushiRemoteHumanReaction>, FabushiHumanReactionError> {
        self.set_direct_message_reaction_bound(
            message_id, emoji, active, Some(expected_user_id),
        )
    }

    fn set_direct_message_reaction_bound(
        &self,
        message_id: &str,
        emoji: &str,
        active: bool,
        expected_user_id: Option<&str>,
    ) -> Result<Vec<FabushiRemoteHumanReaction>, FabushiHumanReactionError> {
        let canonical_message_id = required_trimmed(message_id, "message id")?;
        if message_id
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0 && *value <= 9_007_199_254_740_991
                && value.to_string() == message_id && canonical_message_id == message_id)
            .is_none()
        {
            return Err(FabushiHumanReactionError::Failure(
                "Fabushi Human reaction requires a positive canonical message id".into(),
            ));
        }
        let emoji = required_trimmed(emoji, "reaction emoji")?;
        if emoji.as_bytes().len() > 32 {
            return Err(FabushiHumanReactionError::Failure(
                "Fabushi Human reaction emoji exceeds the backend limit".into(),
            ));
        }
        let credentials = self.read_bound_credentials()?;
        Self::ensure_bound_user(&credentials, expected_user_id)?;
        let path = format!("/api/social/messages/{message_id}/reactions");
        let response = self
            .client
            .post(self.endpoint(&path)?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .json(&ReactionRequest { emoji, active })
            .send()
            .map_err(|error| format!("Fabushi Human reaction mutation failed: {error}"))?;
        // A successful HTTP response is not permission to settle an older
        // account/device request. Preserve the durable intent for replay.
        self.ensure_request_credential_stable(&credentials)?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human reaction response could not be read: {error}"))?;
        self.ensure_request_credential_stable(&credentials)?;
        let envelope: ReactionEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human reaction response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            // A refusal with a success payload is contradictory. Neither the
            // status nor error text authorizes inferring a canonical tombstone.
            let deferrable = matches!(status.as_u16(), 404 | 409)
                && !envelope.success
                && envelope.message_id.is_none()
                && envelope.reactions.is_none();
            let message = format!(
                "Fabushi Human reaction mutation was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            );
            return Err(if deferrable {
                FabushiHumanReactionError::RemoteRefusal { status: status.as_u16(), message }
            } else {
                FabushiHumanReactionError::Failure(message)
            });
        }
        let returned_id = envelope
            .message_id
            .as_ref()
            .ok_or_else(|| "Fabushi Human reaction response omitted the message id.".to_string())
            .and_then(fabushi_identity_text)?;
        if returned_id != message_id {
            return Err(FabushiHumanReactionError::Failure(
                "Fabushi Human reaction backend returned a mismatched message id".into(),
            ));
        }
        Ok(envelope.reactions.ok_or_else(|| {
            "Fabushi Human reaction response omitted the reaction summary".to_string()
        })?)
    }

    pub fn list_friends(&self) -> Result<Vec<FabushiRemoteHumanContact>, String> {
        self.list_friends_bound(None)
    }

    pub fn list_friends_for_user(
        &self,
        expected_user_id: &str,
    ) -> Result<Vec<FabushiRemoteHumanContact>, String> {
        self.list_friends_bound(Some(expected_user_id))
    }

    fn list_friends_bound(
        &self,
        expected_user_id: Option<&str>,
    ) -> Result<Vec<FabushiRemoteHumanContact>, String> {
        let credentials = self.read_bound_credentials()?;
        Self::ensure_bound_user(&credentials, expected_user_id)?;
        let response = self
            .client
            .get(self.endpoint("/api/social/friends")?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .send()
            .map_err(|error| format!("Fabushi Human contact sync failed: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human contact sync response could not be read: {error}"))?;
        // Roster entries create durable conversations and trigger message/call
        // synchronization. A response completed under a retired credential
        // must be rejected before any of those downstream effects.
        self.ensure_request_credential_stable(&credentials)?;
        let envelope: FriendEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human contact sync response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human contact sync was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        Ok(envelope.data.map(|data| data.friends).unwrap_or_default())
    }

    pub fn list_direct_messages(
        &self,
        peer_human_id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FabushiRemoteHumanMessage>, String> {
        self.list_direct_messages_window(peer_human_id, before, None, limit, None)
    }

    pub fn list_direct_messages_after(
        &self,
        peer_human_id: &str,
        after_id: u64,
        limit: usize,
    ) -> Result<Vec<FabushiRemoteHumanMessage>, String> {
        if after_id > 9_007_199_254_740_991 {
            return Err("Fabushi Human sync afterId is not a safe integer".into());
        }
        self.list_direct_messages_window(peer_human_id, None, Some(after_id), limit, None)
    }

    /// Pair queries bind the credential read to the active Human owner.
    pub fn list_direct_messages_after_for_user(
        &self,
        peer_human_id: &str,
        expected_user_id: &str,
        after_id: u64,
        limit: usize,
    ) -> Result<Vec<FabushiRemoteHumanMessage>, String> {
        if after_id > 9_007_199_254_740_991 {
            return Err("Fabushi Human sync afterId is not a safe integer".into());
        }
        self.list_direct_messages_window(
            peer_human_id, None, Some(after_id), limit, Some(expected_user_id),
        )
    }

    pub fn list_direct_messages_for_user(
        &self,
        peer_human_id: &str,
        expected_user_id: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<Vec<FabushiRemoteHumanMessage>, String> {
        self.list_direct_messages_window(
            peer_human_id, before, None, limit, Some(expected_user_id),
        )
    }

    fn list_direct_messages_window(
        &self,
        peer_human_id: &str,
        before: Option<&str>,
        after_id: Option<u64>,
        limit: usize,
        expected_user_id: Option<&str>,
    ) -> Result<Vec<FabushiRemoteHumanMessage>, String> {
        let peer_human_id = required_trimmed(peer_human_id, "peer Human id")?;
        let credentials = self.read_bound_credentials()?;
        Self::ensure_bound_user(&credentials, expected_user_id)?;
        let mut url = self.endpoint("/api/social/messages")?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("contactId", peer_human_id);
            query.append_pair("limit", &limit.clamp(1, 200).to_string());
            if let Some(before) = before.map(str::trim).filter(|value| !value.is_empty()) {
                query.append_pair("before", before);
            }
            if let Some(after_id) = after_id {
                query.append_pair("afterId", &after_id.to_string());
            }
        }
        let response = self.client
            .get(url)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header("x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?)
            .header("accept", "application/json")
            .send()
            .map_err(|error| format!("Fabushi Human message sync failed: {error}"))?;
        let status = response.status();
        let body = response.text()
            .map_err(|error| format!("Fabushi Human message sync response could not be read: {error}"))?;
        let envelope: ListEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!("Fabushi Human message sync response was invalid JSON (HTTP {}): {error}",
                status.as_u16())
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human message sync was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        self.ensure_request_credential_stable(&credentials)?;
        let data = envelope.data
            .ok_or_else(|| "Fabushi Human message sync response omitted the messages envelope".to_string())?;
        if let Some(requested_after_id) = after_id {
            // A legacy Worker may ignore afterId=0 and return a descending tail.
            // Never persist that response as monotonic coverage of all history.
            let echo = data.history_cursor.as_ref()
                .ok_or_else(|| "Fabushi Human history response omitted historyCursor".to_string())?;
            if echo.mode != "after-id" || echo.after_id.as_u64() != Some(requested_after_id) {
                return Err("Fabushi Human history response echoed a mismatched historyCursor".into());
            }
            let next = data.next_after_id.as_ref().and_then(Value::as_u64)
                .ok_or_else(|| "Fabushi Human history response omitted a valid nextAfterId".to_string())?;
            let expected_next = data.messages.last().map(|message| {
                fabushi_message_identity_text(&message.id)?.parse::<u64>()
                    .map_err(|_| "Fabushi Human history nextAfterId is not a numeric message id".to_string())
            }).transpose()?.unwrap_or(requested_after_id);
            if next != expected_next || next > 9_007_199_254_740_991 {
                return Err("Fabushi Human history nextAfterId did not match the page boundary".into());
            }
        }
        Ok(data.messages)
    }

    pub fn get_human_call_ice_servers(
        &self,
    ) -> Result<(Vec<FabushiCallIceServer>, u64), String> {
        let credentials = self.read_bound_credentials()?;
        let response = self
            .client
            .get(self.endpoint("/api/social/calls/ice")?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .send()
            .map_err(|error| format!("Fabushi Human call ICE credential request failed: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human call ICE response could not be read: {error}"))?;
        let envelope: CallIceEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human call ICE response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human call ICE credentials were rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        let ttl_seconds = envelope
            .ttl_seconds
            .filter(|ttl| (300..=86_400).contains(ttl))
            .ok_or_else(|| "Fabushi Human call ICE response contained an invalid TTL".to_string())?;
        if envelope.ice_servers.is_empty() || envelope.ice_servers.len() > 16 {
            return Err("Fabushi Human call ICE response contained an invalid server list".into());
        }
        for server in &envelope.ice_servers {
            if !valid_call_ice_urls(&server.urls)
                || server.username.as_deref().is_some_and(|value| value.len() > 2048)
                || server.credential.as_deref().is_some_and(|value| value.len() > 2048)
            {
                return Err("Fabushi Human call ICE response contained an invalid server".into());
            }
        }
        Ok((envelope.ice_servers, ttl_seconds))
    }

    pub fn list_human_calls(&self, limit: usize) -> Result<Vec<FabushiRemoteHumanCall>, String> {
        let credentials = self.read_bound_credentials()?;
        let mut url = self.endpoint("/api/social/calls")?;
        url.query_pairs_mut()
            .append_pair("limit", &limit.clamp(1, 200).to_string());
        let response = self
            .client
            .get(url)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .send()
            .map_err(|error| format!("Fabushi Human call discovery failed: {error}"))?;
        self.ensure_request_credential_stable(&credentials)?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human call discovery response could not be read: {error}"))?;
        self.ensure_request_credential_stable(&credentials)?;
        let envelope: CallEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human call discovery response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human call discovery was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        Ok(envelope.calls)
    }

    pub fn create_human_call(
        &self,
        call_id: &str,
        peer_human_id: &str,
    ) -> Result<FabushiRemoteHumanCall, String> {
        let call_id = required_trimmed(call_id, "call id")?;
        let peer_human_id = required_trimmed(peer_human_id, "peer Human id")?;
        let credentials = self.read_bound_credentials()?;
        let response = self
            .client
            .post(self.endpoint("/api/social/calls")?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .json(&CreateCallRequest {
                call_id,
                target_user_id: peer_human_id,
            })
            .send()
            .map_err(|error| format!("Fabushi Human call creation failed: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human call creation response could not be read: {error}"))?;
        let envelope: CallEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human call creation response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human call creation was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        let call = envelope
            .call
            .ok_or_else(|| "Fabushi Human call creation response omitted the call.".to_string())?;
        if call.call_id != call_id {
            return Err("Fabushi Human call backend returned a mismatched call id".into());
        }
        Ok(call)
    }

    pub fn get_human_call(
        &self,
        call_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> Result<(FabushiRemoteHumanCall, Vec<FabushiRemoteHumanCallEvent>, u64), String> {
        let call_id = required_trimmed(call_id, "call id")?;
        let encoded_call_id: String =
            url::form_urlencoded::byte_serialize(call_id.as_bytes()).collect();
        let credentials = self.read_bound_credentials()?;
        let mut url = self.endpoint(&format!("/api/social/calls/{encoded_call_id}"))?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("afterSeq", &after_seq.to_string());
            query.append_pair("limit", &limit.clamp(1, 200).to_string());
        }
        let response = self
            .client
            .get(url)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .send()
            .map_err(|error| format!("Fabushi Human call sync failed: {error}"))?;
        self.ensure_request_credential_stable(&credentials)?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human call sync response could not be read: {error}"))?;
        self.ensure_request_credential_stable(&credentials)?;
        let envelope: CallEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human call sync response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human call sync was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        let call = envelope
            .call
            .ok_or_else(|| "Fabushi Human call sync response omitted the call.".to_string())?;
        if call.call_id != call_id {
            return Err("Fabushi Human call backend returned a mismatched call id".into());
        }
        let next_after_seq = envelope
            .next_after_seq
            .unwrap_or_else(|| envelope.events.last().map(|event| event.seq).unwrap_or(after_seq));
        Ok((call, envelope.events, next_after_seq))
    }

    pub fn append_human_call_event(
        &self,
        call_id: &str,
        client_event_id: &str,
        generation: u64,
        kind: &str,
        payload: &Value,
    ) -> Result<(FabushiRemoteHumanCall, FabushiRemoteHumanCallEvent), String> {
        let call_id = required_trimmed(call_id, "call id")?;
        let client_event_id = required_trimmed(client_event_id, "call event id")?;
        let kind = required_trimmed(kind, "call event kind")?;
        if !payload.is_object() {
            return Err("Fabushi Human call event payload must be an object".into());
        }
        let encoded_call_id: String =
            url::form_urlencoded::byte_serialize(call_id.as_bytes()).collect();
        let credentials = self.read_bound_credentials()?;
        let response = self
            .client
            .post(self.endpoint(&format!(
                "/api/social/calls/{encoded_call_id}/events"
            ))?)
            .bearer_auth(required_trimmed(&credentials.access_token, "accessToken")?)
            .header(
                "x-fabushi-device-id",
                required_trimmed(&credentials.device_id, "deviceId")?,
            )
            .header("accept", "application/json")
            .json(&AppendCallEventRequest {
                client_event_id,
                generation,
                kind,
                payload,
            })
            .send()
            .map_err(|error| format!("Fabushi Human call event send failed: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Fabushi Human call event response could not be read: {error}"))?;
        let envelope: CallEnvelope = serde_json::from_str(&body).map_err(|error| {
            format!(
                "Fabushi Human call event response was invalid JSON (HTTP {}): {error}",
                status.as_u16()
            )
        })?;
        if !status.is_success() || !envelope.success {
            return Err(format!(
                "Fabushi Human call event was rejected (HTTP {}): {}",
                status.as_u16(),
                envelope.error.unwrap_or_else(|| "unknown backend error".into())
            ));
        }
        let call = envelope
            .call
            .ok_or_else(|| "Fabushi Human call event response omitted the call.".to_string())?;
        let event = envelope
            .event
            .ok_or_else(|| "Fabushi Human call event response omitted the event.".to_string())?;
        if call.call_id != call_id || event.call_id != call_id {
            return Err("Fabushi Human call backend returned a mismatched call id".into());
        }
        if event.client_event_id != client_event_id
            || event.generation != generation
            || event.kind != kind
            || event.payload != *payload
        {
            return Err("Fabushi Human call backend returned a mismatched canonical event".into());
        }
        Ok((call, event))
    }

    fn endpoint(&self, path: &str) -> Result<Url, String> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| format!("Fabushi messaging endpoint is invalid: {error}"))
    }

    // An HTTP response may wait behind a later Session mutation. Bind its
    // settlement to every credential field, including token-only rotations.
    // The raw bearer and session identifier never leave this private owner.
    pub(super) fn request_credential_fingerprint(&self) -> Result<String, String> {
        let credentials = self.read_bound_credentials()?;
        let snapshot = serde_json::json!([
            "fabushi-human-request-credential-v1",
            credentials.access_token,
            credentials.device_id,
            credentials.user_id,
            credentials.session_id,
            credentials.expires_at_ms,
            credentials.issuer,
        ]);
        Ok(sha256_hex(snapshot.to_string().as_bytes()))
    }

    fn ensure_request_credential_stable(&self, original: &CredentialFile) -> Result<(), String> {
        let current = self.read_bound_credentials()?;
        if current.user_id != original.user_id
            || current.device_id != original.device_id
            || current.access_token != original.access_token
            || current.session_id != original.session_id
            || current.expires_at_ms != original.expires_at_ms
            || current.issuer != original.issuer
        {
            return Err("Fabushi native messaging credential changed during Human request".into());
        }
        Ok(())
    }

    fn ensure_bound_user(
        credentials: &CredentialFile,
        expected_user_id: Option<&str>,
    ) -> Result<(), String> {
        if let Some(expected) = expected_user_id {
            if fabushi_identity_text(&credentials.user_id)? != expected {
                return Err("Fabushi native messaging credential changed Human account".into());
            }
        }
        Ok(())
    }

    fn read_bound_credentials(&self) -> Result<CredentialFile, String> {
        let credentials = read_credentials(&self.credential_path)?;
        let issuer = credential_issuer(credentials.issuer.as_deref())?;
        if !issuer_matches_base(&issuer, &self.base_url) {
            return Err("Fabushi native credential issuer does not match the configured service origin.".into());
        }
        Ok(credentials)
    }
}

fn validate_human_resource_upload_size(length: u64) -> Result<(), String> {
    if length == 0 {
        return Err("Fabushi Human attachment is empty".into());
    }
    if length > MAX_HUMAN_RESOURCE_UPLOAD_BYTES {
        return Err("Fabushi Human attachment exceeds the 25 MiB message upload limit".into());
    }
    Ok(())
}

fn validate_human_resource_upload_file(metadata: &fs::Metadata) -> Result<(), String> {
    if !metadata.is_file() {
        return Err("Fabushi Human attachment must be a regular file".into());
    }
    validate_human_resource_upload_size(metadata.len())
}

fn read_human_resource_upload_bytes(reader: impl Read) -> Result<Vec<u8>, String> {
    // Metadata is only an early rejection. The file can grow after it was
    // observed, so the actual reader must enforce the service budget too.
    let mut bytes = Vec::new();
    reader.take(MAX_HUMAN_RESOURCE_UPLOAD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Fabushi Human attachment could not be read: {error}"))?;
    validate_human_resource_upload_size(bytes.len() as u64)?;
    Ok(bytes)
}

fn read_human_resource_upload_file(path: &Path) -> Result<Vec<u8>, String> {
    let read_error = |error: std::io::Error| format!(
        "Fabushi Human attachment could not be read from {}: {error}", path.display()
    );
    // Check the path before opening so directories and other non-file inputs
    // are rejected without reading. Recheck the actual opened file as well.
    validate_human_resource_upload_file(&fs::symlink_metadata(path).map_err(&read_error)?)?;
    let file = fs::File::open(path).map_err(&read_error)?;
    validate_human_resource_upload_file(&file.metadata().map_err(&read_error)?)?;
    read_human_resource_upload_bytes(file)
}

fn validate_base_url(raw: &str) -> Result<Url, String> {
    let normalized = raw.trim().trim_end_matches('/');
    let mut url = Url::parse(&format!("{normalized}/"))
        .map_err(|error| format!("Fabushi messaging API base URL is invalid: {error}"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(loopback && url.scheme() == "http") {
        return Err("Fabushi messaging API must use HTTPS outside loopback development.".into());
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Fabushi messaging API base URL must not contain credentials, query, or fragment.".into(),
        );
    }
    let origin = url.origin().ascii_serialization();
    if !loopback && origin != DEFAULT_FABUSHI_API_BASE_URL
        && origin != CI_ISOLATED_FABUSHI_AUTH_BASE_URL {
        return Err("Fabushi messaging API must use an approved Native issuer origin.".into());
    }
    if !loopback && url.path() != "/" {
        return Err("Fabushi messaging API origin must not include a path.".into());
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn read_credentials(path: &Path) -> Result<CredentialFile, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Fabushi messaging credential is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err("Fabushi messaging credential must be a private regular file.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("Fabushi messaging credential must be private to the current user.".into());
        }
    }
    serde_json::from_str(
        &fs::read_to_string(path)
            .map_err(|error| format!("Fabushi messaging credential could not be read: {error}"))?,
    )
    .map_err(|error| format!("Fabushi messaging credential JSON is invalid: {error}"))
}

fn credential_claim_from(credentials: &CredentialFile) -> Result<FabushiNativeCredentialClaim, String> {
    let user_id = fabushi_identity_text(&credentials.user_id)?;
    if credentials.user_id.as_str().is_some_and(|raw| raw != user_id) {
        return Err("Fabushi native userId is not in canonical untrimmed form.".into());
    }
    let session_id = required_trimmed(&credentials.session_id, "sessionId")?;
    let device_id = required_trimmed(&credentials.device_id, "deviceId")?.to_string();
    let issuer = credential_issuer(credentials.issuer.as_deref())?;
    if credentials.expires_at_ms <= unix_now_ms() {
        return Err("Fabushi native account credential has expired.".into());
    }
    Ok(FabushiNativeCredentialClaim {
        user_id,
        issuer,
        device_id,
        session_binding_hash: sha256_hex(session_id.as_bytes()),
        expires_at_ms: credentials.expires_at_ms,
    })
}

fn credential_issuer(raw: Option<&str>) -> Result<String, String> {
    let Some(raw) = raw else {
        // Existing account-owner credentials predate issuer metadata and remain
        // valid only for production; they cannot be reinterpreted as isolated.
        return Ok(DEFAULT_FABUSHI_API_BASE_URL.to_string());
    };
    let url = validate_base_url(raw)?;
    let origin = url.origin().ascii_serialization();
    if origin == DEFAULT_FABUSHI_API_BASE_URL || origin == CI_ISOLATED_FABUSHI_AUTH_BASE_URL
        || matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1")) {
        Ok(origin)
    } else {
        Err("Fabushi native credential issuer is unsupported.".into())
    }
}

fn issuer_matches_base(issuer: &str, base_url: &Url) -> bool {
    issuer == base_url.origin().ascii_serialization()
}

fn required_trimmed<'a>(value: &'a str, name: &str) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("Fabushi messaging {name} is empty."))
    } else {
        Ok(value)
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

pub fn fabushi_identity_text(value: &Value) -> Result<String, String> {
    match value {
        Value::String(value) => required_trimmed(value, "userId").map(str::to_string),
        Value::Number(value) => Ok(value.to_string()),
        _ => Err("Fabushi messaging userId is invalid.".into()),
    }
}

// Message primary keys share the service's positive JavaScript-safe integer
// domain. Unlike display/user identities, their raw text must be canonical:
// accepting "01" or padded input would create a second local transcript key.
pub(super) fn fabushi_message_identity_text(value: &Value) -> Result<String, String> {
    let parsed = match value {
        Value::String(raw) => raw.parse::<u64>().ok().filter(|id| id.to_string() == *raw),
        Value::Number(number) => number.as_u64(),
        _ => None,
    };
    parsed.filter(|id| *id > 0 && *id <= 9_007_199_254_740_991)
        .map(|id| id.to_string())
        .ok_or_else(|| "Fabushi Human message identity is not a canonical positive safe integer".to_string())
}

fn classify_identity_http_status(
    status: reqwest::StatusCode,
) -> Result<(), FabushiIdentityVerificationError> {
    match status {
        reqwest::StatusCode::OK => Ok(()),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            Err(FabushiIdentityVerificationError::Unauthenticated)
        }
        // Any received non-success HTTP response is an identity rejection, not
        // a transport outage eligible for the mini-app last-good fallback.
        _ => Err(FabushiIdentityVerificationError::Rejected),
    }
}

#[cfg(test)]
mod native_message_resource_upload_tests {
    use super::{read_human_resource_upload_bytes, FabushiNativeMessagingClient};
    // Literal service contract: the oracle must not inherit an incorrect implementation limit.
    const SERVICE_UPLOAD_LIMIT: u64 = 25 * 1024 * 1024;
    use std::cell::Cell;
    use std::io::{self, Read};
    use std::rc::Rc;

    #[test]
    fn exact_service_limit_is_readable_but_empty_and_read_errors_are_rejected() {
        let bytes = read_human_resource_upload_bytes(
            io::repeat(0x5a).take(SERVICE_UPLOAD_LIMIT)
        ).expect("exact 25 MiB resource is allowed");
        assert_eq!(bytes.len() as u64, SERVICE_UPLOAD_LIMIT);
        assert!(bytes.iter().all(|byte| *byte == 0x5a));
        assert_eq!(
            read_human_resource_upload_bytes(io::empty()).unwrap_err(),
            "Fabushi Human attachment is empty"
        );

        struct ReadFailure;
        impl Read for ReadFailure {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("fixture read failure"))
            }
        }
        let partial = io::Cursor::new([1_u8, 2, 3]).chain(ReadFailure);
        assert!(read_human_resource_upload_bytes(partial)
            .unwrap_err().contains("fixture read failure"));
    }

    #[test]
    fn growth_after_metadata_observation_consumes_at_most_limit_plus_one() {
        struct Counted<R> {
            reader: R,
            consumed: Rc<Cell<u64>>,
        }
        impl<R: Read> Read for Counted<R> {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                let read = self.reader.read(output)?;
                self.consumed.set(self.consumed.get() + read as u64);
                Ok(read)
            }
        }
        // An input that grows beyond an earlier acceptable size must not be
        // fully consumed merely to discover that it exceeds the upload limit.
        let consumed = Rc::new(Cell::new(0));
        let reader = Counted {
            reader: io::repeat(0x37).take(SERVICE_UPLOAD_LIMIT + 1024),
            consumed: Rc::clone(&consumed),
        };
        assert_eq!(
            read_human_resource_upload_bytes(reader).unwrap_err(),
            "Fabushi Human attachment exceeds the 25 MiB message upload limit"
        );
        assert_eq!(consumed.get(), SERVICE_UPLOAD_LIMIT + 1);
    }
    #[test]
    fn byte_upload_rejects_empty_and_oversize_before_credentials_or_http() {
        let credential_path = std::env::temp_dir().join(format!(
            "fabushi-upload-bound-no-credential-{}-{}", std::process::id(), super::unix_now_ms()
        ));
        let client = FabushiNativeMessagingClient::new("http://127.0.0.1:9", credential_path)
            .expect("valid loopback client");
        assert_eq!(client.upload_direct_message_resource_bytes_for_user(
            Vec::new(), "empty", "1"
        ).unwrap_err(), "Fabushi Human attachment is empty");
        assert_eq!(client.upload_direct_message_resource_bytes_for_user(
            vec![0; (SERVICE_UPLOAD_LIMIT + 1) as usize], "oversized", "1"
        ).unwrap_err(), "Fabushi Human attachment exceeds the 25 MiB message upload limit");
    }
}

#[cfg(test)]
mod native_account_scope_tests {
    use super::{
        classify_identity_http_status, credential_issuer, issuer_matches_base,
        unix_now_ms, validate_base_url, FabushiIdentityVerificationError,
        FabushiNativeCredentialClaim, FabushiNativeMessagingClient,
        CI_ISOLATED_FABUSHI_AUTH_BASE_URL, DEFAULT_FABUSHI_API_BASE_URL,
        ISOLATED_AUTH_ID_DOMAIN, sha256_hex,
    };
    use std::fs;

    #[test]
    fn account_scope_hash_is_sha256_of_exact_user_id_utf8() {
        let claim = FabushiNativeCredentialClaim {
            user_id: "123".into(),
            issuer: DEFAULT_FABUSHI_API_BASE_URL.into(),
            device_id: "device".into(),
            session_binding_hash: "not-used-for-account-scope".into(),
            expires_at_ms: u64::MAX,
        };
        assert_eq!(
            claim.account_scope_hash(),
            "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3"
        );
    }

    #[test]
    fn equal_raw_user_ids_have_issuer_separated_mini_app_scopes() {
        let production = FabushiNativeCredentialClaim {
            user_id: "123".into(),
            issuer: DEFAULT_FABUSHI_API_BASE_URL.into(),
            device_id: "device".into(),
            session_binding_hash: "binding".into(),
            expires_at_ms: u64::MAX,
        };
        let isolated = FabushiNativeCredentialClaim {
            issuer: CI_ISOLATED_FABUSHI_AUTH_BASE_URL.into(),
            ..production.clone()
        };
        assert_eq!(production.account_scope_hash(), sha256_hex(b"123"));
        assert_ne!(production.account_scope_hash(), isolated.account_scope_hash());
        let opaque = format!(
            "fabushi-ci-isolated:v1:{}",
            sha256_hex(format!("{ISOLATED_AUTH_ID_DOMAIN}\0{}\0{}", isolated.issuer, isolated.user_id).as_bytes())
        );
        assert_eq!(isolated.account_scope_hash(), sha256_hex(opaque.as_bytes()));
    }

    #[test]
    fn unknown_or_cross_issuer_targets_are_rejected_before_authenticated_transport() {
        assert!(validate_base_url("https://untrusted.example").is_err());
        assert_eq!(
            credential_issuer(None).expect("legacy issuer"),
            DEFAULT_FABUSHI_API_BASE_URL,
        );
        assert!(credential_issuer(Some("https://untrusted.example")).is_err());
        let production = validate_base_url(DEFAULT_FABUSHI_API_BASE_URL).expect("production origin");
        assert!(!issuer_matches_base(CI_ISOLATED_FABUSHI_AUTH_BASE_URL, &production));
        let isolated = validate_base_url(CI_ISOLATED_FABUSHI_AUTH_BASE_URL).expect("isolated origin");
        assert!(!issuer_matches_base(DEFAULT_FABUSHI_API_BASE_URL, &isolated));
        let loopback_target = validate_base_url("http://127.0.0.1:4200").expect("loopback target");
        assert!(!issuer_matches_base("http://127.0.0.1:4300", &loopback_target));
    }

    #[test]
    fn bound_credential_rejects_cross_issuer_before_any_http_request() {
        let path = std::env::temp_dir().join(format!(
            "fabushi-native-issuer-contract-{}-{}.json",
            std::process::id(),
            unix_now_ms(),
        ));
        fs::write(&path, format!(
            r#"{{"accessToken":"fixture-only","deviceId":"device","userId":"123","issuer":"{CI_ISOLATED_FABUSHI_AUTH_BASE_URL}"}}"#
        )).expect("write disposable credential fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private fixture mode");
        }
        let production = FabushiNativeMessagingClient::new(DEFAULT_FABUSHI_API_BASE_URL, &path)
            .expect("fixed production client");
        let result = production.read_bound_credentials();
        let _ = fs::remove_file(&path);
        assert!(result.is_err(), "isolated bearer must be rejected before transport");
    }

    #[test]
    fn identity_http_statuses_only_allow_success_and_explicit_auth_expiry() {
        use reqwest::StatusCode;

        assert_eq!(classify_identity_http_status(StatusCode::OK), Ok(()));
        for status in [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN] {
            assert_eq!(
                classify_identity_http_status(status),
                Err(FabushiIdentityVerificationError::Unauthenticated),
                "status {status} must invalidate the confirmed Native identity"
            );
        }
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_eq!(
                classify_identity_http_status(status),
                Err(FabushiIdentityVerificationError::Rejected),
                "received status {status} must not be eligible for cached-identity fallback"
            );
        }
    }
}
