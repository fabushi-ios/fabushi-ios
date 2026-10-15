use super::csnaps_protocol::{CsnapsResponseTracker, encode_frame};
use super::privacy_mode::{
    CodebaseTelemetryPrivacyMode, PrivacyLookupError, PrivacyMode, PrivacyModeLoader, TelemetryAuth,
};
use super::codebase_snapshot_trigger::{CodebaseSnapshotTrigger, SnapshotReason, SnapshotReasonType};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const CODEBASE_TELEMETRY_FEATURE_GATE: &str = "sand_codebase_telemetry";
pub const CODEBASE_TELEMETRY_UPLOAD_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const CODEBASE_TELEMETRY_RESTART_DELAY: Duration = Duration::from_secs(30);
pub const CODEBASE_TELEMETRY_OPERATION_DEADLINE: Duration = Duration::from_secs(30);

pub trait CodebaseTelemetryHostBridge: Send + Sync {
    fn request(&self, method: &str) -> Result<Value, String>;
    fn lookup_privacy_mode(&self, auth: TelemetryAuth) -> Result<PrivacyMode, PrivacyLookupError>;
}

struct InProcessCsnapsTransport {
    journal: PathBuf,
    tracker: CsnapsResponseTracker,
    next_id: AtomicU64,
    terminal: Mutex<Option<String>>,
    killed: AtomicBool,
    closed: AtomicBool,
}

impl InProcessCsnapsTransport {
    fn open(root: &Path) -> Result<Arc<Self>, String> {
        fs::create_dir_all(root).map_err(|e| format!("create codebase telemetry root: {e}"))?;
        Ok(Arc::new(Self {
            journal: root.join("csnaps.frames"),
            tracker: CsnapsResponseTracker::default(),
            next_id: AtomicU64::new(1),
            terminal: Mutex::new(None),
            killed: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }))
    }

    fn terminal_failure(&self) -> Option<String> {
        self.terminal.lock().unwrap_or_else(|p| p.into_inner()).clone()
            .or_else(|| self.tracker.terminal_failure())
    }

    fn mark_terminal(&self, error: impl Into<String>) {
        let mut slot = self.terminal.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() { *slot = Some(error.into()); }
        self.killed.store(true, Ordering::SeqCst);
    }

    fn kill(&self) {
        self.mark_terminal("csnaps transport killed after terminal operation failure");
    }

    fn operation(&self, method: &str, params: Value) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) || self.killed.load(Ordering::Acquire) {
            return Err(self.terminal_failure().unwrap_or_else(|| "csnaps transport is closed".into()));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.tracker.register(id).map_err(|e| e.to_string())?;
        let payload = serde_json::to_vec(&json!({"id":id,"method":method,"params":params}))
            .map_err(|e| e.to_string())?;
        let frame = encode_frame(&payload).map_err(|e| e.to_string())?;
        let mut file = OpenOptions::new().create(true).append(true).open(&self.journal)
            .map_err(|e| format!("open csnaps journal: {e}"))?;
        file.write_all(&frame).map_err(|e| format!("write csnaps journal: {e}"))?;
        file.flush().map_err(|e| format!("flush csnaps journal: {e}"))?;
        self.tracker.settle(id).map_err(|e| e.to_string())
    }

    fn apply_required_codebases(&self, codebases: &[PathBuf]) -> Result<(), String> {
        if codebases.is_empty() || codebases.iter().any(|path| !path.is_dir()) {
            self.mark_terminal("csnaps rejected a required codebase");
            return Err("csnaps rejected a required codebase".into());
        }
        self.operation("apply_codebase_specs", json!({
            "codebases": codebases.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>()
        }))
    }

    fn close(&self) -> Result<(), String> {
        if self.closed.load(Ordering::Acquire) { return Ok(()); }
        let result = if self.killed.load(Ordering::Acquire) {
            Ok(())
        } else {
            self.operation("shutdown", json!({}))
        };
        self.closed.store(true, Ordering::SeqCst);
        result
    }
}

struct Adapter {
    transport: Arc<InProcessCsnapsTransport>,
    auth: TelemetryAuth,
    stop: Arc<(Mutex<bool>, Condvar)>,
    poller: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

impl Adapter {
    fn start(transport: Arc<InProcessCsnapsTransport>, auth: TelemetryAuth, required: Vec<PathBuf>, interval: Duration)
        -> Result<Arc<Self>, String>
    {
        transport.apply_required_codebases(&required)?;
        let adapter = Arc::new(Self {
            transport,
            auth,
            stop: Arc::new((Mutex::new(false), Condvar::new())),
            poller: Mutex::new(None),
            closed: AtomicBool::new(false),
        });
        adapter.start_upload_poll(interval);
        Ok(adapter)
    }

    fn snapshot(&self, reason: &SnapshotReason) -> Result<(), String> {
        self.run_deadlined("snapshot", json!({
            "type": reason.reason_type.as_str(),
            "requestId": reason.request_id,
        }), CODEBASE_TELEMETRY_OPERATION_DEADLINE)
    }

    fn flush(&self) -> Result<(), String> {
        self.run_deadlined("flush_pending_uploads", json!({
            "authId": self.auth.auth_id,
            "authTokenPresent": !self.auth.auth_token.is_empty(),
            "machineId": self.auth.machine_id,
        }), CODEBASE_TELEMETRY_OPERATION_DEADLINE)
    }

    fn run_deadlined(&self, method: &'static str, params: Value, deadline: Duration) -> Result<(), String> {
        let transport = Arc::clone(&self.transport);
        let (send, receive) = mpsc::sync_channel(1);
        let _ = thread::Builder::new().name(format!("ios-csnaps-{method}")).spawn(move || {
            let _ = send.send(transport.operation(method, params));
        });
        match receive.recv_timeout(deadline) {
            Ok(result) => result,
            Err(_) => {
                self.transport.kill();
                Err(format!("csnaps {method} deadline exceeded; transport entered terminal state"))
            }
        }
    }

    fn terminal_failure(&self) -> Option<String> { self.transport.terminal_failure() }

    fn start_upload_poll(self: &Arc<Self>, interval: Duration) {
        let weak = Arc::downgrade(self);
        let stop = Arc::clone(&self.stop);
        *self.poller.lock().unwrap_or_else(|p| p.into_inner()) =
            thread::Builder::new().name("ios-codebase-upload-poll".into()).spawn(move || loop {
                let (flag, wake) = &*stop;
                let guard = flag.lock().unwrap_or_else(|p| p.into_inner());
                let (guard, wait) = wake.wait_timeout(guard, interval).unwrap_or_else(|p| p.into_inner());
                if *guard { break; }
                drop(guard);
                if wait.timed_out() {
                    let Some(adapter) = weak.upgrade() else { break; };
                    if adapter.terminal_failure().is_some() { break; }
                    let _ = adapter.transport.operation("trigger_upload", json!({
                        "authId": adapter.auth.auth_id,
                        "authTokenPresent": !adapter.auth.auth_token.is_empty(),
                        "machineId": adapter.auth.machine_id,
                    }));
                }
            }).ok();
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) { return; }
        let _ = self.flush();
        {
            let (flag, wake) = &*self.stop;
            *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
            wake.notify_all();
        }
        if let Some(handle) = self.poller.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = handle.join();
        }
        let _ = self.transport.close();
    }
}

impl Drop for Adapter { fn drop(&mut self) { self.close(); } }

struct OwnerState {
    auth_id: Option<String>,
    gate_enabled: bool,
    adapter: Option<Arc<Adapter>>,
}

pub struct MobileCodebaseTelemetryOwner {
    bridge: Arc<dyn CodebaseTelemetryHostBridge>,
    privacy: Arc<CodebaseTelemetryPrivacyMode>,
    trigger: Arc<CodebaseSnapshotTrigger>,
    root: PathBuf,
    required_codebases: Vec<PathBuf>,
    state: Mutex<OwnerState>,
    stop: Arc<(Mutex<bool>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
    restart_delay: Duration,
}

impl MobileCodebaseTelemetryOwner {
    pub fn start(app_data_dir: &Path, bridge: Arc<dyn CodebaseTelemetryHostBridge>) -> Result<Arc<Self>, String> {
        Self::with_policy(app_data_dir, bridge, CODEBASE_TELEMETRY_UPLOAD_INTERVAL, CODEBASE_TELEMETRY_RESTART_DELAY)
    }

    fn with_policy(
        app_data_dir: &Path,
        bridge: Arc<dyn CodebaseTelemetryHostBridge>,
        upload_interval: Duration,
        restart_delay: Duration,
    ) -> Result<Arc<Self>, String> {
        let root = app_data_dir.join("codebase-telemetry");
        fs::create_dir_all(&root).map_err(|e| format!("create iOS codebase telemetry directory: {e}"))?;
        let privacy_bridge = Arc::clone(&bridge);
        let loader: PrivacyModeLoader = Arc::new(move |auth| privacy_bridge.lookup_privacy_mode(auth));
        let owner = Arc::new(Self {
            bridge,
            privacy: Arc::new(CodebaseTelemetryPrivacyMode::new(loader)),
            trigger: Arc::new(CodebaseSnapshotTrigger::default()),
            root,
            required_codebases: vec![app_data_dir.to_path_buf()],
            state: Mutex::new(OwnerState { auth_id: None, gate_enabled: false, adapter: None }),
            stop: Arc::new((Mutex::new(false), Condvar::new())),
            worker: Mutex::new(None),
            restart_delay,
        });
        owner.sync(upload_interval)?;
        owner.start_worker(upload_interval);
        Ok(owner)
    }

    pub fn observe_host_dispatch(&self, input: &str, output: &str) {
        let request = serde_json::from_str::<Value>(input).ok();
        let method = request.as_ref().and_then(|v| v.get("method")).and_then(Value::as_str);
        if matches!(method, Some("feature.auth.logout" | "feature.auth.passwordLogin" | "feature.auth.browserPoll" | "setHostSettings")) {
            let _ = self.sync(CODEBASE_TELEMETRY_UPLOAD_INTERVAL);
        }
        let succeeded = serde_json::from_str::<Value>(output).ok()
            .and_then(|v| v.get("ok").and_then(Value::as_bool)) == Some(true);
        if !succeeded { return; }
        if matches!(method, Some("sendPrompt" | "feature.execute")) {
            let request_id = request.as_ref()
                .and_then(|v| v.get("id").or_else(|| v.get("params").and_then(|p| p.get("requestId"))))
                .and_then(Value::as_str)
                .unwrap_or("host-turn")
                .to_string();
            self.snapshot(SnapshotReason { reason_type: SnapshotReasonType::AgentRequestStart, request_id: request_id.clone() });
            self.snapshot(SnapshotReason { reason_type: SnapshotReasonType::AgentRequestEnd, request_id });
        }
    }

    pub fn privacy_mode(&self) -> PrivacyMode { self.privacy.get() }

    pub fn stop(&self) {
        {
            let (flag, wake) = &*self.stop;
            *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
            wake.notify_all();
        }
        if let Some(handle) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = handle.join();
        }
        let adapter = self.state.lock().unwrap_or_else(|p| p.into_inner()).adapter.take();
        if let Some(adapter) = adapter { adapter.close(); }
        self.privacy.clear_auth();
        self.privacy.dispose();
    }

    fn snapshot(&self, reason: SnapshotReason) {
        if !self.trigger.claim(&reason) { return; }
        let adapter = self.state.lock().unwrap_or_else(|p| p.into_inner()).adapter.clone();
        let Some(adapter) = adapter else {
            self.trigger.release(&reason);
            return;
        };
        if adapter.snapshot(&reason).is_err() {
            self.trigger.release(&reason);
        }
    }

    fn sync(&self, upload_interval: Duration) -> Result<(), String> {
        let settings = self.bridge.request("getHostSettings")?;
        let gate_enabled = settings.get("featureFlagOverrides")
            .and_then(|v| v.get(CODEBASE_TELEMETRY_FEATURE_GATE))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !gate_enabled {
            self.privacy.clear_auth();
            let adapter = {
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.gate_enabled = false;
                state.auth_id = None;
                state.adapter.take()
            };
            if let Some(adapter) = adapter { adapter.close(); }
            return Ok(());
        }

        let session = match self.bridge.request("feature.auth.deviceAgentSession") {
            Ok(value) => value,
            Err(_) => {
                self.privacy.clear_auth();
                let adapter = self.state.lock().unwrap_or_else(|p| p.into_inner()).adapter.take();
                if let Some(adapter) = adapter { adapter.close(); }
                return Ok(());
            }
        };
        let auth_id = session.get("userId").and_then(Value::as_str)
            .or_else(|| session.get("sessionId").and_then(Value::as_str))
            .ok_or_else(|| "codebase telemetry auth session missing userId/sessionId".to_string())?.to_string();
        let auth_token = session.get("accessToken").and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "codebase telemetry auth session missing accessToken".to_string())?.to_string();
        let machine_id = session.get("deviceId").and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "codebase telemetry auth session missing deviceId".to_string())?.to_string();
        let auth = TelemetryAuth { auth_id: auth_id.clone(), auth_token, machine_id };
        self.privacy.set_auth(auth.clone());

        let replace = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.auth_id.as_deref() != Some(auth_id.as_str())
                || state.adapter.as_ref().is_some_and(|a| a.terminal_failure().is_some())
                || state.adapter.is_none()
        };
        if replace {
            let transport = InProcessCsnapsTransport::open(&self.root)?;
            let adapter = Adapter::start(transport, auth, self.required_codebases.clone(), upload_interval)?;
            let previous = {
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.gate_enabled = true;
                state.auth_id = Some(auth_id);
                state.adapter.replace(adapter)
            };
            if let Some(previous) = previous { previous.close(); }
        }
        Ok(())
    }

    fn start_worker(self: &Arc<Self>, upload_interval: Duration) {
        let weak = Arc::downgrade(self);
        let stop = Arc::clone(&self.stop);
        let restart_delay = self.restart_delay;
        *self.worker.lock().unwrap_or_else(|p| p.into_inner()) =
            thread::Builder::new().name("ios-codebase-telemetry-owner".into()).spawn(move || loop {
                let (flag, wake) = &*stop;
                let guard = flag.lock().unwrap_or_else(|p| p.into_inner());
                let (guard, _) = wake.wait_timeout(guard, Duration::from_millis(250))
                    .unwrap_or_else(|p| p.into_inner());
                if *guard { break; }
                drop(guard);
                let Some(owner) = weak.upgrade() else { break; };
                let terminal = owner.state.lock().unwrap_or_else(|p| p.into_inner()).adapter
                    .as_ref().is_some_and(|a| a.terminal_failure().is_some());
                if terminal {
                    thread::sleep(restart_delay);
                    let _ = owner.sync(upload_interval);
                }
            }).ok();
    }
}

impl Drop for MobileCodebaseTelemetryOwner { fn drop(&mut self) { self.stop(); } }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeBridge {
        session: Mutex<Option<Value>>,
        gate: AtomicBool,
        privacy_calls: AtomicUsize,
    }
    impl CodebaseTelemetryHostBridge for FakeBridge {
        fn request(&self, method: &str) -> Result<Value, String> {
            match method {
                "getHostSettings" => Ok(json!({"featureFlagOverrides": {
                    CODEBASE_TELEMETRY_FEATURE_GATE: self.gate.load(Ordering::SeqCst)
                }})),
                "feature.auth.deviceAgentSession" => self.session.lock().unwrap().clone().ok_or_else(|| "signed out".into()),
                _ => Err("unsupported".into()),
            }
        }
        fn lookup_privacy_mode(&self, _auth: TelemetryAuth) -> Result<PrivacyMode, PrivacyLookupError> {
            self.privacy_calls.fetch_add(1, Ordering::SeqCst);
            Ok(PrivacyMode::UsageCodebaseTrainingAllowed)
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("fabushi-codebase-telemetry-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn production_owner_is_auth_and_experiments_fenced_and_restart_durable() {
        let root = temp_root("owner");
        let bridge = Arc::new(FakeBridge {
            session: Mutex::new(Some(json!({
                "userId":"user-a","sessionId":"s-a","accessToken":"token-a","deviceId":"machine-a"
            }))),
            gate: AtomicBool::new(true),
            privacy_calls: AtomicUsize::new(0),
        });
        let owner = MobileCodebaseTelemetryOwner::with_policy(
            &root, bridge.clone(), Duration::from_millis(5), Duration::from_millis(5)
        ).unwrap();
        thread::sleep(Duration::from_millis(20));
        assert!(bridge.privacy_calls.load(Ordering::SeqCst) >= 1);
        assert!(root.join("codebase-telemetry/csnaps.frames").exists());

        bridge.gate.store(false, Ordering::SeqCst);
        owner.sync(Duration::from_millis(5)).unwrap();
        assert_eq!(owner.privacy_mode(), PrivacyMode::Unspecified);
        owner.stop();

        let restarted = MobileCodebaseTelemetryOwner::with_policy(
            &root, bridge.clone(), Duration::from_millis(5), Duration::from_millis(5)
        ).unwrap();
        assert!(root.join("codebase-telemetry/csnaps.frames").exists());
        restarted.stop();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn required_codebase_rejection_is_fail_closed() {
        let root = temp_root("reject");
        let transport = InProcessCsnapsTransport::open(&root).unwrap();
        let missing = root.join("missing");
        assert!(transport.apply_required_codebases(&[missing]).is_err());
        assert!(transport.terminal_failure().is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn timeout_marks_transport_terminal_and_kills_it() {
        let root = temp_root("timeout");
        let transport = InProcessCsnapsTransport::open(&root).unwrap();
        transport.kill();
        assert!(transport.operation("snapshot", json!({})).is_err());
        assert!(transport.terminal_failure().is_some());
        let _ = fs::remove_dir_all(root);
    }
}
