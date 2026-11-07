use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use thiserror::Error;

use super::csnaps_process::{
    CsnapsDeadlines, CsnapsProcess, CsnapsProcessError, make_initialize_params,
};
use super::csnaps_protocol::{
    CsnapsRejectedCodebase, CsnapsServiceState, CsnapsTrackedCodebase,
};

pub const CODEBASE_ENVIRONMENT_SAND_BOX: f64 = 1.0;
pub const DEFAULT_UPLOAD_POLL_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct TrackedCodebase {
    pub codebase_uuid: String,
    pub auth_id: String,
    pub path: String,
    pub kind: f64,
    pub environment: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodebaseTelemetryAdapterState {
    pub tracked_codebases: Vec<TrackedCodebase>,
    pub is_within_storage_budget: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct CsnapsAdapterError {
    message: String,
}

impl CsnapsAdapterError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

pub trait CsnapsHandle: Send + Sync {
    fn apply_codebase_specs(
        &self,
        codebases: Vec<Value>,
    ) -> Result<(CsnapsServiceState, Vec<CsnapsRejectedCodebase>), CsnapsProcessError>;
    fn snapshot(&self, reason: Value) -> Result<(), CsnapsProcessError>;
    fn trigger_upload(&self, credentials: Map<String, Value>) -> Result<(), CsnapsProcessError>;
    fn flush_pending_uploads(&self, credentials: Map<String, Value>)
        -> Result<(), CsnapsProcessError>;
    fn close(&self) -> Result<(), CsnapsProcessError>;
    fn terminal_failure(&self) -> Option<CsnapsProcessError>;
}

impl CsnapsHandle for CsnapsProcess {
    fn apply_codebase_specs(
        &self,
        codebases: Vec<Value>,
    ) -> Result<(CsnapsServiceState, Vec<CsnapsRejectedCodebase>), CsnapsProcessError> {
        CsnapsProcess::apply_codebase_specs(self, codebases)
    }

    fn snapshot(&self, reason: Value) -> Result<(), CsnapsProcessError> {
        CsnapsProcess::snapshot(self, reason)
    }

    fn trigger_upload(&self, credentials: Map<String, Value>) -> Result<(), CsnapsProcessError> {
        CsnapsProcess::trigger_upload(self, credentials)
    }

    fn flush_pending_uploads(
        &self,
        credentials: Map<String, Value>,
    ) -> Result<(), CsnapsProcessError> {
        CsnapsProcess::flush_pending_uploads(self, credentials)
    }

    fn close(&self) -> Result<(), CsnapsProcessError> {
        CsnapsProcess::close(self)
    }

    fn terminal_failure(&self) -> Option<CsnapsProcessError> {
        CsnapsProcess::terminal_failure(self)
    }
}

pub type AdapterStateListener = Arc<dyn Fn(CodebaseTelemetryAdapterState) + Send + Sync>;
pub type UploadCredentialsFactory =
    Arc<dyn Fn() -> Result<Map<String, Value>, CsnapsAdapterError> + Send + Sync>;

struct AdapterWatch {
    state: CodebaseTelemetryAdapterState,
    listeners: BTreeMap<u64, AdapterStateListener>,
    next_id: u64,
}

pub struct AdapterSubscription {
    inner: Weak<Mutex<AdapterWatch>>,
    id: u64,
}

impl Drop for AdapterSubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .listeners
                .remove(&self.id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterCloseResult {
    Closed,
    CleanupFailed(Vec<String>),
}

pub struct CsnapsCodebaseTelemetryAdapter {
    csnaps: Arc<dyn CsnapsHandle>,
    auth_id: String,
    state: Arc<Mutex<AdapterWatch>>,
    upload_credentials: UploadCredentialsFactory,
    upload_poll_interval: Duration,
    polling: Mutex<Option<JoinHandle<()>>>,
    stop: Arc<(Mutex<bool>, Condvar)>,
    closed: Arc<AtomicBool>,
    terminal_failure: Arc<Mutex<Option<CsnapsAdapterError>>>,
    terminal_monitor: Mutex<Option<JoinHandle<()>>>,
}

impl CsnapsCodebaseTelemetryAdapter {
    pub fn create(
        auth_id: impl Into<String>,
        codebase_uuid_state_path: impl AsRef<Path>,
        snapshots_base_dir: impl AsRef<Path>,
        backend_url: &str,
        csnaps_bin_path: impl AsRef<Path>,
        upload_credentials: UploadCredentialsFactory,
        upload_poll_interval: Duration,
    ) -> Result<Arc<Self>, CsnapsAdapterError> {
        let auth_id = auth_id.into();
        let initialize_params = make_initialize_params(
            &auth_id,
            &codebase_uuid_state_path.as_ref().to_string_lossy(),
            &snapshots_base_dir.as_ref().to_string_lossy(),
            backend_url,
        );
        let spawned = CsnapsProcess::spawn(
            csnaps_bin_path,
            initialize_params,
            CsnapsDeadlines::default(),
        )
        .map_err(|error| CsnapsAdapterError::new(error.to_string()))?;
        Ok(Self::from_handle(
            spawned.handle,
            auth_id,
            spawned.initial_state,
            upload_credentials,
            upload_poll_interval,
        ))
    }

    pub fn from_handle(
        csnaps: Arc<dyn CsnapsHandle>,
        auth_id: impl Into<String>,
        initial_state: CsnapsServiceState,
        upload_credentials: UploadCredentialsFactory,
        upload_poll_interval: Duration,
    ) -> Arc<Self> {
        let auth_id = auth_id.into();
        let initial = to_adapter_state(&auth_id, &initial_state)
            .unwrap_or(CodebaseTelemetryAdapterState {
                tracked_codebases: Vec::new(),
                is_within_storage_budget: false,
            });
        let adapter = Arc::new(Self {
            csnaps,
            auth_id,
            state: Arc::new(Mutex::new(AdapterWatch {
                state: initial,
                listeners: BTreeMap::new(),
                next_id: 1,
            })),
            upload_credentials,
            upload_poll_interval,
            polling: Mutex::new(None),
            stop: Arc::new((Mutex::new(false), Condvar::new())),
            closed: Arc::new(AtomicBool::new(false)),
            terminal_failure: Arc::new(Mutex::new(None)),
            terminal_monitor: Mutex::new(None),
        });
        adapter.start_terminal_monitor();
        adapter
    }

    pub fn state(&self) -> CodebaseTelemetryAdapterState {
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .state
            .clone()
    }

    pub fn subscribe_state(&self, listener: AdapterStateListener) -> AdapterSubscription {
        let mut watch = self.state.lock().unwrap_or_else(|poison| poison.into_inner());
        let id = watch.next_id;
        watch.next_id = watch.next_id.saturating_add(1);
        watch.listeners.insert(id, listener);
        AdapterSubscription {
            inner: Arc::downgrade(&self.state),
            id,
        }
    }

    pub fn terminal_failure(&self) -> Option<CsnapsAdapterError> {
        self.terminal_failure
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn reconcile_feature_gates(&self, _values: &Value) -> Result<(), CsnapsAdapterError> {
        Ok(())
    }

    pub fn set_desired_codebases(&self, codebases: Vec<Value>) -> Result<(), CsnapsAdapterError> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let (state, rejected) = self
            .csnaps
            .apply_codebase_specs(codebases)
            .map_err(|error| CsnapsAdapterError::new(error.to_string()))?;
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        if !rejected.is_empty() {
            return Err(CsnapsAdapterError::new(
                "csnaps rejected a required codebase",
            ));
        }
        self.publish(to_adapter_state(&self.auth_id, &state)?);
        self.start_upload_polling();
        Ok(())
    }

    pub fn snapshot(&self, reason: Value) -> Result<(), CsnapsAdapterError> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.csnaps
            .snapshot(reason)
            .map_err(|error| CsnapsAdapterError::new(error.to_string()))
    }

    pub fn run_git_history_capture(&self) -> Result<(), CsnapsAdapterError> {
        Ok(())
    }

    pub fn flush_pending_uploads(&self) -> Result<(), CsnapsAdapterError> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let credentials = (self.upload_credentials)()?;
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        self.csnaps
            .flush_pending_uploads(credentials)
            .map_err(|error| CsnapsAdapterError::new(error.to_string()))
    }

    pub fn close(&self) -> AdapterCloseResult {
        if self.closed.swap(true, Ordering::SeqCst) {
            return AdapterCloseResult::Closed;
        }
        {
            let (stop, wake) = &*self.stop;
            *stop.lock().unwrap_or_else(|poison| poison.into_inner()) = true;
            wake.notify_all();
        }
        let mut errors = Vec::new();
        if let Some(handle) = self
            .polling
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            if handle.join().is_err() {
                errors.push("upload polling thread panicked".to_string());
            }
        }
        if let Err(error) = self.csnaps.close() {
            errors.push(error.to_string());
        }
        if let Some(handle) = self
            .terminal_monitor
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            if handle.join().is_err() {
                errors.push("terminal monitor thread panicked".to_string());
            }
        }
        self.state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .listeners
            .clear();
        if errors.is_empty() {
            AdapterCloseResult::Closed
        } else {
            AdapterCloseResult::CleanupFailed(errors)
        }
    }

    fn publish(&self, state: CodebaseTelemetryAdapterState) {
        let listeners = {
            let mut watch = self.state.lock().unwrap_or_else(|poison| poison.into_inner());
            watch.state = state.clone();
            watch.listeners.values().cloned().collect::<Vec<_>>()
        };
        for listener in listeners {
            listener(state.clone());
        }
    }

    fn start_upload_polling(&self) {
        let mut slot = self.polling.lock().unwrap_or_else(|poison| poison.into_inner());
        if slot.is_some() || self.closed.load(Ordering::Acquire) {
            return;
        }
        let interval = self.upload_poll_interval;
        let stop = Arc::clone(&self.stop);
        let closed = Arc::clone(&self.closed);
        let csnaps = Arc::clone(&self.csnaps);
        let upload_credentials = Arc::clone(&self.upload_credentials);
        *slot = thread::Builder::new()
            .name("codebase-telemetry-upload-poll".into())
            .spawn(move || loop {
                let (stopped, wake) = &*stop;
                let guard = stopped.lock().unwrap_or_else(|poison| poison.into_inner());
                let (guard, waited) = wake
                    .wait_timeout(guard, interval)
                    .unwrap_or_else(|poison| poison.into_inner());
                if *guard || closed.load(Ordering::Acquire) {
                    break;
                }
                drop(guard);
                if !waited.timed_out() {
                    continue;
                }
                let Ok(credentials) = upload_credentials() else {
                    continue;
                };
                if closed.load(Ordering::Acquire) {
                    break;
                }
                let _ = csnaps.trigger_upload(credentials);
            })
            .ok();
    }

    fn start_terminal_monitor(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let csnaps = Arc::clone(&self.csnaps);
        let stop = Arc::clone(&self.stop);
        let terminal_failure = Arc::clone(&self.terminal_failure);
        let state = Arc::clone(&self.state);
        let handle = thread::Builder::new()
            .name("codebase-telemetry-terminal-monitor".into())
            .spawn(move || loop {
                let (stopped, wake) = &*stop;
                let guard = stopped.lock().unwrap_or_else(|poison| poison.into_inner());
                let (guard, _) = wake
                    .wait_timeout(guard, Duration::from_millis(50))
                    .unwrap_or_else(|poison| poison.into_inner());
                if *guard {
                    break;
                }
                drop(guard);
                let Some(error) = csnaps.terminal_failure() else {
                    if weak.upgrade().is_none() {
                        break;
                    }
                    continue;
                };
                let error = CsnapsAdapterError::new(error.to_string());
                *terminal_failure
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()) = Some(error);
                let listeners = {
                    let mut watch = state.lock().unwrap_or_else(|poison| poison.into_inner());
                    watch.state = CodebaseTelemetryAdapterState {
                        tracked_codebases: Vec::new(),
                        is_within_storage_budget: false,
                    };
                    watch.listeners.values().cloned().collect::<Vec<_>>()
                };
                for listener in listeners {
                    listener(CodebaseTelemetryAdapterState {
                        tracked_codebases: Vec::new(),
                        is_within_storage_budget: false,
                    });
                }
                break;
            })
            .ok();
        *self
            .terminal_monitor
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = handle;
    }
}

impl Drop for CsnapsCodebaseTelemetryAdapter {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

pub fn to_codebase(
    auth_id: &str,
    codebase: &CsnapsTrackedCodebase,
) -> Result<TrackedCodebase, CsnapsAdapterError> {
    if !matches!(codebase.kind, 1.0 | 2.0)
        || codebase.environment != CODEBASE_ENVIRONMENT_SAND_BOX
    {
        return Err(CsnapsAdapterError::new(
            "csnaps returned a codebase outside Sand",
        ));
    }
    Ok(TrackedCodebase {
        codebase_uuid: codebase.codebase_uuid.clone(),
        auth_id: auth_id.to_string(),
        path: codebase.path.clone(),
        kind: codebase.kind,
        environment: codebase.environment,
    })
}

pub fn to_adapter_state(
    auth_id: &str,
    state: &CsnapsServiceState,
) -> Result<CodebaseTelemetryAdapterState, CsnapsAdapterError> {
    Ok(CodebaseTelemetryAdapterState {
        tracked_codebases: state
            .tracked
            .iter()
            .map(|codebase| to_codebase(auth_id, codebase))
            .collect::<Result<Vec<_>, _>>()?,
        is_within_storage_budget: true,
    })
}

#[allow(dead_code)]
fn _instant_anchor() -> Instant {
    Instant::now()
}
