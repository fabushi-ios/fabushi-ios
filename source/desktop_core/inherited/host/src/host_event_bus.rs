use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
};

use serde_json::{Value, json};

pub type HostEventFailureReporter = Arc<dyn Fn(&Value) + Send + Sync + 'static>;

static HOST_EVENT_FAILURE_REPORTER: OnceLock<Mutex<Option<HostEventFailureReporter>>> = OnceLock::new();

pub fn pin_host_event_failure_reporter(reporter: Option<HostEventFailureReporter>) {
    let slot = HOST_EVENT_FAILURE_REPORTER.get_or_init(|| Mutex::new(None));
    if let Ok(mut current) = slot.lock() {
        *current = reporter;
    }
}

fn report_pinned_host_event_failure(report: &Value) {
    let reporter = HOST_EVENT_FAILURE_REPORTER
        .get()
        .and_then(|slot| slot.lock().ok().and_then(|current| current.clone()));
    if let Some(reporter) = reporter {
        reporter(report);
    }
}
type HostEventListener = Arc<dyn Fn(&Value) + Send + Sync + 'static>;
type HostCapabilityHandler =
    Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEventFailureMode {
    Continue,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Host event handler failed for {topic}: {message}")]
pub struct HostEventHandlerError {
    pub topic: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SubscriptionKind {
    Listener,
    Topic(String),
}

struct HostEventBusState {
    stream_subscribers: Mutex<Vec<Sender<Value>>>,
    listeners: Mutex<BTreeMap<u64, HostEventListener>>,
    topic_handlers: Mutex<HashMap<String, BTreeMap<u64, HostCapabilityHandler>>>,
    next_id: AtomicU64,
    report_failure: HostEventFailureReporter,
}

#[derive(Clone)]
pub struct SandHostEventBus {
    state: Arc<HostEventBusState>,
}

impl Default for SandHostEventBus {
    fn default() -> Self {
        Self::with_failure_reporter(Arc::new(report_pinned_host_event_failure))
    }
}

impl SandHostEventBus {
    pub fn with_failure_reporter(report_failure: HostEventFailureReporter) -> Self {
        Self {
            state: Arc::new(HostEventBusState {
                stream_subscribers: Mutex::new(Vec::new()),
                listeners: Mutex::new(BTreeMap::new()),
                topic_handlers: Mutex::new(HashMap::new()),
                next_id: AtomicU64::new(1),
                report_failure,
            }),
        }
    }

    /// Shipping Gateway/SSE event emission. This is the production equivalent
    /// of the frozen single-argument SandHostEventBus.emit(event) overload.
    pub fn publish(&self, event: Value) {
        if let Ok(mut subscribers) = self.state.stream_subscribers.lock() {
            subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
        }

        let listeners = self
            .state
            .listeners
            .lock()
            .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for listener in listeners {
            if catch_unwind(AssertUnwindSafe(|| listener(&event))).is_err() {
                (self.state.report_failure)(&json!({
                    "kind": "listener_failed",
                    "errorClass": "panic",
                }));
            }
        }
    }

    /// Stream subscription retained for the Host Gateway SSE contract.
    pub fn subscribe(&self) -> Receiver<Value> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut subscribers) = self.state.stream_subscribers.lock() {
            subscribers.push(sender);
        }
        receiver
    }

    pub fn subscriber_count(&self) -> usize {
        self.state
            .stream_subscribers
            .lock()
            .map(|subscribers| subscribers.len())
            .unwrap_or_default()
    }

    /// Callback subscription matching the frozen subscribe(listener) surface.
    pub fn subscribe_listener<F>(&self, listener: F) -> HostEventSubscription
    where
        F: Fn(&Value) + Send + Sync + 'static,
    {
        let id = self.state.next_id.fetch_add(1, Ordering::Relaxed);
        self.state
            .listeners
            .lock()
            .expect("Host event listener registry poisoned")
            .insert(id, Arc::new(listener));
        HostEventSubscription {
            state: Arc::downgrade(&self.state),
            id,
            kind: SubscriptionKind::Listener,
            active: true,
        }
    }

    /// Capability-topic registration matching the frozen on(topic, handler)
    /// surface. The returned subscription unregisters on Drop or unsubscribe.
    pub fn on<F>(&self, topic: impl Into<String>, handler: F) -> HostEventSubscription
    where
        F: Fn(&Value) -> Result<(), String> + Send + Sync + 'static,
    {
        let topic = topic.into();
        let id = self.state.next_id.fetch_add(1, Ordering::Relaxed);
        self.state
            .topic_handlers
            .lock()
            .expect("Host capability handler registry poisoned")
            .entry(topic.clone())
            .or_default()
            .insert(id, Arc::new(handler));
        HostEventSubscription {
            state: Arc::downgrade(&self.state),
            id,
            kind: SubscriptionKind::Topic(topic),
            active: true,
        }
    }

    /// Frozen createHostEvents-style capability dispatch. Handler failures are
    /// always reported; Reject additionally returns the first failure.
    pub fn emit_topic(
        &self,
        topic: &str,
        payload: &Value,
        failure_mode: HostEventFailureMode,
    ) -> Result<(), HostEventHandlerError> {
        let handlers = self
            .state
            .topic_handlers
            .lock()
            .ok()
            .and_then(|topics| topics.get(topic).cloned())
            .map(|handlers| handlers.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();

        for handler in handlers {
            let outcome = catch_unwind(AssertUnwindSafe(|| handler(payload)));
            let failure = match outcome {
                Ok(Ok(())) => None,
                Ok(Err(message)) => Some((message, "Error")),
                Err(_) => Some(("handler panicked".to_string(), "panic")),
            };
            let Some((message, error_class)) = failure else {
                continue;
            };
            (self.state.report_failure)(&json!({
                "kind": "subscriber_failed",
                "topic": topic,
                "errorClass": error_class,
            }));
            if failure_mode == HostEventFailureMode::Reject {
                return Err(HostEventHandlerError {
                    topic: topic.to_string(),
                    message,
                });
            }
        }
        Ok(())
    }
}

pub struct HostEventSubscription {
    state: Weak<HostEventBusState>,
    id: u64,
    kind: SubscriptionKind,
    active: bool,
}

impl HostEventSubscription {
    pub fn unsubscribe(mut self) {
        self.remove();
    }

    fn remove(&mut self) {
        if !self.active {
            return;
        }
        let Some(state) = self.state.upgrade() else {
            self.active = false;
            return;
        };
        match &self.kind {
            SubscriptionKind::Listener => {
                if let Ok(mut listeners) = state.listeners.lock() {
                    listeners.remove(&self.id);
                }
            }
            SubscriptionKind::Topic(topic) => {
                if let Ok(mut topics) = state.topic_handlers.lock() {
                    let remove_topic = if let Some(handlers) = topics.get_mut(topic) {
                        handlers.remove(&self.id);
                        handlers.is_empty()
                    } else {
                        false
                    };
                    if remove_topic {
                        topics.remove(topic);
                    }
                }
            }
        }
        self.active = false;
    }
}

impl Drop for HostEventSubscription {
    fn drop(&mut self) {
        self.remove();
    }
}
