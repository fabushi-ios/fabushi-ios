use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::ProviderSessionError;
use crate::extensions::cloud_workers::{
    CloudWorkerLeaseScope, CloudWorkerLeasedCapability,
};
use crate::extensions::cloud_workers::extension::CloudWorkerProvisioningExtension;
use crate::extensions::local_exec::extension::HostLocalExecExtension;
use crate::extensions::local_exec::gateway_local_exec_sand_box::{
    GatewayLocalExecManager, GatewayLocalExecOperationRecord, GatewayLocalExecOperationStatus,
    GatewayLocalToolGate, GatewayLocalToolScope,
};
use crate::extensions::local_tool_permission::extension::HostLocalToolPermissionExtension;
use crate::extensions::local_exec::gateway_local_exec_sand_box::TurnScopedGatewayLocalToolGate;
use crate::runner::shell_terminal_watch::{ShellTerminalPollRead, TerminalReadResult};
use crate::runner::tools::sand_external_machine_tools::{
    ExternalMachineExecutor, ExternalMachineReadArgs, ExternalMachineShellArgs,
};

#[derive(Clone)]
pub struct ProductionExternalMachineExecutor {
    manager: Arc<GatewayLocalExecManager>,
    agent_id: String,
}

const MAX_CLOUD_WORKER_REPLAY_KEYS: usize = 1024;

/// Fenced consumer adapter for an already enrolled Cloud Worker capability.
///
/// This type owns no worker lifecycle, device grant, sensitive-result store or
/// provider executor. It revalidates the ephemeral capability against the
/// canonical Cloud Worker owner before every operation and delegates the
/// action to exactly one existing `ExternalMachineExecutor`.
pub struct CloudWorkerScopedExternalMachineExecutor {
    extension: Arc<CloudWorkerProvisioningExtension>,
    capability: CloudWorkerLeasedCapability,
    expected_scope: CloudWorkerLeaseScope,
    delegate: Arc<dyn ExternalMachineExecutor>,
    replay_keys: Mutex<HashSet<String>>,
}

impl CloudWorkerScopedExternalMachineExecutor {
    pub fn new(
        extension: Arc<CloudWorkerProvisioningExtension>,
        capability: CloudWorkerLeasedCapability,
        expected_scope: CloudWorkerLeaseScope,
        delegate: Arc<dyn ExternalMachineExecutor>,
    ) -> Self {
        Self {
            extension,
            capability,
            expected_scope,
            delegate,
            replay_keys: Mutex::new(HashSet::new()),
        }
    }

    fn validate(&self) -> Result<(), ProviderSessionError> {
        if self.capability.scope() != &self.expected_scope {
            return Err(ProviderSessionError::Tool(
                "cloud-worker capability scope does not match the consumer scope".into(),
            ));
        }
        self.extension
            .validate_current_capability(&self.capability)
            .map(|_| ())
            .map_err(|error| ProviderSessionError::Tool(format!(
                "cloud-worker capability rejected before delegation: {error}"
            )))
    }

    fn claim_replay_key(&self, value: &str) -> Result<(), ProviderSessionError> {
        let key = value.trim();
        if key.is_empty() {
            return Err(ProviderSessionError::Tool(
                "cloud-worker operation requires a replay key".into(),
            ));
        }
        let mut keys = self.replay_keys.lock().map_err(|_| {
            ProviderSessionError::Tool("cloud-worker replay-key guard is unavailable".into())
        })?;
        if keys.len() >= MAX_CLOUD_WORKER_REPLAY_KEYS {
            return Err(ProviderSessionError::Tool(
                "cloud-worker replay-key capacity is exhausted".into(),
            ));
        }
        if !keys.insert(key.to_string()) {
            return Err(ProviderSessionError::Tool(
                "cloud-worker replay key was already used".into(),
            ));
        }
        Ok(())
    }
}

impl ExternalMachineExecutor for CloudWorkerScopedExternalMachineExecutor {
    fn execute_shell(
        &self,
        args: &ExternalMachineShellArgs,
    ) -> Result<Value, ProviderSessionError> {
        self.validate()?;
        self.claim_replay_key(&args.tool_call_id)?;
        self.delegate.execute_shell(args)
    }

    fn execute_read(
        &self,
        args: &ExternalMachineReadArgs,
    ) -> Result<Value, ProviderSessionError> {
        self.validate()?;
        self.claim_replay_key(&args.tool_call_id)?;
        self.delegate.execute_read(args)
    }

    fn operation_status(&self, operation_id: &str) -> Result<Value, ProviderSessionError> {
        self.validate()?;
        self.delegate.operation_status(operation_id)
    }

    fn operation_history(&self) -> Result<Value, ProviderSessionError> {
        self.validate()?;
        self.delegate.operation_history()
    }

    fn cancel_operation(&self, operation_id: &str) -> Result<Value, ProviderSessionError> {
        self.validate()?;
        if operation_id.trim().is_empty() {
            return Err(ProviderSessionError::Tool(
                "cloud-worker cancellation requires an operation id".into(),
            ));
        }
        self.delegate.cancel_operation(operation_id)
    }

    fn poll_background_shell_terminal(
        &self,
        shell_id: &str,
        tool_call_id: &str,
    ) -> ShellTerminalPollRead {
        if let Err(error) = self.validate() {
            return ShellTerminalPollRead::TransientFailure(error.to_string());
        }
        if let Err(error) = self.claim_replay_key(tool_call_id) {
            return ShellTerminalPollRead::TransientFailure(error.to_string());
        }
        self.delegate
            .poll_background_shell_terminal(shell_id, tool_call_id)
    }
}

impl ProductionExternalMachineExecutor {
    pub fn new(
        local_exec: Arc<HostLocalExecExtension>,
        local_tool_permission: Arc<HostLocalToolPermissionExtension>,
        agent_id: impl Into<String>,
    ) -> Self {
        let gate: Arc<dyn GatewayLocalToolGate> = local_tool_permission;
        Self::with_gate(local_exec, gate, agent_id)
    }

    pub fn for_turn(
        local_exec: Arc<HostLocalExecExtension>,
        local_tool_permission: Arc<HostLocalToolPermissionExtension>,
        agent_id: impl Into<String>,
        direction_epoch: u64,
    ) -> Self {
        let gate: Arc<dyn GatewayLocalToolGate> = Arc::new(
            TurnScopedGatewayLocalToolGate::new(local_tool_permission, direction_epoch),
        );
        Self::with_gate(local_exec, gate, agent_id)
    }

    fn with_gate(
        local_exec: Arc<HostLocalExecExtension>,
        gate: Arc<dyn GatewayLocalToolGate>,
        agent_id: impl Into<String>,
    ) -> Self {
        let manager = local_exec
            .sandbox(gate)
            .remote_resource_accessor()
            .manager();
        Self {
            manager,
            agent_id: agent_id.into(),
        }
    }

    fn scope(&self, tool_call_id: &str, action: &str) -> GatewayLocalToolScope {
        GatewayLocalToolScope {
            agent_id: Some(self.agent_id.clone()),
            tool_call_id: Some(tool_call_id.to_string()),
            action: Some(action.to_string()),
        }
    }

    fn operation_json(record: GatewayLocalExecOperationRecord) -> Value {
        json!({
            "operationId": record.operation_id,
            "status": match record.status {
                GatewayLocalExecOperationStatus::Accepted => "accepted",
                GatewayLocalExecOperationStatus::Running => "running",
                GatewayLocalExecOperationStatus::Completed => "completed",
                GatewayLocalExecOperationStatus::Cancelled => "cancelled",
                GatewayLocalExecOperationStatus::OutcomeUnknown => "outcome_unknown",
                GatewayLocalExecOperationStatus::Failed => "failed",
            },
            "exitCode": record.exit_code,
            "aborted": record.aborted,
            "requestId": record.operation_id.clone(),
            "resultRef": GatewayLocalExecManager::stable_result_ref(&record.operation_id),
            "stdout": record.stdout,
            "stderr": record.stderr,
            "stdoutTruncated": record.stdout_truncated,
            "stderrTruncated": record.stderr_truncated,
        })
    }
}

impl ExternalMachineExecutor for ProductionExternalMachineExecutor {
    fn execute_shell(
        &self,
        args: &ExternalMachineShellArgs,
    ) -> Result<Value, ProviderSessionError> {
        let result = self
            .manager
            .execute_shell(
                &self.scope(&args.tool_call_id, "run-command"),
                &args.command,
                &args.working_directory,
                &args.tool_call_id,
            )
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        Ok(json!({
            "stdout": result.stdout,
            "stderr": result.stderr,
            "exitCode": result.exit_code,
            "cwd": result.cwd,
            "aborted": result.aborted,
            "requestId": result.request_id.clone(),
            "operationId": result.request_id,
            "outcomeUnknown": result.outcome_unknown,
            "stdoutTruncated": result.stdout_truncated,
            "stderrTruncated": result.stderr_truncated
        }))
    }

    fn operation_status(&self, operation_id: &str) -> Result<Value, ProviderSessionError> {
        Ok(self.manager
            .operation_for_agent(&self.agent_id, operation_id)
            .map(Self::operation_json)
            .unwrap_or_else(|| json!({"operationId":operation_id,"status":"not_found"})))
    }

    fn operation_history(&self) -> Result<Value, ProviderSessionError> {
        Ok(Value::Array(
            self.manager
                .operation_history_for_agent(&self.agent_id)
                .into_iter()
                .map(Self::operation_json)
                .collect(),
        ))
    }

    fn cancel_operation(&self, operation_id: &str) -> Result<Value, ProviderSessionError> {
        Ok(json!({
            "operationId": operation_id,
            "cancelRequested": self.manager.cancel_operation_for_agent(&self.agent_id, operation_id),
        }))
    }

    fn poll_background_shell_terminal(
        &self,
        shell_id: &str,
        tool_call_id: &str,
    ) -> ShellTerminalPollRead {
        let folder = self.manager.terminals_folder();
        let output_path = format!("{}/{}.txt", folder.trim_end_matches('/'), shell_id.trim());
        let value = match self.execute_read(&ExternalMachineReadArgs {
            path: output_path.clone(),
            offset: None,
            limit: None,
            encoding_hint: None,
            tool_call_id: tool_call_id.to_string(),
        }) {
            Ok(value) => value,
            Err(error) => {
                let message = error.to_string();
                let lower = message.to_ascii_lowercase();
                if lower.contains("permission") || lower.contains("approved") {
                    return ShellTerminalPollRead::PermissionDenied { output_path: Some(output_path) };
                }
                if lower.contains("not found") || lower.contains("no such file") {
                    return ShellTerminalPollRead::Snapshot {
                        output_path,
                        result: TerminalReadResult::FileNotFound,
                    };
                }
                return ShellTerminalPollRead::TransientFailure(message);
            }
        };
        if value.get("fileNotFound").is_some() {
            return ShellTerminalPollRead::Snapshot { output_path, result: TerminalReadResult::FileNotFound };
        }
        if value.get("permissionDenied").is_some() {
            return ShellTerminalPollRead::PermissionDenied { output_path: Some(output_path) };
        }
        let Some(success) = value.get("success") else {
            return ShellTerminalPollRead::TransientFailure(format!(
                "local-exec terminal read returned an unsupported result: {value}"
            ));
        };
        let content = success.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
        ShellTerminalPollRead::Snapshot {
            output_path,
            result: TerminalReadResult::SuccessText(content),
        }
    }

    fn execute_read(
        &self,
        args: &ExternalMachineReadArgs,
    ) -> Result<Value, ProviderSessionError> {
        self.manager
            .execute_read(
                &self.scope(&args.tool_call_id, "read-file"),
                &args.path,
                &args.tool_call_id,
                args.offset,
                args.limit,
                args.encoding_hint.as_deref(),
            )
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))
    }
}
