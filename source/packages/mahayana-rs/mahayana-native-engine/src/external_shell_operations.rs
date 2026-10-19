use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const EXTERNAL_SHELL_JOURNAL_VERSION: u32 = 1;
const DEFAULT_EXTERNAL_SHELL_HISTORY_LIMIT: usize = 128;
const MAX_DURABLE_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExternalShellOperationStatus {
    Accepted,
    Running,
    Completed,
    Cancelled,
    OutcomeUnknown,
    Failed,
}

impl ExternalShellOperationStatus {
    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::OutcomeUnknown | Self::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalShellOperationRecord {
    operation_id: String,
    scope_id: String,
    account_generation: u64,
    request_id: String,
    status: ExternalShellOperationStatus,
    #[serde(default)]
    exit_code: Option<i64>,
    #[serde(default)]
    aborted: bool,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    stdout_truncated: bool,
    #[serde(default)]
    stderr_truncated: bool,
    #[serde(default)]
    settled_at_ms: Option<u64>,
}

impl ExternalShellOperationRecord {
    fn public_value(&self) -> Value {
        json!({
            "operationId": self.operation_id,
            "requestId": self.operation_id,
            "status": self.status,
            "exitCode": self.exit_code,
            "aborted": self.aborted,
            "stdout": self.stdout,
            "stderr": self.stderr,
            "stdoutTruncated": self.stdout_truncated,
            "stderrTruncated": self.stderr_truncated,
            "settledAtMs": self.settled_at_ms,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExternalShellOperationJournal {
    version: u32,
    records: Vec<ExternalShellOperationRecord>,
}

#[derive(Default)]
struct LedgerState {
    records: VecDeque<ExternalShellOperationRecord>,
    journal_error: Option<String>,
}

pub(crate) struct ExternalShellOperationLedger {
    state: Mutex<LedgerState>,
    journal_path: Option<PathBuf>,
    history_limit: usize,
}

impl ExternalShellOperationLedger {
    pub(crate) fn memory() -> Self {
        Self {
            state: Mutex::new(LedgerState::default()),
            journal_path: None,
            history_limit: DEFAULT_EXTERNAL_SHELL_HISTORY_LIMIT,
        }
    }

    pub(crate) fn durable(path: PathBuf) -> Self {
        let history_limit = DEFAULT_EXTERNAL_SHELL_HISTORY_LIMIT;
        let (records, recovered_uncertain, mut journal_error) =
            match Self::load_records(&path, history_limit) {
                Ok(value) => (value.0, value.1, None),
                Err(error) => (VecDeque::new(), false, Some(error)),
            };

        if recovered_uncertain && journal_error.is_none() {
            if let Err(error) = Self::write_records(&path, &records) {
                journal_error = Some(format!(
                    "external shell process-death recovery could not persist outcome_unknown state: {error}"
                ));
            }
        }

        Self {
            state: Mutex::new(LedgerState {
                records,
                journal_error,
            }),
            journal_path: Some(path),
            history_limit,
        }
    }

    pub(crate) fn register(
        &self,
        scope_id: &str,
        account_generation: u64,
        operation_id: &str,
        request_id: &str,
    ) -> Result<(), String> {
        if scope_id.trim().is_empty() || operation_id.trim().is_empty() || request_id.trim().is_empty() {
            return Err("external shell operation scope, operationId, and requestId are required".into());
        }
        let mut state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        if state.records.iter().any(|record| record.operation_id == operation_id) {
            return Err(format!(
                "external shell operationId already exists and will not be replayed: {operation_id}"
            ));
        }
        state.records.push_back(ExternalShellOperationRecord {
            operation_id: operation_id.to_string(),
            scope_id: scope_id.to_string(),
            account_generation,
            request_id: request_id.to_string(),
            status: ExternalShellOperationStatus::Accepted,
            exit_code: None,
            aborted: false,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            settled_at_ms: None,
        });
        self.trim_locked(&mut state);
        self.persist_locked(&mut state)
    }

    pub(crate) fn mark_running(&self, request_id: &str) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        let Some(record) = state.records.iter_mut().find(|record| record.request_id == request_id) else {
            return Ok(());
        };
        match record.status {
            ExternalShellOperationStatus::Accepted => {
                record.status = ExternalShellOperationStatus::Running;
                self.persist_locked(&mut state)
            }
            ExternalShellOperationStatus::Running => Ok(()),
            _ => Ok(()),
        }
    }

    pub(crate) fn settle_success(&self, request_id: &str, result: &Value) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        let Some(record) = state.records.iter_mut().find(|record| record.request_id == request_id) else {
            return Ok(());
        };
        if record.status.is_terminal() {
            return Ok(());
        }
        let outcome_unknown = result.get("outcomeUnknown").and_then(Value::as_bool).unwrap_or(false);
        let aborted = result.get("aborted").and_then(Value::as_bool).unwrap_or(false);
        let success = result.get("success").and_then(Value::as_bool).unwrap_or(true);
        record.status = if outcome_unknown {
            ExternalShellOperationStatus::OutcomeUnknown
        } else if aborted {
            ExternalShellOperationStatus::Cancelled
        } else if success {
            ExternalShellOperationStatus::Completed
        } else {
            ExternalShellOperationStatus::Failed
        };
        record.exit_code = result
            .get("code")
            .or_else(|| result.get("exitCode"))
            .and_then(Value::as_i64);
        record.aborted = aborted;
        let (stdout, stdout_truncated) = bounded_output(
            result.get("stdout").and_then(Value::as_str).unwrap_or_default(),
        );
        let (stderr, stderr_truncated) = bounded_output(
            result.get("stderr").and_then(Value::as_str).unwrap_or_default(),
        );
        record.stdout = stdout;
        record.stderr = stderr;
        record.stdout_truncated = stdout_truncated;
        record.stderr_truncated = stderr_truncated;
        record.settled_at_ms = Some(now_ms());
        self.trim_locked(&mut state);
        self.persist_locked(&mut state)
    }

    pub(crate) fn settle_error(&self, request_id: &str, message: &str) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        let Some(record) = state.records.iter_mut().find(|record| record.request_id == request_id) else {
            return Ok(());
        };
        if record.status.is_terminal() {
            return Ok(());
        }
        let was_running = record.status == ExternalShellOperationStatus::Running;
        record.status = if message.starts_with("cancelled:") {
            ExternalShellOperationStatus::Cancelled
        } else if message.contains("stale_execution_scope")
            || (was_running && message.contains("remote_execution_failed"))
        {
            ExternalShellOperationStatus::OutcomeUnknown
        } else {
            ExternalShellOperationStatus::Failed
        };
        record.settled_at_ms = Some(now_ms());
        let (stderr, stderr_truncated) = bounded_output(message);
        record.stderr = stderr;
        record.stderr_truncated = stderr_truncated;
        self.trim_locked(&mut state);
        self.persist_locked(&mut state)
    }

    pub(crate) fn mark_cancelled(&self, request_id: &str) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        let Some(record) = state.records.iter_mut().find(|record| record.request_id == request_id) else {
            return Ok(());
        };
        if record.status.is_terminal() {
            return Ok(());
        }
        record.status = ExternalShellOperationStatus::Cancelled;
        record.aborted = true;
        record.settled_at_ms = Some(now_ms());
        self.trim_locked(&mut state);
        self.persist_locked(&mut state)
    }

    pub(crate) fn mark_all_outcome_unknown(&self) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        let mut changed = false;
        for record in &mut state.records {
            if matches!(
                record.status,
                ExternalShellOperationStatus::Accepted | ExternalShellOperationStatus::Running
            ) {
                record.status = ExternalShellOperationStatus::OutcomeUnknown;
                record.settled_at_ms = Some(now_ms());
                changed = true;
            }
        }
        if changed {
            self.persist_locked(&mut state)?;
        }
        Ok(())
    }

    pub(crate) fn status(
        &self,
        scope_id: &str,
        account_generation: u64,
        operation_id: &str,
    ) -> Result<Value, String> {
        let state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        let record = state
            .records
            .iter()
            .find(|record| {
                record.operation_id == operation_id
                    && record.scope_id == scope_id
                    && record.account_generation == account_generation
            })
            .ok_or_else(|| format!("external shell operation is unknown in the current account/generation scope: {operation_id}"))?;
        Ok(record.public_value())
    }

    pub(crate) fn history(
        &self,
        scope_id: &str,
        account_generation: u64,
    ) -> Result<Value, String> {
        let state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        Ok(Value::Array(
            state
                .records
                .iter()
                .filter(|record| {
                    record.scope_id == scope_id && record.account_generation == account_generation
                })
                .map(ExternalShellOperationRecord::public_value)
                .collect(),
        ))
    }

    pub(crate) fn active_request(
        &self,
        scope_id: &str,
        account_generation: u64,
        operation_id: &str,
    ) -> Result<Option<String>, String> {
        let state = self.state.lock().map_err(|_| "external shell operation ledger poisoned".to_string())?;
        Self::ensure_healthy(&state)?;
        let record = state
            .records
            .iter()
            .find(|record| {
                record.operation_id == operation_id
                    && record.scope_id == scope_id
                    && record.account_generation == account_generation
            })
            .ok_or_else(|| format!("external shell operation is unknown in the current account/generation scope: {operation_id}"))?;
        Ok((!record.status.is_terminal()).then(|| record.request_id.clone()))
    }

    pub(crate) fn has_request(&self, request_id: &str) -> bool {
        self.state
            .lock()
            .map(|state| state.records.iter().any(|record| record.request_id == request_id))
            .unwrap_or(false)
    }

    pub(crate) fn is_terminal_request(&self, request_id: &str) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|state| {
                state
                    .records
                    .iter()
                    .find(|record| record.request_id == request_id)
                    .map(|record| record.status.is_terminal())
            })
            .unwrap_or(false)
    }

    pub(crate) fn is_cancelled_request(&self, request_id: &str) -> bool {
        self.state
            .lock()
            .ok()
            .and_then(|state| {
                state
                    .records
                    .iter()
                    .find(|record| record.request_id == request_id)
                    .map(|record| record.status == ExternalShellOperationStatus::Cancelled)
            })
            .unwrap_or(false)
    }

    fn ensure_healthy(state: &LedgerState) -> Result<(), String> {
        match &state.journal_error {
            Some(error) => Err(format!("external shell operation journal is fail-closed: {error}")),
            None => Ok(()),
        }
    }

    fn persist_locked(&self, state: &mut LedgerState) -> Result<(), String> {
        let Some(path) = self.journal_path.as_deref() else {
            return Ok(());
        };
        if let Err(error) = Self::write_records(path, &state.records) {
            let message = format!("external shell operation journal persist failed closed: {error}");
            state.journal_error = Some(message.clone());
            return Err(message);
        }
        Ok(())
    }

    fn trim_locked(&self, state: &mut LedgerState) {
        while state.records.len() > self.history_limit.max(1) {
            if let Some(index) = state.records.iter().position(|record| record.status.is_terminal()) {
                state.records.remove(index);
            } else {
                break;
            }
        }
    }

    fn load_records(
        path: &Path,
        limit: usize,
    ) -> Result<(VecDeque<ExternalShellOperationRecord>, bool), String> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((VecDeque::new(), false));
            }
            Err(error) => {
                return Err(format!("journal read failed: {error}"));
            }
        };
        let mut journal: ExternalShellOperationJournal = serde_json::from_slice(&bytes)
            .map_err(|error| format!("journal is corrupt: {error}"))?;
        if journal.version != EXTERNAL_SHELL_JOURNAL_VERSION {
            return Err(format!(
                "journal version {} is unsupported; expected {}",
                journal.version, EXTERNAL_SHELL_JOURNAL_VERSION
            ));
        }
        let mut seen = HashSet::new();
        if journal.records.iter().any(|record| !seen.insert(record.operation_id.clone())) {
            return Err("journal contains duplicate operationId records".into());
        }
        let mut recovered_uncertain = false;
        for record in &mut journal.records {
            if matches!(
                record.status,
                ExternalShellOperationStatus::Accepted | ExternalShellOperationStatus::Running
            ) {
                record.status = ExternalShellOperationStatus::OutcomeUnknown;
                record.settled_at_ms = Some(now_ms());
                recovered_uncertain = true;
            }
        }
        if journal.records.len() > limit.max(1) {
            let keep_from = journal.records.len() - limit.max(1);
            journal.records.drain(..keep_from);
        }
        Ok((journal.records.into_iter().collect(), recovered_uncertain))
    }

    fn write_records(
        path: &Path,
        records: &VecDeque<ExternalShellOperationRecord>,
    ) -> Result<(), String> {
        let parent = path.parent().ok_or_else(|| "journal path has no parent".to_string())?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let journal = ExternalShellOperationJournal {
            version: EXTERNAL_SHELL_JOURNAL_VERSION,
            records: records.iter().cloned().collect(),
        };
        let bytes = serde_json::to_vec(&journal).map_err(|error| error.to_string())?;
        let temporary = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    }
}

fn bounded_output(value: &str) -> (String, bool) {
    if value.len() <= MAX_DURABLE_OUTPUT_BYTES {
        return (value.to_string(), false);
    }
    let mut end = MAX_DURABLE_OUTPUT_BYTES;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("mahayana-external-shell-{label}-{}.json", Uuid::new_v4()))
    }

    #[test]
    fn durable_recovery_converts_inflight_to_outcome_unknown() {
        let path = temp_path("recovery");
        let ledger = ExternalShellOperationLedger::durable(path.clone());
        ledger.register("conversation-a", 7, "op-a", "request-a").unwrap();
        ledger.mark_running("request-a").unwrap();
        drop(ledger);

        let recovered = ExternalShellOperationLedger::durable(path.clone());
        let status = recovered.status("conversation-a", 7, "op-a").unwrap();
        assert_eq!(status["status"], "outcome_unknown");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn duplicate_operation_id_is_fail_closed_before_replay() {
        let ledger = ExternalShellOperationLedger::memory();
        ledger.register("conversation-a", 3, "stable-op", "request-a").unwrap();
        let error = ledger
            .register("conversation-a", 3, "stable-op", "request-b")
            .unwrap_err();
        assert!(error.contains("already exists"));
    }

    #[test]
    fn history_is_account_generation_and_conversation_fenced() {
        let ledger = ExternalShellOperationLedger::memory();
        ledger.register("conversation-a", 3, "op-a", "request-a").unwrap();
        ledger.settle_success("request-a", &json!({"success":true,"stdout":"ok"})).unwrap();
        assert_eq!(ledger.history("conversation-a", 3).unwrap().as_array().unwrap().len(), 1);
        assert!(ledger.history("conversation-b", 3).unwrap().as_array().unwrap().is_empty());
        assert!(ledger.history("conversation-a", 4).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn terminal_history_is_bounded() {
        let ledger = ExternalShellOperationLedger::memory();
        for index in 0..(DEFAULT_EXTERNAL_SHELL_HISTORY_LIMIT + 12) {
            let operation_id = format!("operation-{index}");
            let request_id = format!("request-{index}");
            ledger
                .register("conversation-a", 1, &operation_id, &request_id)
                .unwrap();
            ledger
                .settle_success(&request_id, &json!({"success":true}))
                .unwrap();
        }
        assert_eq!(
            ledger.history("conversation-a", 1).unwrap().as_array().unwrap().len(),
            DEFAULT_EXTERNAL_SHELL_HISTORY_LIMIT
        );
        assert!(ledger.status("conversation-a", 1, "operation-0").is_err());
    }
}
