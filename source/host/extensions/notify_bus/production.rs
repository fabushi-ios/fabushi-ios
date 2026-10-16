use super::notify_bus_client::{
    NotifyBusClientDependencies, SandNotifyBusClient, SandNotifyTopic,
};
use crate::codebase_telemetry::cursor_backend::{
    configured_backend_url, sand_client_version, sand_namespace,
};
use serde_json::Value;
use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

pub const SAND_NOTIFY_BUS_GATE: &str = "sand_notify_bus";
pub const SAND_NOTIFY_SAFETY_POLL_GATE: &str = "sand_notify_safety_poll";
const SAND_CLIENT_TYPE: &str = "sand";

pub trait NotifyBusHostBridge: Send + Sync {
    fn request(&self, method: &str) -> Result<Value, String>;
    fn feature_gate(&self, name: &str) -> Option<bool>;
}

pub type NotifyBusHandler = Arc<dyn Fn() + Send + Sync>;
pub type XuserNotifyHandler = NotifyBusHandler;
type HandlerRegistry = BTreeMap<SandNotifyTopic, BTreeMap<u64, NotifyBusHandler>>;

pub struct NotifyBusSubscription {
    handlers: Weak<Mutex<HandlerRegistry>>,
    topic: SandNotifyTopic,
    id: u64,
}

impl Drop for NotifyBusSubscription {
    fn drop(&mut self) {
        if let Some(handlers) = self.handlers.upgrade() {
            if let Some(topic_handlers) = handlers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get_mut(&self.topic)
            {
                topic_handlers.remove(&self.id);
            }
        }
    }
}

fn fire_topic(handlers: &Arc<Mutex<HandlerRegistry>>, topic: SandNotifyTopic) {
    let callbacks = handlers
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&topic)
        .map(|entries| entries.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    for callback in callbacks {
        let _ = catch_unwind(AssertUnwindSafe(|| callback()));
    }
}

fn fire_all_topics(handlers: &Arc<Mutex<HandlerRegistry>>) {
    for topic in SandNotifyTopic::ALL {
        fire_topic(handlers, topic);
    }
}

struct OwnerState {
    connection_key: Option<u64>,
    client: Option<Arc<SandNotifyBusClient>>,
}

pub struct MobileNotifyBusOwner {
    bridge: Arc<dyn NotifyBusHostBridge>,
    state: Mutex<OwnerState>,
    handlers: Arc<Mutex<HandlerRegistry>>,
    next_handler_id: AtomicU64,
    xuser_subscription: Mutex<Option<NotifyBusSubscription>>,
    safety_poll_enabled: AtomicBool,
    ready: AtomicBool,
    stopped: AtomicBool,
}

impl MobileNotifyBusOwner {
    pub fn start(bridge: Arc<dyn NotifyBusHostBridge>) -> Result<Arc<Self>, String> {
        let handlers = Arc::new(Mutex::new(
            SandNotifyTopic::ALL
                .into_iter()
                .map(|topic| (topic, BTreeMap::new()))
                .collect(),
        ));
        Ok(Arc::new(Self {
            bridge,
            state: Mutex::new(OwnerState {
                connection_key: None,
                client: None,
            }),
            handlers,
            next_handler_id: AtomicU64::new(0),
            xuser_subscription: Mutex::new(None),
            safety_poll_enabled: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        }))
    }

    pub fn mark_background_work_ready(&self) {
        if self.stopped.load(Ordering::Acquire)
            || self.ready.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let _ = self.sync();
    }

    pub fn on_notify(
        &self,
        topic: SandNotifyTopic,
        handler: NotifyBusHandler,
    ) -> NotifyBusSubscription {
        let id = self.next_handler_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.handlers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(topic)
            .or_default()
            .insert(id, handler);
        NotifyBusSubscription {
            handlers: Arc::downgrade(&self.handlers),
            topic,
            id,
        }
    }

    pub fn set_xuser_notify_handler(&self, handler: XuserNotifyHandler) {
        let subscription = self.on_notify(SandNotifyTopic::XuserEvents, handler);
        *self
            .xuser_subscription
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(subscription);
    }

    pub fn is_connected(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .client
            .as_ref()
            .is_some_and(|client| client.is_connected())
    }

    pub fn is_safety_poll_enabled(&self) -> bool {
        self.safety_poll_enabled.load(Ordering::Acquire)
    }

    pub fn observe_host_dispatch(&self, input: &str, output: &str) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let Ok(request) = serde_json::from_str::<Value>(input) else {
            return;
        };
        let Ok(response) = serde_json::from_str::<Value>(output) else {
            return;
        };
        if response.get("ok").and_then(Value::as_bool) != Some(true) {
            return;
        }
        if matches!(
            request.get("method").and_then(Value::as_str),
            Some(
                "setHostSettings"
                    | "feature.auth.logout"
                    | "feature.auth.passwordLogin"
                    | "feature.auth.browserPoll"
            )
        ) {
            let _ = self.sync();
        }
    }

    pub fn sync(&self) -> Result<(), String> {
        if self.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        if !self.ready.load(Ordering::Acquire) {
            return Ok(());
        }
        let enabled = self.bridge.feature_gate(SAND_NOTIFY_BUS_GATE).unwrap_or(false);
        let safety = self
            .bridge
            .feature_gate(SAND_NOTIFY_SAFETY_POLL_GATE)
            .unwrap_or(true);
        self.safety_poll_enabled.store(safety, Ordering::Release);

        if !enabled {
            self.replace_client(None, None);
            return Ok(());
        }

        let session = match self.bridge.request("feature.auth.deviceAgentSession") {
            Ok(session) => session,
            Err(_) => {
                self.replace_client(None, None);
                return Ok(());
            }
        };
        let access_token = session
            .get("accessToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "notify bus auth session missing accessToken".to_string())?
            .to_string();
        let auth_id = session
            .get("userId")
            .and_then(Value::as_str)
            .or_else(|| session.get("sessionId").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "notify bus auth session missing account identity".to_string())?
            .to_string();
        let backend = configured_backend_url();

        let mut hasher = DefaultHasher::new();
        backend.hash(&mut hasher);
        auth_id.hash(&mut hasher);
        access_token.hash(&mut hasher);
        let key = hasher.finish();
        if self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .connection_key
            == Some(key)
        {
            return Ok(());
        }

        let connected_handlers = Arc::clone(&self.handlers);
        let notify_handlers = Arc::clone(&self.handlers);
        let deps = NotifyBusClientDependencies::production(
            backend,
            access_token,
            SAND_CLIENT_TYPE.to_string(),
            sand_client_version(),
            sand_namespace().to_string(),
            Arc::new(move || fire_all_topics(&connected_handlers)),
            Arc::new(move |topic| fire_topic(&notify_handlers, topic)),
            Arc::new(|_| {}),
        );
        let client = Arc::new(SandNotifyBusClient::new(deps)?);
        client.start();
        self.replace_client(Some(key), Some(client));
        Ok(())
    }

    fn replace_client(
        &self,
        connection_key: Option<u64>,
        client: Option<Arc<SandNotifyBusClient>>,
    ) {
        let previous = {
            let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.connection_key = connection_key;
            std::mem::replace(&mut state.client, client)
        };
        if let Some(previous) = previous {
            previous.stop();
        }
    }

    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.replace_client(None, None);
        self.ready.store(false, Ordering::Release);
        self.safety_poll_enabled.store(false, Ordering::Release);
        self.xuser_subscription
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let mut handlers = self.handlers.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        for entries in handlers.values_mut() {
            entries.clear();
        }
    }
}

impl Drop for MobileNotifyBusOwner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FakeBridge {
        settings: Mutex<Value>,
        session: Mutex<Option<Value>>,
    }

    impl NotifyBusHostBridge for FakeBridge {
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
            self.settings
                .lock()
                .unwrap()
                .get("featureGates")
                .and_then(|value| value.get(name))
                .and_then(Value::as_bool)
        }
    }

    #[test]
    fn topic_registry_fires_connected_drains_and_subscription_drop_unsubscribes() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let bridge = Arc::new(FakeBridge {
            settings: Mutex::new(json!({"featureGates": {}})),
            session: Mutex::new(None),
        });
        let owner = MobileNotifyBusOwner::start(bridge).unwrap();
        assert!(
            owner.is_safety_poll_enabled(),
            "safety poll must default on before experiment gates resolve"
        );

        let automation = Arc::new(AtomicUsize::new(0));
        let listener = Arc::new(AtomicUsize::new(0));
        let xuser = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&automation);
        let l = Arc::clone(&listener);
        let x = Arc::clone(&xuser);
        let automation_sub = owner.on_notify(
            SandNotifyTopic::AutomationFires,
            Arc::new(move || { a.fetch_add(1, AtomicOrdering::SeqCst); }),
        );
        let _listener_sub = owner.on_notify(
            SandNotifyTopic::ListenerEvents,
            Arc::new(move || { l.fetch_add(1, AtomicOrdering::SeqCst); }),
        );
        let _xuser_sub = owner.on_notify(
            SandNotifyTopic::XuserEvents,
            Arc::new(move || { x.fetch_add(1, AtomicOrdering::SeqCst); }),
        );

        fire_all_topics(&owner.handlers);
        assert_eq!(automation.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(listener.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(xuser.load(AtomicOrdering::SeqCst), 1);

        drop(automation_sub);
        fire_topic(&owner.handlers, SandNotifyTopic::AutomationFires);
        assert_eq!(automation.load(AtomicOrdering::SeqCst), 1);
        owner.stop();
    }

    #[test]
    fn disabled_notify_bus_is_fail_closed_and_safety_gate_is_independent() {
        let bridge = Arc::new(FakeBridge {
            settings: Mutex::new(json!({
                "featureGates": {
                    SAND_NOTIFY_BUS_GATE: false,
                    SAND_NOTIFY_SAFETY_POLL_GATE: true
                }
            })),
            session: Mutex::new(None),
        });
        let owner = MobileNotifyBusOwner::start(bridge).unwrap();
        assert!(!owner.is_connected());
        assert!(
            owner.is_safety_poll_enabled(),
            "Desktop contract defaults safety poll on before readiness/gate resolution"
        );
        owner.mark_background_work_ready();
        assert!(!owner.is_connected());
        assert!(owner.is_safety_poll_enabled());
        owner.stop();
        assert!(!owner.is_safety_poll_enabled());
    }
}
