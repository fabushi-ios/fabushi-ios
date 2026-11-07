use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::extensions::browser_ua::extension::StopSubscription;
use crate::extensions::notify_bus::extension::HostNotifyBusExtension;
use crate::extensions::notify_bus::notify_bus_client::SandNotifyTopic;

use super::backend_relay_source::{BackendRelayRuntime, RelayStatus};
use super::backend_transport::{AutomationsBackendError, AutomationsBackendTransport};
use super::sand_automation_fire_consumer::{
    AutomationFireBackendRuntime, BackendAutomationFire, FireCompletion,
};

pub const RELAY_POLL_INTERVAL_MS: u64 = 4_000;
pub const FIRE_POLL_INTERVAL_MS: u64 = 4_000;

pub type RelayListeners = Arc<dyn Fn() -> (Vec<Value>, Vec<Value>) + Send + Sync>;
pub type RelayEventSink = Arc<dyn Fn(Value) -> bool + Send + Sync>;
pub type FireDispatch = Arc<
    dyn Fn(BackendAutomationFire, Arc<dyn Fn(Option<FireCompletion>) + Send + Sync>) -> bool
        + Send
        + Sync,
>;
pub type FirePollStateReader = Arc<dyn Fn() -> (bool, bool) + Send + Sync>;
pub type FirePollErrorSink = Arc<dyn Fn(&AutomationsBackendError) + Send + Sync>;
pub type ProductionLog = Arc<dyn Fn(&str) + Send + Sync>;

struct WakeState {
    requested: Mutex<bool>,
    wake: Condvar,
    stopped: AtomicBool,
    suspended: AtomicBool,
}

impl WakeState {
    fn new() -> Self {
        Self {
            requested: Mutex::new(true),
            wake: Condvar::new(),
            stopped: AtomicBool::new(false),
            suspended: AtomicBool::new(false),
        }
    }

    fn request(&self) {
        *self
            .requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.wake.notify_all();
    }

    fn wait(&self, timeout: Duration) {
        let guard = self
            .requested
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut guard, _) = self
            .wake
            .wait_timeout_while(guard, timeout, |requested| !*requested)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = false;
    }
}

pub struct ProductionAutomationsBackendRuntime {
    relay: Arc<Mutex<BackendRelayRuntime>>,
    fire: Arc<Mutex<AutomationFireBackendRuntime>>,
    wake: Arc<WakeState>,
    worker: Mutex<Option<JoinHandle<()>>>,
    notify_stops: Mutex<Vec<StopSubscription>>,
}

impl ProductionAutomationsBackendRuntime {
    pub fn start(
        transport: Arc<dyn AutomationsBackendTransport>,
        notify_bus: HostNotifyBusExtension,
        listeners: RelayListeners,
        relay_sink: RelayEventSink,
        fire_dispatch: FireDispatch,
        fire_poll_state: FirePollStateReader,
        fire_poll_error: FirePollErrorSink,
        log: ProductionLog,
    ) -> Arc<Self> {
        let runtime = Arc::new(Self {
            relay: Arc::new(Mutex::new(BackendRelayRuntime::new(Arc::clone(&transport)))),
            fire: Arc::new(Mutex::new(AutomationFireBackendRuntime::new(transport))),
            wake: Arc::new(WakeState::new()),
            worker: Mutex::new(None),
            notify_stops: Mutex::new(Vec::new()),
        });

        let relay_wake = Arc::clone(&runtime.wake);
        let relay_runtime = Arc::clone(&runtime.relay);
        let relay_stop = notify_bus.on_notify(
            SandNotifyTopic::ListenerEvents,
            Arc::new(move || {
                relay_runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .request_drain();
                relay_wake.request();
            }),
        );
        let fire_wake = Arc::clone(&runtime.wake);
        let fire_runtime = Arc::clone(&runtime.fire);
        let fire_stop = notify_bus.on_notify(
            SandNotifyTopic::AutomationFires,
            Arc::new(move || {
                fire_runtime
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .request_drain();
                fire_wake.request();
            }),
        );
        runtime
            .notify_stops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend([relay_stop, fire_stop]);

        let weak = Arc::downgrade(&runtime);
        let worker_notify_bus = notify_bus;
        let worker = thread::spawn(move || {
            while let Some(runtime) = weak.upgrade() {
                if runtime.wake.stopped.load(Ordering::Acquire) {
                    break;
                }
                if runtime.wake.suspended.load(Ordering::Acquire) {
                    runtime.wake.wait(Duration::from_secs(60 * 60));
                    continue;
                }
                let now = now_ms();
                let (slack, github) = listeners();
                {
                    let mut relay = runtime
                        .relay
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    relay.set_listeners(slack, github);
                    let result = relay.tick(
                        now,
                        worker_notify_bus.is_connected(),
                        worker_notify_bus.is_safety_poll_enabled(),
                        |event| relay_sink(event.clone()),
                    );
                    if let Err(error) = result {
                        log(&format!("[sand:automations] listener relay tick failed: {error}"));
                    }
                }

                let mut pending = Vec::new();
                {
                    let mut fire = runtime
                        .fire
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let (is_ready, has_server_schedulable) = fire_poll_state();
                    let result = fire.tick_with_state(
                        now,
                        worker_notify_bus.is_connected(),
                        worker_notify_bus.is_safety_poll_enabled(),
                        is_ready,
                        has_server_schedulable,
                        |event| {
                            pending.push(event.clone());
                            true
                        },
                    );
                    if let Err(error) = result {
                        fire_poll_error(&error);
                        log(&format!("[sand:automations] fire poll failed: {error}"));
                    }
                }
                for event in pending {
                    let fire_state = Arc::clone(&runtime.fire);
                    let event_id = event.id.clone();
                    let completion: Arc<dyn Fn(Option<FireCompletion>) + Send + Sync> =
                        Arc::new(move |completion| {
                            let mut state = fire_state
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            match completion {
                                Some(completion) => {
                                    let _ = state.complete(&event_id, completion);
                                }
                                None => state.abandon(&event_id),
                            }
                        });
                    if !fire_dispatch(event.clone(), Arc::clone(&completion)) {
                        runtime
                            .fire
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .abandon(&event.id);
                    }
                }

                runtime.wake.wait(Duration::from_millis(
                    RELAY_POLL_INTERVAL_MS.min(FIRE_POLL_INTERVAL_MS),
                ));
            }
        });
        *runtime
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(worker);
        runtime
    }

    pub fn listener_source_status(&self, platform: &str) -> Option<RelayStatus> {
        let relay = self
            .relay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match platform {
            "slack" => Some(relay.slack_status.clone()),
            "github" => Some(relay.github_status.clone()),
            _ => None,
        }
    }

    pub fn suspend(&self) {
        if self.wake.stopped.load(Ordering::Acquire) {
            return;
        }
        self.wake.suspended.store(true, Ordering::Release);
        self.fire
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stop();
        self.wake.request();
    }

    pub fn resume(&self) {
        if self.wake.stopped.load(Ordering::Acquire) {
            return;
        }
        self.fire
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .start();
        self.wake.suspended.store(false, Ordering::Release);
        self.wake.request();
    }

    pub fn request_reconcile(&self) {
        self.relay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .request_drain();
        self.fire
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reset_poll_delay();
        self.wake.request();
    }

    pub fn stop(&self) {
        if self.wake.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.relay
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .set_listeners(Vec::new(), Vec::new());
        self.fire
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stop();
        let mut stops = self
            .notify_stops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while let Some(stop) = stops.pop() {
            stop();
        }
        drop(stops);
        self.wake.request();
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for ProductionAutomationsBackendRuntime {
    fn drop(&mut self) {
        if !self.wake.stopped.swap(true, Ordering::AcqRel) {
            self.wake.request();
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
