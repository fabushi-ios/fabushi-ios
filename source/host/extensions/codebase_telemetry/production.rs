use super::codebase_snapshot_trigger::{CodebaseSnapshotTrigger, SnapshotReason, SnapshotReasonType};
use super::cursor_backend::{configured_backend_url, sand_client_version, sand_namespace};
use super::native_csnaps::{NativeCsnapsDeadlines, NativeCsnapsTransport};
use super::privacy_mode::{
    CodebaseTelemetryPrivacyMode, PrivacyLookupError, PrivacyMode, PrivacyModeLoader, TelemetryAuth,
};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const CODEBASE_TELEMETRY_FEATURE_GATE: &str = "sand_codebase_telemetry";
pub const CODEBASE_TELEMETRY_UPLOAD_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const CODEBASE_TELEMETRY_RESTART_DELAY: Duration = Duration::from_secs(30);

pub trait CodebaseTelemetryHostBridge: Send + Sync {
    fn request(&self, method: &str) -> Result<Value, String>;
    fn feature_gate(&self, name: &str) -> Option<bool>;
    fn lookup_privacy_mode(&self, auth: TelemetryAuth) -> Result<PrivacyMode, PrivacyLookupError>;
}

struct Adapter {
    transport: Arc<NativeCsnapsTransport>,
    auth: TelemetryAuth,
    stop: Arc<(Mutex<bool>, Condvar)>,
    poller: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

impl Adapter {
    fn start(
        root: &Path,
        auth: TelemetryAuth,
        required: Vec<PathBuf>,
        interval: Duration,
    ) -> Result<Arc<Self>, String> {
        let spawned = NativeCsnapsTransport::spawn(
            root,
            &auth.auth_id,
            &configured_backend_url(),
            NativeCsnapsDeadlines::default(),
        )
        .map_err(|error| error.to_string())?;
        let specs = required
            .iter()
            .map(|path| json!({"path":path.to_string_lossy(),"kind":1}))
            .collect::<Vec<_>>();
        let (state, rejected) = spawned
            .handle
            .apply_codebase_specs(specs)
            .map_err(|error| error.to_string())?;
        if !rejected.is_empty()
            || state.tracked.len() != required.len()
            || state.tracked.iter().any(|codebase| codebase.environment != 1.0)
        {
            let _ = spawned.handle.close();
            return Err("csnaps rejected a required codebase".into());
        }

        let adapter = Arc::new(Self {
            transport: spawned.handle,
            auth,
            stop: Arc::new((Mutex::new(false), Condvar::new())),
            poller: Mutex::new(None),
            closed: AtomicBool::new(false),
        });
        adapter.start_upload_poll(interval);
        Ok(adapter)
    }

    fn snapshot(&self, reason: &SnapshotReason) -> Result<(), String> {
        self.transport
            .snapshot(json!({
                "type": reason.reason_type.as_str(),
                "requestId": reason.request_id,
            }))
            .map_err(|error| error.to_string())
    }

    fn upload_credentials(&self) -> Map<String, Value> {
        Map::from_iter([
            ("authToken".into(), json!(self.auth.auth_token)),
            (
                "requestHeaders".into(),
                json!({
                    "x-cursor-client-type":"sand",
                    "x-cursor-client-version":sand_client_version(),
                    "x-sand-box-namespace":sand_namespace(),
                    "x-ghost-mode":"false",
                    "x-device-id":self.auth.machine_id,
                }),
            ),
        ])
    }

    fn flush(&self) -> Result<(), String> {
        self.transport
            .flush_pending_uploads(self.upload_credentials())
            .map_err(|error| error.to_string())
    }

    fn terminal_failure(&self) -> Option<String> {
        self.transport.terminal_failure()
    }

    fn start_upload_poll(self: &Arc<Self>, interval: Duration) {
        let weak = Arc::downgrade(self);
        let stop = Arc::clone(&self.stop);
        *self.poller.lock().unwrap_or_else(|p| p.into_inner()) =
            thread::Builder::new()
                .name("ios-codebase-upload-poll".into())
                .spawn(move || loop {
                    let (flag, wake) = &*stop;
                    let guard = flag.lock().unwrap_or_else(|p| p.into_inner());
                    let (guard, waited) = wake
                        .wait_timeout(guard, interval)
                        .unwrap_or_else(|p| p.into_inner());
                    if *guard {
                        break;
                    }
                    drop(guard);
                    if !waited.timed_out() {
                        continue;
                    }
                    let Some(adapter) = weak.upgrade() else {
                        break;
                    };
                    if adapter.terminal_failure().is_some() {
                        break;
                    }
                    let _ = adapter
                        .transport
                        .trigger_upload(adapter.upload_credentials());
                })
                .ok();
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
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

impl Drop for Adapter {
    fn drop(&mut self) {
        self.close();
    }
}

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
    pub fn start(
        app_data_dir: &Path,
        bridge: Arc<dyn CodebaseTelemetryHostBridge>,
    ) -> Result<Arc<Self>, String> {
        Self::with_policy(
            app_data_dir,
            bridge,
            CODEBASE_TELEMETRY_UPLOAD_INTERVAL,
            CODEBASE_TELEMETRY_RESTART_DELAY,
        )
    }

    fn with_policy(
        app_data_dir: &Path,
        bridge: Arc<dyn CodebaseTelemetryHostBridge>,
        upload_interval: Duration,
        restart_delay: Duration,
    ) -> Result<Arc<Self>, String> {
        let root = app_data_dir.join("codebase-telemetry");
        fs::create_dir_all(&root)
            .map_err(|error| format!("create iOS codebase telemetry directory: {error}"))?;
        let privacy_bridge = Arc::clone(&bridge);
        let loader: PrivacyModeLoader =
            Arc::new(move |auth| privacy_bridge.lookup_privacy_mode(auth));
        let owner = Arc::new(Self {
            bridge,
            privacy: Arc::new(CodebaseTelemetryPrivacyMode::new(loader)),
            trigger: Arc::new(CodebaseSnapshotTrigger::default()),
            root,
            required_codebases: vec![app_data_dir.to_path_buf()],
            state: Mutex::new(OwnerState {
                auth_id: None,
                gate_enabled: false,
                adapter: None,
            }),
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
        let method = request
            .as_ref()
            .and_then(|value| value.get("method"))
            .and_then(Value::as_str);
        if matches!(
            method,
            Some(
                "feature.auth.logout"
                    | "feature.auth.passwordLogin"
                    | "feature.auth.browserPoll"
                    | "setHostSettings"
            )
        ) {
            let _ = self.sync(CODEBASE_TELEMETRY_UPLOAD_INTERVAL);
        }

        let succeeded = serde_json::from_str::<Value>(output)
            .ok()
            .and_then(|value| value.get("ok").and_then(Value::as_bool))
            == Some(true);
        if !succeeded {
            return;
        }
        if matches!(method, Some("sendPrompt" | "feature.execute")) {
            let request_id = request
                .as_ref()
                .and_then(|value| {
                    value
                        .get("id")
                        .or_else(|| value.get("params").and_then(|params| params.get("requestId")))
                })
                .and_then(Value::as_str)
                .unwrap_or("host-turn")
                .to_string();
            self.snapshot(SnapshotReason {
                reason_type: SnapshotReasonType::AgentRequestStart,
                request_id: request_id.clone(),
            });
            self.snapshot(SnapshotReason {
                reason_type: SnapshotReasonType::AgentRequestEnd,
                request_id,
            });
        }
    }

    pub fn privacy_mode(&self) -> PrivacyMode {
        self.privacy.get()
    }

    pub fn stop(&self) {
        {
            let (flag, wake) = &*self.stop;
            *flag.lock().unwrap_or_else(|p| p.into_inner()) = true;
            wake.notify_all();
        }
        if let Some(handle) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = handle.join();
        }
        let adapter = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .adapter
            .take();
        if let Some(adapter) = adapter {
            adapter.close();
        }
        self.privacy.clear_auth();
        self.privacy.dispose();
    }

    fn snapshot(&self, reason: SnapshotReason) {
        if !self.trigger.claim(&reason) {
            return;
        }
        let adapter = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .adapter
            .clone();
        let Some(adapter) = adapter else {
            self.trigger.release(&reason);
            return;
        };
        if adapter.snapshot(&reason).is_err() {
            self.trigger.release(&reason);
        }
    }

    pub fn refresh_runtime_dependencies(&self) -> Result<(), String> {
        self.sync(CODEBASE_TELEMETRY_UPLOAD_INTERVAL)
    }

    fn sync(&self, upload_interval: Duration) -> Result<(), String> {
        let gate_enabled = self
            .bridge
            .feature_gate(CODEBASE_TELEMETRY_FEATURE_GATE)
            .unwrap_or(false);
        if !gate_enabled {
            self.privacy.clear_auth();
            let adapter = {
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.gate_enabled = false;
                state.auth_id = None;
                state.adapter.take()
            };
            if let Some(adapter) = adapter {
                adapter.close();
            }
            return Ok(());
        }

        let session = match self.bridge.request("feature.auth.deviceAgentSession") {
            Ok(value) => value,
            Err(_) => {
                self.privacy.clear_auth();
                let adapter = self
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .adapter
                    .take();
                if let Some(adapter) = adapter {
                    adapter.close();
                }
                return Ok(());
            }
        };

        let auth_id = session
            .get("userId")
            .and_then(Value::as_str)
            .or_else(|| session.get("sessionId").and_then(Value::as_str))
            .ok_or_else(|| "codebase telemetry auth session missing userId/sessionId".to_string())?
            .to_string();
        let auth_token = session
            .get("accessToken")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "codebase telemetry auth session missing accessToken".to_string())?
            .to_string();
        let machine_id = session
            .get("deviceId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "codebase telemetry auth session missing deviceId".to_string())?
            .to_string();
        let auth = TelemetryAuth {
            auth_id: auth_id.clone(),
            auth_token,
            machine_id,
        };
        self.privacy.set_auth(auth.clone());

        let replace = {
            let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.auth_id.as_deref() != Some(auth_id.as_str())
                || state
                    .adapter
                    .as_ref()
                    .is_some_and(|adapter| adapter.terminal_failure().is_some())
                || state.adapter.is_none()
        };
        if replace {
            let adapter = Adapter::start(
                &self.root,
                auth,
                self.required_codebases.clone(),
                upload_interval,
            )?;
            let previous = {
                let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
                state.gate_enabled = true;
                state.auth_id = Some(auth_id);
                state.adapter.replace(adapter)
            };
            if let Some(previous) = previous {
                previous.close();
            }
        }
        Ok(())
    }

    fn start_worker(self: &Arc<Self>, upload_interval: Duration) {
        let weak = Arc::downgrade(self);
        let stop = Arc::clone(&self.stop);
        let restart_delay = self.restart_delay;
        *self.worker.lock().unwrap_or_else(|p| p.into_inner()) =
            thread::Builder::new()
                .name("ios-codebase-telemetry-owner".into())
                .spawn(move || loop {
                    let (flag, wake) = &*stop;
                    let guard = flag.lock().unwrap_or_else(|p| p.into_inner());
                    let (guard, _) = wake
                        .wait_timeout(guard, Duration::from_millis(250))
                        .unwrap_or_else(|p| p.into_inner());
                    if *guard {
                        break;
                    }
                    drop(guard);
                    let Some(owner) = weak.upgrade() else {
                        break;
                    };
                    let terminal = owner
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .adapter
                        .as_ref()
                        .is_some_and(|adapter| adapter.terminal_failure().is_some());
                    if terminal {
                        let previous = owner
                            .state
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .adapter
                            .take();
                        if let Some(previous) = previous {
                            previous.close();
                        }
                        thread::sleep(restart_delay);
                        let _ = owner.sync(upload_interval);
                    }
                })
                .ok();
    }
}

impl Drop for MobileCodebaseTelemetryOwner {
    fn drop(&mut self) {
        self.stop();
    }
}

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
                "feature.auth.deviceAgentSession" => self
                    .session
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| "signed out".into()),
                _ => Err("unsupported".into()),
            }
        }

        fn feature_gate(&self, name: &str) -> Option<bool> {
            (name == CODEBASE_TELEMETRY_FEATURE_GATE)
                .then(|| self.gate.load(Ordering::SeqCst))
        }

        fn lookup_privacy_mode(
            &self,
            _auth: TelemetryAuth,
        ) -> Result<PrivacyMode, PrivacyLookupError> {
            self.privacy_calls.fetch_add(1, Ordering::SeqCst);
            Ok(PrivacyMode::UsageCodebaseTrainingAllowed)
        }
    }

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fabushi-codebase-telemetry-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn production_owner_uses_native_protocol_service_and_is_auth_gate_fenced() {
        let root = temp_root("owner");
        let bridge = Arc::new(FakeBridge {
            session: Mutex::new(Some(json!({
                "userId":"user-a",
                "sessionId":"s-a",
                "accessToken":"token-a",
                "deviceId":"machine-a"
            }))),
            gate: AtomicBool::new(true),
            privacy_calls: AtomicUsize::new(0),
        });
        let owner = MobileCodebaseTelemetryOwner::with_policy(
            &root,
            bridge.clone(),
            Duration::from_millis(5),
            Duration::from_millis(5),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(20));
        assert!(bridge.privacy_calls.load(Ordering::SeqCst) >= 1);
        assert!(root.join("codebase-telemetry/codebase-uuids.json").exists());

        owner.snapshot(SnapshotReason {
            reason_type: SnapshotReasonType::AgentRequestStart,
            request_id: "req".into(),
        });
        assert!(
            fs::read_dir(root.join("codebase-telemetry/snapshots/pending"))
                .unwrap()
                .next()
                .is_some()
        );

        bridge.gate.store(false, Ordering::SeqCst);
        owner.sync(Duration::from_millis(5)).unwrap();
        assert_eq!(owner.privacy_mode(), PrivacyMode::Unspecified);
        owner.stop();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_required_codebase_fails_closed() {
        let root = temp_root("reject");
        let auth = TelemetryAuth {
            auth_id: "user".into(),
            auth_token: "token".into(),
            machine_id: "machine".into(),
        };
        let error = Adapter::start(
            &root.join("runtime"),
            auth,
            vec![root.join("missing")],
            Duration::from_secs(60),
        )
        .err()
        .expect("required codebase must reject");
        assert!(error.contains("rejected a required codebase"));
        let _ = fs::remove_dir_all(root);
    }
}
