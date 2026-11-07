use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::browser_ua::extension::StopSubscription;
use crate::extensions::experiments::HostExperimentsExtension;
use crate::extensions::extension_ids_generated::HostExtensionId;

use super::notify_bus_client::{
    NotifyBusClientDependencies, NotifyBusTiming, SandNotifyBusClient, SandNotifyStreamError,
    SandNotifyTopic,
};

pub const SAFETY_POLL_DEFAULT_BEFORE_GATE_RESOLVES: bool = true;
pub const NOTIFY_BUS_DEPENDENCIES: &[HostExtensionId] =
    &[HostExtensionId::Auth, HostExtensionId::Experiments];

pub fn notify_bus_extension_id() -> HostExtensionId {
    HostExtensionId::NotifyBus
}

pub type NotifyBusLog = Arc<dyn Fn(&str) + Send + Sync>;
pub type NotifyBusHandler = Arc<dyn Fn() + Send + Sync>;

pub trait NotifyBusExperimentsApi: Send + Sync {
    fn check_feature_gate(&self, name: &str) -> bool;
    fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) -> StopSubscription;
}

impl NotifyBusExperimentsApi for HostExperimentsExtension {
    fn check_feature_gate(&self, name: &str) -> bool {
        HostExperimentsExtension::check_feature_gate(self, name)
    }

    fn subscribe(&self, listener: Arc<dyn Fn() + Send + Sync>) -> StopSubscription {
        HostExperimentsExtension::subscribe(self, listener)
    }
}

struct NotifyBusShared {
    handlers: Mutex<BTreeMap<SandNotifyTopic, BTreeMap<u64, NotifyBusHandler>>>,
    next_handler_id: AtomicU64,
    safety_poll_enabled: AtomicBool,
    stopped: AtomicBool,
    ready: AtomicBool,
    client_started: AtomicBool,
    log: NotifyBusLog,
}

impl NotifyBusShared {
    fn new(log: NotifyBusLog) -> Self {
        let mut handlers = BTreeMap::new();
        for topic in SandNotifyTopic::ALL {
            handlers.insert(topic, BTreeMap::new());
        }
        Self {
            handlers: Mutex::new(handlers),
            next_handler_id: AtomicU64::new(0),
            safety_poll_enabled: AtomicBool::new(SAFETY_POLL_DEFAULT_BEFORE_GATE_RESOLVES),
            stopped: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            client_started: AtomicBool::new(false),
            log,
        }
    }

    fn fire(&self, topic: SandNotifyTopic) {
        let handlers = self
            .handlers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&topic)
            .map(|entries| entries.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for handler in handlers {
            if catch_unwind(AssertUnwindSafe(|| handler())).is_err() {
                (self.log)(&format!(
                    "[sand:notify-bus] {} drain handler failed: panic",
                    topic.as_str()
                ));
            }
        }
    }

    fn clear_handlers(&self) {
        let mut handlers = self
            .handlers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for entries in handlers.values_mut() {
            entries.clear();
        }
    }
}

struct NotifyBusInner {
    shared: Arc<NotifyBusShared>,
    experiments: Arc<dyn NotifyBusExperimentsApi>,
    client: Arc<SandNotifyBusClient>,
    experiment_stops: Mutex<Vec<StopSubscription>>,
}

impl NotifyBusInner {
    fn reconcile_gates(&self) {
        if self.shared.stopped.load(Ordering::Acquire)
            || !self.shared.ready.load(Ordering::Acquire)
        {
            return;
        }

        let should_start = self.experiments.check_feature_gate("sand_notify_bus");
        let was_started = self
            .shared
            .client_started
            .swap(should_start, Ordering::AcqRel);
        if should_start != was_started {
            if should_start {
                self.client.start();
            } else {
                self.client.stop();
            }
        }
        self.shared.safety_poll_enabled.store(
            self.experiments
                .check_feature_gate("sand_notify_safety_poll"),
            Ordering::Release,
        );
    }

    fn stop(&self) {
        if self.shared.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut stops = self
            .experiment_stops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while let Some(stop) = stops.pop() {
            stop();
        }
        drop(stops);
        self.shared.client_started.store(false, Ordering::Release);
        self.client.stop();
        self.shared.clear_handlers();
    }
}

#[derive(Clone)]
pub struct HostNotifyBusExtension {
    inner: Arc<NotifyBusInner>,
}

impl HostNotifyBusExtension {
    pub fn mark_background_work_ready(&self) {
        if self.inner.shared.stopped.load(Ordering::Acquire)
            || self.inner.shared.ready.swap(true, Ordering::AcqRel)
        {
            return;
        }

        let weak: Weak<NotifyBusInner> = Arc::downgrade(&self.inner);
        let stop = self.inner.experiments.subscribe(Arc::new(move || {
            if let Some(inner) = weak.upgrade() {
                inner.reconcile_gates();
            }
        }));
        self.inner
            .experiment_stops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(stop);
        self.inner.reconcile_gates();
    }

    pub fn on_notify(
        &self,
        topic: SandNotifyTopic,
        handler: NotifyBusHandler,
    ) -> StopSubscription {
        let id = self
            .inner
            .shared
            .next_handler_id
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        self.inner
            .shared
            .handlers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(topic)
            .or_default()
            .insert(id, handler);

        let shared = Arc::downgrade(&self.inner.shared);
        Box::new(move || {
            if let Some(shared) = shared.upgrade() {
                if let Some(entries) = shared
                    .handlers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get_mut(&topic)
                {
                    entries.remove(&id);
                }
            }
        })
    }

    pub fn is_connected(&self) -> bool {
        self.inner.client.is_connected()
    }

    pub fn is_safety_poll_enabled(&self) -> bool {
        self.inner
            .shared
            .safety_poll_enabled
            .load(Ordering::Acquire)
    }

    pub fn stop(&self) {
        self.inner.stop();
    }
}

impl Drop for HostNotifyBusExtension {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.inner.stop();
        }
    }
}

pub struct NotifyBusExtensionOptions {
    pub get_backend_url: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub get_access_token: Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>,
    pub now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
    pub timing: NotifyBusTiming,
    pub log: NotifyBusLog,
}

pub fn start_notify_bus_extension_with_options(
    experiments: Arc<dyn NotifyBusExperimentsApi>,
    options: NotifyBusExtensionOptions,
) -> Result<HostNotifyBusExtension, SandNotifyStreamError> {
    let shared = Arc::new(NotifyBusShared::new(Arc::clone(&options.log)));

    let connected_shared = Arc::clone(&shared);
    let notify_shared = Arc::clone(&shared);
    let stream_log = Arc::clone(&options.log);
    let client = Arc::new(SandNotifyBusClient::new(NotifyBusClientDependencies {
        get_backend_url: options.get_backend_url,
        get_access_token: options.get_access_token,
        on_connected: Arc::new(move || {
            for topic in SandNotifyTopic::ALL {
                connected_shared.fire(topic);
            }
        }),
        on_notify: Arc::new(move |topic| notify_shared.fire(topic)),
        on_stream_error: Arc::new(move |error| {
            stream_log(&format!("[sand:notify-bus] stream failed: {error}"))
        }),
        now_ms: options.now_ms,
        timing: options.timing,
    })?);

    Ok(HostNotifyBusExtension {
        inner: Arc::new(NotifyBusInner {
            shared,
            experiments,
            client,
            experiment_stops: Mutex::new(Vec::new()),
        }),
    })
}

pub fn start_notify_bus_extension(
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
    log: NotifyBusLog,
) -> Result<HostNotifyBusExtension, SandNotifyStreamError> {
    let token_auth = Arc::clone(&auth);
    start_notify_bus_extension_with_options(
        experiments,
        NotifyBusExtensionOptions {
            get_backend_url: Arc::new(|| {
                get_configured_backend_url().map_err(|error| error.to_string())
            }),
            get_access_token: Arc::new(move |_backend_url| {
                token_auth.get_access_token().map_err(|error| error.to_string())
            }),
            now_ms: Arc::new(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX)
            }),
            timing: NotifyBusTiming::default(),
            log,
        },
    )
}
