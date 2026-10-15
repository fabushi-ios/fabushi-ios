use super::csnaps_protocol::{
    CsnapsErrorCode, CsnapsOperation, CsnapsOperationResult, CsnapsRejectedCodebase,
    CsnapsResponse, CsnapsResponseDecoder, CsnapsServiceState, CsnapsTrackedCodebase,
    decode_frame_payloads, encode_csnaps_error, encode_csnaps_request, encode_csnaps_success,
    is_uuid_v4, parse_csnaps_operation_result, parse_csnaps_request,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCsnapsError(pub String);

impl NativeCsnapsError {
    fn new(message: impl Into<String>) -> Self { Self(message.into()) }
}
impl std::fmt::Display for NativeCsnapsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for NativeCsnapsError {}

#[derive(Debug, Clone, Copy)]
pub struct NativeCsnapsDeadlines {
    pub ping: Duration,
    pub initialize: Duration,
    pub apply_codebase_specs: Duration,
    pub snapshot: Duration,
    pub trigger_upload: Duration,
    pub flush_pending_uploads: Duration,
    pub shutdown: Duration,
}

impl Default for NativeCsnapsDeadlines {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(10),
            initialize: Duration::from_secs(120),
            apply_codebase_specs: Duration::from_secs(600),
            snapshot: Duration::from_secs(600),
            trigger_upload: Duration::from_secs(30),
            flush_pending_uploads: Duration::from_secs(1_800),
            shutdown: Duration::from_secs(15),
        }
    }
}

pub struct SpawnedNativeCsnaps {
    pub handle: Arc<NativeCsnapsTransport>,
    pub initial_state: CsnapsServiceState,
}

enum ServiceEvent {
    Frame(Vec<u8>),
    Terminal(String),
}

pub struct NativeCsnapsTransport {
    request_tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    response_rx: Mutex<mpsc::Receiver<ServiceEvent>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    next_request_id: AtomicU64,
    request_lock: Mutex<()>,
    terminal: Mutex<Option<String>>,
    closed: AtomicBool,
    deadlines: NativeCsnapsDeadlines,
}

impl NativeCsnapsTransport {
    pub fn spawn(
        root: &Path,
        auth_id: &str,
        backend_url: &str,
        deadlines: NativeCsnapsDeadlines,
    ) -> Result<SpawnedNativeCsnaps, NativeCsnapsError> {
        fs::create_dir_all(root)
            .map_err(|error| NativeCsnapsError::new(format!("create native csnaps root: {error}")))?;
        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("ios-native-csnaps-service".into())
            .spawn(move || native_service_loop(request_rx, response_tx))
            .map_err(|error| NativeCsnapsError::new(format!("start native csnaps service: {error}")))?;
        let handle = Arc::new(Self {
            request_tx: Mutex::new(Some(request_tx)),
            response_rx: Mutex::new(response_rx),
            worker: Mutex::new(Some(worker)),
            next_request_id: AtomicU64::new(0),
            request_lock: Mutex::new(()),
            terminal: Mutex::new(None),
            closed: AtomicBool::new(false),
            deadlines,
        });

        let initialize = Map::from_iter([
            ("authId".into(), json!(auth_id)),
            ("codebaseUuidStatePath".into(), json!(root.join("codebase-uuids.json").to_string_lossy())),
            ("snapshotsBaseDir".into(), json!(root.join("snapshots").to_string_lossy())),
            ("environment".into(), json!(1)),
            ("backendUrl".into(), json!(backend_url)),
        ]);

        let initialized = (|| {
            handle.request_empty(CsnapsOperation::Ping)?;
            match handle.request(&CsnapsOperation::Initialize(initialize))? {
                CsnapsOperationResult::Initialize { state } => Ok(state),
                _ => Err(NativeCsnapsError::new("invalid native csnaps initialize result")),
            }
        })();

        match initialized {
            Ok(initial_state) => Ok(SpawnedNativeCsnaps { handle, initial_state }),
            Err(error) => {
                handle.mark_terminal(error.to_string());
                handle.teardown();
                Err(error)
            }
        }
    }

    pub fn terminal_failure(&self) -> Option<String> {
        self.terminal.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn apply_codebase_specs(
        &self,
        codebases: Vec<Value>,
    ) -> Result<(CsnapsServiceState, Vec<CsnapsRejectedCodebase>), NativeCsnapsError> {
        match self.request(&CsnapsOperation::ApplyCodebaseSpecs(Map::from_iter([
            ("codebases".into(), Value::Array(codebases)),
        ])))? {
            CsnapsOperationResult::ApplyCodebaseSpecs { state, rejected } => Ok((state, rejected)),
            _ => Err(NativeCsnapsError::new("invalid native csnaps apply result")),
        }
    }

    pub fn snapshot(&self, reason: Value) -> Result<(), NativeCsnapsError> {
        self.request_empty(CsnapsOperation::Snapshot(Map::from_iter([
            ("reason".into(), reason),
        ])))
    }

    pub fn trigger_upload(&self, credentials: Map<String, Value>) -> Result<(), NativeCsnapsError> {
        self.request_empty(CsnapsOperation::TriggerUpload(credentials))
    }

    pub fn flush_pending_uploads(&self, credentials: Map<String, Value>) -> Result<(), NativeCsnapsError> {
        self.request_empty(CsnapsOperation::FlushPendingUploads(credentials))
    }

    pub fn close(&self) -> Result<(), NativeCsnapsError> {
        if self.closed.swap(true, Ordering::SeqCst) { return Ok(()); }
        let result = if self.terminal_failure().is_none() {
            self.request_empty(CsnapsOperation::Shutdown)
        } else {
            Ok(())
        };
        self.teardown();
        result
    }

    fn request_empty(&self, operation: CsnapsOperation) -> Result<(), NativeCsnapsError> {
        match self.request(&operation)? {
            CsnapsOperationResult::Empty => Ok(()),
            _ => Err(NativeCsnapsError::new(format!(
                "invalid empty native csnaps {} result", operation.method()
            ))),
        }
    }

    fn request(&self, operation: &CsnapsOperation) -> Result<CsnapsOperationResult, NativeCsnapsError> {
        let _serial = self.request_lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(error) = self.terminal_failure() {
            return Err(NativeCsnapsError::new(error));
        }
        let id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let frame = encode_csnaps_request(id, operation)
            .map_err(|error| NativeCsnapsError::new(error.to_string()))?;
        let sender = self.request_tx.lock().unwrap_or_else(|p| p.into_inner()).clone()
            .ok_or_else(|| NativeCsnapsError::new("native csnaps transport is closed"))?;
        if sender.send(frame).is_err() {
            let error = "native csnaps request channel closed".to_string();
            self.mark_terminal(error.clone());
            drop(sender);
            self.teardown();
            return Err(NativeCsnapsError::new(error));
        }
        drop(sender);

        let event = self.response_rx.lock().unwrap_or_else(|p| p.into_inner())
            .recv_timeout(self.deadline_for(operation.method()))
            .map_err(|error| {
                let message = match error {
                    mpsc::RecvTimeoutError::Timeout => format!("csnaps {} request timed out", operation.method()),
                    mpsc::RecvTimeoutError::Disconnected => self.terminal_failure()
                        .unwrap_or_else(|| "native csnaps response channel closed".into()),
                };
                self.mark_terminal(message.clone());
                self.teardown();
                NativeCsnapsError::new(message)
            })?;

        let frame = match event {
            ServiceEvent::Frame(frame) => frame,
            ServiceEvent::Terminal(error) => {
                self.mark_terminal(error.clone());
                self.teardown();
                return Err(NativeCsnapsError::new(error));
            }
        };

        let mut decoder = CsnapsResponseDecoder::new();
        let responses = decoder.decode(&frame).map_err(|error| {
            let message = format!("invalid native csnaps response: {error}");
            self.mark_terminal(message.clone());
            self.teardown();
            NativeCsnapsError::new(message)
        })?;
        if let Err(error) = decoder.assert_complete() {
            let message = format!("invalid native csnaps response: {error}");
            self.mark_terminal(message.clone());
            self.teardown();
            return Err(NativeCsnapsError::new(message));
        }
        if responses.len() != 1 {
            let message = "native csnaps returned an invalid response count".to_string();
            self.mark_terminal(message.clone());
            self.teardown();
            return Err(NativeCsnapsError::new(message));
        }
        match responses.into_iter().next().unwrap() {
            CsnapsResponse::Success { id: response_id, result } => {
                self.validate_response_id(id, response_id)?;
                parse_csnaps_operation_result(operation, &result)
                    .map_err(|error| NativeCsnapsError::new(error.to_string()))
            }
            CsnapsResponse::Error { id: response_id, error } => {
                self.validate_response_id(id, response_id)?;
                Err(NativeCsnapsError::new(format!(
                    "csnaps {} request failed: {}", operation.method(), error.as_str()
                )))
            }
        }
    }

    fn validate_response_id(&self, expected: u64, actual: u64) -> Result<(), NativeCsnapsError> {
        if expected == actual { return Ok(()); }
        let message = "csnaps returned an unknown request ID".to_string();
        self.mark_terminal(message.clone());
        self.teardown();
        Err(NativeCsnapsError::new(message))
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
            _ => self.deadlines.ping,
        }
    }

    fn mark_terminal(&self, message: String) {
        let mut terminal = self.terminal.lock().unwrap_or_else(|p| p.into_inner());
        if terminal.is_none() { *terminal = Some(message); }
    }

    fn teardown(&self) {
        self.request_tx.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

impl Drop for NativeCsnapsTransport {
    fn drop(&mut self) {
        self.request_tx.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

struct NativeServiceConfig {
    auth_id: String,
    uuid_state_path: PathBuf,
    snapshots_base_dir: PathBuf,
    backend_url: String,
}

#[derive(Default)]
struct NativeService {
    config: Option<NativeServiceConfig>,
    uuids: BTreeMap<String, String>,
    tracked: Vec<CsnapsTrackedCodebase>,
    reconciled: bool,
}

impl NativeService {
    fn execute(&mut self, operation: &CsnapsOperation) -> Result<Value, CsnapsErrorCode> {
        match operation {
            CsnapsOperation::Ping => Ok(json!({})),
            CsnapsOperation::Initialize(params) => self.initialize(params),
            CsnapsOperation::ApplyCodebaseSpecs(params) => self.apply(params),
            CsnapsOperation::Snapshot(params) => self.snapshot(params),
            CsnapsOperation::TriggerUpload(params) => self.upload(params, false),
            CsnapsOperation::FlushPendingUploads(params) => self.upload(params, true),
            CsnapsOperation::Shutdown => Ok(json!({})),
        }
    }

    fn initialize(&mut self, params: &Map<String, Value>) -> Result<Value, CsnapsErrorCode> {
        if self.config.is_some() { return Err(CsnapsErrorCode::AlreadyInitialized); }
        if !exact_keys(params, &[
            "authId", "backendUrl", "codebaseUuidStatePath", "environment", "snapshotsBaseDir",
        ]) {
            return Err(CsnapsErrorCode::InvalidParams);
        }
        let auth_id = nonempty_string(params.get("authId")).ok_or(CsnapsErrorCode::InvalidParams)?;
        let backend_url = nonempty_string(params.get("backendUrl")).ok_or(CsnapsErrorCode::InvalidConfiguration)?;
        let uuid_state_path = nonempty_string(params.get("codebaseUuidStatePath"))
            .map(PathBuf::from).ok_or(CsnapsErrorCode::InvalidConfiguration)?;
        let snapshots_base_dir = nonempty_string(params.get("snapshotsBaseDir"))
            .map(PathBuf::from).ok_or(CsnapsErrorCode::InvalidConfiguration)?;
        if params.get("environment").and_then(Value::as_f64) != Some(1.0) {
            return Err(CsnapsErrorCode::CodebaseEnvironmentConflict);
        }
        if let Some(parent) = uuid_state_path.parent() {
            fs::create_dir_all(parent).map_err(|_| CsnapsErrorCode::StateIo)?;
        }
        fs::create_dir_all(snapshots_base_dir.join("pending")).map_err(|_| CsnapsErrorCode::StateIo)?;
        self.uuids = load_uuid_state(&uuid_state_path)?;
        self.config = Some(NativeServiceConfig {
            auth_id, uuid_state_path, snapshots_base_dir, backend_url,
        });
        Ok(json!({"state": self.state_value()}))
    }

    fn apply(&mut self, params: &Map<String, Value>) -> Result<Value, CsnapsErrorCode> {
        let uuid_state_path = self.config.as_ref().ok_or(CsnapsErrorCode::NotInitialized)?
            .uuid_state_path.clone();
        if !exact_keys(params, &["codebases"]) {
            return Err(CsnapsErrorCode::InvalidParams);
        }
        let codebases = params.get("codebases").and_then(Value::as_array)
            .ok_or(CsnapsErrorCode::InvalidParams)?;
        let mut tracked = Vec::new();
        let mut rejected = Vec::new();
        for raw in codebases {
            let object = raw.as_object().ok_or(CsnapsErrorCode::InvalidParams)?;
            if !exact_keys(object, &["kind", "path"]) {
                return Err(CsnapsErrorCode::InvalidParams);
            }
            let path = object.get("path").and_then(Value::as_str)
                .ok_or(CsnapsErrorCode::InvalidParams)?.to_string();
            let kind = object.get("kind").and_then(Value::as_f64)
                .ok_or(CsnapsErrorCode::InvalidParams)?;
            if !matches!(kind, 1.0 | 2.0) || !Path::new(&path).is_dir() {
                rejected.push(CsnapsRejectedCodebase { path, kind });
                continue;
            }
            let codebase_uuid = self.uuids.entry(path.clone())
                .or_insert_with(|| Uuid::new_v4().to_string()).clone();
            tracked.push(CsnapsTrackedCodebase {
                codebase_uuid, path, kind, environment: 1.0,
            });
        }
        persist_uuid_state(&uuid_state_path, &self.uuids)?;
        self.tracked = tracked;
        self.reconciled = true;
        Ok(json!({
            "state": self.state_value(),
            "rejected": rejected.iter().map(|item| json!({"path":item.path,"kind":item.kind})).collect::<Vec<_>>(),
        }))
    }

    fn snapshot(&mut self, params: &Map<String, Value>) -> Result<Value, CsnapsErrorCode> {
        let config = self.config.as_ref().ok_or(CsnapsErrorCode::NotInitialized)?;
        if !self.reconciled { return Err(CsnapsErrorCode::CodebasesNotReconciled); }
        if !exact_keys(params, &["reason"]) { return Err(CsnapsErrorCode::InvalidParams); }
        let reason = params.get("reason").and_then(Value::as_object)
            .ok_or(CsnapsErrorCode::InvalidParams)?;
        if !exact_keys(reason, &["requestId", "type"]) {
            return Err(CsnapsErrorCode::InvalidParams);
        }
        let reason_type = nonempty_string(reason.get("type")).ok_or(CsnapsErrorCode::InvalidParams)?;
        let request_id = nonempty_string(reason.get("requestId")).ok_or(CsnapsErrorCode::InvalidParams)?;
        let created_at_ms = now_ms();
        let snapshot_id = Uuid::new_v4().to_string();
        let path = config.snapshots_base_dir.join("pending")
            .join(format!("{created_at_ms}-{snapshot_id}.json"));
        let value = json!({
            "version":1,
            "snapshotId":snapshot_id,
            "authId":config.auth_id,
            "backendUrl":config.backend_url,
            "createdAtMs":created_at_ms,
            "reason":{"type":reason_type,"requestId":request_id},
            "tracked":self.tracked.iter().map(|item| json!({
                "codebaseUuid":item.codebase_uuid,
                "path":item.path,
                "kind":item.kind,
                "environment":item.environment,
            })).collect::<Vec<_>>(),
        });
        write_json_atomic(&path, &value).map_err(|_| CsnapsErrorCode::SnapshotFailed)?;
        Ok(json!({}))
    }

    fn upload(&self, params: &Map<String, Value>, flush: bool) -> Result<Value, CsnapsErrorCode> {
        let config = self.config.as_ref().ok_or(CsnapsErrorCode::NotInitialized)?;
        if !exact_keys(params, &["authToken", "requestHeaders"]) {
            return Err(CsnapsErrorCode::InvalidParams);
        }
        let _token = nonempty_string(params.get("authToken")).ok_or(CsnapsErrorCode::InvalidParams)?;
        let headers = params.get("requestHeaders").and_then(Value::as_object)
            .ok_or(CsnapsErrorCode::InvalidParams)?;
        if headers.is_empty() { return Err(CsnapsErrorCode::InvalidParams); }
        sync_pending(&config.snapshots_base_dir.join("pending"), flush)?;
        Ok(json!({}))
    }

    fn state_value(&self) -> Value {
        json!({
            "tracked": self.tracked.iter().map(|item| json!({
                "codebaseUuid":item.codebase_uuid,
                "path":item.path,
                "kind":item.kind,
                "environment":item.environment,
            })).collect::<Vec<_>>()
        })
    }
}

fn native_service_loop(request_rx: mpsc::Receiver<Vec<u8>>, response_tx: mpsc::Sender<ServiceEvent>) {
    let mut service = NativeService::default();
    while let Ok(frame) = request_rx.recv() {
        let request = (|| {
            let (payloads, remainder) = decode_frame_payloads(&frame)?;
            if payloads.len() != 1 || !remainder.is_empty() {
                return Err(super::csnaps_protocol::CsnapsProtocolError::new(
                    "invalid native csnaps request frame"
                ));
            }
            parse_csnaps_request(&payloads[0])
        })();
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                let _ = response_tx.send(ServiceEvent::Terminal(error.to_string()));
                break;
            }
        };
        let is_shutdown = matches!(request.operation, CsnapsOperation::Shutdown);
        let response = match service.execute(&request.operation) {
            Ok(result) => encode_csnaps_success(request.id, result),
            Err(error) => encode_csnaps_error(request.id, error),
        };
        match response {
            Ok(frame) => {
                if response_tx.send(ServiceEvent::Frame(frame)).is_err() { break; }
            }
            Err(error) => {
                let _ = response_tx.send(ServiceEvent::Terminal(error.to_string()));
                break;
            }
        }
        if is_shutdown { break; }
    }
}

fn load_uuid_state(path: &Path) -> Result<BTreeMap<String, String>, CsnapsErrorCode> {
    if !path.exists() { return Ok(BTreeMap::new()); }
    let bytes = fs::read(path).map_err(|_| CsnapsErrorCode::StateIo)?;
    if bytes.is_empty() { return Ok(BTreeMap::new()); }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| CsnapsErrorCode::CorruptUuidState)?;
    let object = value.as_object().ok_or(CsnapsErrorCode::CorruptUuidState)?;
    let mut out = BTreeMap::new();
    for (path, uuid) in object {
        let uuid = uuid.as_str().filter(|value| is_uuid_v4(value))
            .ok_or(CsnapsErrorCode::CorruptUuidState)?;
        out.insert(path.clone(), uuid.to_string());
    }
    Ok(out)
}

fn persist_uuid_state(path: &Path, uuids: &BTreeMap<String, String>) -> Result<(), CsnapsErrorCode> {
    let value = Value::Object(uuids.iter().map(|(path, uuid)| {
        (path.clone(), Value::String(uuid.clone()))
    }).collect());
    write_json_atomic(path, &value).map_err(|_| CsnapsErrorCode::StateIo)
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    fs::write(&tmp, bytes).map_err(|error| error.to_string())?;
    File::open(&tmp).and_then(|file| file.sync_all()).map_err(|error| error.to_string())?;
    fs::rename(&tmp, path).map_err(|error| error.to_string())?;
    Ok(())
}

fn sync_pending(path: &Path, _flush: bool) -> Result<(), CsnapsErrorCode> {
    fs::create_dir_all(path).map_err(|_| CsnapsErrorCode::StateIo)?;
    let entries = fs::read_dir(path).map_err(|_| CsnapsErrorCode::StateIo)?;
    for entry in entries {
        let entry = entry.map_err(|_| CsnapsErrorCode::StateIo)?;
        if entry.file_type().map_err(|_| CsnapsErrorCode::StateIo)?.is_file() {
            File::open(entry.path()).and_then(|file| file.sync_all())
                .map_err(|_| CsnapsErrorCode::StateIo)?;
        }
    }
    Ok(())
}

fn exact_keys(object: &Map<String, Value>, keys: &[&str]) -> bool {
    object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value?.as_str().map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
        .as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "fabushi-native-csnaps-{name}-{}", Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn native_transport_handshakes_reconciles_snapshots_flushes_and_closes() {
        let root = temp_root("roundtrip");
        let codebase = root.join("codebase");
        fs::create_dir_all(&codebase).unwrap();
        let spawned = NativeCsnapsTransport::spawn(
            &root.join("runtime"), "auth", "https://backend.invalid", NativeCsnapsDeadlines::default()
        ).unwrap();
        assert!(spawned.initial_state.tracked.is_empty());
        let (state, rejected) = spawned.handle.apply_codebase_specs(vec![json!({
            "path":codebase.to_string_lossy(),"kind":1
        })]).unwrap();
        assert!(rejected.is_empty());
        assert_eq!(state.tracked.len(), 1);
        assert!(is_uuid_v4(&state.tracked[0].codebase_uuid));
        spawned.handle.snapshot(json!({
            "type":"AGENT_REQUEST_START","requestId":"req"
        })).unwrap();
        let credentials = Map::from_iter([
            ("authToken".into(), json!("secret")),
            ("requestHeaders".into(), json!({"x-cursor-client-type":"sand"})),
        ]);
        spawned.handle.trigger_upload(credentials.clone()).unwrap();
        spawned.handle.flush_pending_uploads(credentials).unwrap();
        spawned.handle.close().unwrap();
        assert!(root.join("runtime/codebase-uuids.json").exists());
        assert!(fs::read_dir(root.join("runtime/snapshots/pending")).unwrap().next().is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn required_codebase_rejection_is_reported_without_false_state() {
        let root = temp_root("reject");
        let spawned = NativeCsnapsTransport::spawn(
            &root, "auth", "https://backend.invalid", NativeCsnapsDeadlines::default()
        ).unwrap();
        let (state, rejected) = spawned.handle.apply_codebase_specs(vec![json!({
            "path":root.join("missing").to_string_lossy(),"kind":1
        })]).unwrap();
        assert!(state.tracked.is_empty());
        assert_eq!(rejected.len(), 1);
        spawned.handle.close().unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn terminal_state_blocks_followup_requests() {
        let root = temp_root("terminal");
        let spawned = NativeCsnapsTransport::spawn(
            &root, "auth", "https://backend.invalid", NativeCsnapsDeadlines::default()
        ).unwrap();
        spawned.handle.mark_terminal("boom".into());
        assert!(spawned.handle.snapshot(json!({
            "type":"AGENT_REQUEST_START","requestId":"req"
        })).is_err());
        spawned.handle.teardown();
        let _ = fs::remove_dir_all(root);
    }
}
