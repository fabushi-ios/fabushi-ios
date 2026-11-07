use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, AtomicU64},
};
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::local_exec_bridge::{
    LocalExecCancellationHandle, LocalExecComputer, SAND_NO_LOCAL_MACHINE_MESSAGE,
    SandLocalExecBridge,
};
use super::local_exec_error::SandLocalExecError;
use super::local_exec_failure_classifier::{
    LocalExecFailureClass, classify_local_exec_failure,
};
use super::production::{PRODUCTION_LOCAL_EXEC_CODEC, RemoteResourceAccessor};
use crate::r#box::box_transfer::TransferBox;
use crate::extensions::local_tool_permission::extension::HostLocalToolPermissionExtension;
use crate::extensions::local_tool_permission::local_tool_permission_controller::{
    SandLocalToolRequest, SandLocalToolScope,
};
use crate::extensions::process::command_custody::CommandProcessCustody;

pub const FALLBACK_TERMINALS_FOLDER: &str = "terminals";
pub const DEFAULT_MAX_LOCAL_EXEC_FILE_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_LOCAL_EXEC_SHELL_STREAM_BYTES: usize = 256 * 1024;
pub const MAX_LOCAL_EXEC_DURABLE_OUTPUT_BYTES: usize = 64 * 1024;
pub const DEFAULT_LOCAL_EXEC_OPERATION_HISTORY_LIMIT: usize = 128;
pub const LOCAL_EXEC_OPERATION_JOURNAL_VERSION: u32 = 2;
pub(crate) static LOCAL_EXEC_OPERATION_JOURNAL_TMP_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn describe_local_exec_bytes(bytes: usize) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

pub fn local_exec_file_too_large_message(actual_bytes: usize, max_bytes: usize) -> String {
    format!(
        "File is {}, which exceeds Fabushi's {} limit for reading or transferring a single file over local-exec. Read a slice with offset/limit, or use a shell command (grep, head, tail) to extract just what you need.",
        describe_local_exec_bytes(actual_bytes),
        describe_local_exec_bytes(max_bytes),
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayLocalToolScope {
    pub agent_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub action: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayLocalExecFailureReport {
    pub error_class: LocalExecFailureClass,
    pub errno: Option<String>,
    pub site: String,
    pub conversation_id: Option<String>,
}

pub type GatewayLocalExecFailureReporter = Arc<dyn Fn(GatewayLocalExecFailureReport) + Send + Sync>;

pub trait GatewayLocalToolGate: Send + Sync {
    fn blocked_reason(&self) -> Option<String>;
    fn requires_approval(&self) -> bool;
    fn authorize(
        &self,
        scope: &GatewayLocalToolScope,
        action: &str,
        target: &str,
    ) -> Result<Option<String>, SandLocalExecError>;
}

fn authorize_host_local_tool(
    permission: &HostLocalToolPermissionExtension,
    scope: &GatewayLocalToolScope,
    action: &str,
    target: &str,
    direction_epoch: Option<u64>,
) -> Result<Option<String>, SandLocalExecError> {
    let sand_scope = scope.agent_id.as_ref().map(|agent_id| SandLocalToolScope {
        agent_id: agent_id.clone(),
        session_key: None,
        tool_call_id: scope.tool_call_id.clone(),
        action: scope.action.clone(),
        direction_epoch,
    });
    let decision = HostLocalToolPermissionExtension::authorize(
        permission,
        sand_scope.as_ref(),
        &SandLocalToolRequest::simple(action, target),
    );
    if decision.allowed {
        Ok(decision.approval_id)
    } else {
        Err(SandLocalExecError::new(decision.reason.unwrap_or_else(
            || "Local tool permission denied".to_string(),
        )))
    }
}

impl GatewayLocalToolGate for HostLocalToolPermissionExtension {
    fn blocked_reason(&self) -> Option<String> {
        HostLocalToolPermissionExtension::blocked_reason(self)
    }

    fn requires_approval(&self) -> bool {
        HostLocalToolPermissionExtension::requires_approval(self)
    }

    fn authorize(
        &self,
        scope: &GatewayLocalToolScope,
        action: &str,
        target: &str,
    ) -> Result<Option<String>, SandLocalExecError> {
        authorize_host_local_tool(self, scope, action, target, None)
    }
}

#[derive(Clone)]
pub struct TurnScopedGatewayLocalToolGate {
    permission: Arc<HostLocalToolPermissionExtension>,
    direction_epoch: u64,
}

impl TurnScopedGatewayLocalToolGate {
    pub fn new(permission: Arc<HostLocalToolPermissionExtension>, direction_epoch: u64) -> Self {
        Self {
            permission,
            direction_epoch,
        }
    }
}

impl GatewayLocalToolGate for TurnScopedGatewayLocalToolGate {
    fn blocked_reason(&self) -> Option<String> {
        self.permission.blocked_reason()
    }

    fn requires_approval(&self) -> bool {
        self.permission.requires_approval()
    }

    fn authorize(
        &self,
        scope: &GatewayLocalToolScope,
        action: &str,
        target: &str,
    ) -> Result<Option<String>, SandLocalExecError> {
        authorize_host_local_tool(
            self.permission.as_ref(),
            scope,
            action,
            target,
            Some(self.direction_epoch),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayLocalExecReady {
    pub vnc_url: String,
    pub terminals_folder: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayLocalExecShellResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<u32>,
    pub cwd: Option<String>,
    pub aborted: bool,
    pub request_id: String,
    pub outcome_unknown: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayLocalExecOperationStatus {
    Accepted,
    Running,
    Completed,
    Cancelled,
    OutcomeUnknown,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayLocalExecOperationRecord {
    pub operation_id: String,
    pub agent_id: Option<String>,
    pub bridge_request_id: String,
    pub status: GatewayLocalExecOperationStatus,
    pub exit_code: Option<u32>,
    pub aborted: bool,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
    #[serde(default)]
    pub settled_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GatewayLocalExecOperationJournal {
    pub(crate) version: u32,
    pub(crate) records: Vec<GatewayLocalExecOperationRecord>,
}

#[derive(Clone)]
pub(crate) struct GatewayLocalExecActiveOperation {
    pub(crate) agent_id: Option<String>,
    pub(crate) bridge_request_id: String,
    pub(crate) cancellation: LocalExecCancellationHandle,
    pub(crate) started: bool,
    pub(crate) cancel_requested: bool,
}

#[derive(Clone)]
pub struct GatewayLocalExecManager {
    pub(crate) sandbox: GatewayLocalExecSandBox,
    pub(crate) process_custody: Arc<CommandProcessCustody>,
    pub(crate) next_exec_id: Arc<AtomicU32>,
    pub(crate) active_operations: Arc<Mutex<HashMap<String, GatewayLocalExecActiveOperation>>>,
    pub(crate) operation_history: Arc<Mutex<VecDeque<GatewayLocalExecOperationRecord>>>,
    pub(crate) operation_history_limit: usize,
    pub(crate) operation_journal_path: Arc<PathBuf>,
    pub(crate) operation_journal_error: Arc<Mutex<Option<String>>>,
}

#[derive(Clone)]
pub struct GatewayLocalExecSandBox {
    pub(crate) bridge: SandLocalExecBridge,
    pub(crate) gate: Arc<dyn GatewayLocalToolGate>,
    pub(crate) process_custody: Arc<CommandProcessCustody>,
    pub(crate) computer_id: Option<String>,
    pub(crate) max_file_bytes: usize,
    pub(crate) failure_reporter: Option<GatewayLocalExecFailureReporter>,
}

impl GatewayLocalExecSandBox {
    pub fn new(bridge: SandLocalExecBridge, gate: Arc<dyn GatewayLocalToolGate>) -> Self {
        Self {
            bridge,
            gate,
            process_custody: Arc::new(CommandProcessCustody::new()),
            computer_id: None,
            max_file_bytes: DEFAULT_MAX_LOCAL_EXEC_FILE_BYTES,
            failure_reporter: None,
        }
    }

    pub fn for_computer(mut self, computer_id: impl Into<String>) -> Self {
        self.computer_id = Some(computer_id.into());
        self
    }

    pub fn with_max_file_bytes(mut self, max_file_bytes: usize) -> Self {
        self.max_file_bytes = max_file_bytes;
        self
    }

    pub fn with_failure_reporter(
        mut self,
        failure_reporter: GatewayLocalExecFailureReporter,
    ) -> Self {
        self.failure_reporter = Some(failure_reporter);
        self
    }

    pub fn with_process_custody(mut self, process_custody: Arc<CommandProcessCustody>) -> Self {
        self.process_custody = process_custody;
        self
    }

    pub fn terminals_folder(&self) -> String {
        self.bridge
            .get_provider_info()
            .map(|info| info.terminals_folder)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| FALLBACK_TERMINALS_FOLDER.to_string())
    }

    pub fn ensure_ready(&self) -> GatewayLocalExecReady {
        GatewayLocalExecReady {
            vnc_url: String::new(),
            terminals_folder: self.terminals_folder(),
        }
    }

    pub fn exec_manager(&self) -> GatewayLocalExecManager {
        GatewayLocalExecManager::new(self.clone())
    }

    pub fn remote_resource_accessor(&self) -> RemoteResourceAccessor<GatewayLocalExecManager> {
        PRODUCTION_LOCAL_EXEC_CODEC.create_remote_accessor(Arc::new(self.exec_manager()))
    }

    pub fn run_state(&self) -> &'static str {
        if self.bridge.has_provider() {
            "running"
        } else {
            "absent"
        }
    }

    pub fn list_boxes(&self) -> Vec<Value> {
        Vec::new()
    }

    pub fn upload_file(
        &self,
        agent_id: Option<&str>,
        box_path: &str,
        data: &[u8],
    ) -> Result<(), SandLocalExecError> {
        self.upload_file_with_scope(
            &GatewayLocalToolScope {
                agent_id: agent_id.map(str::to_string),
                ..GatewayLocalToolScope::default()
            },
            box_path,
            data,
        )
    }

    pub fn upload_file_with_scope(
        &self,
        scope: &GatewayLocalToolScope,
        box_path: &str,
        data: &[u8],
    ) -> Result<(), SandLocalExecError> {
        const CHUNK_BYTES: usize = 256 * 1024;
        if data.len() > self.max_file_bytes {
            return Err(SandLocalExecError::new(local_exec_file_too_large_message(
                data.len(),
                self.max_file_bytes,
            )));
        }
        self.check_blocked()?;
        self.bridge.assert_computer_available(
            self.computer_id.as_deref(),
            "upload",
            scope.agent_id.as_deref(),
        )?;
        let approval_id = self.gate.authorize(scope, "write-file", box_path)?;
        let transfer_id = Uuid::new_v4().to_string();
        let digest = Sha256::digest(data);
        let sha256 = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();

        let send_frame = |mut frame: Value| -> Result<(), SandLocalExecError> {
            if let Some(approval_id) = approval_id.as_ref() {
                frame["approvalId"] = Value::String(approval_id.clone());
            }
            let mut request = self.bridge.request(frame, self.computer_id.as_deref())?;
            loop {
                let response = request
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|_| SandLocalExecError::new(SAND_NO_LOCAL_MACHINE_MESSAGE))?;
                match response.get("kind").and_then(Value::as_str) {
                    Some("file") => {
                        request.close();
                        return Ok(());
                    }
                    Some("file-error") => {
                        let message = response
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or(SAND_NO_LOCAL_MACHINE_MESSAGE);
                        request.close();
                        return Err(SandLocalExecError::new(message));
                    }
                    _ => {}
                }
            }
        };

        send_frame(json!({
            "kind": "upload-start",
            "transferId": transfer_id,
            "path": box_path,
            "totalBytes": data.len(),
            "sha256": sha256,
        }))?;

        for (index, chunk) in data.chunks(CHUNK_BYTES).enumerate() {
            send_frame(json!({
                "kind": "upload-chunk",
                "transferId": transfer_id,
                "path": box_path,
                "offset": index * CHUNK_BYTES,
                "bytesBase64": BASE64.encode(chunk),
            }))?;
        }

        send_frame(json!({
            "kind": "upload-commit",
            "transferId": transfer_id,
            "path": box_path,
        }))
    }

    pub fn download_file(
        &self,
        agent_id: Option<&str>,
        box_path: &str,
    ) -> Result<Vec<u8>, SandLocalExecError> {
        self.download_file_with_scope(
            &GatewayLocalToolScope {
                agent_id: agent_id.map(str::to_string),
                ..GatewayLocalToolScope::default()
            },
            box_path,
        )
    }

    pub fn download_file_with_scope(
        &self,
        scope: &GatewayLocalToolScope,
        box_path: &str,
    ) -> Result<Vec<u8>, SandLocalExecError> {
        self.check_blocked()?;
        self.bridge.assert_computer_available(
            self.computer_id.as_deref(),
            "download",
            scope.agent_id.as_deref(),
        )?;
        let approval_id = self.gate.authorize(scope, "read-file", box_path)?;
        let mut frame = json!({
            "kind": "download",
            "path": box_path,
        });
        if let Some(approval_id) = approval_id {
            frame["approvalId"] = Value::String(approval_id);
        }
        let mut request = self.bridge.request(frame, self.computer_id.as_deref())?;
        loop {
            let response = request
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| SandLocalExecError::new(SAND_NO_LOCAL_MACHINE_MESSAGE))?;
            match response.get("kind").and_then(Value::as_str) {
                Some("file") => {
                    let encoded = response
                        .get("bytesBase64")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            SandLocalExecError::new(
                                "local-exec download response is missing bytesBase64",
                            )
                        })?;
                    let bytes = BASE64.decode(encoded).map_err(|error| {
                        SandLocalExecError::new(format!(
                            "local-exec download returned invalid base64: {error}"
                        ))
                    })?;
                    if bytes.len() > self.max_file_bytes {
                        request.close();
                        return Err(SandLocalExecError::new(local_exec_file_too_large_message(
                            bytes.len(),
                            self.max_file_bytes,
                        )));
                    }
                    request.close();
                    return Ok(bytes);
                }
                Some("file-error") => {
                    let message = response
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or(SAND_NO_LOCAL_MACHINE_MESSAGE);
                    request.close();
                    return Err(SandLocalExecError::new(message));
                }
                _ => {}
            }
        }
    }

    pub(crate) fn report_exec_failure(&self, scope: &GatewayLocalToolScope, message: &str) {
        let Some(reporter) = self.failure_reporter.as_ref() else {
            return;
        };
        let classification = classify_local_exec_failure(message);
        reporter(GatewayLocalExecFailureReport {
            error_class: classification.error_class,
            errno: classification.errno,
            site: "exec".to_string(),
            conversation_id: scope.agent_id.clone(),
        });
    }

    pub(crate) fn check_blocked(&self) -> Result<(), SandLocalExecError> {
        match self.gate.blocked_reason() {
            Some(reason) => Err(SandLocalExecError::new(reason)),
            None => Ok(()),
        }
    }
}

pub(crate) use super::gateway_local_exec_sandbox_ocrm::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalExecTransferContext {
    pub agent_id: String,
    pub tool_call_id: String,
}

impl TransferBox<LocalExecTransferContext> for GatewayLocalExecSandBox {
    type Error = SandLocalExecError;

    fn download_file(
        &self,
        ctx: &LocalExecTransferContext,
        _agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        self.download_file_with_scope(
            &GatewayLocalToolScope {
                agent_id: Some(ctx.agent_id.clone()),
                tool_call_id: Some(ctx.tool_call_id.clone()),
                action: None,
            },
            path,
        )
    }

    fn upload_file(
        &self,
        ctx: &LocalExecTransferContext,
        _agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        self.upload_file_with_scope(
            &GatewayLocalToolScope {
                agent_id: Some(ctx.agent_id.clone()),
                tool_call_id: Some(ctx.tool_call_id.clone()),
                action: None,
            },
            path,
            data,
        )
    }
}

#[derive(Clone)]
pub struct BridgeUserComputers {
    bridge: SandLocalExecBridge,
    gate: Arc<dyn GatewayLocalToolGate>,
}

impl BridgeUserComputers {
    pub fn new(bridge: SandLocalExecBridge, gate: Arc<dyn GatewayLocalToolGate>) -> Self {
        Self { bridge, gate }
    }

    pub fn list(&self) -> Vec<LocalExecComputer> {
        self.bridge.list_computers()
    }

    pub fn resolve(
        &self,
        id: Option<&str>,
    ) -> Option<(LocalExecComputer, GatewayLocalExecSandBox)> {
        let selected = match id {
            Some(id) => self
                .bridge
                .list_computers()
                .into_iter()
                .find(|computer| computer.id == id),
            None => self.bridge.active_computer(),
        }?;
        let box_owner = GatewayLocalExecSandBox::new(self.bridge.clone(), Arc::clone(&self.gate))
            .for_computer(selected.id.clone());
        Some((selected, box_owner))
    }
}

pub fn create_bridge_user_computers(
    bridge: SandLocalExecBridge,
    gate: Arc<dyn GatewayLocalToolGate>,
) -> BridgeUserComputers {
    BridgeUserComputers::new(bridge, gate)
}

#[cfg(test)]
mod operation_journal_fail_closed_tests {
    use super::*;
    use std::{fs, io};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

    struct AllowGate;
    impl GatewayLocalToolGate for AllowGate {
        fn blocked_reason(&self) -> Option<String> {
            None
        }
        fn requires_approval(&self) -> bool {
            false
        }
        fn authorize(
            &self,
            _scope: &GatewayLocalToolScope,
            _action: &str,
            _target: &str,
        ) -> Result<Option<String>, SandLocalExecError> {
            Ok(None)
        }
    }

    fn unique_test_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fabushi-local-exec-{label}-{}-{}.json",
            std::process::id(),
            LOCAL_EXEC_OPERATION_JOURNAL_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn test_record(
        operation_id: &str,
        status: GatewayLocalExecOperationStatus,
    ) -> GatewayLocalExecOperationRecord {
        GatewayLocalExecOperationRecord {
            operation_id: operation_id.to_string(),
            agent_id: Some("agent-test".into()),
            bridge_request_id: "bridge-test".into(),
            status,
            exit_code: None,
            aborted: false,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            settled_at_ms: None,
        }
    }

    #[test]
    fn corrupt_journal_fails_closed_instead_of_becoming_empty_history() {
        let path = unique_test_path("corrupt");
        fs::write(&path, b"{not-json").expect("write corrupt journal");
        let error = GatewayLocalExecManager::load_operation_history(&path, 8)
            .expect_err("corrupt journal must fail closed");
        assert!(error.message.contains("corrupt"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unknown_journal_version_fails_closed() {
        let path = unique_test_path("version");
        fs::write(&path, br#"{"version":999,"records":[]}"#).expect("write versioned journal");
        let error = GatewayLocalExecManager::load_operation_history(&path, 8)
            .expect_err("unknown version must fail closed");
        assert!(error.message.contains("unsupported"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn accepted_start_ambiguity_recovers_durably_as_outcome_unknown() {
        let path = unique_test_path("accepted-recovery");
        let mut history = VecDeque::new();
        history.push_back(test_record(
            "operation-accepted-ambiguous",
            GatewayLocalExecOperationStatus::Accepted,
        ));
        GatewayLocalExecManager::persist_operation_history(&path, &history)
            .expect("persist accepted journal");

        let recovered = GatewayLocalExecManager::load_operation_history(&path, 8)
            .expect("recover accepted journal");
        let record = recovered.front().expect("accepted record");
        assert_eq!(
            record.status,
            GatewayLocalExecOperationStatus::OutcomeUnknown
        );
        assert!(record.settled_at_ms.is_some());

        let second = GatewayLocalExecManager::load_operation_history(&path, 8)
            .expect("re-read accepted recovery");
        assert_eq!(
            second.front().expect("accepted record").status,
            GatewayLocalExecOperationStatus::OutcomeUnknown
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn process_death_recovery_durably_marks_running_as_outcome_unknown() {
        let path = unique_test_path("recovery");
        let mut history = VecDeque::new();
        history.push_back(test_record(
            "operation-recover",
            GatewayLocalExecOperationStatus::Running,
        ));
        GatewayLocalExecManager::persist_operation_history(&path, &history)
            .expect("persist running journal");

        let recovered =
            GatewayLocalExecManager::load_operation_history(&path, 8).expect("recover journal");
        let record = recovered.front().expect("record");
        assert_eq!(
            record.status,
            GatewayLocalExecOperationStatus::OutcomeUnknown
        );
        assert!(record.settled_at_ms.is_some());

        let second = GatewayLocalExecManager::load_operation_history(&path, 8)
            .expect("re-read recovered journal");
        assert_eq!(
            second.front().expect("record").status,
            GatewayLocalExecOperationStatus::OutcomeUnknown
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn atomic_persistence_failure_is_reported() {
        let parent_file = unique_test_path("parent-file");
        fs::write(&parent_file, b"not-a-directory").expect("write parent sentinel");
        let path = parent_file.join("journal.json");
        let error = GatewayLocalExecManager::persist_operation_history(&path, &VecDeque::new())
            .expect_err("invalid parent must fail persistence");
        assert!(
            matches!(
                error.kind(),
                io::ErrorKind::NotADirectory
                    | io::ErrorKind::AlreadyExists
                    | io::ErrorKind::PermissionDenied
                    | io::ErrorKind::Other
            ),
            "unexpected persistence error: {error}"
        );
        let _ = fs::remove_file(parent_file);
    }

    #[test]
    fn pre_send_journal_failure_does_not_send_exec_frame() {
        let bridge = SandLocalExecBridge::with_sources(
            Arc::new(|| 1000),
            Arc::new(|| "provider-test".to_string()),
        );
        let (send, receive) = mpsc::channel();
        let _registration = bridge.register_provider(send);
        let _ = receive
            .recv_timeout(Duration::from_secs(1))
            .expect("welcome");
        bridge.submit_responses(json!({
            "providerId":"provider-test",
            "frames":[{
                "kind":"hello",
                "localRoot":"/tmp",
                "terminalsFolder":"terminals",
                "computerId":"computer-test",
                "label":"Test"
            }]
        }));

        let parent_file = unique_test_path("presend-parent");
        fs::write(&parent_file, b"not-a-directory").expect("write parent sentinel");
        let journal_path = parent_file.join("journal.json");
        let sandbox = GatewayLocalExecSandBox::new(bridge, Arc::new(AllowGate));
        let manager =
            GatewayLocalExecManager::new_with_operation_journal_path(sandbox, journal_path);
        let error = manager
            .execute_shell(
                &GatewayLocalToolScope {
                    agent_id: Some("agent-test".into()),
                    tool_call_id: Some("operation-presend".into()),
                    action: Some("run-command".into()),
                },
                "echo should-not-run",
                "/tmp",
                "operation-presend",
            )
            .expect_err("journal failure must fail before provider send");
        assert!(error.message.contains("fail-closed"));
        assert!(receive.recv_timeout(Duration::from_millis(50)).is_err());
        let _ = fs::remove_file(parent_file);
    }

    #[test]
    fn completed_bounded_output_round_trips_through_journal() {
        let path = unique_test_path("bounded-output");
        let oversized = "x".repeat(MAX_LOCAL_EXEC_DURABLE_OUTPUT_BYTES + 128);
        let (stdout, stdout_truncated) = durable_shell_output(&oversized, false);
        let mut record = test_record(
            "operation-output",
            GatewayLocalExecOperationStatus::Completed,
        );
        record.stdout = stdout;
        record.stdout_truncated = stdout_truncated;
        let mut history = VecDeque::new();
        history.push_back(record);
        GatewayLocalExecManager::persist_operation_history(&path, &history)
            .expect("persist completed output");

        let recovered = GatewayLocalExecManager::load_operation_history(&path, 8)
            .expect("reload completed output");
        let recovered = recovered.front().expect("recovered record");
        assert_eq!(recovered.status, GatewayLocalExecOperationStatus::Completed);
        assert_eq!(recovered.stdout.len(), MAX_LOCAL_EXEC_DURABLE_OUTPUT_BYTES);
        assert!(recovered.stdout_truncated);
        assert!(recovered.stderr.is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn duplicate_operation_id_from_durable_history_is_not_replayed() {
        let path = unique_test_path("duplicate");
        let mut history = VecDeque::new();
        history.push_back(test_record(
            "operation-stable",
            GatewayLocalExecOperationStatus::Completed,
        ));
        GatewayLocalExecManager::persist_operation_history(&path, &history)
            .expect("persist history");

        let bridge = SandLocalExecBridge::with_sources(
            Arc::new(|| 1000),
            Arc::new(|| "provider-test".to_string()),
        );
        let (send, receive) = mpsc::channel();
        let _registration = bridge.register_provider(send);
        let _ = receive
            .recv_timeout(Duration::from_secs(1))
            .expect("welcome");
        bridge.submit_responses(json!({
            "providerId":"provider-test",
            "frames":[{
                "kind":"hello",
                "localRoot":"/tmp",
                "terminalsFolder":"terminals",
                "computerId":"computer-test",
                "label":"Test"
            }]
        }));
        let sandbox = GatewayLocalExecSandBox::new(bridge, Arc::new(AllowGate));
        let manager =
            GatewayLocalExecManager::new_with_operation_journal_path(sandbox, path.clone());
        let error = manager
            .execute_shell(
                &GatewayLocalToolScope {
                    agent_id: Some("agent-test".into()),
                    tool_call_id: Some("operation-stable".into()),
                    action: Some("run-command".into()),
                },
                "echo should-not-replay",
                "/tmp",
                "operation-stable",
            )
            .expect_err("duplicate operationId must not replay");
        assert!(error.message.contains("already exists"));
        assert!(receive.recv_timeout(Duration::from_millis(50)).is_err());
        let _ = fs::remove_file(path);
    }
}
