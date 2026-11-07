use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::cursor_backend::{fetch_sand_privacy_mode, CursorBackendError, SandPrivacyMode};

pub const DEFAULT_PRIVACY_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_PRIVACY_RETRY_DELAY: Duration = Duration::from_secs(10);
pub const DEFAULT_PRIVACY_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryAuth {
    pub auth_id: String,
    pub auth_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PrivacyLookupError {
    message: String,
    retryable: bool,
}

impl PrivacyLookupError {
    pub fn new(message: impl Into<String>, retryable: bool) -> Self {
        Self { message: message.into(), retryable }
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }
}

pub type PrivacyModeLoader =
    Arc<dyn Fn(TelemetryAuth) -> Result<SandPrivacyMode, PrivacyLookupError> + Send + Sync>;
pub type PrivacyModeListener = Arc<dyn Fn(SandPrivacyMode) + Send + Sync>;

struct PrivacyState {
    auth: Option<TelemetryAuth>,
    mode: SandPrivacyMode,
    generation: u64,
    listeners: BTreeMap<u64, PrivacyModeListener>,
    disposed: bool,
}

struct PrivacyInner {
    state: Mutex<PrivacyState>,
    wake: Condvar,
    next_listener_id: AtomicU64,
    loader: PrivacyModeLoader,
    lookup_timeout: Duration,
    retry_delay: Duration,
    refresh_interval: Duration,
    stop: AtomicBool,
}

pub struct PrivacyModeSubscription {
    inner: Weak<PrivacyInner>,
    id: u64,
}

impl PrivacyModeSubscription {
    pub fn unsubscribe(&mut self) {
        let Some(inner) = self.inner.upgrade() else { return; };
        inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .listeners
            .remove(&self.id);
        self.id = 0;
    }
}

impl Drop for PrivacyModeSubscription {
    fn drop(&mut self) {
        if self.id != 0 {
            self.unsubscribe();
        }
    }
}

pub struct CodebaseTelemetryPrivacyMode {
    inner: Arc<PrivacyInner>,
    polling: Mutex<Option<JoinHandle<()>>>,
}

impl CodebaseTelemetryPrivacyMode {
    pub fn new(loader: PrivacyModeLoader) -> Self {
        Self::with_policy(
            loader,
            DEFAULT_PRIVACY_LOOKUP_TIMEOUT,
            DEFAULT_PRIVACY_RETRY_DELAY,
            DEFAULT_PRIVACY_REFRESH_INTERVAL,
        )
    }

    pub fn with_policy(
        loader: PrivacyModeLoader,
        lookup_timeout: Duration,
        retry_delay: Duration,
        refresh_interval: Duration,
    ) -> Self {
        let inner = Arc::new(PrivacyInner {
            state: Mutex::new(PrivacyState {
                auth: None,
                mode: SandPrivacyMode::Unspecified,
                generation: 0,
                listeners: BTreeMap::new(),
                disposed: false,
            }),
            wake: Condvar::new(),
            next_listener_id: AtomicU64::new(1),
            loader,
            lookup_timeout,
            retry_delay,
            refresh_interval,
            stop: AtomicBool::new(false),
        });
        let worker_inner = Arc::clone(&inner);
        let polling = thread::Builder::new()
            .name("codebase-telemetry-privacy-poll".into())
            .spawn(move || polling_loop(worker_inner))
            .ok();
        Self { inner, polling: Mutex::new(polling) }
    }

    pub fn get(&self) -> SandPrivacyMode {
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .mode
    }

    pub fn subscribe(&self, listener: PrivacyModeListener) -> PrivacyModeSubscription {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.inner
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .listeners
            .insert(id, listener);
        PrivacyModeSubscription { inner: Arc::downgrade(&self.inner), id }
    }

    pub fn set_auth(&self, auth: TelemetryAuth) {
        let generation = {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.disposed { return; }
            let changed = state.auth.as_ref().map(|v| &v.auth_id) != Some(&auth.auth_id);
            state.auth = Some(auth);
            state.generation = state.generation.saturating_add(1);
            if changed {
                publish_locked(&mut state, SandPrivacyMode::Unspecified);
            }
            state.generation
        };
        spawn_refresh(Arc::clone(&self.inner), generation);
    }

    pub fn clear_auth(&self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.disposed { return; }
        state.auth = None;
        state.generation = state.generation.saturating_add(1);
        publish_locked(&mut state, SandPrivacyMode::Unspecified);
        drop(state);
    }

    pub fn refresh_now(&self) {
        let generation = self.inner
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .generation;
        spawn_refresh(Arc::clone(&self.inner), generation);
    }

    pub fn dispose(&self) {
        if self.inner.stop.swap(true, Ordering::SeqCst) { return; }
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            state.disposed = true;
            state.generation = state.generation.saturating_add(1);
            state.auth = None;
            state.listeners.clear();
        }
        self.inner.wake.notify_all();
        if let Some(handle) = self.polling.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CodebaseTelemetryPrivacyMode {
    fn drop(&mut self) {
        self.dispose();
    }
}

fn publish_locked(state: &mut PrivacyState, value: SandPrivacyMode) {
    if state.mode == value { return; }
    state.mode = value;
    let listeners = state.listeners.values().cloned().collect::<Vec<_>>();
    for listener in listeners {
        listener(value);
    }
}

fn polling_loop(inner: Arc<PrivacyInner>) {
    let mut state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
    while !inner.stop.load(Ordering::Acquire) {
        let (next_state, wait) = inner
            .wake
            .wait_timeout(state, inner.refresh_interval)
            .unwrap_or_else(|p| p.into_inner());
        state = next_state;
        if inner.stop.load(Ordering::Acquire) { break; }
        if !wait.timed_out() { continue; }
        let generation = state.generation;
        let has_auth = state.auth.is_some();
        drop(state);
        if has_auth {
            spawn_refresh(Arc::clone(&inner), generation);
        }
        state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
    }
}

fn spawn_refresh(inner: Arc<PrivacyInner>, generation: u64) {
    let auth = {
        let state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.disposed || state.generation != generation { return; }
        state.auth.clone()
    };
    let Some(auth) = auth else { return; };
    let _ = thread::Builder::new()
        .name("codebase-telemetry-privacy-refresh".into())
        .spawn(move || {
            let mut attempt = 0;
            loop {
                attempt += 1;
                let outcome = lookup_with_deadline(Arc::clone(&inner.loader), auth.clone(), inner.lookup_timeout);
                match outcome {
                    Ok(mode) => {
                        let mut state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
                        if !state.disposed && state.generation == generation {
                            publish_locked(&mut state, mode);
                        }
                        return;
                    }
                    Err(error) if attempt < 2 && error.retryable() => {
                        thread::sleep(inner.retry_delay);
                    }
                    Err(_) => return,
                }
                if inner.stop.load(Ordering::Acquire) { return; }
                let state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
                if state.disposed || state.generation != generation { return; }
            }
        });
}

fn lookup_with_deadline(
    loader: PrivacyModeLoader,
    auth: TelemetryAuth,
    timeout: Duration,
) -> Result<SandPrivacyMode, PrivacyLookupError> {
    // Measure the deadline before spawning the loader. On a saturated runner the
    // current thread can be descheduled after spawn while the lookup completes;
    // starting recv_timeout only after that stall would incorrectly accept a
    // result that already exceeded the contract deadline.
    let started = Instant::now();
    let (send, receive) = mpsc::sync_channel(1);
    let _ = thread::Builder::new()
        .name("codebase-telemetry-privacy-lookup".into())
        .spawn(move || { let _ = send.send(loader(auth)); });
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(PrivacyLookupError::new(
            "privacy mode lookup deadline exceeded",
            true,
        ));
    }
    match receive.recv_timeout(remaining) {
        Ok(result) if started.elapsed() <= timeout => result,
        Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => Err(PrivacyLookupError::new(
            "privacy mode lookup deadline exceeded",
            true,
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(PrivacyLookupError::new(
            "privacy mode lookup worker stopped",
            true,
        )),
    }
}

pub fn create_sand_privacy_mode_loader(
    backend_url: String,
    machine_id: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
) -> PrivacyModeLoader {
    Arc::new(move |auth: TelemetryAuth| {
        let machine_id = machine_id()?;
        fetch_sand_privacy_mode(&backend_url, &auth.auth_token, &machine_id)
            .map(|mode| mode.unwrap_or(SandPrivacyMode::Unspecified))
            .map_err(privacy_backend_error)
    })
}

fn privacy_backend_error(error: CursorBackendError) -> PrivacyLookupError {
    let retryable = match &error {
        CursorBackendError::Transport(_) | CursorBackendError::Timeout(_) => true,
        CursorBackendError::HttpStatus { status, .. } => *status == 503 || *status == 504,
        CursorBackendError::InvalidBackendUrl(_)
        | CursorBackendError::InvalidProto(_)
        | CursorBackendError::Cancelled(_) => false,
    };
    PrivacyLookupError::new(error.to_string(), retryable)
}

impl From<String> for PrivacyLookupError {
    fn from(message: String) -> Self {
        Self::new(message, false)
    }
}
