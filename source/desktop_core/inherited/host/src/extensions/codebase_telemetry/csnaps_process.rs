use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};
use thiserror::Error;

use super::csnaps_protocol::{
    CsnapsErrorCode, CsnapsOperation, CsnapsOperationResult, CsnapsResponse,
    CsnapsResponseDecoder, CsnapsServiceState, encode_csnaps_request,
    parse_csnaps_operation_result,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct CsnapsProcessError {
    message: String,
}

impl CsnapsProcessError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("csnaps {method} request failed: {code}")]
pub struct CsnapsRequestError {
    pub method: String,
    pub code: &'static str,
}

impl CsnapsRequestError {
    fn new(method: impl Into<String>, code: CsnapsErrorCode) -> Self {
        Self { method: method.into(), code: code.as_str() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsnapsDeadlines {
    pub ping: Duration,
    pub initialize: Duration,
    pub apply_codebase_specs: Duration,
    pub snapshot: Duration,
    pub trigger_upload: Duration,
    pub flush_pending_uploads: Duration,
    pub shutdown: Duration,
    pub exit: Duration,
}

impl Default for CsnapsDeadlines {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(10),
            initialize: Duration::from_secs(120),
            apply_codebase_specs: Duration::from_secs(600),
            snapshot: Duration::from_secs(600),
            trigger_upload: Duration::from_secs(30),
            flush_pending_uploads: Duration::from_secs(1_800),
            shutdown: Duration::from_secs(15),
            exit: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpawnedCsnaps {
    pub handle: Arc<CsnapsProcess>,
    pub initial_state: CsnapsServiceState,
}

#[derive(Debug)]
enum ReaderEvent {
    Response(CsnapsResponse),
    Terminal(CsnapsProcessError),
}

#[derive(Debug)]
pub struct CsnapsProcess {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    responses: Mutex<mpsc::Receiver<ReaderEvent>>,
    next_request_id: AtomicU64,
    request_lock: Mutex<()>,
    terminal_error: Arc<Mutex<Option<CsnapsProcessError>>>,
    is_closing: AtomicBool,
    deadlines: CsnapsDeadlines,
}

impl CsnapsProcess {
    pub fn spawn(
        executable_path: impl AsRef<Path>,
        initialize_params: Map<String, Value>,
        deadlines: CsnapsDeadlines,
    ) -> Result<SpawnedCsnaps, CsnapsProcessError> {
        let mut child = Command::new(executable_path.as_ref())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| CsnapsProcessError::new(format!("could not spawn csnaps: {error}")))?;

        let stdin = child.stdin.take().ok_or_else(|| CsnapsProcessError::new("csnaps stdin unavailable"))?;
        let mut stdout = child.stdout.take().ok_or_else(|| CsnapsProcessError::new("csnaps stdout unavailable"))?;
        let (tx, rx) = mpsc::channel();
        let terminal_error = Arc::new(Mutex::new(None));
        let terminal_for_reader = Arc::clone(&terminal_error);

        thread::Builder::new()
            .name("csnaps-response-reader".to_string())
            .spawn(move || {
                let mut decoder = CsnapsResponseDecoder::new();
                let mut buf = [0_u8; 16 * 1024];
                loop {
                    match stdout.read(&mut buf) {
                        Ok(0) => {
                            let error = match decoder.assert_complete() {
                                Ok(()) => CsnapsProcessError::new("csnaps process exited"),
                                Err(error) => CsnapsProcessError::new(error.to_string()),
                            };
                            set_terminal(&terminal_for_reader, error.clone());
                            let _ = tx.send(ReaderEvent::Terminal(error));
                            break;
                        }
                        Ok(read) => match decoder.decode(&buf[..read]) {
                            Ok(responses) => {
                                for response in responses {
                                    if tx.send(ReaderEvent::Response(response)).is_err() {
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                let error = CsnapsProcessError::new(format!("invalid csnaps response: {error}"));
                                set_terminal(&terminal_for_reader, error.clone());
                                let _ = tx.send(ReaderEvent::Terminal(error));
                                break;
                            }
                        },
                        Err(error) => {
                            let error = CsnapsProcessError::new(format!("csnaps stdout failed: {error}"));
                            set_terminal(&terminal_for_reader, error.clone());
                            let _ = tx.send(ReaderEvent::Terminal(error));
                            break;
                        }
                    }
                }
            })
            .map_err(|error| CsnapsProcessError::new(format!("could not start csnaps reader: {error}")))?;

        let handle = Arc::new(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            responses: Mutex::new(rx),
            next_request_id: AtomicU64::new(0),
            request_lock: Mutex::new(()),
            terminal_error,
            is_closing: AtomicBool::new(false),
            deadlines,
        });

        let initialized = (|| {
            handle.ping()?;
            handle.initialize(initialize_params)
        })();

        match initialized {
            Ok(initial_state) => Ok(SpawnedCsnaps { handle, initial_state }),
            Err(error) => {
                let _ = handle.terminate();
                Err(error)
            }
        }
    }

    pub fn terminal_failure(&self) -> Option<CsnapsProcessError> {
        self.terminal_error
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn ping(&self) -> Result<(), CsnapsProcessError> {
        self.request_empty(CsnapsOperation::Ping)
    }

    pub fn initialize(&self, params: Map<String, Value>) -> Result<CsnapsServiceState, CsnapsProcessError> {
        let operation = CsnapsOperation::Initialize(params);
        match self.request(&operation)? {
            CsnapsOperationResult::Initialize { state } => Ok(state),
            _ => Err(CsnapsProcessError::new("invalid csnaps initialize result")),
        }
    }

    pub fn apply_codebase_specs(
        &self,
        codebases: Vec<Value>,
    ) -> Result<(CsnapsServiceState, Vec<super::csnaps_protocol::CsnapsRejectedCodebase>), CsnapsProcessError> {
        let operation = CsnapsOperation::ApplyCodebaseSpecs(
            Map::from_iter([("codebases".to_string(), Value::Array(codebases))]),
        );
        match self.request(&operation)? {
            CsnapsOperationResult::ApplyCodebaseSpecs { state, rejected } => Ok((state, rejected)),
            _ => Err(CsnapsProcessError::new("invalid csnaps apply result")),
        }
    }

    pub fn snapshot(&self, reason: Value) -> Result<(), CsnapsProcessError> {
        self.request_empty(CsnapsOperation::Snapshot(
            Map::from_iter([("reason".to_string(), reason)]),
        ))
    }

    pub fn trigger_upload(&self, credentials: Map<String, Value>) -> Result<(), CsnapsProcessError> {
        self.request_empty(CsnapsOperation::TriggerUpload(credentials))
    }

    pub fn flush_pending_uploads(&self, credentials: Map<String, Value>) -> Result<(), CsnapsProcessError> {
        self.request_empty(CsnapsOperation::FlushPendingUploads(credentials))
    }

    pub fn close(&self) -> Result<(), CsnapsProcessError> {
        self.is_closing.store(true, Ordering::SeqCst);
        let shutdown = self.request_empty(CsnapsOperation::Shutdown);
        if shutdown.is_ok() && self.wait_for_child_close(self.deadlines.exit).is_ok() {
            return Ok(());
        }
        self.terminate()
    }

    pub fn terminate(&self) -> Result<(), CsnapsProcessError> {
        self.is_closing.store(true, Ordering::SeqCst);
        drop(self.stdin.lock().unwrap_or_else(|poison| poison.into_inner()));
        let mut child = self.child.lock().unwrap_or_else(|poison| poison.into_inner());
        if child.try_wait().map_err(|error| CsnapsProcessError::new(error.to_string()))?.is_none() {
            child.kill().map_err(|error| CsnapsProcessError::new(format!("could not terminate csnaps: {error}")))?;
        }
        let started = Instant::now();
        loop {
            if child.try_wait().map_err(|error| CsnapsProcessError::new(error.to_string()))?.is_some() {
                return Ok(());
            }
            if started.elapsed() >= self.deadlines.exit {
                return Err(CsnapsProcessError::new("csnaps process did not close after terminate"));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn request_empty(&self, operation: CsnapsOperation) -> Result<(), CsnapsProcessError> {
        match self.request(&operation)? {
            CsnapsOperationResult::Empty => Ok(()),
            _ => Err(CsnapsProcessError::new(format!(
                "invalid empty csnaps {} result",
                operation.method()
            ))),
        }
    }

    fn request(&self, operation: &CsnapsOperation) -> Result<CsnapsOperationResult, CsnapsProcessError> {
        let _serial = self.request_lock.lock().unwrap_or_else(|poison| poison.into_inner());

        if let Some(error) = self.terminal_failure() {
            return Err(error);
        }

        let id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let frame = encode_csnaps_request(id, operation)
            .map_err(|error| CsnapsProcessError::new(error.to_string()))?;

        {
            let mut stdin = self.stdin.lock().unwrap_or_else(|poison| poison.into_inner());
            stdin
                .write_all(&frame)
                .and_then(|_| stdin.flush())
                .map_err(|error| {
                    let failure = CsnapsProcessError::new(format!("csnaps stdin write failed: {error}"));
                    set_terminal(&self.terminal_error, failure.clone());
                    failure
                })?;
        }

        let timeout = self.deadline_for(operation.method());
        let event = self
            .responses
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    let failure = CsnapsProcessError::new(format!(
                        "csnaps {} request timed out",
                        operation.method()
                    ));
                    set_terminal(&self.terminal_error, failure.clone());
                    let _ = self.kill_after_failure();
                    failure
                }
                mpsc::RecvTimeoutError::Disconnected => self
                    .terminal_failure()
                    .unwrap_or_else(|| CsnapsProcessError::new("csnaps response channel closed")),
            })?;

        match event {
            ReaderEvent::Terminal(error) => Err(error),
            ReaderEvent::Response(CsnapsResponse::Success { id: response_id, result }) => {
                if let Err(failure) = validate_response_id(id, response_id) {
                    set_terminal(&self.terminal_error, failure.clone());
                    let _ = self.kill_after_failure();
                    return Err(failure);
                }
                parse_csnaps_operation_result(operation, &result)
                    .map_err(|error| CsnapsProcessError::new(error.to_string()))
            }
            ReaderEvent::Response(CsnapsResponse::Error { id: response_id, error }) => {
                if let Err(failure) = validate_response_id(id, response_id) {
                    set_terminal(&self.terminal_error, failure.clone());
                    let _ = self.kill_after_failure();
                    return Err(failure);
                }
                Err(CsnapsProcessError::new(
                    CsnapsRequestError::new(operation.method(), error).to_string(),
                ))
            }
        }
    }

    fn deadline_for(&self, method: &str) -> Duration {
        match method {
            "ping" => self.deadlines.ping,
            "initialize" => self.deadlines.initialize,
            "apply_codebase_specs" => self.deadlines.apply_codebase_specs,
            "snapshot" => self.deadlines.snapshot,
            "trigger_upload" => self.deadlines.trigger_upload,
            "flush_pending_uploads" => self.deadlines.flush_pending_uploads,
            "shutdown" => self.deadlines.shutdown,
            _ => self.deadlines.exit,
        }
    }

    fn kill_after_failure(&self) -> Result<(), CsnapsProcessError> {
        let mut child = self.child.lock().unwrap_or_else(|poison| poison.into_inner());
        if child.try_wait().map_err(|error| CsnapsProcessError::new(error.to_string()))?.is_none() {
            child.kill().map_err(|error| CsnapsProcessError::new(format!("could not terminate csnaps: {error}")))?;
        }
        Ok(())
    }

    fn wait_for_child_close(&self, timeout: Duration) -> Result<(), CsnapsProcessError> {
        let started = Instant::now();
        loop {
            let mut child = self.child.lock().unwrap_or_else(|poison| poison.into_inner());
            if child.try_wait().map_err(|error| CsnapsProcessError::new(error.to_string()))?.is_some() {
                return Ok(());
            }
            drop(child);
            if started.elapsed() >= timeout {
                return Err(CsnapsProcessError::new("csnaps process did not close after shutdown"));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

pub fn validate_response_id(
    expected: u64,
    actual: u64,
) -> Result<(), CsnapsProcessError> {
    if expected == actual {
        Ok(())
    } else {
        Err(CsnapsProcessError::new(
            "csnaps returned an unknown request ID",
        ))
    }
}

fn set_terminal(slot: &Arc<Mutex<Option<CsnapsProcessError>>>, error: CsnapsProcessError) {
    let mut guard = slot.lock().unwrap_or_else(|poison| poison.into_inner());
    if guard.is_none() {
        *guard = Some(error);
    }
}

pub fn describe_csnaps_exit(exit_code: Option<i32>, signal: Option<&str>) -> String {
    match (exit_code, signal) {
        (Some(code), _) => format!("csnaps process exited with code {code}"),
        (None, Some(signal)) => format!("csnaps process exited due to signal {signal}"),
        _ => "csnaps process exited".to_string(),
    }
}

pub fn make_initialize_params(
    auth_id: &str,
    codebase_uuid_state_path: &str,
    snapshots_base_dir: &str,
    backend_url: &str,
) -> Map<String, Value> {
    Map::from_iter([
        ("authId".to_string(), json!(auth_id)),
        ("codebaseUuidStatePath".to_string(), json!(codebase_uuid_state_path)),
        ("snapshotsBaseDir".to_string(), json!(snapshots_base_dir)),
        ("environment".to_string(), json!(1)),
        ("backendUrl".to_string(), json!(backend_url)),
    ])
}
