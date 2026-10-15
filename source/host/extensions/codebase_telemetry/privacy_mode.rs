use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const DEFAULT_PRIVACY_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_PRIVACY_RETRY_DELAY: Duration = Duration::from_secs(10);
pub const DEFAULT_PRIVACY_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum PrivacyMode {
    Unspecified = 0,
    NoStorage = 1,
    NoTraining = 2,
    UsageDataTrainingAllowed = 3,
    UsageCodebaseTrainingAllowed = 4,
}

impl PrivacyMode {
    pub fn from_proto(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::Unspecified,
            1 => Self::NoStorage,
            2 => Self::NoTraining,
            3 => Self::UsageDataTrainingAllowed,
            4 => Self::UsageCodebaseTrainingAllowed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryAuth {
    pub auth_id: String,
    pub auth_token: String,
    pub machine_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyLookupError {
    message: String,
    retryable: bool,
}

impl PrivacyLookupError {
    pub fn new(message: impl Into<String>, retryable: bool) -> Self {
        Self { message: message.into(), retryable }
    }
    pub fn retryable(&self) -> bool { self.retryable }
    pub fn message(&self) -> &str { &self.message }
}

pub type PrivacyModeLoader =
    Arc<dyn Fn(TelemetryAuth) -> Result<PrivacyMode, PrivacyLookupError> + Send + Sync>;
pub type PrivacyModeListener = Arc<dyn Fn(PrivacyMode) + Send + Sync>;

struct PrivacyState {
    auth: Option<TelemetryAuth>,
    mode: PrivacyMode,
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
impl Drop for PrivacyModeSubscription {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner.state.lock().unwrap_or_else(|p| p.into_inner()).listeners.remove(&self.id);
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
                mode: PrivacyMode::Unspecified,
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
        let worker = Arc::clone(&inner);
        let polling = thread::Builder::new()
            .name("ios-codebase-privacy-poll".into())
            .spawn(move || polling_loop(worker))
            .ok();
        Self { inner, polling: Mutex::new(polling) }
    }

    pub fn get(&self) -> PrivacyMode {
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner()).mode
    }

    pub fn subscribe(&self, listener: PrivacyModeListener) -> PrivacyModeSubscription {
        let id = self.inner.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner()).listeners.insert(id, listener);
        PrivacyModeSubscription { inner: Arc::downgrade(&self.inner), id }
    }

    pub fn set_auth(&self, auth: TelemetryAuth) {
        let generation = {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            if state.disposed { return; }
            let account_changed = state.auth.as_ref().map(|v| v.auth_id.as_str()) != Some(auth.auth_id.as_str());
            state.auth = Some(auth);
            state.generation = state.generation.saturating_add(1);
            if account_changed {
                publish_locked(&mut state, PrivacyMode::Unspecified);
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
        publish_locked(&mut state, PrivacyMode::Unspecified);
    }

    pub fn refresh_now(&self) {
        let generation = self.inner.state.lock().unwrap_or_else(|p| p.into_inner()).generation;
        spawn_refresh(Arc::clone(&self.inner), generation);
    }

    pub fn dispose(&self) {
        if self.inner.stop.swap(true, Ordering::SeqCst) { return; }
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|p| p.into_inner());
            state.disposed = true;
            state.generation = state.generation.saturating_add(1);
            state.auth = None;
            state.mode = PrivacyMode::Unspecified;
            state.listeners.clear();
        }
        self.inner.wake.notify_all();
        if let Some(handle) = self.polling.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CodebaseTelemetryPrivacyMode {
    fn drop(&mut self) { self.dispose(); }
}

fn publish_locked(state: &mut PrivacyState, value: PrivacyMode) {
    if state.mode == value { return; }
    state.mode = value;
    for listener in state.listeners.values().cloned().collect::<Vec<_>>() {
        listener(value);
    }
}

fn polling_loop(inner: Arc<PrivacyInner>) {
    let mut state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
    while !inner.stop.load(Ordering::Acquire) {
        let (next, waited) = inner.wake.wait_timeout(state, inner.refresh_interval)
            .unwrap_or_else(|p| p.into_inner());
        state = next;
        if inner.stop.load(Ordering::Acquire) { break; }
        if !waited.timed_out() { continue; }
        let generation = state.generation;
        let has_auth = state.auth.is_some();
        drop(state);
        if has_auth { spawn_refresh(Arc::clone(&inner), generation); }
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
    let _ = thread::Builder::new().name("ios-codebase-privacy-refresh".into()).spawn(move || {
        for attempt in 0..2 {
            let outcome = lookup_with_deadline(Arc::clone(&inner.loader), auth.clone(), inner.lookup_timeout);
            match outcome {
                Ok(mode) => {
                    let mut state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
                    if !state.disposed && state.generation == generation {
                        publish_locked(&mut state, mode);
                    }
                    return;
                }
                Err(error) if attempt == 0 && error.retryable() => {
                    let state = inner.state.lock().unwrap_or_else(|p| p.into_inner());
                    if state.disposed || state.generation != generation { return; }
                    drop(state);
                    thread::sleep(inner.retry_delay);
                }
                Err(_) => return,
            }
            if inner.stop.load(Ordering::Acquire) { return; }
        }
    });
}

fn lookup_with_deadline(
    loader: PrivacyModeLoader,
    auth: TelemetryAuth,
    timeout: Duration,
) -> Result<PrivacyMode, PrivacyLookupError> {
    let started = Instant::now();
    let (send, receive) = mpsc::sync_channel(1);
    let _ = thread::Builder::new().name("ios-codebase-privacy-lookup".into()).spawn(move || {
        let _ = send.send(loader(auth));
    });
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(PrivacyLookupError::new("privacy mode lookup deadline exceeded", true));
    }
    match receive.recv_timeout(remaining) {
        Ok(result) if started.elapsed() <= timeout => result,
        Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(PrivacyLookupError::new("privacy mode lookup deadline exceeded", true))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(PrivacyLookupError::new("privacy mode lookup worker stopped", true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn auth(id: &str) -> TelemetryAuth {
        TelemetryAuth { auth_id: id.into(), auth_token: format!("token-{id}"), machine_id: "machine".into() }
    }

    #[test]
    fn account_switch_immediately_resets_to_unspecified_and_stale_lookup_cannot_write_back() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let gate_loader = Arc::clone(&gate);
        let loader: PrivacyModeLoader = Arc::new(move |credentials| {
            if credentials.auth_id == "a" {
                let (lock, wake) = &*gate_loader;
                let mut open = lock.lock().unwrap();
                while !*open { open = wake.wait(open).unwrap(); }
                Ok(PrivacyMode::UsageCodebaseTrainingAllowed)
            } else {
                Ok(PrivacyMode::NoTraining)
            }
        });
        let privacy = CodebaseTelemetryPrivacyMode::with_policy(
            loader, Duration::from_secs(1), Duration::from_millis(1), Duration::from_secs(60)
        );
        privacy.set_auth(auth("a"));
        privacy.set_auth(auth("b"));
        assert_eq!(privacy.get(), PrivacyMode::Unspecified);
        {
            let (lock, wake) = &*gate;
            *lock.lock().unwrap() = true;
            wake.notify_all();
        }
        thread::sleep(Duration::from_millis(30));
        assert_ne!(privacy.get(), PrivacyMode::UsageCodebaseTrainingAllowed);
        privacy.dispose();
    }

    #[test]
    fn retryable_failure_retries_at_most_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let loader: PrivacyModeLoader = Arc::new(move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            Err(PrivacyLookupError::new("retry", true))
        });
        let privacy = CodebaseTelemetryPrivacyMode::with_policy(
            loader, Duration::from_millis(50), Duration::from_millis(1), Duration::from_secs(60)
        );
        privacy.set_auth(auth("a"));
        thread::sleep(Duration::from_millis(30));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(privacy.get(), PrivacyMode::Unspecified);
        privacy.dispose();
    }

    #[test]
    fn clear_auth_is_fail_closed() {
        let loader: PrivacyModeLoader = Arc::new(|_| Ok(PrivacyMode::UsageCodebaseTrainingAllowed));
        let privacy = CodebaseTelemetryPrivacyMode::with_policy(
            loader, Duration::from_millis(50), Duration::from_millis(1), Duration::from_secs(60)
        );
        privacy.set_auth(auth("a"));
        thread::sleep(Duration::from_millis(20));
        privacy.clear_auth();
        assert_eq!(privacy.get(), PrivacyMode::Unspecified);
        privacy.dispose();
    }
}
