use super::notify_bus_client::{
    NotifyBusClientDependencies, SandNotifyBusClient, SandNotifyTopic,
};
use crate::codebase_telemetry::cursor_backend::{
    configured_backend_url, sand_client_version, sand_namespace,
};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const SAND_NOTIFY_BUS_GATE: &str = "sand_notify_bus";
const SAND_NOTIFY_SAFETY_POLL_GATE: &str = "sand_notify_safety_poll";
const SAND_CLIENT_TYPE: &str = "sand";

pub trait NotifyBusHostBridge: Send + Sync {
    fn request(&self, method: &str) -> Result<Value, String>;
}

pub type XuserNotifyHandler = Arc<dyn Fn() + Send + Sync>;

struct OwnerState {
    connection_key: Option<u64>,
    client: Option<Arc<SandNotifyBusClient>>,
}

pub struct MobileNotifyBusOwner {
    bridge: Arc<dyn NotifyBusHostBridge>,
    state: Mutex<OwnerState>,
    safety_poll_enabled: AtomicBool,
    xuser_handler: Arc<Mutex<Option<XuserNotifyHandler>>>,
    stopped: AtomicBool,
}

impl MobileNotifyBusOwner {
    pub fn start(bridge: Arc<dyn NotifyBusHostBridge>) -> Result<Arc<Self>, String> {
        let owner = Arc::new(Self {
            bridge,
            state: Mutex::new(OwnerState {
                connection_key: None,
                client: None,
            }),
            safety_poll_enabled: AtomicBool::new(false),
            xuser_handler: Arc::new(Mutex::new(None)),
            stopped: AtomicBool::new(false),
        });
        // NotifyBus is an optimization/wake source. Failure to resolve an
        // optional experiment or auth snapshot must not make the canonical Host
        // unavailable; disconnected relay polling remains the fail-closed fallback.
        let _ = owner.sync();
        Ok(owner)
    }

    pub fn set_xuser_notify_handler(&self, handler: XuserNotifyHandler) {
        *self.xuser_handler.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(handler);
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
        let settings = self.bridge.request("getHostSettings")?;
        let overrides = settings.get("featureFlagOverrides").and_then(Value::as_object);
        let enabled = overrides
            .and_then(|values| values.get(SAND_NOTIFY_BUS_GATE))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let safety = overrides
            .and_then(|values| values.get(SAND_NOTIFY_SAFETY_POLL_GATE))
            .and_then(Value::as_bool)
            .unwrap_or(false);
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

        let handler = Arc::clone(&self.xuser_handler);
        let on_notify = Arc::new(move |topic: SandNotifyTopic| {
            if topic == SandNotifyTopic::XuserEvents {
                if let Some(callback) = handler
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
                {
                    callback();
                }
            }
        });
        let deps = NotifyBusClientDependencies::production(
            backend,
            access_token,
            SAND_CLIENT_TYPE.to_string(),
            sand_client_version(),
            sand_namespace().to_string(),
            on_notify,
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
        self.safety_poll_enabled.store(false, Ordering::Release);
        self.xuser_handler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
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
                "getHostSettings" => Ok(self.settings.lock().unwrap().clone()),
                "feature.auth.deviceAgentSession" => self
                    .session
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| "signed out".into()),
                _ => Err("unsupported".into()),
            }
        }
    }

    #[test]
    fn disabled_notify_bus_is_fail_closed_and_safety_gate_is_independent() {
        let bridge = Arc::new(FakeBridge {
            settings: Mutex::new(json!({
                "featureFlagOverrides": {
                    SAND_NOTIFY_BUS_GATE: false,
                    SAND_NOTIFY_SAFETY_POLL_GATE: true
                }
            })),
            session: Mutex::new(None),
        });
        let owner = MobileNotifyBusOwner::start(bridge).unwrap();
        assert!(!owner.is_connected());
        assert!(owner.is_safety_poll_enabled());
        owner.stop();
        assert!(!owner.is_safety_poll_enabled());
    }
}
