use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::listener_connect_watcher::ListenerConnectWatcher;
use super::listener_integrations::PlatformConnectionReader;

pub const LISTENER_CONNECT_POLL_INTERVAL_MS: u64 = 5_000;

pub type ListenerConnectedCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;

struct Wake {
    pending: Mutex<bool>,
    condvar: Condvar,
    stopped: AtomicBool,
}

impl Wake {
    fn new() -> Self {
        Self {
            pending: Mutex::new(true),
            condvar: Condvar::new(),
            stopped: AtomicBool::new(false),
        }
    }

    fn signal(&self) {
        *self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.condvar.notify_all();
    }

    fn wait(&self, timeout: Duration) {
        let guard = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut guard, _) = self
            .condvar
            .wait_timeout_while(guard, timeout, |pending| !*pending)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = false;
    }
}

pub struct ProductionListenerConnectWatcher {
    watcher: Arc<Mutex<ListenerConnectWatcher>>,
    wake: Arc<Wake>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl ProductionListenerConnectWatcher {
    pub fn start(
        reader: Arc<dyn PlatformConnectionReader>,
        on_connected: ListenerConnectedCallback,
    ) -> Arc<Self> {
        Self::start_with_interval(
            reader,
            on_connected,
            Duration::from_millis(LISTENER_CONNECT_POLL_INTERVAL_MS),
        )
    }

    #[doc(hidden)]
    pub fn start_with_interval(
        reader: Arc<dyn PlatformConnectionReader>,
        on_connected: ListenerConnectedCallback,
        poll_interval: Duration,
    ) -> Arc<Self> {
        let runtime = Arc::new(Self {
            watcher: Arc::new(Mutex::new(ListenerConnectWatcher::default())),
            wake: Arc::new(Wake::new()),
            worker: Mutex::new(None),
        });
        let weak = Arc::downgrade(&runtime);
        let worker = thread::spawn(move || {
            while let Some(runtime) = weak.upgrade() {
                if runtime.wake.stopped.load(Ordering::Acquire) {
                    break;
                }

                let pending = {
                    let watcher = runtime
                        .watcher
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    !watcher.is_suspended() && !watcher.pending().is_empty()
                };
                if pending {
                    let connected = {
                        let mut watcher = runtime
                            .watcher
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        watcher.tick(now_ms(), |platform| {
                            reader
                                .is_platform_connected(platform)
                                .map_err(|error| error.to_string())
                        })
                    };
                    for (agent_id, platform) in connected {
                        on_connected(&agent_id, &platform);
                    }
                    runtime.wake.wait(poll_interval);
                } else {
                    runtime.wake.wait(Duration::from_secs(60 * 60));
                }
            }
        });
        *runtime
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(worker);
        runtime
    }

    pub fn watch(&self, agent_id: impl Into<String>, platform: impl Into<String>) {
        self.watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .watch(agent_id, platform, now_ms());
        self.wake.signal();
    }

    pub fn suspend(&self) {
        self.watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .suspend();
        self.wake.signal();
    }

    pub fn resume(&self) {
        self.watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .resume();
        self.wake.signal();
    }

    pub fn stop(&self) {
        if self.wake.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .dispose();
        self.wake.signal();
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }

    pub fn pending_len(&self) -> usize {
        self.watcher
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending()
            .len()
    }
}

impl Drop for ProductionListenerConnectWatcher {
    fn drop(&mut self) {
        if !self.wake.stopped.swap(true, Ordering::AcqRel) {
            self.wake.signal();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
