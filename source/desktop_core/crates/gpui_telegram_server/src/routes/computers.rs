use crate::db::computer_store::{
    decrypt_computer_secret, parse_cloud_auth_method, resolve_computer_master_key,
    DEFAULT_SANDBOX_NODE_ID,
};
use crate::db::schema::UserRecord;
use crate::db::ServerDatabase;
use crate::routes::{err_resp, hermes::{load_bot_persona_config, save_bot_persona_kv}, map_telegram_err};
use crate::server::{AppState, AuthBearer};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use base64::Engine as _;
use gpui_telegram_agent::computer::cloud_bridge::{
    redact_cloud_secrets, AgentCloudServerConfigInput, AgentComputerExecInput,
    AgentComputerExecOutput, AgentComputerNodeSummary, BoxFuture, ComputerCloudProvider,
};
use gpui_telegram_core::error::{Result as CoreResult, TelegramError};
use gpui_telegram_core::types::{UserId};
use gpui_telegram_proto::computer_contracts::{
    ApproveComputerCommandRequest, AuthorizedControllerKind, BotComputerProfileDto, CloudAuthMethod,
    ComputerListResponse, ComputerNodeDto, ComputerNodeKind, ComputerRunPolicy,
    PendingComputerCommandDto, RegisterLocalComputerRequest, RemoteComputerCommandRequest,
    RemoteComputerCommandResult, RemoteComputerOp, UpdateBotComputerRequest,
    UpdateComputerPolicyRequest, UpsertCloudComputerRequest,
};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Component, Path as StdPath, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

const LOCAL_COMMAND_TIMEOUT_SECS: u64 = 25;
const REMOTE_EXEC_TIMEOUT_MS_DEFAULT: u64 = 15_000;
const REMOTE_EXEC_TIMEOUT_MS_MAX: u64 = 60_000;
const MAX_READ_BYTES_DEFAULT: usize = 65_536;
const MAX_READ_BYTES_CAP: usize = 512 * 1024;
const MAX_OUTPUT_BYTES: usize = 256 * 1024;
const AGENT_EXEC_TOKEN_ENV: &str = "GPUI_COMPUTER_AGENT_TOKEN";

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
struct PendingLocalCommandEntry {
    owner_user_id: u64,
    dto: PendingComputerCommandDto,
}

#[derive(Debug)]
struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn create(prefix: &str) -> std::io::Result<Self> {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        Ok(Self { path: dir })
    }

    fn path(&self) -> &StdPath {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AgentExecWireRequest {
    Wrapped {
        operation: RemoteComputerOp,
        #[serde(default)]
        workdir: Option<String>,
        #[serde(default)]
        node_id: Option<String>,
        #[serde(default)]
        command_id: Option<String>,
    },
    Direct(RemoteComputerOp),
}

pub struct ComputerControlService {
    db: Arc<ServerDatabase>,
    pending_commands: Mutex<HashMap<String, PendingLocalCommandEntry>>,
    waiters: Mutex<HashMap<String, oneshot::Sender<RemoteComputerCommandResult>>>,
    master_key: Zeroizing<Vec<u8>>,
    http_client: reqwest::Client,
    agent_exec_token: RwLock<Option<String>>,
    tailnet: RwLock<Option<Arc<crate::routes::tailnet::TailnetCoordService>>>,
}

impl ComputerControlService {
    pub fn new(db: Arc<ServerDatabase>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            db,
            pending_commands: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            master_key: resolve_computer_master_key(),
            http_client,
            agent_exec_token: RwLock::new(None),
            tailnet: RwLock::new(None),
        }
    }

    pub fn set_tailnet_service(&self, svc: Arc<crate::routes::tailnet::TailnetCoordService>) {
        *self.tailnet.write() = Some(svc);
    }

    fn tailnet_service(&self) -> Option<Arc<crate::routes::tailnet::TailnetCoordService>> {
        self.tailnet.read().clone()
    }

    fn apply_derp_online_override(&self, owner_user_id: u64, dto: &mut ComputerNodeDto) {
        if dto.kind == ComputerNodeKind::LocalDevice && !dto.online {
            if let Some(ref did) = dto.device_id {
                if self
                    .tailnet_service()
                    .is_some_and(|t| t.is_device_derp_connected(owner_user_id, did))
                {
                    dto.online = true;
                }
            }
        }
    }

    pub fn db(&self) -> &ServerDatabase {
        &self.db
    }

    pub fn set_agent_exec_token(&self, token: Option<String>) {
        let normalized = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        *self.agent_exec_token.write() = normalized;
    }

    fn effective_agent_exec_token(&self) -> Option<Zeroizing<String>> {
        if let Some(ref configured) = *self.agent_exec_token.read() {
            if !configured.trim().is_empty() {
                return Some(Zeroizing::new(configured.clone()));
            }
        }
        std::env::var(AGENT_EXEC_TOKEN_ENV)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .map(Zeroizing::new)
    }

    pub fn master_key_bytes(&self) -> &[u8] {
        &self.master_key
    }

    pub fn pending_approvals_for_device(
        &self,
        owner_user_id: u64,
        device_id: Option<&str>,
    ) -> Vec<PendingComputerCommandDto> {
        let Some(dev) = device_id.map(str::trim).filter(|s| !s.is_empty()) else {
            return Vec::new();
        };
        let mut items: Vec<PendingComputerCommandDto> = self
            .pending_commands
            .lock()
            .values()
            .filter(|entry| {
                entry.owner_user_id == owner_user_id
                    && entry.dto.target_device_id == dev
                    && entry.dto.requires_user_approval
                    && !entry.dto.approved
            })
            .map(|entry| entry.dto.clone())
            .collect();
        items.sort_by_key(|d| d.created_at_ms);
        items
    }

    pub fn poll_commands_for_device(
        &self,
        owner_user_id: u64,
        device_id: &str,
    ) -> Vec<PendingComputerCommandDto> {
        let trimmed = device_id.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let mut items: Vec<PendingComputerCommandDto> = self
            .pending_commands
            .lock()
            .values()
            .filter(|entry| entry.owner_user_id == owner_user_id && entry.dto.target_device_id == trimmed)
            .map(|entry| entry.dto.clone())
            .collect();
        items.sort_by_key(|d| d.created_at_ms);
        items
    }

    pub fn approve_pending_command(
        &self,
        owner_user_id: u64,
        command_id: &str,
        approved: bool,
    ) -> CoreResult<PendingComputerCommandDto> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let dto = {
            let mut guard = self.pending_commands.lock();
            if approved {
                let entry = guard.get_mut(command_id).ok_or_else(|| {
                    TelegramError::NotFound(format!("Pending command `{command_id}` not found"))
                })?;
                if entry.owner_user_id != owner_user_id {
                    return Err(TelegramError::PermissionDenied(
                        "Command does not belong to caller".to_string(),
                    ));
                }
                entry.dto.approved = true;
                return Ok(entry.dto.clone());
            } else {
                let entry = guard.get(command_id).ok_or_else(|| {
                    TelegramError::NotFound(format!("Pending command `{command_id}` not found"))
                })?;
                if entry.owner_user_id != owner_user_id {
                    return Err(TelegramError::PermissionDenied(
                        "Command does not belong to caller".to_string(),
                    ));
                }
                let mut removed = guard.remove(command_id).expect("checked exists");
                removed.dto.approved = false;
                removed.dto
            }
        };

        // Denied: record audit and wake any awaiting caller immediately.
        let _ = self.db.record_computer_audit(
            command_id,
            owner_user_id,
            &dto.node_id,
            &dto.caller_label,
            op_summary(&dto.operation),
            false,
            403,
            now_ms,
        );
        if let Some(tx) = self.waiters.lock().remove(command_id) {
            let _ = tx.send(RemoteComputerCommandResult {
                command_id: command_id.to_string(),
                node_id: dto.node_id.clone(),
                ok: false,
                exit_code: 403,
                stdout: String::new(),
                stderr: "Denied by device user".to_string(),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by: dto.target_device_id.clone(),
                duration_ms: 0,
            });
        }

        Ok(dto)
    }

    pub fn complete_pending_command(
        &self,
        owner_user_id: u64,
        command_id: &str,
        mut result: RemoteComputerCommandResult,
    ) -> CoreResult<()> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let removed_entry = {
            let mut guard = self.pending_commands.lock();
            if let Some(existing) = guard.get(command_id) {
                if existing.owner_user_id != owner_user_id {
                    return Err(TelegramError::PermissionDenied(
                        "Command does not belong to caller".to_string(),
                    ));
                }
            } else if !self.waiters.lock().contains_key(command_id) {
                return Err(TelegramError::NotFound(format!(
                    "Pending command `{command_id}` not found"
                )));
            }
            guard.remove(command_id)
        };

        if result.command_id.is_empty() {
            result.command_id = command_id.to_string();
        }
        if result.node_id.is_empty() {
            if let Some(ref entry) = removed_entry {
                result.node_id = entry.dto.node_id.clone();
            }
        }

        if !result.node_id.is_empty() {
            let _ = self.db.touch_computer_node_heartbeat_and_frame(
                owner_user_id,
                &result.node_id,
                result.frame_data_url.as_deref(),
                now_ms,
            );
        }

        if let Some(ref entry) = removed_entry {
            let _ = self.db.record_computer_audit(
                command_id,
                owner_user_id,
                &entry.dto.node_id,
                &entry.dto.caller_label,
                op_summary(&entry.dto.operation),
                result.ok,
                result.exit_code,
                now_ms,
            );
        }

        if let Some(tx) = self.waiters.lock().remove(command_id) {
            let _ = tx.send(result);
        }
        Ok(())
    }

    pub async fn verify_cloud_request(
        &self,
        owner_user_id: u64,
        req: &UpsertCloudComputerRequest,
    ) -> CoreResult<()> {
        let secret = match req.secret.as_deref() {
            Some(s) if !s.is_empty() => Zeroizing::new(s.to_string()),
            _ => {
                if let Some(ref existing_id) = req.node_id {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    let rec = self
                        .db
                        .get_computer_node_with_secret(owner_user_id, existing_id, None, now_ms)?
                        .ok_or_else(|| {
                            TelegramError::NotFound(format!(
                                "Cloud computer `{existing_id}` not found"
                            ))
                        })?;
                    let ct = rec.encrypted_secret.as_deref().ok_or_else(|| {
                        TelegramError::Validation("Missing cloud credential".to_string())
                    })?;
                    let nonce = rec.secret_nonce.as_deref().ok_or_else(|| {
                        TelegramError::Validation("Missing credential nonce".to_string())
                    })?;
                    decrypt_computer_secret(&self.master_key, owner_user_id, ct, nonce)?
                } else {
                    return Err(TelegramError::Validation(
                        "Cloud credential secret is required to verify connection".to_string(),
                    ));
                }
            }
        };

        let host_trimmed = req.host.trim();
        let agent_url = req
            .agent_endpoint_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let temp_dto = ComputerNodeDto {
            node_id: req
                .node_id
                .clone()
                .unwrap_or_else(|| "cloud:verify".to_string()),
            owner_user_id,
            kind: ComputerNodeKind::CloudServer,
            display_name: req.display_name.clone(),
            ip_address: host_trimmed.to_string(),
            host: if host_trimmed.is_empty() {
                None
            } else {
                Some(host_trimmed.to_string())
            },
            ssh_port: Some(if req.port == 0 { 22 } else { req.port }),
            ssh_username: Some(if req.username.trim().is_empty() {
                "root".to_string()
            } else {
                req.username.trim().to_string()
            }),
            auth_method: Some(req.auth_method),
            agent_endpoint_url: agent_url,
            workdir: req.workdir.clone(),
            has_saved_credential: true,
            ..Default::default()
        };

        let secret_for_redaction = secret.clone();
        let res = execute_cloud_operation_with_secret(
            &self.http_client,
            &temp_dto,
            Some(secret),
            "verify-conn",
            &RemoteComputerOp::TestConnection,
        )
        .await;
        if res.ok {
            Ok(())
        } else {
            let reason = if !res.stderr.trim().is_empty() {
                res.stderr
            } else {
                format!("Connection verification exited with code {}", res.exit_code)
            };
            let scrubbed = if !secret_for_redaction.trim().is_empty() {
                reason.replace(secret_for_redaction.as_str(), "[REDACTED]")
            } else {
                reason
            };
            Err(TelegramError::Network(redact_cloud_secrets(&scrubbed)))
        }
    }

    pub async fn execute_remote_command(
        &self,
        owner_user_id: u64,
        req: RemoteComputerCommandRequest,
        header_device_id: Option<&str>,
    ) -> CoreResult<RemoteComputerCommandResult> {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let caller_device_id = req
            .caller_device_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                header_device_id
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .map(ToString::to_string);
        let caller_label = if req.caller_label.trim().is_empty() {
            if let Some(bid) = req.caller_bot_id {
                format!("Bot {bid}")
            } else if let Some(ref did) = caller_device_id {
                format!("Device {did}")
            } else {
                format!("User {owner_user_id}")
            }
        } else {
            req.caller_label.trim().to_string()
        };

        let mut node_rec = self
            .db
            .get_computer_node_with_secret(
                owner_user_id,
                req.node_id.trim(),
                caller_device_id.as_deref(),
                now_ms,
            )?
            .ok_or_else(|| {
                TelegramError::NotFound(format!("Computer node `{}` not found", req.node_id))
            })?;
        self.apply_derp_online_override(owner_user_id, &mut node_rec.dto);

        let command_id = format!("cmd:{}", uuid::Uuid::new_v4());
        let policy = node_rec.dto.run_policy;

        if policy == ComputerRunPolicy::Disabled {
            let _ = self.db.record_computer_audit(
                &command_id,
                owner_user_id,
                &node_rec.dto.node_id,
                &caller_label,
                op_summary(&req.operation),
                false,
                403,
                now_ms,
            );
            return Err(TelegramError::PermissionDenied(
                "Computer execution is disabled by node run policy".to_string(),
            ));
        }

        let bot_profile = if let Some(bid) = req.caller_bot_id {
            Some(self.db.get_bot_computer_profile_or_default(owner_user_id, bid)?)
        } else {
            None
        };

        if let Some(ref profile) = bot_profile {
            if profile.approval_mode == ComputerRunPolicy::Disabled {
                let _ = self.db.record_computer_audit(
                    &command_id,
                    owner_user_id,
                    &node_rec.dto.node_id,
                    &caller_label,
                    op_summary(&req.operation),
                    false,
                    403,
                    now_ms,
                );
                return Err(TelegramError::PermissionDenied(
                    "Bot computer execution is disabled in bot profile".to_string(),
                ));
            }
            if node_rec.dto.kind == ComputerNodeKind::LocalDevice
                && !profile.allow_cross_device_control
                && profile.bound_node_id.as_deref() != Some(node_rec.dto.node_id.as_str())
            {
                let _ = self.db.record_computer_audit(
                    &command_id,
                    owner_user_id,
                    &node_rec.dto.node_id,
                    &caller_label,
                    op_summary(&req.operation),
                    false,
                    403,
                    now_ms,
                );
                return Err(TelegramError::PermissionDenied(
                    "Cross-device control is disabled for this bot".to_string(),
                ));
            }
        }

        let is_same_device_owner = node_rec.dto.kind == ComputerNodeKind::LocalDevice
            && req.caller_bot_id.is_none()
            && caller_device_id.is_some()
            && caller_device_id == node_rec.dto.device_id;

        let requires_user_approval = match evaluate_node_authorization(
            &node_rec.dto,
            caller_device_id.as_deref(),
            req.caller_bot_id,
            &req.operation,
            is_same_device_owner,
            bot_profile.as_ref(),
        ) {
            Ok(req_approval) => req_approval,
            Err(reason) => {
                let _ = self.db.record_computer_audit(
                    &command_id,
                    owner_user_id,
                    &node_rec.dto.node_id,
                    &caller_label,
                    op_summary(&req.operation),
                    false,
                    403,
                    now_ms,
                );
                return Err(TelegramError::PermissionDenied(reason));
            }
        };

        match node_rec.dto.kind {
            ComputerNodeKind::LocalDevice => {
                let target_device_id = node_rec.dto.device_id.clone().ok_or_else(|| {
                    TelegramError::Validation("Local device node missing device_id".to_string())
                })?;
                if !node_rec.dto.online {
                    let _ = self.db.record_computer_audit(
                        &command_id,
                        owner_user_id,
                        &node_rec.dto.node_id,
                        &caller_label,
                        op_summary(&req.operation),
                        false,
                        503,
                        now_ms,
                    );
                    return Ok(RemoteComputerCommandResult {
                        command_id,
                        node_id: node_rec.dto.node_id,
                        ok: false,
                        exit_code: 503,
                        stdout: String::new(),
                        stderr: "Target local device is offline".to_string(),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: target_device_id,
                        duration_ms: 0,
                    });
                }

                let pending = PendingComputerCommandDto {
                    command_id: command_id.clone(),
                    node_id: node_rec.dto.node_id.clone(),
                    target_device_id: target_device_id.clone(),
                    caller_device_id,
                    caller_bot_id: req.caller_bot_id,
                    caller_label: caller_label.clone(),
                    requires_user_approval,
                    approved: !requires_user_approval,
                    operation: req.operation.clone(),
                    created_at_ms: now_ms,
                };

                let (tx, rx) = oneshot::channel();
                {
                    self.waiters.lock().insert(command_id.clone(), tx);
                    self.pending_commands.lock().insert(
                        command_id.clone(),
                        PendingLocalCommandEntry {
                            owner_user_id,
                            dto: pending.clone(),
                        },
                    );
                }
                if let Some(tailnet) = self.tailnet_service() {
                    let _ = tailnet.dispatch_command_to_device(
                        owner_user_id,
                        &target_device_id,
                        pending,
                    );
                }

                match tokio::time::timeout(Duration::from_secs(LOCAL_COMMAND_TIMEOUT_SECS), rx).await
                {
                    Ok(Ok(res)) => Ok(res),
                    _ => {
                        self.pending_commands.lock().remove(&command_id);
                        self.waiters.lock().remove(&command_id);
                        let end_ms = chrono::Utc::now().timestamp_millis();
                        let _ = self.db.record_computer_audit(
                            &command_id,
                            owner_user_id,
                            &node_rec.dto.node_id,
                            &caller_label,
                            op_summary(&req.operation),
                            false,
                            124,
                            end_ms,
                        );
                        Ok(RemoteComputerCommandResult {
                            command_id,
                            node_id: node_rec.dto.node_id,
                            ok: false,
                            exit_code: 124,
                            stdout: String::new(),
                            stderr: format!(
                                "Timed out after {LOCAL_COMMAND_TIMEOUT_SECS}s waiting for local device `{target_device_id}`"
                            ),
                            entries: Vec::new(),
                            frame_data_url: None,
                            executed_by: target_device_id,
                            duration_ms: LOCAL_COMMAND_TIMEOUT_SECS * 1000,
                        })
                    }
                }
            }
            ComputerNodeKind::CloudSandbox => {
                let sandbox_dir = sandbox_root_dir(owner_user_id, node_rec.dto.workdir.as_deref());
                let res = execute_local_fs_and_shell_op(
                    &node_rec.dto.node_id,
                    &command_id,
                    &req.operation,
                    Some(&sandbox_dir),
                    true,
                    "cloud_sandbox",
                )
                .await;
                let end_ms = chrono::Utc::now().timestamp_millis();
                let _ = self.db.touch_computer_node_heartbeat_and_frame(
                    owner_user_id,
                    &node_rec.dto.node_id,
                    res.frame_data_url.as_deref(),
                    end_ms,
                );
                let _ = self.db.record_computer_audit(
                    &command_id,
                    owner_user_id,
                    &node_rec.dto.node_id,
                    &caller_label,
                    op_summary(&req.operation),
                    res.ok,
                    res.exit_code,
                    end_ms,
                );
                Ok(res)
            }
            ComputerNodeKind::CloudServer => {
                let decrypted = match (
                    node_rec.encrypted_secret.as_deref(),
                    node_rec.secret_nonce.as_deref(),
                ) {
                    (Some(ct), Some(nonce)) => Some(decrypt_computer_secret(
                        &self.master_key,
                        owner_user_id,
                        ct,
                        nonce,
                    )?),
                    _ => None,
                };
                let mut res = execute_cloud_operation_with_secret(
                    &self.http_client,
                    &node_rec.dto,
                    decrypted,
                    &command_id,
                    &req.operation,
                )
                .await;
                res.stdout = redact_cloud_secrets(&res.stdout);
                res.stderr = redact_cloud_secrets(&res.stderr);
                let end_ms = chrono::Utc::now().timestamp_millis();
                let _ = self.db.touch_computer_node_heartbeat_and_frame(
                    owner_user_id,
                    &node_rec.dto.node_id,
                    res.frame_data_url.as_deref(),
                    end_ms,
                );
                let _ = self.db.record_computer_audit(
                    &command_id,
                    owner_user_id,
                    &node_rec.dto.node_id,
                    &caller_label,
                    op_summary(&req.operation),
                    res.ok,
                    res.exit_code,
                    end_ms,
                );
                Ok(res)
            }
        }
    }
}

fn evaluate_node_authorization(
    node: &ComputerNodeDto,
    caller_device_id: Option<&str>,
    caller_bot_id: Option<u64>,
    op: &RemoteComputerOp,
    is_same_device_owner: bool,
    bot_profile: Option<&BotComputerProfileDto>,
) -> std::result::Result<bool, String> {
    let mut requires_approval = match node.run_policy {
        ComputerRunPolicy::Disabled => {
            return Err("Computer execution is disabled by node run policy".to_string());
        }
        ComputerRunPolicy::AskEveryTime => !is_same_device_owner,
        ComputerRunPolicy::AllowReadOnly => {
            if is_same_device_owner || op.is_read_only() {
                false
            } else {
                true
            }
        }
        ComputerRunPolicy::AlwaysAllowAuthorized => {
            if is_same_device_owner {
                false
            } else if node.authorized_controllers.is_empty() {
                match node.kind {
                    ComputerNodeKind::LocalDevice => {
                        if caller_bot_id.is_some() {
                            return Err(
                                "Bot is not listed in local device authorized_controllers"
                                    .to_string(),
                            );
                        }
                        false
                    }
                    ComputerNodeKind::CloudServer | ComputerNodeKind::CloudSandbox => false,
                }
            } else {
                let authorized = node.authorized_controllers.iter().any(|entry| {
                    match entry.kind {
                        AuthorizedControllerKind::AccountAllBots => caller_bot_id.is_some(),
                        AuthorizedControllerKind::Bot => caller_bot_id.is_some_and(|bid| {
                            let id_str = bid.to_string();
                            entry.controller_id == id_str
                                || entry.controller_id == format!("bot:{bid}")
                        }),
                        AuthorizedControllerKind::Device => {
                            entry.controller_id == "account_all_devices"
                                || entry.controller_id == "*"
                                || caller_device_id.is_some_and(|did| {
                                    entry.controller_id == did
                                        || entry.controller_id == format!("device:{did}")
                                })
                        }
                    }
                });
                if !authorized {
                    return Err("Caller is not authorized on this computer node".to_string());
                }
                false
            }
        }
    };

    if node.kind == ComputerNodeKind::LocalDevice {
        if let Some(profile) = bot_profile {
            match profile.approval_mode {
                ComputerRunPolicy::AskEveryTime => requires_approval = true,
                ComputerRunPolicy::AllowReadOnly if !op.is_read_only() => {
                    requires_approval = true;
                }
                _ => {}
            }
        }
    }

    Ok(requires_approval)
}

impl ComputerCloudProvider for ComputerControlService {
    fn list_computers<'a>(
        &'a self,
        user_id: u64,
        bot_id: Option<u64>,
    ) -> BoxFuture<'a, CoreResult<Vec<AgentComputerNodeSummary>>> {
        Box::pin(async move {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let mut nodes = self.db.list_computer_nodes(user_id, None, now_ms)?;
            for dto in &mut nodes {
                self.apply_derp_online_override(user_id, dto);
            }
            let bound_node_id = if let Some(bid) = bot_id {
                self.db
                    .get_bot_computer_profile(user_id, bid)?
                    .and_then(|p| p.bound_node_id)
            } else {
                None
            };
            Ok(nodes
                .into_iter()
                .map(|dto| dto_to_agent_summary(&dto, bot_id, bound_node_id.as_deref()))
                .collect())
        })
    }

    fn bind_bot_computer<'a>(
        &'a self,
        user_id: u64,
        bot_id: u64,
        node_id: Option<String>,
    ) -> BoxFuture<'a, CoreResult<AgentComputerNodeSummary>> {
        Box::pin(async move {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let trimmed_id = node_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string);
            let update = UpdateBotComputerRequest {
                bound_node_id: trimmed_id.clone(),
                clear_bound_node: trimmed_id.is_none(),
                ..Default::default()
            };
            let profile = self
                .db
                .upsert_bot_computer_profile(user_id, bot_id, &update, now_ms)?;
            if let Some(ref target_id) = profile.bound_node_id {
                let mut node = self
                    .db
                    .get_computer_node(user_id, target_id, None, now_ms)?
                    .ok_or_else(|| {
                        TelegramError::NotFound(format!("Computer node `{target_id}` not found"))
                    })?;
                self.apply_derp_online_override(user_id, &mut node);
                Ok(dto_to_agent_summary(
                    &node,
                    Some(bot_id),
                    profile.bound_node_id.as_deref(),
                ))
            } else {
                let sandbox = self
                    .db
                    .get_computer_node(user_id, DEFAULT_SANDBOX_NODE_ID, None, now_ms)?
                    .ok_or_else(|| {
                        TelegramError::Internal("Default cloud sandbox missing".to_string())
                    })?;
                let mut summary = dto_to_agent_summary(&sandbox, Some(bot_id), None);
                summary.bound_to_this_bot = false;
                Ok(summary)
            }
        })
    }

    fn configure_cloud_server<'a>(
        &'a self,
        user_id: u64,
        bot_id: Option<u64>,
        cfg: AgentCloudServerConfigInput,
    ) -> BoxFuture<'a, CoreResult<AgentComputerNodeSummary>> {
        Box::pin(async move {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if let Some(bid) = bot_id {
                let profile = self.db.get_bot_computer_profile_or_default(user_id, bid)?;
                if !profile.allow_cloud_self_config {
                    return Err(TelegramError::PermissionDenied(
                        "Bot is not permitted to configure cloud servers (allow_cloud_self_config=false)"
                            .to_string(),
                    ));
                }
            }

            let auth_method = parse_cloud_auth_method(&cfg.auth_method).unwrap_or_else(|| {
                if cfg
                    .agent_endpoint_url
                    .as_deref()
                    .is_some_and(|u| !u.trim().is_empty())
                {
                    CloudAuthMethod::AgentHttpToken
                } else if cfg.secret.contains("PRIVATE KEY") {
                    CloudAuthMethod::SshPrivateKey
                } else {
                    CloudAuthMethod::SshPassword
                }
            });

            if cfg.node_id.is_none() && cfg.secret.trim().is_empty() {
                return Err(TelegramError::Validation(
                    "Cloud server secret/password/token must not be empty".to_string(),
                ));
            }

            let upsert_req = UpsertCloudComputerRequest {
                node_id: cfg.node_id.clone(),
                display_name: cfg.display_name,
                host: cfg.host,
                port: if cfg.port == 0 && auth_method != CloudAuthMethod::AgentHttpToken {
                    22
                } else {
                    cfg.port
                },
                username: cfg.username,
                auth_method,
                secret: if cfg.secret.is_empty() {
                    None
                } else {
                    Some(cfg.secret)
                },
                agent_endpoint_url: cfg.agent_endpoint_url,
                workdir: cfg.workdir,
                bind_bot_id: if cfg.bind_to_bot { bot_id } else { None },
                verify_connection: true,
            };

            self.verify_cloud_request(user_id, &upsert_req).await?;

            let saved = self.db.upsert_cloud_computer_node(
                user_id,
                &upsert_req,
                &self.master_key,
                None,
                now_ms,
            )?;

            let bound_id = if cfg.bind_to_bot && bot_id.is_some() {
                Some(saved.node_id.as_str())
            } else {
                None
            };
            Ok(dto_to_agent_summary(&saved, bot_id, bound_id))
        })
    }

    fn execute_on_computer<'a>(
        &'a self,
        user_id: u64,
        req: AgentComputerExecInput,
    ) -> BoxFuture<'a, CoreResult<AgentComputerExecOutput>> {
        Box::pin(async move {
            let now_ms = chrono::Utc::now().timestamp_millis();
            let bot_profile = if let Some(bid) = req.caller_bot_id {
                Some(self.db.get_bot_computer_profile_or_default(user_id, bid)?)
            } else {
                None
            };

            let resolved_node_id = if let Some(explicit) = req
                .node_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                explicit.to_string()
            } else if let Some(bound) = bot_profile
                .as_ref()
                .and_then(|p| p.bound_node_id.as_deref())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                bound.to_string()
            } else {
                let mut nodes = self.db.list_computer_nodes(user_id, None, now_ms)?;
                for dto in &mut nodes {
                    self.apply_derp_online_override(user_id, dto);
                }
                nodes
                    .iter()
                    .find(|n| n.kind == ComputerNodeKind::CloudServer && n.online)
                    .or_else(|| {
                        nodes
                            .iter()
                            .find(|n| n.kind == ComputerNodeKind::LocalDevice && n.online)
                    })
                    .map(|n| n.node_id.clone())
                    .unwrap_or_else(|| DEFAULT_SANDBOX_NODE_ID.to_string())
            };

            let operation = agent_input_to_remote_op(&req)?;
            let caller_label = req
                .caller_bot_id
                .map(|bid| format!("Bot {bid}"))
                .unwrap_or_else(|| format!("Hermes User {user_id}"));

            let cmd_req = RemoteComputerCommandRequest {
                node_id: resolved_node_id.clone(),
                caller_device_id: None,
                caller_bot_id: req.caller_bot_id,
                caller_label,
                operation,
            };

            match self.execute_remote_command(user_id, cmd_req, None).await {
                Ok(res) => Ok(AgentComputerExecOutput {
                    node_id: res.node_id,
                    ok: res.ok,
                    exit_code: res.exit_code,
                    stdout: res.stdout,
                    stderr: res.stderr,
                    entries: res.entries,
                    frame_data_url: res.frame_data_url,
                    executed_by: res.executed_by,
                }),
                Err(TelegramError::PermissionDenied(reason)) => Ok(AgentComputerExecOutput {
                    node_id: resolved_node_id,
                    ok: false,
                    exit_code: 403,
                    stdout: String::new(),
                    stderr: reason,
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: "policy_guard".to_string(),
                }),
                Err(err) => Err(err),
            }
        })
    }
}

fn dto_to_agent_summary(
    dto: &ComputerNodeDto,
    bot_id: Option<u64>,
    bound_node_id: Option<&str>,
) -> AgentComputerNodeSummary {
    let bound_to_this_bot = bot_id.is_some_and(|bid| {
        dto.bound_bot_ids.contains(&bid) || bound_node_id == Some(dto.node_id.as_str())
    });
    AgentComputerNodeSummary {
        node_id: dto.node_id.clone(),
        kind: dto.kind.as_str().to_string(),
        display_name: dto.display_name.clone(),
        ip_address: dto.ip_address.clone(),
        host: dto.host.clone(),
        os_name: dto.os_name.clone(),
        online: dto.online,
        is_current_device: dto.is_current_device,
        bound_to_this_bot,
        run_policy: dto.run_policy.as_str().to_string(),
        tier_badge: dto.tier_badge.clone(),
    }
}

fn agent_input_to_remote_op(req: &AgentComputerExecInput) -> CoreResult<RemoteComputerOp> {
    let op = req.op.trim().to_ascii_lowercase();
    match op.as_str() {
        "shell_exec" | "exec" => {
            let command = req
                .command
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    TelegramError::Validation("Missing command for shell_exec".to_string())
                })?
                .to_string();
            Ok(RemoteComputerOp::ShellExec {
                command,
                cwd: req.cwd.clone(),
                timeout_ms: Some(REMOTE_EXEC_TIMEOUT_MS_DEFAULT),
            })
        }
        "read_file" | "read" => {
            let path = req
                .path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    TelegramError::Validation("Missing path for read_file".to_string())
                })?
                .to_string();
            Ok(RemoteComputerOp::ReadFile {
                path,
                max_bytes: Some(MAX_READ_BYTES_CAP),
            })
        }
        "write_file" | "write" => {
            let path = req
                .path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    TelegramError::Validation("Missing path for write_file".to_string())
                })?
                .to_string();
            let content = req.content.clone().unwrap_or_default();
            Ok(RemoteComputerOp::WriteFile {
                path,
                content,
                create_dirs: true,
            })
        }
        "list_dir" | "list" => {
            let path = req
                .path
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(".")
                .to_string();
            Ok(RemoteComputerOp::ListDir { path })
        }
        "capture_screen" | "screenshot" => Ok(RemoteComputerOp::CaptureScreen),
        "click" | "move" | "type" | "key" | "gui_input" => Ok(RemoteComputerOp::GuiInput {
            action: op,
            x: req.x,
            y: req.y,
            text: req.text.clone(),
            key: req.key.clone(),
        }),
        "test_connection" | "test" => Ok(RemoteComputerOp::TestConnection),
        other => Err(TelegramError::Validation(format!(
            "Unsupported remote computer operation `{other}`"
        ))),
    }
}

pub(crate) fn format_computer_context_block(
    state: &AppState,
    user_id: u64,
    bot_id: Option<u64>,
    profile: Option<&BotComputerProfileDto>,
    now_ms: i64,
) -> String {
    let mut nodes = state
        .db
        .list_computer_nodes(user_id, None, now_ms)
        .unwrap_or_default();
    for dto in &mut nodes {
        if dto.kind == ComputerNodeKind::LocalDevice && !dto.online {
            if let Some(ref did) = dto.device_id {
                if state.tailnet.is_device_derp_connected(user_id, did) {
                    dto.online = true;
                }
            }
        }
    }
    let bound_node_id = profile.and_then(|p| p.bound_node_id.as_deref());

    let mut lines = Vec::new();
    lines.push("## Computer & Cloud Server Guidelines".to_string());
    lines.push("[Computer & Cloud Server Context]".to_string());
    if let Some(p) = profile {
        lines.push(format!(
            "- Bot computer policy: bound_node_id={}, approval_mode={}, allow_cross_device_control={}, allow_cloud_self_config={}",
            p.bound_node_id.as_deref().unwrap_or("none"),
            p.approval_mode.as_str(),
            p.allow_cross_device_control,
            p.allow_cloud_self_config
        ));
    }
    lines.push(format!("- Controllable computers ({}):", nodes.len()));
    for n in &nodes {
        let host_label = n
            .agent_endpoint_url
            .as_deref()
            .or(n.host.as_deref())
            .filter(|s| !s.is_empty())
            .unwrap_or(if n.ip_address.is_empty() {
                "local"
            } else {
                n.ip_address.as_str()
            });
        let bound_tag = if bound_node_id == Some(n.node_id.as_str())
            || bot_id.is_some_and(|bid| n.bound_bot_ids.contains(&bid))
        {
            " [BOUND]"
        } else {
            ""
        };
        lines.push(format!(
            "  * {} | {} | kind={} | tier={} | online={} | policy={} | host={}{}",
            n.node_id,
            n.display_name,
            n.kind.as_str(),
            n.tier_badge,
            n.online,
            n.run_policy.as_str(),
            host_label,
            bound_tag
        ));
    }
    lines.push(
        "- Tools: computer_list, computer_bind, computer_configure_cloud, computer_exec, computer_file, computer_screen."
            .to_string(),
    );
    lines.push(
        "- Security: When user provides cloud IP/host + SSH/HTTP credentials, call `computer_configure_cloud` to encrypt and store them; NEVER echo raw passwords, private keys, or tokens back in replies."
            .to_string(),
    );

    lines.join("\n")
}

pub(crate) fn enrich_bot_turn_prompt_and_model(
    state: &AppState,
    user_id: u64,
    bot_id: Option<u64>,
    base_system_prompt: Option<String>,
    explicit_model: Option<String>,
    fallback_model: Option<String>,
) -> (Option<String>, Option<String>) {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let profile = bot_id.and_then(|bid| {
        state
            .db
            .get_bot_computer_profile_or_default(user_id, bid)
            .ok()
    });
    let effective_base = match base_system_prompt {
        Some(sp) if !sp.trim().is_empty() => Some(sp),
        _ => profile
            .as_ref()
            .map(|p| p.persona_prompt.trim())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
    };
    let effective_model = explicit_model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .or_else(|| {
            profile
                .as_ref()
                .map(|p| p.preferred_model.trim())
                .filter(|m| !m.is_empty())
                .map(ToString::to_string)
        })
        .or_else(|| {
            fallback_model
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty())
        });

    let ctx_block = format_computer_context_block(state, user_id, bot_id, profile.as_ref(), now_ms);
    let merged_prompt = merge_bot_system_prompt(effective_base, Some(ctx_block));
    (merged_prompt, effective_model)
}

pub(crate) fn merge_bot_system_prompt(
    base_prompt: Option<String>,
    computer_context: Option<String>,
) -> Option<String> {
    match (base_prompt, computer_context) {
        (Some(base), Some(ctx)) => {
            let b = base.trim();
            let c = ctx.trim();
            if b.is_empty() {
                (!c.is_empty()).then(|| c.to_string())
            } else if c.is_empty() {
                Some(b.to_string())
            } else {
                Some(format!("{b}\n\n{c}"))
            }
        }
        (Some(b), None) => {
            let t = b.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        (None, Some(c)) => {
            let t = c.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        (None, None) => None,
    }
}

fn header_device_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

fn op_summary(op: &RemoteComputerOp) -> &'static str {
    match op {
        RemoteComputerOp::ShellExec { .. } => "shell_exec",
        RemoteComputerOp::ReadFile { .. } => "read_file",
        RemoteComputerOp::WriteFile { .. } => "write_file",
        RemoteComputerOp::ListDir { .. } => "list_dir",
        RemoteComputerOp::CaptureScreen => "capture_screen",
        RemoteComputerOp::GuiInput { .. } => "gui_input",
        RemoteComputerOp::TestConnection => "test_connection",
    }
}

fn sandbox_root_dir(owner_user_id: u64, configured_workdir: Option<&str>) -> PathBuf {
    if let Some(raw) = configured_workdir.map(str::trim).filter(|s| !s.is_empty()) {
        PathBuf::from(raw)
    } else {
        std::env::temp_dir().join(format!("gpui_sandbox_{owner_user_id}"))
    }
}

fn resolve_contained_path(base: &StdPath, raw_path: &str) -> CoreResult<PathBuf> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() || trimmed == "." {
        return Ok(base.to_path_buf());
    }
    let candidate = StdPath::new(trimmed);
    let mut normalized = PathBuf::new();

    if candidate.is_absolute() {
        let stripped = candidate.strip_prefix(base).map_err(|_| {
            TelegramError::PermissionDenied(format!(
                "Sandbox path escapes root directory: `{trimmed}`"
            ))
        })?;
        for comp in stripped.components() {
            match comp {
                Component::Normal(p) => normalized.push(p),
                Component::CurDir => {}
                _ => {
                    return Err(TelegramError::PermissionDenied(format!(
                        "Sandbox path traversal rejected: `{trimmed}`"
                    )));
                }
            }
        }
        return Ok(base.join(normalized));
    }

    for comp in candidate.components() {
        match comp {
            Component::Normal(p) => normalized.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(TelegramError::PermissionDenied(format!(
                        "Sandbox path traversal rejected: `{trimmed}`"
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(TelegramError::PermissionDenied(format!(
                    "Sandbox path escapes root directory: `{trimmed}`"
                )));
            }
        }
    }
    Ok(base.join(normalized))
}

fn canonicalize_under_sandbox_for_existing(base_canon: &StdPath, target: &StdPath) -> CoreResult<()> {
    let mut probe = target.to_path_buf();
    while !probe.exists() {
        if !probe.pop() {
            break;
        }
    }
    if probe.exists() {
        let canon = std::fs::canonicalize(&probe)?;
        if !canon.starts_with(base_canon) {
            return Err(TelegramError::PermissionDenied(
                "Symlink escape out of sandbox blocked".to_string(),
            ));
        }
    }
    Ok(())
}

fn truncate_output_utf8(bytes: &[u8], limit: usize) -> String {
    if bytes.len() <= limit {
        String::from_utf8_lossy(bytes).to_string()
    } else {
        let mut s = String::from_utf8_lossy(&bytes[..limit]).to_string();
        s.push_str("\n...[truncated]");
        s
    }
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn push_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32_ieee(&crc_input).to_be_bytes());
}

fn headless_frame_data_url() -> &'static str {
    static FRAME: OnceLock<String> = OnceLock::new();
    FRAME.get_or_init(|| {
        let mut png = Vec::from(*b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(6); // RGBA
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(0); // interlace
        push_png_chunk(&mut png, b"IHDR", &ihdr);

        let raw_scanline = [0u8, 24, 34, 52, 255];
        let mut zlib = Vec::with_capacity(16);
        zlib.push(0x78);
        zlib.push(0x01);
        zlib.push(0x01); // BFINAL=1, BTYPE=00 (stored)
        let len = raw_scanline.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(&raw_scanline);
        zlib.extend_from_slice(&adler32(&raw_scanline).to_be_bytes());
        push_png_chunk(&mut png, b"IDAT", &zlib);
        push_png_chunk(&mut png, b"IEND", &[]);

        let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
        format!("data:image/png;base64,{encoded}")
    })
}

async fn execute_local_fs_and_shell_op(
    node_id: &str,
    command_id: &str,
    op: &RemoteComputerOp,
    workdir: Option<&StdPath>,
    contain_to_workdir: bool,
    executed_by: &str,
) -> RemoteComputerCommandResult {
    let started = Instant::now();
    let base_canon = if let Some(dir) = workdir {
        if let Err(err) = tokio::fs::create_dir_all(dir).await {
            return RemoteComputerCommandResult {
                command_id: command_id.to_string(),
                node_id: node_id.to_string(),
                ok: false,
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("Failed to prepare workdir: {err}"),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by: executed_by.to_string(),
                duration_ms: started.elapsed().as_millis() as u64,
            };
        }
        match std::fs::canonicalize(dir) {
            Ok(c) => Some(c),
            Err(err) => {
                return RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node_id.to_string(),
                    ok: false,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Failed to canonicalize workdir: {err}"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: executed_by.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        }
    } else {
        None
    };

    let resolve_path = |raw: &str| -> CoreResult<PathBuf> {
        if contain_to_workdir {
            let base = base_canon.as_deref().ok_or_else(|| {
                TelegramError::Internal("Sandbox base directory missing".to_string())
            })?;
            let resolved = resolve_contained_path(base, raw)?;
            canonicalize_under_sandbox_for_existing(base, &resolved)?;
            Ok(resolved)
        } else if let Some(base) = base_canon.as_deref() {
            let p = StdPath::new(raw.trim());
            if p.is_absolute() {
                Ok(p.to_path_buf())
            } else {
                Ok(base.join(p))
            }
        } else {
            Ok(PathBuf::from(raw.trim()))
        }
    };

    match op {
        RemoteComputerOp::TestConnection => RemoteComputerCommandResult {
            command_id: command_id.to_string(),
            node_id: node_id.to_string(),
            ok: true,
            exit_code: 0,
            stdout: format!(
                "{executed_by} online (workdir={})",
                base_canon
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "default".to_string())
            ),
            stderr: String::new(),
            entries: Vec::new(),
            frame_data_url: None,
            executed_by: executed_by.to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
        },
        RemoteComputerOp::CaptureScreen => RemoteComputerCommandResult {
            command_id: command_id.to_string(),
            node_id: node_id.to_string(),
            ok: true,
            exit_code: 0,
            stdout: "Captured headless display frame".to_string(),
            stderr: String::new(),
            entries: Vec::new(),
            frame_data_url: Some(headless_frame_data_url().to_string()),
            executed_by: executed_by.to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
        },
        RemoteComputerOp::GuiInput { action, .. } => RemoteComputerCommandResult {
            command_id: command_id.to_string(),
            node_id: node_id.to_string(),
            ok: false,
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("GUI input `{action}` is unavailable in headless {executed_by}"),
            entries: Vec::new(),
            frame_data_url: None,
            executed_by: executed_by.to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
        },
        RemoteComputerOp::ReadFile { path, max_bytes } => {
            let target = match resolve_path(path) {
                Ok(p) => p,
                Err(e) => {
                    return RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node_id.to_string(),
                        ok: false,
                        exit_code: 403,
                        stdout: String::new(),
                        stderr: e.to_string(),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: executed_by.to_string(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    };
                }
            };
            let cap = max_bytes
                .unwrap_or(MAX_READ_BYTES_DEFAULT)
                .clamp(1, MAX_READ_BYTES_CAP);
            match tokio::fs::File::open(&target).await {
                Ok(mut f) => {
                    let mut buf = vec![0u8; cap];
                    let mut read_total = 0usize;
                    while read_total < cap {
                        match f.read(&mut buf[read_total..]).await {
                            Ok(0) => break,
                            Ok(n) => read_total += n,
                            Err(e) => {
                                return RemoteComputerCommandResult {
                                    command_id: command_id.to_string(),
                                    node_id: node_id.to_string(),
                                    ok: false,
                                    exit_code: 1,
                                    stdout: String::new(),
                                    stderr: format!("Read error: {e}"),
                                    entries: Vec::new(),
                                    frame_data_url: None,
                                    executed_by: executed_by.to_string(),
                                    duration_ms: started.elapsed().as_millis() as u64,
                                };
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&buf[..read_total]).to_string();
                    RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node_id.to_string(),
                        ok: true,
                        exit_code: 0,
                        stdout: text,
                        stderr: String::new(),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: executed_by.to_string(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    }
                }
                Err(e) => RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node_id.to_string(),
                    ok: false,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Failed to open `{path}`: {e}"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: executed_by.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            }
        }
        RemoteComputerOp::WriteFile {
            path,
            content,
            create_dirs,
        } => {
            let target = match resolve_path(path) {
                Ok(p) => p,
                Err(e) => {
                    return RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node_id.to_string(),
                        ok: false,
                        exit_code: 403,
                        stdout: String::new(),
                        stderr: e.to_string(),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: executed_by.to_string(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    };
                }
            };
            if *create_dirs {
                if let Some(parent) = target.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return RemoteComputerCommandResult {
                            command_id: command_id.to_string(),
                            node_id: node_id.to_string(),
                            ok: false,
                            exit_code: 1,
                            stdout: String::new(),
                            stderr: format!("Failed to create parent directories: {e}"),
                            entries: Vec::new(),
                            frame_data_url: None,
                            executed_by: executed_by.to_string(),
                            duration_ms: started.elapsed().as_millis() as u64,
                        };
                    }
                }
            }
            match tokio::fs::write(&target, content.as_bytes()).await {
                Ok(()) => RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node_id.to_string(),
                    ok: true,
                    exit_code: 0,
                    stdout: format!("Wrote {} bytes to {}", content.len(), target.display()),
                    stderr: String::new(),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: executed_by.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
                Err(e) => RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node_id.to_string(),
                    ok: false,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Failed to write `{path}`: {e}"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: executed_by.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            }
        }
        RemoteComputerOp::ListDir { path } => {
            let target = match resolve_path(path) {
                Ok(p) => p,
                Err(e) => {
                    return RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node_id.to_string(),
                        ok: false,
                        exit_code: 403,
                        stdout: String::new(),
                        stderr: e.to_string(),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: executed_by.to_string(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    };
                }
            };
            match tokio::fs::read_dir(&target).await {
                Ok(mut dir) => {
                    let mut entries = Vec::new();
                    while let Ok(Some(ent)) = dir.next_entry().await {
                        let name = ent.file_name().to_string_lossy().to_string();
                        let suffix = match ent.file_type().await {
                            Ok(ft) if ft.is_dir() => "/",
                            _ => "",
                        };
                        entries.push(format!("{name}{suffix}"));
                    }
                    entries.sort();
                    let stdout = entries.join("\n");
                    RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node_id.to_string(),
                        ok: true,
                        exit_code: 0,
                        stdout,
                        stderr: String::new(),
                        entries,
                        frame_data_url: None,
                        executed_by: executed_by.to_string(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    }
                }
                Err(e) => RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node_id.to_string(),
                    ok: false,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Failed to list `{path}`: {e}"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: executed_by.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            }
        }
        RemoteComputerOp::ShellExec {
            command,
            cwd,
            timeout_ms,
        } => {
            let effective_cwd = match cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(raw) => match resolve_path(raw) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        return RemoteComputerCommandResult {
                            command_id: command_id.to_string(),
                            node_id: node_id.to_string(),
                            ok: false,
                            exit_code: 403,
                            stdout: String::new(),
                            stderr: e.to_string(),
                            entries: Vec::new(),
                            frame_data_url: None,
                            executed_by: executed_by.to_string(),
                            duration_ms: started.elapsed().as_millis() as u64,
                        };
                    }
                },
                None => base_canon,
            };

            let mut cmd = if cfg!(target_os = "windows") {
                let mut c = Command::new("cmd");
                c.args(["/C", command]);
                c
            } else {
                let mut c = Command::new("sh");
                c.args(["-c", command]);
                c
            };
            if let Some(dir) = effective_cwd {
                cmd.current_dir(dir);
            }
            cmd.kill_on_drop(true);
            let wait_ms = timeout_ms
                .unwrap_or(REMOTE_EXEC_TIMEOUT_MS_DEFAULT)
                .clamp(500, REMOTE_EXEC_TIMEOUT_MS_MAX);

            match tokio::time::timeout(Duration::from_millis(wait_ms), cmd.output()).await {
                Ok(Ok(out)) => {
                    let exit_code = out.status.code().unwrap_or(if out.status.success() { 0 } else { 1 });
                    RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node_id.to_string(),
                        ok: out.status.success(),
                        exit_code,
                        stdout: truncate_output_utf8(&out.stdout, MAX_OUTPUT_BYTES),
                        stderr: truncate_output_utf8(&out.stderr, MAX_OUTPUT_BYTES),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by: executed_by.to_string(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    }
                }
                Ok(Err(e)) => RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node_id.to_string(),
                    ok: false,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Failed to spawn shell command: {e}"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: executed_by.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
                Err(_) => RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node_id.to_string(),
                    ok: false,
                    exit_code: 124,
                    stdout: String::new(),
                    stderr: format!("Shell command timed out after {wait_ms}ms"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by: executed_by.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            }
        }
    }
}

fn resolve_agent_exec_url(node: &ComputerNodeDto) -> CoreResult<String> {
    if let Some(raw) = node
        .agent_endpoint_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !(raw.starts_with("http://") || raw.starts_with("https://")) {
            return Err(TelegramError::Validation(format!(
                "Agent endpoint URL must use http:// or https://: `{raw}`"
            )));
        }
        let trimmed = raw.trim_end_matches('/');
        if trimmed.ends_with("/api/v1/computer-agent/exec") || trimmed.ends_with("/exec") {
            return Ok(trimmed.to_string());
        }
        return Ok(format!("{trimmed}/api/v1/computer-agent/exec"));
    }

    let host = node
        .host
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            TelegramError::Validation(
                "Cloud agent node requires agent_endpoint_url or host".to_string(),
            )
        })?;
    let port = node.ssh_port.unwrap_or(8080);
    Ok(format!("http://{host}:{port}/api/v1/computer-agent/exec"))
}

fn shell_quote_posix(s: &str) -> String {
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn validate_ssh_token(field_name: &str, value: &str) -> CoreResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('-')
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(TelegramError::Validation(format!(
            "Invalid SSH {field_name}: `{trimmed}`"
        )));
    }
    Ok(trimmed.to_string())
}

fn build_remote_posix_script(op: &RemoteComputerOp, workdir: Option<&str>) -> String {
    let join_with_workdir = |raw: &str| -> String {
        let trimmed = raw.trim();
        if trimmed.starts_with('/') || trimmed.starts_with("~/") {
            trimmed.to_string()
        } else if let Some(wd) = workdir.map(str::trim).filter(|s| !s.is_empty()) {
            format!("{}/{}", wd.trim_end_matches('/'), trimmed)
        } else {
            trimmed.to_string()
        }
    };

    match op {
        RemoteComputerOp::TestConnection => {
            "printf 'gpui-cloud-ok:%s\\n' \"$(uname -srm)\"".to_string()
        }
        RemoteComputerOp::ShellExec { command, cwd, .. } => {
            let effective_cwd = cwd
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .or_else(|| {
                    workdir
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                });
            if let Some(dir) = effective_cwd {
                format!("cd {} && {}", shell_quote_posix(&dir), command)
            } else {
                command.clone()
            }
        }
        RemoteComputerOp::ReadFile { path, max_bytes } => {
            let cap = max_bytes
                .unwrap_or(MAX_READ_BYTES_DEFAULT)
                .clamp(1, MAX_READ_BYTES_CAP);
            let target = shell_quote_posix(&join_with_workdir(path));
            format!("head -c {cap} {target}")
        }
        RemoteComputerOp::WriteFile {
            path,
            content,
            create_dirs,
        } => {
            let full = join_with_workdir(path);
            let target = shell_quote_posix(&full);
            let b64 = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
            let dir_part = if *create_dirs {
                format!("mkdir -p \"$(dirname {target})\" && ")
            } else {
                String::new()
            };
            format!(
                "{dir_part}printf '%s' '{}' | (base64 -d 2>/dev/null || base64 -D) > {target}",
                b64
            )
        }
        RemoteComputerOp::ListDir { path } => {
            let target = shell_quote_posix(&join_with_workdir(path));
            format!("ls -1A {target}")
        }
        RemoteComputerOp::CaptureScreen => {
            "echo 'capture_screen is unsupported without a remote graphical agent' >&2; exit 1"
                .to_string()
        }
        RemoteComputerOp::GuiInput { action, .. } => {
            format!(
                "echo {} >&2; exit 1",
                shell_quote_posix(&format!(
                    "gui_input `{action}` requires an HTTP desktop/agent endpoint"
                ))
            )
        }
    }
}

async fn execute_cloud_operation_with_secret(
    http_client: &reqwest::Client,
    node: &ComputerNodeDto,
    secret: Option<Zeroizing<String>>,
    command_id: &str,
    op: &RemoteComputerOp,
) -> RemoteComputerCommandResult {
    let started = Instant::now();
    let method = node.auth_method.unwrap_or(CloudAuthMethod::SshPassword);
    let executed_by = format!(
        "{}:{}",
        method.as_str(),
        node.host
            .as_deref()
            .or(node.agent_endpoint_url.as_deref())
            .unwrap_or(&node.node_id)
    );

    if method == CloudAuthMethod::AgentHttpToken {
        let url = match resolve_agent_exec_url(node) {
            Ok(u) => u,
            Err(e) => {
                return RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node.node_id.clone(),
                    ok: false,
                    exit_code: 400,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        };
        let Some(token) = secret.filter(|s| !s.trim().is_empty()) else {
            return RemoteComputerCommandResult {
                command_id: command_id.to_string(),
                node_id: node.node_id.clone(),
                ok: false,
                exit_code: 401,
                stdout: String::new(),
                stderr: "Missing encrypted agent HTTP token".to_string(),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        };

        let body = json!({
            "command_id": command_id,
            "node_id": node.node_id,
            "workdir": node.workdir,
            "operation": op,
        });

        match http_client
            .post(&url)
            .bearer_auth(token.as_str().trim())
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status.is_success() {
                    if let Ok(mut parsed) = serde_json::from_str::<RemoteComputerCommandResult>(&text) {
                        if parsed.command_id.is_empty() {
                            parsed.command_id = command_id.to_string();
                        }
                        if parsed.node_id.is_empty() {
                            parsed.node_id = node.node_id.clone();
                        }
                        if parsed.executed_by.is_empty() {
                            parsed.executed_by = executed_by;
                        }
                        return parsed;
                    }
                    return RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node.node_id.clone(),
                        ok: true,
                        exit_code: 0,
                        stdout: text,
                        stderr: String::new(),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by,
                        duration_ms: started.elapsed().as_millis() as u64,
                    };
                }
                RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node.node_id.clone(),
                    ok: false,
                    exit_code: status.as_u16() as i32,
                    stdout: String::new(),
                    stderr: format!("Remote HTTP agent error ({status}): {text}"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by,
                    duration_ms: started.elapsed().as_millis() as u64,
                }
            }
            Err(e) => RemoteComputerCommandResult {
                command_id: command_id.to_string(),
                node_id: node.node_id.clone(),
                ok: false,
                exit_code: 502,
                stdout: String::new(),
                stderr: format!("Failed to reach remote HTTP agent `{url}`: {e}"),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by,
                duration_ms: started.elapsed().as_millis() as u64,
            },
        }
    } else {
        // SSH transport (SshPrivateKey or SshPassword)
        let host = match validate_ssh_token("host", node.host.as_deref().unwrap_or("")) {
            Ok(h) => h,
            Err(e) => {
                return RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node.node_id.clone(),
                    ok: false,
                    exit_code: 400,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        };
        let username = match validate_ssh_token(
            "username",
            node.ssh_username.as_deref().unwrap_or("root"),
        ) {
            Ok(u) => u,
            Err(e) => {
                return RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node.node_id.clone(),
                    ok: false,
                    exit_code: 400,
                    stdout: String::new(),
                    stderr: e.to_string(),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        };
        let port = node.ssh_port.unwrap_or(22);
        let Some(secret_val) = secret.filter(|s| !s.is_empty()) else {
            return RemoteComputerCommandResult {
                command_id: command_id.to_string(),
                node_id: node.node_id.clone(),
                ok: false,
                exit_code: 401,
                stdout: String::new(),
                stderr: "Missing encrypted SSH credential".to_string(),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by,
                duration_ms: started.elapsed().as_millis() as u64,
            };
        };

        let temp_guard = match TempDirGuard::create("gpui-ssh") {
            Ok(g) => g,
            Err(e) => {
                return RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node.node_id.clone(),
                    ok: false,
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Failed to prepare SSH temp directory: {e}"),
                    entries: Vec::new(),
                    frame_data_url: None,
                    executed_by,
                    duration_ms: started.elapsed().as_millis() as u64,
                };
            }
        };

        let mut cmd = Command::new("ssh");
        cmd.arg("-o")
            .arg("ConnectTimeout=10")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-p")
            .arg(port.to_string());

        match method {
            CloudAuthMethod::SshPrivateKey => {
                let key_path = temp_guard.path().join("id_key");
                let mut key_contents = secret_val.as_str().trim_end_matches('\r').to_string();
                if !key_contents.ends_with('\n') {
                    key_contents.push('\n');
                }
                if let Err(e) = std::fs::write(&key_path, key_contents.as_bytes()) {
                    return RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node.node_id.clone(),
                        ok: false,
                        exit_code: 1,
                        stdout: String::new(),
                        stderr: format!("Failed to write temp SSH key: {e}"),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by,
                        duration_ms: started.elapsed().as_millis() as u64,
                    };
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &key_path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
                cmd.arg("-i")
                    .arg(&key_path)
                    .arg("-o")
                    .arg("IdentitiesOnly=yes")
                    .arg("-o")
                    .arg("BatchMode=yes");
            }
            CloudAuthMethod::SshPassword => {
                let askpass_path = temp_guard.path().join("askpass.sh");
                let script = "#!/bin/sh\nprintf '%s\\n' \"$GPUI_SSH_PASS\"\n";
                if let Err(e) = std::fs::write(&askpass_path, script.as_bytes()) {
                    return RemoteComputerCommandResult {
                        command_id: command_id.to_string(),
                        node_id: node.node_id.clone(),
                        ok: false,
                        exit_code: 1,
                        stdout: String::new(),
                        stderr: format!("Failed to prepare SSH askpass helper: {e}"),
                        entries: Vec::new(),
                        frame_data_url: None,
                        executed_by,
                        duration_ms: started.elapsed().as_millis() as u64,
                    };
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &askpass_path,
                        std::fs::Permissions::from_mode(0o700),
                    );
                }
                cmd.arg("-o")
                    .arg("BatchMode=no")
                    .arg("-o")
                    .arg("PubkeyAuthentication=no")
                    .arg("-o")
                    .arg("PreferredAuthentications=password,keyboard-interactive");
                cmd.env("SSH_ASKPASS", &askpass_path);
                cmd.env("SSH_ASKPASS_REQUIRE", "force");
                cmd.env("DISPLAY", "gpui-ssh-askpass:0");
                cmd.env("GPUI_SSH_PASS", secret_val.as_str());
                cmd.stdin(std::process::Stdio::null());
            }
            CloudAuthMethod::AgentHttpToken => unreachable!(),
        }

        let remote_script = build_remote_posix_script(op, node.workdir.as_deref());
        let remote_cmd = format!("sh -lc {}", shell_quote_posix(&remote_script));
        cmd.arg(format!("{username}@{host}")).arg(remote_cmd);
        cmd.kill_on_drop(true);

        let timeout_ms = match op {
            RemoteComputerOp::ShellExec { timeout_ms, .. } => timeout_ms
                .unwrap_or(REMOTE_EXEC_TIMEOUT_MS_DEFAULT)
                .clamp(1_000, REMOTE_EXEC_TIMEOUT_MS_MAX),
            _ => REMOTE_EXEC_TIMEOUT_MS_DEFAULT,
        };

        let result = tokio::time::timeout(Duration::from_millis(timeout_ms), cmd.output()).await;
        drop(temp_guard);

        match result {
            Ok(Ok(out)) => {
                let exit_code = out.status.code().unwrap_or(if out.status.success() { 0 } else { 1 });
                let stdout = truncate_output_utf8(&out.stdout, MAX_OUTPUT_BYTES);
                let entries = if matches!(op, RemoteComputerOp::ListDir { .. }) && out.status.success() {
                    stdout
                        .lines()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                        .collect()
                } else {
                    Vec::new()
                };
                RemoteComputerCommandResult {
                    command_id: command_id.to_string(),
                    node_id: node.node_id.clone(),
                    ok: out.status.success(),
                    exit_code,
                    stdout,
                    stderr: truncate_output_utf8(&out.stderr, MAX_OUTPUT_BYTES),
                    entries,
                    frame_data_url: None,
                    executed_by,
                    duration_ms: started.elapsed().as_millis() as u64,
                }
            }
            Ok(Err(e)) => RemoteComputerCommandResult {
                command_id: command_id.to_string(),
                node_id: node.node_id.clone(),
                ok: false,
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("Failed to invoke ssh client: {e}"),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by,
                duration_ms: started.elapsed().as_millis() as u64,
            },
            Err(_) => RemoteComputerCommandResult {
                command_id: command_id.to_string(),
                node_id: node.node_id.clone(),
                ok: false,
                exit_code: 124,
                stdout: String::new(),
                stderr: format!("SSH operation timed out after {timeout_ms}ms"),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by,
                duration_ms: started.elapsed().as_millis() as u64,
            },
        }
    }
}

fn validate_target_bot_exists(state: &AppState, bot_id: u64) -> std::result::Result<UserRecord, Response> {
    if bot_id == 0 {
        return Err(err_resp(
            StatusCode::BAD_REQUEST,
            "invalid_bot_id",
            "bot_id must be a positive integer",
        ));
    }
    match state.db.get_user(UserId(bot_id)) {
        Ok(Some(u)) if u.is_bot => Ok(u),
        Ok(Some(_)) => Err(err_resp(
            StatusCode::BAD_REQUEST,
            "not_a_bot",
            "Target user_id is not a bot account",
        )),
        Ok(None) => Err(err_resp(
            StatusCode::NOT_FOUND,
            "not_found",
            "Target bot user not found",
        )),
        Err(e) => Err(map_telegram_err(e)),
    }
}

// ============================================================================
// HTTP Route Handlers
// ============================================================================

pub async fn computers_list(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let req_dev = header_device_id(&headers);
    let mut computers = match state.db.list_computer_nodes(
        auth.user_id.0,
        req_dev.as_deref(),
        now_ms,
    ) {
        Ok(list) => list,
        Err(e) => return map_telegram_err(e),
    };
    for dto in &mut computers {
        if dto.kind == ComputerNodeKind::LocalDevice && !dto.online {
            if let Some(ref did) = dto.device_id {
                if state.tailnet.is_device_derp_connected(auth.user_id.0, did) {
                    dto.online = true;
                }
            }
        }
    }
    let pending_approvals = state
        .computers
        .pending_approvals_for_device(auth.user_id.0, req_dev.as_deref());

    (
        StatusCode::OK,
        Json(ComputerListResponse {
            computers,
            pending_approvals,
        }),
    )
        .into_response()
}

pub async fn computers_local_register(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(mut req): Json<RegisterLocalComputerRequest>,
) -> Response {
    if req.device_id.trim().is_empty() {
        if let Some(hdr) = header_device_id(&headers) {
            req.device_id = hdr;
        }
    }
    let now_ms = chrono::Utc::now().timestamp_millis();
    match state
        .db
        .upsert_local_computer_node(auth.user_id.0, &req, now_ms)
    {
        Ok(dto) => (StatusCode::OK, Json(dto)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn computers_local_poll(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let Some(device_id) = header_device_id(&headers) else {
        return (StatusCode::OK, Json(Vec::<PendingComputerCommandDto>::new())).into_response();
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let _ = state
        .db
        .touch_local_device_heartbeat_by_device_id(auth.user_id.0, &device_id, now_ms);
    let pending = state
        .computers
        .poll_commands_for_device(auth.user_id.0, &device_id);
    (StatusCode::OK, Json(pending)).into_response()
}

pub async fn computers_command_approve(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(command_id): Path<String>,
    Json(body): Json<ApproveComputerCommandRequest>,
) -> Response {
    match state
        .computers
        .approve_pending_command(auth.user_id.0, &command_id, body.approved)
    {
        Ok(dto) => {
            if body.approved {
                let _ = state.tailnet.dispatch_command_to_device(
                    auth.user_id.0,
                    &dto.target_device_id,
                    dto.clone(),
                );
            }
            (StatusCode::OK, Json(dto)).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn computers_command_complete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(command_id): Path<String>,
    Json(result): Json<RemoteComputerCommandResult>,
) -> Response {
    match state
        .computers
        .complete_pending_command(auth.user_id.0, &command_id, result)
    {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn computers_cloud_upsert(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpsertCloudComputerRequest>,
) -> Response {
    if req.verify_connection {
        if let Err(err) = state
            .computers
            .verify_cloud_request(auth.user_id.0, &req)
            .await
        {
            return err_resp(
                StatusCode::BAD_GATEWAY,
                "cloud_connection_failed",
                &err.to_string(),
            );
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let req_dev = header_device_id(&headers);
    match state.db.upsert_cloud_computer_node(
        auth.user_id.0,
        &req,
        state.computers.master_key_bytes(),
        req_dev.as_deref(),
        now_ms,
    ) {
        Ok(dto) => (StatusCode::OK, Json(dto)).into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn computers_policy_patch(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
    Json(req): Json<UpdateComputerPolicyRequest>,
) -> Response {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let req_dev = header_device_id(&headers);
    match state.db.update_computer_node_policy(
        auth.user_id.0,
        &node_id,
        &req,
        req_dev.as_deref(),
        now_ms,
    ) {
        Ok(Some(dto)) => {
            state.tailnet.broadcast_netmap_for_user(auth.user_id.0);
            (StatusCode::OK, Json(dto)).into_response()
        }
        Ok(None) => err_resp(StatusCode::NOT_FOUND, "not_found", "Computer node not found"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn computers_delete(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
) -> Response {
    let now_ms = chrono::Utc::now().timestamp_millis();
    match state.db.delete_computer_node(auth.user_id.0, &node_id, now_ms) {
        Ok(true) => {
            state.tailnet.broadcast_netmap_for_user(auth.user_id.0);
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Ok(false) => err_resp(StatusCode::NOT_FOUND, "not_found", "Computer node not found"),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn computers_test(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Path(node_id): Path<String>,
) -> Response {
    let hdr_dev = header_device_id(&headers);
    let req = RemoteComputerCommandRequest {
        node_id,
        caller_device_id: hdr_dev.clone(),
        caller_bot_id: None,
        caller_label: format!("User {}", auth.user_id.0),
        operation: RemoteComputerOp::TestConnection,
    };
    match state
        .computers
        .execute_remote_command(auth.user_id.0, req, hdr_dev.as_deref())
        .await
    {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
        Err(TelegramError::PermissionDenied(reason)) => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "forbidden",
                "message": reason,
                "ok": false,
                "exit_code": 403,
                "stderr": reason,
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn computers_exec(
    auth: AuthBearer,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(req): Json<RemoteComputerCommandRequest>,
) -> Response {
    let hdr_dev = header_device_id(&headers);
    match state
        .computers
        .execute_remote_command(auth.user_id.0, req, hdr_dev.as_deref())
        .await
    {
        Ok(res) => (StatusCode::OK, Json(res)).into_response(),
        Err(TelegramError::PermissionDenied(reason)) => (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "forbidden",
                "message": reason,
                "ok": false,
                "exit_code": 403,
                "stderr": reason,
            })),
        )
            .into_response(),
        Err(e) => map_telegram_err(e),
    }
}

pub async fn bot_computer_get(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<u64>,
) -> Response {
    let bot_user = match validate_target_bot_exists(&state, bot_id) {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match state.db.get_bot_computer_profile(auth.user_id.0, bot_id) {
        Ok(Some(mut profile)) => {
            if profile.persona_prompt.trim().is_empty() || profile.preferred_model.trim().is_empty()
            {
                let (fallback_sp, fallback_model, _) =
                    load_bot_persona_config(&state, UserId(bot_id));
                if profile.persona_prompt.trim().is_empty() {
                    if let Some(sp) = fallback_sp {
                        profile.persona_prompt = sp;
                    }
                }
                if profile.preferred_model.trim().is_empty() {
                    if let Some(m) = fallback_model {
                        profile.preferred_model = m;
                    }
                }
            }
            (StatusCode::OK, Json(profile)).into_response()
        }
        Ok(None) => {
            let mut profile = state
                .db
                .get_bot_computer_profile_or_default(auth.user_id.0, bot_id)
                .unwrap_or_default();
            let (fallback_sp, fallback_model, _) = load_bot_persona_config(&state, UserId(bot_id));
            if let Some(sp) = fallback_sp {
                profile.persona_prompt = sp;
            }
            if let Some(m) = fallback_model {
                profile.preferred_model = m;
            }
            let _ = bot_user;
            (StatusCode::OK, Json(profile)).into_response()
        }
        Err(e) => map_telegram_err(e),
    }
}

pub async fn bot_computer_put(
    auth: AuthBearer,
    State(state): State<Arc<AppState>>,
    Path(bot_id): Path<u64>,
    Json(req): Json<UpdateBotComputerRequest>,
) -> Response {
    let bot_user = match validate_target_bot_exists(&state, bot_id) {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let saved = match state
        .db
        .upsert_bot_computer_profile(auth.user_id.0, bot_id, &req, now_ms)
    {
        Ok(p) => p,
        Err(e) => return map_telegram_err(e),
    };

    if req.persona_prompt.is_some() || req.preferred_model.is_some() {
        let (_, existing_model, existing_welcome) =
            load_bot_persona_config(&state, UserId(bot_id));
        let model_opt = if !saved.preferred_model.trim().is_empty() {
            Some(saved.preferred_model.as_str())
        } else {
            existing_model.as_deref()
        };
        let persona_opt = if req.persona_prompt.is_some() {
            Some(saved.persona_prompt.as_str())
        } else {
            None
        };
        if let Some(p) = persona_opt {
            let _ = state.db.sync_bot_ai_persona(bot_id, p, now_ms);
            let bot_key = bot_id.to_string();
            if let Ok(Some(_existing_soul)) = state.hermes.store().kv_get("bot_soul", &bot_key) {
                let soul_payload = json!({
                    "bot_id": bot_id,
                    "system_prompt": p,
                    "content": p,
                    "updated_at_ms": now_ms,
                });
                if let Ok(raw) = serde_json::to_string(&soul_payload) {
                    let _ = state.hermes.store().kv_set("bot_soul", &bot_key, &raw);
                }
            }
        }
        let display_name = if !bot_user.first_name.trim().is_empty() {
            bot_user.first_name.clone()
        } else {
            bot_user
                .username
                .clone()
                .unwrap_or_else(|| format!("Bot {bot_id}"))
        };
        save_bot_persona_kv(
            &state,
            UserId(bot_id),
            None,
            &display_name,
            persona_opt,
            persona_opt,
            model_opt,
            existing_welcome.as_deref(),
        );
    }

    (StatusCode::OK, Json(saved)).into_response()
}

pub async fn computer_agent_exec(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AgentExecWireRequest>,
) -> Response {
    let Some(expected_token) = state.computers.effective_agent_exec_token() else {
        return err_resp(
            StatusCode::UNAUTHORIZED,
            "agent_token_unconfigured",
            "Computer HTTP agent execution token is not configured",
        );
    };

    let provided_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");

    let matches = !provided_token.is_empty()
        && provided_token.as_bytes().len() == expected_token.as_bytes().len()
        && provided_token
            .as_bytes()
            .ct_eq(expected_token.as_bytes())
            .into();

    if !matches {
        return err_resp(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Invalid or missing Bearer token for computer-agent/exec",
        );
    }

    let (op, workdir, node_id, command_id) = match payload {
        AgentExecWireRequest::Wrapped {
            operation,
            workdir,
            node_id,
            command_id,
        } => (
            operation,
            workdir.filter(|s| !s.trim().is_empty()).map(PathBuf::from),
            node_id.unwrap_or_else(|| "cloud:agent".to_string()),
            command_id.unwrap_or_else(|| format!("agent:{}", uuid::Uuid::new_v4())),
        ),
        AgentExecWireRequest::Direct(operation) => (
            operation,
            None,
            "cloud:agent".to_string(),
            format!("agent:{}", uuid::Uuid::new_v4()),
        ),
    };

    let res = execute_local_fs_and_shell_op(
        &node_id,
        &command_id,
        &op,
        workdir.as_deref(),
        false,
        "computer_agent_http",
    )
    .await;

    (StatusCode::OK, Json(res)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session_manager::hash_password;
    use crate::config::ServerConfig;
    use crate::server::{build_router_with_state, AppState};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use gpui_telegram_proto::computer_contracts::AuthorizedControllerEntry;
    use serde_json::Value;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    fn make_test_state() -> (Arc<AppState>, TempDirGuard) {
        let media_guard = TempDirGuard::create("gpui-test-media").expect("media temp");
        let mut cfg = ServerConfig::default();
        cfg.database_url = ":memory:".to_string();
        cfg.media_storage_path = media_guard.path().to_string_lossy().to_string();
        let db = Arc::new(ServerDatabase::open_in_memory().expect("db"));
        let state = Arc::new(AppState::with_db(cfg, db));
        (state, media_guard)
    }

    fn seed_user_and_token(
        state: &AppState,
        user_id: u64,
        is_bot: bool,
        device_id: &str,
    ) -> String {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let rec = UserRecord {
            user_id: UserId(user_id),
            username: Some(format!("user_{user_id}")),
            phone: Some(format!("+1555000{user_id:04}")),
            first_name: format!("User {user_id}"),
            last_name: None,
            bio: None,
            password_hash: Some(hash_password("pw123").expect("hash")),
            is_bot,
            is_verified: false,
            is_support: false,
            is_scam: false,
            is_fake: false,
            profile_photo_file_id: None,
            account_generation: 1,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        state.db.create_user(&rec).expect("create user");
        let (_sess, raw_token) = state.session_manager.create_session(
            UserId(user_id),
            device_id,
            chrono::Duration::days(7),
        );
        raw_token
    }

    async fn json_request(
        app: &axum::Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        device_id: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(d) = device_id {
            builder = builder.header("X-Device-Id", d);
        }
        let body_bytes = if let Some(b) = body {
            builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
            Body::from(b.to_string())
        } else {
            Body::empty()
        };
        let resp = app
            .clone()
            .oneshot(builder.body(body_bytes).expect("request"))
            .await
            .expect("oneshot");
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 32 * 1024 * 1024)
            .await
            .expect("bytes");
        let val = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        (status, val)
    }

    #[tokio::test]
    async fn test_local_device_relay_approval_and_acl_policies() {
        let (state, _media_guard) = make_test_state();
        let token = seed_user_and_token(&state, 1001, false, "mac-host");
        let app = build_router_with_state(state.clone());

        // 1. Register local computer and check list + current device + virtual free sandbox.
        let (st, reg_val) = json_request(
            &app,
            "POST",
            "/api/v1/computers/local/register",
            Some(&token),
            Some("mac-host"),
            Some(json!(RegisterLocalComputerRequest {
                device_id: "mac-host".to_string(),
                display_name: "Mac Studio".to_string(),
                ip_address: "10.0.0.12".to_string(),
                os_name: "macOS".to_string(),
                resolution: "2560x1440".to_string(),
            })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let reg_dto: ComputerNodeDto = serde_json::from_value(reg_val).expect("dto");
        assert!(reg_dto.is_current_device);
        assert!(reg_dto.online);
        assert_eq!(reg_dto.run_policy, ComputerRunPolicy::AskEveryTime);

        let (st, list_val) = json_request(
            &app,
            "GET",
            "/api/v1/computers",
            Some(&token),
            Some("mac-host"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let list: ComputerListResponse = serde_json::from_value(list_val).expect("list");
        assert!(list.computers.iter().any(|c| c.node_id == reg_dto.node_id && c.is_current_device));
        assert!(list.computers.iter().any(|c| c.node_id == DEFAULT_SANDBOX_NODE_ID && c.tier_badge == "免费"));

        // 2. AskEveryTime cross-device command waits for approval, then poll & complete resolves caller.
        let exec_app = app.clone();
        let exec_token = token.clone();
        let exec_node_id = reg_dto.node_id.clone();
        let exec_task = tokio::spawn(async move {
            json_request(
                &exec_app,
                "POST",
                "/api/v1/computers/exec",
                Some(&exec_token),
                Some("iphone-caller"),
                Some(json!(RemoteComputerCommandRequest {
                    node_id: exec_node_id,
                    caller_device_id: Some("iphone-caller".to_string()),
                    caller_bot_id: None,
                    caller_label: "iPhone".to_string(),
                    operation: RemoteComputerOp::ShellExec {
                        command: "echo ok".to_string(),
                        cwd: None,
                        timeout_ms: None,
                    },
                })),
            )
            .await
        });

        let mut polled_cmd_id = String::new();
        for _ in 0..30 {
            let (pst, pval) = json_request(
                &app,
                "GET",
                "/api/v1/computers/local/poll",
                Some(&token),
                Some("mac-host"),
                None,
            )
            .await;
            assert_eq!(pst, StatusCode::OK);
            let pending: Vec<PendingComputerCommandDto> = serde_json::from_value(pval).expect("poll");
            if let Some(first) = pending.first() {
                assert!(first.requires_user_approval);
                assert!(!first.approved);
                polled_cmd_id = first.command_id.clone();
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!polled_cmd_id.is_empty());

        let (ast, aval) = json_request(
            &app,
            "POST",
            &format!("/api/v1/computers/commands/{polled_cmd_id}/approve"),
            Some(&token),
            Some("mac-host"),
            Some(json!(ApproveComputerCommandRequest { approved: true })),
        )
        .await;
        assert_eq!(ast, StatusCode::OK);
        let approved_dto: PendingComputerCommandDto = serde_json::from_value(aval).expect("approve");
        assert!(approved_dto.approved);

        let (cst, _) = json_request(
            &app,
            "POST",
            &format!("/api/v1/computers/commands/{polled_cmd_id}/complete"),
            Some(&token),
            Some("mac-host"),
            Some(json!(RemoteComputerCommandResult {
                command_id: polled_cmd_id.clone(),
                node_id: reg_dto.node_id.clone(),
                ok: true,
                exit_code: 0,
                stdout: "ok\n".to_string(),
                stderr: String::new(),
                entries: Vec::new(),
                frame_data_url: None,
                executed_by: "mac-host".to_string(),
                duration_ms: 5,
            })),
        )
        .await;
        assert_eq!(cst, StatusCode::OK);

        let (exec_st, exec_val) = exec_task.await.expect("join");
        assert_eq!(exec_st, StatusCode::OK);
        let exec_res: RemoteComputerCommandResult = serde_json::from_value(exec_val).expect("res");
        assert!(exec_res.ok);
        assert_eq!(exec_res.stdout, "ok\n");

        // 3. Patch policy to Disabled -> immediate 403 Forbidden.
        let (pst, _) = json_request(
            &app,
            "PATCH",
            &format!("/api/v1/computers/{}/policy", reg_dto.node_id),
            Some(&token),
            Some("mac-host"),
            Some(json!(UpdateComputerPolicyRequest {
                run_policy: Some(ComputerRunPolicy::Disabled),
                ..Default::default()
            })),
        )
        .await;
        assert_eq!(pst, StatusCode::OK);

        let (dst, _) = json_request(
            &app,
            "POST",
            "/api/v1/computers/exec",
            Some(&token),
            Some("mac-host"),
            Some(json!(RemoteComputerCommandRequest {
                node_id: reg_dto.node_id.clone(),
                caller_device_id: Some("mac-host".to_string()),
                caller_bot_id: None,
                caller_label: "Mac".to_string(),
                operation: RemoteComputerOp::TestConnection,
            })),
        )
        .await;
        assert_eq!(dst, StatusCode::FORBIDDEN);

        // 4. AlwaysAllowAuthorized ACL enforcement for bots.
        let _ = json_request(
            &app,
            "PATCH",
            &format!("/api/v1/computers/{}/policy", reg_dto.node_id),
            Some(&token),
            Some("mac-host"),
            Some(json!(UpdateComputerPolicyRequest {
                run_policy: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                add_controller: Some(AuthorizedControllerEntry {
                    controller_id: "bot:500".to_string(),
                    kind: AuthorizedControllerKind::Bot,
                    display_name: "AllowedBot".to_string(),
                    added_at_ms: 1,
                }),
                ..Default::default()
            })),
        )
        .await;

        let (unauth_st, _) = json_request(
            &app,
            "POST",
            "/api/v1/computers/exec",
            Some(&token),
            None,
            Some(json!(RemoteComputerCommandRequest {
                node_id: reg_dto.node_id.clone(),
                caller_device_id: None,
                caller_bot_id: Some(999),
                caller_label: "OtherBot".to_string(),
                operation: RemoteComputerOp::TestConnection,
            })),
        )
        .await;
        assert_eq!(unauth_st, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_sandbox_execution_and_path_containment() {
        let (state, _media_guard) = make_test_state();
        let token = seed_user_and_token(&state, 2002, false, "dev-2");
        let app = build_router_with_state(state.clone());
        let sandbox_dir = std::env::temp_dir().join("gpui_sandbox_2002");

        // Traversal outside sandbox is blocked cleanly.
        let (st, val) = json_request(
            &app,
            "POST",
            "/api/v1/computers/exec",
            Some(&token),
            Some("dev-2"),
            Some(json!(RemoteComputerCommandRequest {
                node_id: DEFAULT_SANDBOX_NODE_ID.to_string(),
                caller_device_id: Some("dev-2".to_string()),
                caller_bot_id: None,
                caller_label: "User".to_string(),
                operation: RemoteComputerOp::ReadFile {
                    path: "../../etc/passwd".to_string(),
                    max_bytes: Some(128),
                },
            })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let blocked: RemoteComputerCommandResult = serde_json::from_value(val).expect("res");
        assert!(!blocked.ok);
        assert_eq!(blocked.exit_code, 403);

        // Write, read, list, shell_exec, capture_screen in sandbox succeed.
        let (_, wval) = json_request(
            &app,
            "POST",
            "/api/v1/computers/exec",
            Some(&token),
            Some("dev-2"),
            Some(json!(RemoteComputerCommandRequest {
                node_id: DEFAULT_SANDBOX_NODE_ID.to_string(),
                caller_device_id: Some("dev-2".to_string()),
                caller_bot_id: None,
                caller_label: "User".to_string(),
                operation: RemoteComputerOp::WriteFile {
                    path: "notes/hello.txt".to_string(),
                    content: "sandbox ok".to_string(),
                    create_dirs: true,
                },
            })),
        )
        .await;
        assert!(serde_json::from_value::<RemoteComputerCommandResult>(wval).unwrap().ok);

        let (_, rval) = json_request(
            &app,
            "POST",
            "/api/v1/computers/exec",
            Some(&token),
            Some("dev-2"),
            Some(json!(RemoteComputerCommandRequest {
                node_id: DEFAULT_SANDBOX_NODE_ID.to_string(),
                caller_device_id: Some("dev-2".to_string()),
                caller_bot_id: None,
                caller_label: "User".to_string(),
                operation: RemoteComputerOp::ReadFile {
                    path: "notes/hello.txt".to_string(),
                    max_bytes: None,
                },
            })),
        )
        .await;
        let read_res: RemoteComputerCommandResult = serde_json::from_value(rval).unwrap();
        assert!(read_res.ok);
        assert_eq!(read_res.stdout, "sandbox ok");

        let (_, sval) = json_request(
            &app,
            "POST",
            "/api/v1/computers/exec",
            Some(&token),
            Some("dev-2"),
            Some(json!(RemoteComputerCommandRequest {
                node_id: DEFAULT_SANDBOX_NODE_ID.to_string(),
                caller_device_id: Some("dev-2".to_string()),
                caller_bot_id: None,
                caller_label: "User".to_string(),
                operation: RemoteComputerOp::CaptureScreen,
            })),
        )
        .await;
        let screen_res: RemoteComputerCommandResult = serde_json::from_value(sval).unwrap();
        assert!(screen_res.ok);
        assert!(screen_res
            .frame_data_url
            .as_deref()
            .is_some_and(|u| u.starts_with("data:image/png;base64,")));

        let _ = std::fs::remove_dir_all(&sandbox_dir);
    }

    #[tokio::test]
    async fn test_cloud_agent_http_dispatch_and_bot_profile_binding() {
        let (state, _media_guard) = make_test_state();
        state
            .computers
            .set_agent_exec_token(Some("agent-secret-token-xyz".to_string()));
        let token = seed_user_and_token(&state, 3003, false, "dev-3");
        let _bot = seed_user_and_token(&state, 8801, true, "bot-dev");
        let app = build_router_with_state(state.clone());

        // Verify /api/v1/computer-agent/exec rejects unauthenticated callers.
        let (unauth_st, _) = json_request(
            &app,
            "POST",
            "/api/v1/computer-agent/exec",
            Some("wrong-token"),
            None,
            Some(json!(RemoteComputerOp::TestConnection)),
        )
        .await;
        assert_eq!(unauth_st, StatusCode::UNAUTHORIZED);

        // Bind TCP listener serving the real router for CloudServer AgentHttpToken verification.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let srv_app = app.clone();
        let srv_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, srv_app).await;
        });

        let workdir_guard = TempDirGuard::create("gpui-cloud-agent-test").expect("temp");
        let workdir_str = workdir_guard.path().to_string_lossy().to_string();

        let (cst, cval) = json_request(
            &app,
            "POST",
            "/api/v1/computers/cloud",
            Some(&token),
            Some("dev-3"),
            Some(json!(UpsertCloudComputerRequest {
                node_id: None,
                display_name: "HTTP Cloud Agent".to_string(),
                host: "127.0.0.1".to_string(),
                port: addr.port(),
                username: "root".to_string(),
                auth_method: CloudAuthMethod::AgentHttpToken,
                secret: Some("agent-secret-token-xyz".to_string()),
                agent_endpoint_url: Some(format!("http://{addr}/api/v1/computer-agent/exec")),
                workdir: Some(workdir_str),
                bind_bot_id: Some(8801),
                verify_connection: true,
            })),
        )
        .await;
        assert_eq!(cst, StatusCode::OK, "cloud upsert failed: {cval}");
        let cloud_dto: ComputerNodeDto = serde_json::from_value(cval).expect("cloud dto");
        assert!(cloud_dto.has_saved_credential);
        assert_eq!(cloud_dto.bound_bot_ids, vec![8801]);

        // Bot executes command via ComputerCloudProvider without specifying node_id (uses bound node).
        let exec_out = state
            .computers
            .execute_on_computer(
                3003,
                AgentComputerExecInput {
                    node_id: None,
                    caller_bot_id: Some(8801),
                    op: "shell_exec".to_string(),
                    command: Some("printf 'from-cloud-agent'".to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("exec");
        assert!(exec_out.ok);
        assert_eq!(exec_out.node_id, cloud_dto.node_id);
        assert_eq!(exec_out.stdout, "from-cloud-agent");

        // Bot computer PUT updates persona + preferred_model and syncs into load_bot_persona_config.
        let (bst, bval) = json_request(
            &app,
            "PUT",
            "/api/v1/bots/8801/computer",
            Some(&token),
            None,
            Some(json!(UpdateBotComputerRequest {
                bound_node_id: Some(cloud_dto.node_id.clone()),
                persona_prompt: Some("You manage cloud Linux hosts safely.".to_string()),
                preferred_model: Some("gpt-5-mini".to_string()),
                ..Default::default()
            })),
        )
        .await;
        assert_eq!(bst, StatusCode::OK);
        let profile: BotComputerProfileDto = serde_json::from_value(bval).expect("profile");
        assert_eq!(profile.preferred_model, "gpt-5-mini");

        let (loaded_sp, loaded_model, _) = load_bot_persona_config(&state, UserId(8801));
        assert_eq!(loaded_sp.as_deref(), Some("You manage cloud Linux hosts safely."));
        assert_eq!(loaded_model.as_deref(), Some("gpt-5-mini"));

        let (get_bst, get_bval) = json_request(
            &app,
            "GET",
            "/api/v1/bots/8801/computer",
            Some(&token),
            None,
            None,
        )
        .await;
        assert_eq!(get_bst, StatusCode::OK);
        let fetched_profile: BotComputerProfileDto =
            serde_json::from_value(get_bval).expect("get bot computer");
        assert_eq!(fetched_profile.bound_node_id.as_deref(), Some(cloud_dto.node_id.as_str()));

        let (test_st, test_val) = json_request(
            &app,
            "POST",
            &format!("/api/v1/computers/{}/test", cloud_dto.node_id),
            Some(&token),
            Some("dev-3"),
            Some(json!({})),
        )
        .await;
        assert_eq!(test_st, StatusCode::OK);
        assert!(serde_json::from_value::<RemoteComputerCommandResult>(test_val).unwrap().ok);

        let (ctx_prompt, ctx_model) =
            enrich_bot_turn_prompt_and_model(&state, 3003, Some(8801), None, None, None);
        let ctx_text = ctx_prompt.expect("context");
        assert!(ctx_text.contains("[Computer & Cloud Server Context]"));
        assert!(ctx_text.contains(&cloud_dto.node_id));
        assert!(!ctx_text.contains("agent-secret-token-xyz"));
        assert_eq!(ctx_model.as_deref(), Some("gpt-5-mini"));

        // Verify /api/v1/hermes/turns injects ComputerCloudProvider + ToolsetPreset::HermesTelegram.
        let (ht_st, ht_val) = json_request(
            &app,
            "POST",
            "/api/v1/hermes/turns",
            Some(&token),
            None,
            Some(json!({
                "bot_id": "8801",
                "prompt": "列出我有哪些电脑和云服务器"
            })),
        )
        .await;
        assert_eq!(ht_st, StatusCode::OK, "hermes turn failed: {ht_val}");
        let ht_text = ht_val.to_string();
        assert!(
            ht_text.contains(&cloud_dto.node_id) || ht_text.contains("HTTP Cloud Agent"),
            "expected computer_list result in turn output: {ht_val}"
        );

        let (del_st, _) = json_request(
            &app,
            "DELETE",
            &format!("/api/v1/computers/{}", cloud_dto.node_id),
            Some(&token),
            None,
            None,
        )
        .await;
        assert_eq!(del_st, StatusCode::OK);

        let (get_after_del_st, get_after_del_val) = json_request(
            &app,
            "GET",
            "/api/v1/bots/8801/computer",
            Some(&token),
            None,
            None,
        )
        .await;
        assert_eq!(get_after_del_st, StatusCode::OK);
        let cleared_profile: BotComputerProfileDto =
            serde_json::from_value(get_after_del_val).unwrap();
        assert_eq!(cleared_profile.bound_node_id, None);

        // Verify ComputerCloudProvider::configure_cloud_server verifies connection BEFORE persisting.
        let bad_secret = "bad-unverified-token-999";
        let bad_cfg_err = state
            .computers
            .configure_cloud_server(
                3003,
                Some(8801),
                AgentCloudServerConfigInput {
                    node_id: None,
                    display_name: "Unverified Cloud".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: addr.port(),
                    username: "root".to_string(),
                    auth_method: "agent_http_token".to_string(),
                    secret: bad_secret.to_string(),
                    agent_endpoint_url: Some(format!("http://{addr}/api/v1/computer-agent/exec")),
                    workdir: None,
                    bind_to_bot: true,
                },
            )
            .await
            .expect_err("wrong agent token must fail verification");
        assert!(!bad_cfg_err.to_string().contains(bad_secret));
        let nodes_after_fail = state
            .db
            .list_computer_nodes(3003, None, chrono::Utc::now().timestamp_millis())
            .expect("list");
        assert!(!nodes_after_fail.iter().any(|n| n.display_name == "Unverified Cloud"));

        let verified_summary = state
            .computers
            .configure_cloud_server(
                3003,
                Some(8801),
                AgentCloudServerConfigInput {
                    node_id: None,
                    display_name: "AI Verified Cloud".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: addr.port(),
                    username: "root".to_string(),
                    auth_method: "agent_http_token".to_string(),
                    secret: "agent-secret-token-xyz".to_string(),
                    agent_endpoint_url: Some(format!("http://{addr}/api/v1/computer-agent/exec")),
                    workdir: None,
                    bind_to_bot: true,
                },
            )
            .await
            .expect("verified cloud config succeeds");
        assert!(verified_summary.bound_to_this_bot);
        assert!(verified_summary.online);

        srv_handle.abort();
    }

    #[tokio::test]
    async fn test_complete_command_supports_large_screenshot_payload_over_3mb() {
        let (state, _media_guard) = make_test_state();
        let token = seed_user_and_token(&state, 4004, false, "retina-mac");
        let app = build_router_with_state(state.clone());

        let (st, reg_val) = json_request(
            &app,
            "POST",
            "/api/v1/computers/local/register",
            Some(&token),
            Some("retina-mac"),
            Some(json!(RegisterLocalComputerRequest {
                device_id: "retina-mac".to_string(),
                display_name: "Retina Mac".to_string(),
                ip_address: "10.0.0.44".to_string(),
                os_name: "macOS".to_string(),
                resolution: "3456x2234".to_string(),
            })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let node: ComputerNodeDto = serde_json::from_value(reg_val).expect("dto");

        let _ = json_request(
            &app,
            "PATCH",
            &format!("/api/v1/computers/{}/policy", node.node_id),
            Some(&token),
            Some("retina-mac"),
            Some(json!(UpdateComputerPolicyRequest {
                run_policy: Some(ComputerRunPolicy::AlwaysAllowAuthorized),
                ..Default::default()
            })),
        )
        .await;

        let exec_app = app.clone();
        let exec_token = token.clone();
        let exec_node_id = node.node_id.clone();
        let exec_task = tokio::spawn(async move {
            json_request(
                &exec_app,
                "POST",
                "/api/v1/computers/exec",
                Some(&exec_token),
                Some("caller-dev"),
                Some(json!(RemoteComputerCommandRequest {
                    node_id: exec_node_id,
                    caller_device_id: Some("caller-dev".to_string()),
                    caller_bot_id: None,
                    caller_label: "Remote Caller".to_string(),
                    operation: RemoteComputerOp::CaptureScreen,
                })),
            )
            .await
        });

        let mut cmd_id = String::new();
        for _ in 0..30 {
            let (pst, pval) = json_request(
                &app,
                "GET",
                "/api/v1/computers/local/poll",
                Some(&token),
                Some("retina-mac"),
                None,
            )
            .await;
            assert_eq!(pst, StatusCode::OK);
            let pending: Vec<PendingComputerCommandDto> = serde_json::from_value(pval).unwrap();
            if let Some(first) = pending.first() {
                cmd_id = first.command_id.clone();
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!cmd_id.is_empty());

        // Exceeds Axum's default 2 MiB limit (~3.2 MiB base64 payload).
        let large_frame = format!("data:image/png;base64,{}", "A".repeat(3_300_000));
        let (cst, _) = json_request(
            &app,
            "POST",
            &format!("/api/v1/computers/commands/{cmd_id}/complete"),
            Some(&token),
            Some("retina-mac"),
            Some(json!(RemoteComputerCommandResult {
                command_id: cmd_id.clone(),
                node_id: node.node_id.clone(),
                ok: true,
                exit_code: 0,
                stdout: "captured".to_string(),
                stderr: String::new(),
                entries: Vec::new(),
                frame_data_url: Some(large_frame.clone()),
                executed_by: "retina-mac".to_string(),
                duration_ms: 42,
            })),
        )
        .await;
        assert_eq!(cst, StatusCode::OK);

        let (exec_st, exec_val) = exec_task.await.expect("join");
        assert_eq!(exec_st, StatusCode::OK);
        let exec_res: RemoteComputerCommandResult = serde_json::from_value(exec_val).unwrap();
        assert!(exec_res.ok);
        assert_eq!(exec_res.frame_data_url.as_deref(), Some(large_frame.as_str()));
    }
}
