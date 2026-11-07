//! OCRM Local-Exec Gateway Sandbox Submodule (OCRM-MODULAR-01)
//!
//! Decomposed from `gateway_local_exec_sand_box.rs` to satisfy single responsibility
//! and cohesive file size budget. Holds `GatewayLocalExecManager` execution,
//! journaling, durable outputs, and process lifecycle controls.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use super::gateway_local_exec_sand_box::*;
use super::local_exec_bridge::{
    LocalExecCancellationHandle, SAND_NO_LOCAL_MACHINE_MESSAGE,
};
use super::local_exec_error::SandLocalExecError;
use super::local_exec_failure_classifier::{
    CommandErrorResult, CommandTermination, format_command_error,
};
use super::production::{
    GatewayExecControl, PRODUCTION_LOCAL_EXEC_CODEC, ProductionExecClientPayload,
    ProductionShellStreamEvent,
};
use crate::durable_file_policy::SAND_LOCAL_EXEC_OPERATION_JOURNAL_FILE_NAME;
use crate::host_paths::get_sand_root_dir;

pub(crate) fn append_bounded_output(target: &mut String, chunk: &str, max_bytes: usize) -> bool {
    let remaining = max_bytes.saturating_sub(target.len());
    if remaining == 0 {
        return !chunk.is_empty();
    }
    if chunk.len() <= remaining {
        target.push_str(chunk);
        return false;
    }
    let mut end = 0;
    for (index, ch) in chunk.char_indices() {
        let next = index + ch.len_utf8();
        if next > remaining {
            break;
        }
        end = next;
    }
    target.push_str(&chunk[..end]);
    true
}

pub(crate) fn append_bounded_shell_output(target: &mut String, chunk: &str) -> bool {
    append_bounded_output(target, chunk, MAX_LOCAL_EXEC_SHELL_STREAM_BYTES)
}

pub(crate) fn durable_shell_output(value: &str, already_truncated: bool) -> (String, bool) {
    let mut output = String::new();
    let truncated = append_bounded_output(&mut output, value, MAX_LOCAL_EXEC_DURABLE_OUTPUT_BYTES)
        || already_truncated;
    (output, truncated)
}

impl GatewayLocalExecManager {
    pub(crate) fn new(sandbox: GatewayLocalExecSandBox) -> Self {
        Self::new_with_operation_journal_path(
            sandbox,
            get_sand_root_dir().join(SAND_LOCAL_EXEC_OPERATION_JOURNAL_FILE_NAME),
        )
    }

    pub(crate) fn new_with_operation_journal_path(
        sandbox: GatewayLocalExecSandBox,
        operation_journal_path: PathBuf,
    ) -> Self {
        let process_custody = Arc::clone(&sandbox.process_custody);
        let (history, operation_journal_error) = match Self::load_operation_history(
            &operation_journal_path,
            DEFAULT_LOCAL_EXEC_OPERATION_HISTORY_LIMIT,
        ) {
            Ok(history) => (history, None),
            Err(error) => (VecDeque::new(), Some(error.message)),
        };
        Self {
            sandbox,
            process_custody,
            next_exec_id: Arc::new(AtomicU32::new(1)),
            active_operations: Arc::new(Mutex::new(HashMap::new())),
            operation_history: Arc::new(Mutex::new(history)),
            operation_history_limit: DEFAULT_LOCAL_EXEC_OPERATION_HISTORY_LIMIT,
            operation_journal_path: Arc::new(operation_journal_path),
            operation_journal_error: Arc::new(Mutex::new(operation_journal_error)),
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    pub(crate) fn load_operation_history(
        path: &Path,
        limit: usize,
    ) -> Result<VecDeque<GatewayLocalExecOperationRecord>, SandLocalExecError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(VecDeque::new()),
            Err(error) => {
                return Err(SandLocalExecError::new(format!(
                    "local-exec operation journal read failed closed: {error}"
                )));
            }
        };
        let mut journal = serde_json::from_slice::<GatewayLocalExecOperationJournal>(&bytes)
            .map_err(|error| {
                SandLocalExecError::new(format!(
                    "local-exec operation journal is corrupt and execution is disabled: {error}"
                ))
            })?;
        if journal.version != LOCAL_EXEC_OPERATION_JOURNAL_VERSION {
            return Err(SandLocalExecError::new(format!(
                "local-exec operation journal version {} is unsupported; expected {}; execution is disabled",
                journal.version, LOCAL_EXEC_OPERATION_JOURNAL_VERSION
            )));
        }
        let mut seen = std::collections::HashSet::new();
        if journal
            .records
            .iter()
            .any(|record| !seen.insert(record.operation_id.clone()))
        {
            return Err(SandLocalExecError::new(
                "local-exec operation journal contains duplicate operationId records; execution is disabled",
            ));
        }
        let mut recovered_uncertain = false;
        for record in &mut journal.records {
            if matches!(
                record.status,
                GatewayLocalExecOperationStatus::Accepted
                    | GatewayLocalExecOperationStatus::Running
            ) {
                record.status = GatewayLocalExecOperationStatus::OutcomeUnknown;
                record.settled_at_ms = Some(Self::now_ms());
                recovered_uncertain = true;
            }
        }
        if journal.records.len() > limit.max(1) {
            let keep_from = journal.records.len() - limit.max(1);
            journal.records.drain(..keep_from);
        }
        let history = journal.records.into_iter().collect::<VecDeque<_>>();
        if recovered_uncertain {
            Self::persist_operation_history(path, &history).map_err(|error| {
                SandLocalExecError::new(format!(
                    "local-exec process-death recovery could not persist outcome_unknown state; execution is disabled: {error}"
                ))
            })?;
        }
        Ok(history)
    }

    #[cfg(windows)]
    fn replace_operation_journal(tmp: &Path, path: &Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "Kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(
                existing_file_name: *const u16,
                new_file_name: *const u16,
                flags: u32,
            ) -> i32;
        }
        const MOVEFILE_REPLACE_EXISTING: u32 = 0x00000001;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x00000008;
        let from = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let to = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let result = unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(not(windows))]
    fn replace_operation_journal(tmp: &Path, path: &Path) -> io::Result<()> {
        fs::rename(tmp, path)
    }

    pub(crate) fn persist_operation_history(
        path: &Path,
        history: &VecDeque<GatewayLocalExecOperationRecord>,
    ) -> io::Result<()> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "journal path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let journal = GatewayLocalExecOperationJournal {
            version: LOCAL_EXEC_OPERATION_JOURNAL_VERSION,
            records: history.iter().cloned().collect(),
        };
        let bytes =
            serde_json::to_vec(&journal).map_err(|error| io::Error::other(error.to_string()))?;
        let nonce = LOCAL_EXEC_OPERATION_JOURNAL_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
        let write_result = (|| -> io::Result<()> {
            let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            Self::replace_operation_journal(&tmp, path)?;
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        write_result
    }

    fn set_operation_journal_error(&self, message: String) {
        *self
            .operation_journal_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message);
    }

    pub fn ensure_operation_journal_healthy(&self) -> Result<(), SandLocalExecError> {
        match self
            .operation_journal_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            Some(error) => Err(SandLocalExecError::new(format!(
                "local-exec operation journal unavailable; shell execution is fail-closed: {error}"
            ))),
            None => Ok(()),
        }
    }

    pub fn stable_result_ref(operation_id: &str) -> String {
        format!("fabushi-local-exec://operation/{operation_id}")
    }

    fn persist_operation_snapshot(&self) -> Result<(), SandLocalExecError> {
        let mut records = self
            .operation_history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        for (operation_id, active) in self
            .active_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
        {
            records.push_back(GatewayLocalExecOperationRecord {
                operation_id: operation_id.clone(),
                agent_id: active.agent_id.clone(),
                bridge_request_id: active.bridge_request_id.clone(),
                status: if active.started {
                    GatewayLocalExecOperationStatus::Running
                } else {
                    GatewayLocalExecOperationStatus::Accepted
                },
                exit_code: None,
                aborted: false,
                stdout: String::new(),
                stderr: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                settled_at_ms: None,
            });
        }
        Self::persist_operation_history(&self.operation_journal_path, &records).map_err(|error| {
            let message =
                format!("local-exec operation journal persistence is fail-closed: {error}");
            self.set_operation_journal_error(message.clone());
            SandLocalExecError::new(message)
        })
    }

    pub fn operation(&self, operation_id: &str) -> Option<GatewayLocalExecOperationRecord> {
        if let Some(active) = self
            .active_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(operation_id)
            .cloned()
        {
            return Some(GatewayLocalExecOperationRecord {
                operation_id: operation_id.to_string(),
                agent_id: active.agent_id,
                bridge_request_id: active.bridge_request_id,
                status: if active.started {
                    GatewayLocalExecOperationStatus::Running
                } else {
                    GatewayLocalExecOperationStatus::Accepted
                },
                exit_code: None,
                aborted: false,
                stdout: String::new(),
                stderr: String::new(),
                stdout_truncated: false,
                stderr_truncated: false,
                settled_at_ms: None,
            });
        }
        self.operation_history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .rev()
            .find(|record| record.operation_id == operation_id)
            .cloned()
    }

    pub fn operation_for_agent(
        &self,
        agent_id: &str,
        operation_id: &str,
    ) -> Option<GatewayLocalExecOperationRecord> {
        self.operation(operation_id)
            .filter(|record| record.agent_id.as_deref() == Some(agent_id))
    }

    pub fn operation_history(&self) -> Vec<GatewayLocalExecOperationRecord> {
        self.operation_history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn operation_history_for_agent(
        &self,
        agent_id: &str,
    ) -> Vec<GatewayLocalExecOperationRecord> {
        self.operation_history()
            .into_iter()
            .filter(|record| record.agent_id.as_deref() == Some(agent_id))
            .collect()
    }

    pub fn cancel_operation(&self, operation_id: &str) -> bool {
        let mut active = self
            .active_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(operation) = active.get_mut(operation_id) else {
            return false;
        };
        let delivered = operation.cancellation.cancel();
        if delivered {
            operation.cancel_requested = true;
        }
        delivered
    }

    pub fn cancel_operation_for_agent(&self, agent_id: &str, operation_id: &str) -> bool {
        let owned = self
            .active_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(operation_id)
            .is_some_and(|operation| operation.agent_id.as_deref() == Some(agent_id));
        owned && self.cancel_operation(operation_id)
    }

    fn register_operation(
        &self,
        operation_id: &str,
        agent_id: Option<&str>,
        cancellation: LocalExecCancellationHandle,
    ) -> Result<(), SandLocalExecError> {
        self.ensure_operation_journal_healthy()?;
        let mut active = self
            .active_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.contains_key(operation_id)
            || self
                .operation_history
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .any(|record| record.operation_id == operation_id)
        {
            return Err(SandLocalExecError::new(format!(
                "local-exec operationId already exists and will not be replayed: {operation_id}"
            )));
        }
        active.insert(
            operation_id.to_string(),
            GatewayLocalExecActiveOperation {
                agent_id: agent_id.map(str::to_string),
                bridge_request_id: cancellation.request_id().to_string(),
                cancellation,
                started: false,
                cancel_requested: false,
            },
        );
        drop(active);
        if let Err(error) = self.persist_operation_snapshot() {
            self.active_operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(operation_id);
            return Err(error);
        }
        Ok(())
    }

    fn mark_operation_started(&self, operation_id: &str) -> Result<(), SandLocalExecError> {
        let changed = {
            let mut active = self
                .active_operations
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let operation = active.get_mut(operation_id).ok_or_else(|| {
                SandLocalExecError::new(format!(
                    "local-exec operationId disappeared before started journal: {operation_id}"
                ))
            })?;
            if operation.started {
                false
            } else {
                operation.started = true;
                true
            }
        };
        if changed {
            self.persist_operation_snapshot()?;
        }
        Ok(())
    }

    fn cancellation_requested(&self, operation_id: &str) -> bool {
        self.active_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(operation_id)
            .is_some_and(|operation| operation.cancel_requested)
    }

    fn finish_operation(
        &self,
        operation_id: &str,
        status: GatewayLocalExecOperationStatus,
        result: Option<&GatewayLocalExecShellResult>,
    ) -> Result<(), SandLocalExecError> {
        let active = self
            .active_operations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(operation_id);
        let Some(active) = active else {
            return Ok(());
        };
        let mut history = self
            .operation_history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while history.len() >= self.operation_history_limit.max(1) {
            history.pop_front();
        }
        let (stdout, stdout_truncated) = result
            .map(|result| durable_shell_output(&result.stdout, result.stdout_truncated))
            .unwrap_or_default();
        let (stderr, stderr_truncated) = result
            .map(|result| durable_shell_output(&result.stderr, result.stderr_truncated))
            .unwrap_or_default();
        history.push_back(GatewayLocalExecOperationRecord {
            operation_id: operation_id.to_string(),
            agent_id: active.agent_id,
            bridge_request_id: active.bridge_request_id,
            status,
            exit_code: result.and_then(|result| result.exit_code),
            aborted: result.is_some_and(|result| result.aborted),
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
            settled_at_ms: Some(Self::now_ms()),
        });
        drop(history);
        self.persist_operation_snapshot().map_err(|error| {
            SandLocalExecError::new(format!(
                "{}; operation result is not safely durable (resultRef={})",
                error.message,
                Self::stable_result_ref(operation_id)
            ))
        })
    }

    pub fn terminals_folder(&self) -> String {
        self.sandbox.terminals_folder()
    }

    fn next_id(&self) -> u32 {
        let id = self.next_exec_id.fetch_add(1, Ordering::Relaxed);
        if id == 0 { 1 } else { id }
    }

    fn open_exec(
        &self,
        scope: &GatewayLocalToolScope,
        action: &str,
        target: &str,
        server_message: Value,
    ) -> Result<super::local_exec_bridge::LocalExecRequest, SandLocalExecError> {
        self.open_exec_with_before_send(scope, action, target, server_message, |_| Ok(()))
    }

    fn open_exec_with_before_send<F>(
        &self,
        scope: &GatewayLocalToolScope,
        action: &str,
        target: &str,
        server_message: Value,
        before_send: F,
    ) -> Result<super::local_exec_bridge::LocalExecRequest, SandLocalExecError>
    where
        F: FnOnce(LocalExecCancellationHandle) -> Result<(), SandLocalExecError>,
    {
        self.sandbox.check_blocked()?;
        self.sandbox.bridge.assert_computer_available(
            self.sandbox.computer_id.as_deref(),
            "exec",
            scope.agent_id.as_deref(),
        )?;
        let approval_id = self.sandbox.gate.authorize(scope, action, target)?;
        let mut frame = json!({
            "kind": "exec",
            "serverMessage": server_message,
        });
        if let Some(approval_id) = approval_id {
            frame["approvalId"] = Value::String(approval_id);
        }
        self.sandbox.bridge.request_with_before_send(
            frame,
            self.sandbox.computer_id.as_deref(),
            before_send,
        )
    }

    pub fn execute_shell(
        &self,
        scope: &GatewayLocalToolScope,
        command: &str,
        working_directory: &str,
        tool_call_id: &str,
    ) -> Result<GatewayLocalExecShellResult, SandLocalExecError> {
        self.execute_shell_with_timeout(scope, command, working_directory, tool_call_id, None)
    }

    /// Executes a shell operation while preserving the existing operation
    /// journal and provider cancellation owner. A node-host deadline requests
    /// cancellation through the same registered operation; if no terminal
    /// provider frame arrives, the result remains outcome-unknown rather than
    /// claiming that the side effect was undone.
    pub fn execute_shell_with_timeout(
        &self,
        scope: &GatewayLocalToolScope,
        command: &str,
        working_directory: &str,
        tool_call_id: &str,
        timeout: Option<Duration>,
    ) -> Result<GatewayLocalExecShellResult, SandLocalExecError> {
        self.ensure_operation_journal_healthy()?;
        let _process_slot = self
            .process_custody
            .acquire_slot("local-exec")
            .map_err(SandLocalExecError::new)?;
        let started_at = Instant::now();
        let message = PRODUCTION_LOCAL_EXEC_CODEC.shell_stream_server_message(
            self.next_id(),
            command,
            working_directory,
            tool_call_id,
        );
        let mut request = match self.open_exec_with_before_send(
            scope,
            "run-command",
            command,
            message,
            |cancellation| {
                self.register_operation(tool_call_id, scope.agent_id.as_deref(), cancellation)
            },
        ) {
            Ok(request) => request,
            Err(error) => {
                // The registration callback runs before the bridge sends the
                // frame. If that send fails, close the already-journaled
                // operation instead of leaving it indefinitely Accepted.
                if let Err(cleanup_error) = self.finish_operation(
                    tool_call_id,
                    GatewayLocalExecOperationStatus::Failed,
                    None,
                ) {
                    return Err(SandLocalExecError::new(format!(
                        "{}; operation cleanup was not safely durable: {}",
                        error.message, cleanup_error.message
                    )));
                }
                return Err(error);
            }
        };
        let mut result = GatewayLocalExecShellResult {
            request_id: tool_call_id.to_string(),
            ..GatewayLocalExecShellResult::default()
        };
        let mut terminal_status = None;
        let outcome = (|| -> Result<GatewayLocalExecShellResult, SandLocalExecError> {
            loop {
                let wait = timeout
                    .map(|timeout| timeout.saturating_sub(started_at.elapsed()))
                    .unwrap_or(Duration::from_secs(10))
                    .min(Duration::from_secs(10));
                let response = match request.recv_timeout(wait) {
                    Ok(response) => response,
                    Err(_) => {
                        let timed_out = timeout.is_some_and(|timeout| started_at.elapsed() >= timeout);
                        let cancelled = timed_out && self.cancel_operation(tool_call_id);
                        terminal_status = Some(if cancelled {
                            GatewayLocalExecOperationStatus::Cancelled
                        } else {
                            GatewayLocalExecOperationStatus::OutcomeUnknown
                        });
                        result.aborted = cancelled;
                        result.outcome_unknown = true;
                        request.close();
                        return Ok(result.clone());
                    }
                };
                match response.get("kind").and_then(Value::as_str) {
                    Some("client") => {
                        let message = response.get("message").cloned().ok_or_else(|| {
                            SandLocalExecError::new("local-exec client frame is missing message")
                        })?;
                        let decoded = PRODUCTION_LOCAL_EXEC_CODEC
                            .decode_client(message)
                            .map_err(SandLocalExecError::new)?;
                        match decoded.payload() {
                            ProductionExecClientPayload::ShellStream(event) => match event {
                                ProductionShellStreamEvent::Stdout(data) => {
                                    self.mark_operation_started(tool_call_id)?;
                                    result.stdout_truncated |=
                                        append_bounded_shell_output(&mut result.stdout, data);
                                }
                                ProductionShellStreamEvent::Stderr(data) => {
                                    self.mark_operation_started(tool_call_id)?;
                                    result.stderr_truncated |=
                                        append_bounded_shell_output(&mut result.stderr, data);
                                }
                                ProductionShellStreamEvent::Exit { code, cwd, aborted } => {
                                    result.exit_code = Some(*code);
                                    result.cwd = (!cwd.is_empty()).then(|| cwd.clone());
                                    result.aborted = *aborted;
                                    terminal_status = Some(
                                        if *aborted || self.cancellation_requested(tool_call_id) {
                                            GatewayLocalExecOperationStatus::Cancelled
                                        } else {
                                            GatewayLocalExecOperationStatus::Completed
                                        },
                                    );
                                    request.close();
                                    return Ok(result.clone());
                                }
                                ProductionShellStreamEvent::Rejected(reason)
                                | ProductionShellStreamEvent::PermissionDenied(reason) => {
                                    terminal_status =
                                        Some(if self.cancellation_requested(tool_call_id) {
                                            GatewayLocalExecOperationStatus::Cancelled
                                        } else {
                                            GatewayLocalExecOperationStatus::Failed
                                        });
                                    request.close();
                                    return Err(SandLocalExecError::new(reason.clone()));
                                }
                                ProductionShellStreamEvent::Start
                                | ProductionShellStreamEvent::Backgrounded => {
                                    self.mark_operation_started(tool_call_id)?;
                                }
                                ProductionShellStreamEvent::Other => {}
                            },
                            ProductionExecClientPayload::ReadResult(_)
                            | ProductionExecClientPayload::Other => {}
                        }
                    }
                    Some("control") => {
                        let message = response.get("message").ok_or_else(|| {
                            SandLocalExecError::new("local-exec control frame is missing message")
                        })?;
                        match PRODUCTION_LOCAL_EXEC_CODEC
                            .decode_control(message)
                            .map_err(SandLocalExecError::new)?
                        {
                            GatewayExecControl::Throw { error, stack_trace } => {
                                self.sandbox.report_exec_failure(scope, &error);
                                terminal_status =
                                    Some(if self.cancellation_requested(tool_call_id) {
                                        GatewayLocalExecOperationStatus::Cancelled
                                    } else {
                                        GatewayLocalExecOperationStatus::Failed
                                    });
                                request.close();
                                let detail = stack_trace
                                    .filter(|stack| !stack.trim().is_empty())
                                    .map(|stack| format!("{error}\n{stack}"))
                                    .unwrap_or(error);
                                return Err(SandLocalExecError::new(format_command_error(
                                    command,
                                    &CommandErrorResult {
                                        code: None,
                                        termination: CommandTermination::Error,
                                        signal: None,
                                        output_limit_exceeded: false,
                                    },
                                    &detail,
                                    "",
                                    0,
                                )));
                            }
                            GatewayExecControl::StreamClose => {
                                let cancelled = self.cancellation_requested(tool_call_id);
                                terminal_status = Some(if cancelled {
                                    GatewayLocalExecOperationStatus::Cancelled
                                } else {
                                    GatewayLocalExecOperationStatus::OutcomeUnknown
                                });
                                request.close();
                                if !cancelled {
                                    result.outcome_unknown = true;
                                }
                                return Ok(result.clone());
                            }
                            GatewayExecControl::Unknown => {}
                        }
                    }
                    Some("file-error") => {
                        let error = response
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or(SAND_NO_LOCAL_MACHINE_MESSAGE)
                            .to_string();
                        terminal_status = Some(if self.cancellation_requested(tool_call_id) {
                            GatewayLocalExecOperationStatus::Cancelled
                        } else {
                            GatewayLocalExecOperationStatus::Failed
                        });
                        request.close();
                        return Err(SandLocalExecError::new(format_command_error(
                            command,
                            &CommandErrorResult {
                                code: None,
                                termination: CommandTermination::Error,
                                signal: None,
                                output_limit_exceeded: false,
                            },
                            &error,
                            "",
                            0,
                        )));
                    }
                    _ => {}
                }
            }
        })();

        let status = terminal_status.unwrap_or_else(|| {
            if self.cancellation_requested(tool_call_id) {
                GatewayLocalExecOperationStatus::Cancelled
            } else {
                GatewayLocalExecOperationStatus::Failed
            }
        });
        if let Err(error) = self.finish_operation(tool_call_id, status, outcome.as_ref().ok()) {
            return Err(error);
        }
        outcome
    }

    pub fn execute_read(
        &self,
        scope: &GatewayLocalToolScope,
        path: &str,
        tool_call_id: &str,
        offset: Option<i32>,
        limit: Option<u32>,
        encoding_hint: Option<&str>,
    ) -> Result<Value, SandLocalExecError> {
        let message = PRODUCTION_LOCAL_EXEC_CODEC.read_server_message(
            self.next_id(),
            path,
            tool_call_id,
            offset,
            limit,
            encoding_hint,
        );
        let mut request = self.open_exec(scope, "read-file", path, message)?;
        loop {
            let response = request
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| SandLocalExecError::new("local-exec exec response timed out"))?;
            match response.get("kind").and_then(Value::as_str) {
                Some("client") => {
                    let message = response.get("message").cloned().ok_or_else(|| {
                        SandLocalExecError::new("local-exec client frame is missing message")
                    })?;
                    let decoded = PRODUCTION_LOCAL_EXEC_CODEC
                        .decode_client(message)
                        .map_err(SandLocalExecError::new)?;
                    if let ProductionExecClientPayload::ReadResult(result) = decoded.payload() {
                        let result = result.clone();
                        request.close();
                        return Ok(result);
                    }
                }
                Some("control") => {
                    let message = response.get("message").ok_or_else(|| {
                        SandLocalExecError::new("local-exec control frame is missing message")
                    })?;
                    match PRODUCTION_LOCAL_EXEC_CODEC
                        .decode_control(message)
                        .map_err(SandLocalExecError::new)?
                    {
                        GatewayExecControl::Throw { error, stack_trace } => {
                            self.sandbox.report_exec_failure(scope, &error);
                            request.close();
                            let detail = stack_trace
                                .filter(|stack| !stack.trim().is_empty())
                                .map(|stack| format!("{error}\n{stack}"))
                                .unwrap_or(error);
                            return Err(SandLocalExecError::new(detail));
                        }
                        GatewayExecControl::StreamClose => {
                            request.close();
                            return Err(SandLocalExecError::new(
                                "local-exec read stream closed without a ReadResult",
                            ));
                        }
                        GatewayExecControl::Unknown => {}
                    }
                }
                Some("file-error") => {
                    let error = response
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or(SAND_NO_LOCAL_MACHINE_MESSAGE)
                        .to_string();
                    request.close();
                    return Err(SandLocalExecError::new(error));
                }
                _ => {}
            }
        }
    }
}
