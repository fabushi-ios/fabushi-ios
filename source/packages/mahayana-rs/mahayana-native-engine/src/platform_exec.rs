use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::external_shell_operations::ExternalShellOperationLedger;

const DEFAULT_TERMINAL_CACHE: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformExecRequest {
    pub request_id: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestState {
    Queued,
    Leased,
}

struct PendingRequest {
    request: PlatformExecRequest,
    state: RequestState,
    sender: Option<oneshot::Sender<Result<Value, String>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TerminalRecord {
    request_id: String,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExternalShellExecutionIdentity {
    account_scope: String,
    account_revision: u64,
    host_generation: u64,
}

#[derive(Default)]
struct BrokerState {
    closed: bool,
    external_shell_identity: Option<ExternalShellExecutionIdentity>,
    queue: VecDeque<String>,
    pending: HashMap<String, PendingRequest>,
    terminals: VecDeque<TerminalRecord>,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PlatformExecBrokerError {
    #[error("platform execution broker is closed")]
    Closed,
    #[error("platform execution queue is full")]
    QueueFull,
    #[error("platform execution request is unknown: {0}")]
    UnknownRequest(String),
    #[error("conflicting duplicate platform execution settlement: {0}")]
    ConflictingSettlement(String),
    #[error("platform execution broker state is poisoned")]
    Poisoned,
}

pub struct PlatformExecBroker {
    capacity: usize,
    terminal_cache: usize,
    state: Mutex<BrokerState>,
    external_shell_operations: ExternalShellOperationLedger,
}

impl fmt::Debug for PlatformExecBroker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PlatformExecBroker")
            .field("capacity", &self.capacity)
            .field("pending", &self.pending_count().unwrap_or_default())
            .finish()
    }
}

struct PendingGuard {
    broker: Arc<PlatformExecBroker>,
    request_id: String,
    armed: bool,
}

impl PendingGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = self
                .broker
                .cancel(&self.request_id, "platform execution waiter dropped");
        }
    }
}

impl PlatformExecBroker {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            terminal_cache: DEFAULT_TERMINAL_CACHE,
            state: Mutex::new(BrokerState::default()),
            external_shell_operations: ExternalShellOperationLedger::memory(),
        }
    }

    pub fn new_with_external_shell_journal(
        capacity: usize,
        journal_path: std::path::PathBuf,
    ) -> Self {
        Self {
            capacity: capacity.max(1),
            terminal_cache: DEFAULT_TERMINAL_CACHE,
            state: Mutex::new(BrokerState::default()),
            external_shell_operations: ExternalShellOperationLedger::durable(journal_path),
        }
    }

    pub fn bind_external_shell_identity(
        &self,
        account_scope: &str,
        account_revision: u64,
        host_generation: u64,
    ) -> Result<(), String> {
        let account_scope = account_scope.trim();
        if account_scope.is_empty() {
            return Err("external shell account scope is empty".into());
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned.to_string())?;
        if state.closed {
            return Err(PlatformExecBrokerError::Closed.to_string());
        }
        state.external_shell_identity = Some(ExternalShellExecutionIdentity {
            account_scope: account_scope.to_string(),
            account_revision,
            host_generation,
        });
        Ok(())
    }

    pub fn clear_external_shell_identity(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned.to_string())?;
        state.external_shell_identity = None;
        Ok(())
    }

    fn scoped_external_shell_scope(&self, conversation_scope: &str) -> Result<String, String> {
        let conversation_scope = conversation_scope.trim();
        if conversation_scope.is_empty() {
            return Err("external shell conversation scope is empty".into());
        }
        let state = self
            .state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned.to_string())?;
        let identity = state
            .external_shell_identity
            .as_ref()
            .ok_or_else(|| "external shell account identity is unavailable".to_string())?;
        Ok(format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            identity.account_scope,
            identity.account_revision,
            identity.host_generation,
            conversation_scope
        ))
    }

    pub async fn execute(
        self: &Arc<Self>,
        method: impl Into<String>,
        params: Value,
    ) -> Result<Value, String> {
        let request_id = Uuid::new_v4().to_string();
        self.execute_with_request_id(request_id, method.into(), params).await
    }

    pub async fn execute_external_shell(
        self: &Arc<Self>,
        scope_id: &str,
        account_generation: u64,
        operation_id: &str,
        params: Value,
    ) -> Result<Value, String> {
        let request_id = Uuid::new_v4().to_string();
        let scoped_identity = self.scoped_external_shell_scope(scope_id)?;
        self.external_shell_operations.register(
            &scoped_identity,
            account_generation,
            operation_id,
            &request_id,
        )?;
        let result = self
            .execute_with_request_id(request_id.clone(), "shell.exec".into(), params)
            .await;
        match &result {
            Ok(value) => self.external_shell_operations.settle_success(&request_id, value)?,
            Err(message) => self.external_shell_operations.settle_error(&request_id, message)?,
        }
        result.map(|mut value| {
            if let Some(object) = value.as_object_mut() {
                object.insert("operationId".into(), Value::String(operation_id.to_string()));
            }
            value
        })
    }

    pub fn start_external_shell_detached(
        self: &Arc<Self>,
        scope_id: &str,
        account_generation: u64,
        operation_id: &str,
        params: Value,
    ) -> Result<tokio::task::JoinHandle<Result<Value, String>>, String> {
        let request_id = Uuid::new_v4().to_string();
        let scoped_identity = self.scoped_external_shell_scope(scope_id)?;
        self.external_shell_operations.register(
            &scoped_identity,
            account_generation,
            operation_id,
            &request_id,
        )?;
        let broker = Arc::clone(self);
        let operation_id = operation_id.to_string();
        Ok(tokio::spawn(async move {
            let result = broker
                .execute_with_request_id(request_id.clone(), "shell.exec".into(), params)
                .await;
            match &result {
                Ok(value) => broker
                    .external_shell_operations
                    .settle_success(&request_id, value)?,
                Err(message) => broker
                    .external_shell_operations
                    .settle_error(&request_id, message)?,
            }
            result.map(|mut value| {
                if let Some(object) = value.as_object_mut() {
                    object.insert("operationId".into(), Value::String(operation_id));
                }
                value
            })
        }))
    }

    pub fn external_shell_status(
        &self,
        scope_id: &str,
        account_generation: u64,
        operation_id: &str,
    ) -> Result<Value, String> {
        let scoped_identity = self.scoped_external_shell_scope(scope_id)?;
        self.external_shell_operations
            .status(&scoped_identity, account_generation, operation_id)
    }

    pub fn external_shell_history(
        &self,
        scope_id: &str,
        account_generation: u64,
    ) -> Result<Value, String> {
        let scoped_identity = self.scoped_external_shell_scope(scope_id)?;
        self.external_shell_operations.history(&scoped_identity, account_generation)
    }

    pub fn cancel_external_shell(
        &self,
        scope_id: &str,
        account_generation: u64,
        operation_id: &str,
    ) -> Result<bool, String> {
        let scoped_identity = self.scoped_external_shell_scope(scope_id)?;
        let Some(request_id) = self
            .external_shell_operations
            .active_request(&scoped_identity, account_generation, operation_id)?
        else {
            return Ok(false);
        };
        self.external_shell_operations.request_cancel(&request_id)
    }

    pub fn external_shell_cancelled_request(&self, request_id: &str) -> bool {
        self.external_shell_operations.is_cancelled_request(request_id)
    }

    async fn execute_with_request_id(
        self: &Arc<Self>,
        request_id: String,
        method: String,
        params: Value,
    ) -> Result<Value, String> {
        if method.trim().is_empty() {
            return Err("platform execution method is empty".into());
        }
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| PlatformExecBrokerError::Poisoned.to_string())?;
            if state.closed {
                return Err(PlatformExecBrokerError::Closed.to_string());
            }
            if state.pending.len() >= self.capacity {
                return Err(PlatformExecBrokerError::QueueFull.to_string());
            }
            state.queue.push_back(request_id.clone());
            state.pending.insert(
                request_id.clone(),
                PendingRequest {
                    request: PlatformExecRequest {
                        request_id: request_id.clone(),
                        method,
                        params,
                    },
                    state: RequestState::Queued,
                    sender: Some(sender),
                },
            );
        }
        let mut guard = PendingGuard {
            broker: Arc::clone(self),
            request_id,
            armed: true,
        };
        let result = receiver
            .await
            .map_err(|_| "platform execution broker dropped the terminal result".to_string())?;
        guard.disarm();
        result
    }

    pub fn next_request(
        &self,
    ) -> Result<Option<PlatformExecRequest>, PlatformExecBrokerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned)?;
        if state.closed {
            return Err(PlatformExecBrokerError::Closed);
        }
        while let Some(request_id) = state.queue.pop_front() {
            let Some(pending) = state.pending.get_mut(&request_id) else {
                continue;
            };
            if pending.state != RequestState::Queued {
                continue;
            }
            pending.state = RequestState::Leased;
            if self.external_shell_operations.has_request(&request_id) {
                self.external_shell_operations
                    .mark_running(&request_id)
                    .map_err(|_| PlatformExecBrokerError::Poisoned)?;
            }
            return Ok(Some(pending.request.clone()));
        }
        Ok(None)
    }

    pub fn resolve(
        &self,
        request_id: &str,
        result: Value,
    ) -> Result<(), PlatformExecBrokerError> {
        self.settle(request_id, Ok(result))
    }

    pub fn reject(
        &self,
        request_id: &str,
        code: &str,
        message: &str,
    ) -> Result<(), PlatformExecBrokerError> {
        let code = if code.trim().is_empty() {
            "platform_execution_failed"
        } else {
            code.trim()
        };
        let message = if message.trim().is_empty() {
            "platform execution failed"
        } else {
            message.trim()
        };
        self.settle(request_id, Err(format!("{code}: {message}")))
    }

    pub fn cancel(
        &self,
        request_id: &str,
        reason: &str,
    ) -> Result<(), PlatformExecBrokerError> {
        let reason = if reason.trim().is_empty() {
            "platform execution cancelled"
        } else {
            reason.trim()
        };
        self.settle(request_id, Err(format!("cancelled: {reason}")))
    }

    pub fn cancel_all(&self, reason: &str) -> Result<(), PlatformExecBrokerError> {
        let reason = if reason.trim().is_empty() {
            "platform execution cancelled"
        } else {
            reason.trim()
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned)?;
        let pending = std::mem::take(&mut state.pending);
        state.queue.clear();
        for (_, mut entry) in pending {
            if let Some(sender) = entry.sender.take() {
                let _ = sender.send(Err(format!("cancelled: {reason}")));
            }
        }
        Ok(())
    }

    pub fn close(&self, reason: &str) -> Result<(), PlatformExecBrokerError> {
        self.external_shell_operations
            .mark_all_outcome_unknown()
            .map_err(|_| PlatformExecBrokerError::Poisoned)?;
        self.cancel_all(reason)?;
        self.state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned)?
            .closed = true;
        Ok(())
    }

    pub fn pending_count(&self) -> Result<usize, PlatformExecBrokerError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned)?
            .pending
            .len())
    }

    fn settle(
        &self,
        request_id: &str,
        terminal: Result<Value, String>,
    ) -> Result<(), PlatformExecBrokerError> {
        let fingerprint = terminal_fingerprint(&terminal);
        let mut state = self
            .state
            .lock()
            .map_err(|_| PlatformExecBrokerError::Poisoned)?;
        if let Some(record) = state
            .terminals
            .iter()
            .find(|record| record.request_id == request_id)
        {
            return if record.fingerprint == fingerprint {
                Ok(())
            } else {
                Err(PlatformExecBrokerError::ConflictingSettlement(
                    request_id.to_string(),
                ))
            };
        }
        let Some(mut pending) = state.pending.remove(request_id) else {
            return Err(PlatformExecBrokerError::UnknownRequest(
                request_id.to_string(),
            ));
        };
        state.queue.retain(|candidate| candidate != request_id);
        if let Some(sender) = pending.sender.take() {
            let _ = sender.send(terminal);
        }
        state.terminals.push_back(TerminalRecord {
            request_id: request_id.to_string(),
            fingerprint,
        });
        while state.terminals.len() > self.terminal_cache {
            state.terminals.pop_front();
        }
        Ok(())
    }
}

fn terminal_fingerprint(terminal: &Result<Value, String>) -> String {
    match terminal {
        Ok(value) => format!(
            "ok:{}",
            serde_json::to_string(value).unwrap_or_else(|_| "null".into())
        ),
        Err(message) => format!("err:{message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broker_leases_once_and_resolves_waiter() {
        let broker = Arc::new(PlatformExecBroker::new(4));
        let caller = {
            let broker = Arc::clone(&broker);
            tokio::spawn(async move {
                broker
                    .execute("process.exec", serde_json::json!({"program":"git"}))
                    .await
            })
        };
        tokio::task::yield_now().await;
        let request = broker.next_request().unwrap().expect("queued request");
        assert_eq!(request.method, "process.exec");
        assert!(broker.next_request().unwrap().is_none());
        broker
            .resolve(
                &request.request_id,
                serde_json::json!({"success":true,"stdout":"ok"}),
            )
            .unwrap();
        assert_eq!(
            caller.await.unwrap().unwrap(),
            serde_json::json!({"success":true,"stdout":"ok"})
        );
        assert_eq!(broker.pending_count().unwrap(), 0);
    }

    #[test]
    fn platform_exec_request_round_trips_camel_case_request_id_and_typed_payload() {
        let request = PlatformExecRequest {
            request_id: "request-42".into(),
            method: "box.exec".into(),
            params: serde_json::json!({
                "case": "shellArgs",
                "args": {
                    "command": "git status --short",
                    "toolCallId": "call-42"
                }
            }),
        };

        let encoded = serde_json::to_value(&request).expect("serialize platform exec request");
        assert_eq!(
            encoded,
            serde_json::json!({
                "requestId": "request-42",
                "method": "box.exec",
                "params": {
                    "case": "shellArgs",
                    "args": {
                        "command": "git status --short",
                        "toolCallId": "call-42"
                    }
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<PlatformExecRequest>(encoded)
                .expect("deserialize platform exec request"),
            request
        );
    }

    #[tokio::test]
    async fn cancelled_platform_exec_rejects_late_conflicting_success() {
        let broker = Arc::new(PlatformExecBroker::new(4));
        let caller = {
            let broker = Arc::clone(&broker);
            tokio::spawn(async move {
                broker
                    .execute("box.exec", serde_json::json!({"case": "shellArgs"}))
                    .await
            })
        };

        tokio::task::yield_now().await;
        let request = broker.next_request().unwrap().expect("queued request");
        broker
            .cancel(&request.request_id, "remote execution cancelled")
            .unwrap();
        assert!(caller.await.unwrap().unwrap_err().contains("cancelled"));
        assert!(matches!(
            broker.resolve(
                &request.request_id,
                serde_json::json!({"case": "success"})
            ),
            Err(PlatformExecBrokerError::ConflictingSettlement(request_id))
                if request_id == request.request_id
        ));
        assert_eq!(broker.pending_count().unwrap(), 0);
    }

    #[tokio::test]
    async fn duplicate_terminal_is_idempotent_but_conflict_fails_closed() {
        let broker = Arc::new(PlatformExecBroker::new(4));
        let caller = {
            let broker = Arc::clone(&broker);
            tokio::spawn(async move {
                broker.execute("shell", serde_json::json!({})).await
            })
        };
        tokio::task::yield_now().await;
        let request = broker.next_request().unwrap().unwrap();
        let result = serde_json::json!({"ok":true});
        broker.resolve(&request.request_id, result.clone()).unwrap();
        broker.resolve(&request.request_id, result).unwrap();
        assert!(matches!(
            broker.resolve(
                &request.request_id,
                serde_json::json!({"ok":false})
            ),
            Err(PlatformExecBrokerError::ConflictingSettlement(_))
        ));
        assert!(caller.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn external_shell_uses_stable_operation_id_and_real_cancel_signal() {
        let broker = Arc::new(PlatformExecBroker::new(4));
        broker
            .bind_external_shell_identity("account-a", 5, 17)
            .unwrap();
        let caller = {
            let broker = Arc::clone(&broker);
            tokio::spawn(async move {
                broker
                    .execute_external_shell(
                        "conversation-a",
                        11,
                        "tool-call-stable",
                        serde_json::json!({
                            "command":"sleep 60",
                            "operationId":"tool-call-stable"
                        }),
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        let request = broker.next_request().unwrap().expect("queued shell request");
        assert_eq!(request.method, "shell.exec");
        assert_eq!(
            broker
                .external_shell_status("conversation-a", 11, "tool-call-stable")
                .unwrap()["status"],
            "running"
        );
        assert!(broker
            .cancel_external_shell("conversation-a", 11, "tool-call-stable")
            .unwrap());
        assert!(broker.external_shell_cancelled_request(&request.request_id));
        assert_eq!(
            broker
                .external_shell_status("conversation-a", 11, "tool-call-stable")
                .unwrap()["status"],
            "running"
        );
        assert!(!caller.is_finished());
        broker
            .reject(&request.request_id, "cancelled", "remote execution cancelled")
            .unwrap();
        assert!(caller.await.unwrap().unwrap_err().contains("cancelled"));
        assert_eq!(
            broker
                .external_shell_status("conversation-a", 11, "tool-call-stable")
                .unwrap()["status"],
            "cancelled"
        );

        let duplicate = broker
            .execute_external_shell(
                "conversation-a",
                11,
                "tool-call-stable",
                serde_json::json!({"command":"echo replay"}),
            )
            .await
            .unwrap_err();
        assert!(duplicate.contains("already exists"));
    }

    #[tokio::test]
    async fn detached_external_shell_registers_before_return_and_settles_later() {
        let broker = Arc::new(PlatformExecBroker::new(4));
        broker
            .bind_external_shell_identity("account-a", 5, 17)
            .unwrap();
        let handle = broker
            .start_external_shell_detached(
                "conversation-a",
                11,
                "tool-call-background",
                serde_json::json!({
                    "command":"sleep 60",
                    "operationId":"tool-call-background"
                }),
            )
            .expect("start detached shell");
        let initial_status = broker
            .external_shell_status("conversation-a", 11, "tool-call-background")
            .unwrap()["status"]
            .as_str()
            .expect("initial durable status")
            .to_string();
        assert!(matches!(initial_status.as_str(), "accepted" | "running"));
        let request = loop {
            if let Some(request) = broker.next_request().unwrap() {
                break request;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(request.method, "shell.exec");
        broker
            .resolve(
                &request.request_id,
                serde_json::json!({
                    "success":true,
                    "code":0,
                    "stdout":"done",
                    "stderr":"",
                    "aborted":false
                }),
            )
            .unwrap();
        let result = handle.await.unwrap().unwrap();
        assert_eq!(result["operationId"], "tool-call-background");
        assert_eq!(result["stdout"], "done");
        assert_eq!(
            broker
                .external_shell_status("conversation-a", 11, "tool-call-background")
                .unwrap()["status"],
            "completed"
        );
    }

    #[tokio::test]
    async fn external_shell_history_is_fenced_by_bound_account_identity() {
        let broker = Arc::new(PlatformExecBroker::new(4));
        broker
            .bind_external_shell_identity("account-a", 1, 9)
            .unwrap();
        let caller = {
            let broker = Arc::clone(&broker);
            tokio::spawn(async move {
                broker
                    .execute_external_shell(
                        "conversation-a",
                        3,
                        "operation-a",
                        serde_json::json!({"command":"echo ok"}),
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        let request = broker.next_request().unwrap().unwrap();
        broker
            .resolve(&request.request_id, serde_json::json!({"success":true}))
            .unwrap();
        caller.await.unwrap().unwrap();

        broker
            .bind_external_shell_identity("account-b", 1, 9)
            .unwrap();
        assert!(broker
            .external_shell_history("conversation-a", 3)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert!(broker
            .external_shell_status("conversation-a", 3, "operation-a")
            .is_err());

        broker
            .bind_external_shell_identity("account-a", 2, 9)
            .unwrap();
        assert!(broker
            .external_shell_history("conversation-a", 3)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn close_rejects_outstanding_and_releases_resources() {
        let broker = Arc::new(PlatformExecBroker::new(4));
        let caller = {
            let broker = Arc::clone(&broker);
            tokio::spawn(async move {
                broker
                    .execute("process.exec", serde_json::json!({}))
                    .await
            })
        };
        tokio::task::yield_now().await;
        let _ = broker.next_request().unwrap().unwrap();
        broker.close("host closed").unwrap();
        assert!(caller.await.unwrap().unwrap_err().contains("host closed"));
        assert_eq!(broker.pending_count().unwrap(), 0);
        assert!(matches!(
            broker.next_request(),
            Err(PlatformExecBrokerError::Closed)
        ));
    }
}
