use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::host_event_bus::{
    HostEventFailureMode, HostEventSubscription, SandHostEventBus,
};

use super::codebase_snapshot_trigger::{
    CodebaseSnapshotTrigger, SnapshotReason, SnapshotReasonType,
};
use super::codebase_telemetry_adapter::{
    AdapterCloseResult, CsnapsAdapterError, CsnapsCodebaseTelemetryAdapter,
};
use super::codebase_telemetry_host::{
    AuthListener, SandCodebaseTelemetryHost,
};
use super::privacy_mode::TelemetryAuth;

pub trait ServiceSubscription: Send {}
impl<T: Send> ServiceSubscription for T {}

pub trait CodebaseTelemetryHostApi: Send + Sync {
    fn auth(&self) -> Option<TelemetryAuth>;
    fn subscribe_auth(&self, listener: AuthListener) -> Box<dyn ServiceSubscription>;
    fn desired_codebases(&self) -> Vec<Value>;
    fn create_adapter(
        &self,
        auth: TelemetryAuth,
    ) -> Result<Arc<CsnapsCodebaseTelemetryAdapter>, CsnapsAdapterError>;
    fn dispose_host(&self);
}

impl CodebaseTelemetryHostApi for SandCodebaseTelemetryHost {
    fn auth(&self) -> Option<TelemetryAuth> {
        SandCodebaseTelemetryHost::auth(self)
    }

    fn subscribe_auth(&self, listener: AuthListener) -> Box<dyn ServiceSubscription> {
        Box::new(SandCodebaseTelemetryHost::subscribe_auth(self, listener))
    }

    fn desired_codebases(&self) -> Vec<Value> {
        SandCodebaseTelemetryHost::desired_codebases(self)
    }

    fn create_adapter(
        &self,
        auth: TelemetryAuth,
    ) -> Result<Arc<CsnapsCodebaseTelemetryAdapter>, CsnapsAdapterError> {
        SandCodebaseTelemetryHost::create_adapter(self, auth)
    }

    fn dispose_host(&self) {
        self.dispose();
        self.privacy_mode().dispose();
    }
}

pub type ServiceLog = Arc<dyn Fn(&str) + Send + Sync>;

pub fn create_sand_codebase_telemetry_logger(log: ServiceLog) -> ServiceLog {
    Arc::new(move |message| {
        log(&format!("[codebase-telemetry] {message}"));
    })
}

pub struct CodebaseTelemetryService {
    host: Arc<dyn CodebaseTelemetryHostApi>,
    events: SandHostEventBus,
    adapter: Mutex<Option<Arc<CsnapsCodebaseTelemetryAdapter>>>,
    generation: AtomicU64,
    stopping: AtomicBool,
    restart_in_flight: AtomicBool,
    restart_delay: Duration,
    shutdown_deadline: Duration,
    logger: ServiceLog,
    auth_subscription: Mutex<Option<Box<dyn ServiceSubscription>>>,
    event_subscriptions: Mutex<Vec<HostEventSubscription>>,
}

impl CodebaseTelemetryService {
    pub fn start(
        host: Arc<dyn CodebaseTelemetryHostApi>,
        events: SandHostEventBus,
        restart_delay: Duration,
        shutdown_deadline: Duration,
        logger: ServiceLog,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            host,
            events,
            adapter: Mutex::new(None),
            generation: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            restart_in_flight: AtomicBool::new(false),
            restart_delay,
            shutdown_deadline,
            logger,
            auth_subscription: Mutex::new(None),
            event_subscriptions: Mutex::new(Vec::new()),
        });
        service.install();
        service
    }

    pub fn flush_pending_uploads(&self) -> Result<(), CsnapsAdapterError> {
        let adapter = self
            .adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        match adapter {
            Some(adapter) => adapter.flush_pending_uploads(),
            None => Ok(()),
        }
    }

    pub fn current_adapter(&self) -> Option<Arc<CsnapsCodebaseTelemetryAdapter>> {
        self.adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn dispose(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.auth_subscription
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        self.event_subscriptions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();

        let previous = self
            .adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(adapter) = previous {
            let (send, receive) = mpsc::sync_channel(1);
            let _ = thread::Builder::new()
                .name("codebase-telemetry-shutdown".into())
                .spawn(move || {
                    let _ = send.send(adapter.close());
                });
            if receive.recv_timeout(self.shutdown_deadline).is_err() {
                (self.logger)("warn: Controller shutdown exceeded its deadline");
            }
        }
        self.host.dispose_host();
    }

    fn install(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let auth_subscription = self.host.subscribe_auth(Arc::new(move |credentials| {
            if let Some(service) = weak.upgrade() {
                service.ensure_adapter(credentials);
            }
        }));
        *self
            .auth_subscription
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(auth_subscription);

        let weak = Arc::downgrade(self);
        let start = self.events.on("transcript.run-started", move |event| {
            if let Some(service) = weak.upgrade() {
                service.snapshot_event(event, SnapshotReasonType::AgentRequestStart);
            }
            Ok(())
        });
        let weak = Arc::downgrade(self);
        let end = self.events.on("transcript.run-ended", move |event| {
            if let Some(service) = weak.upgrade() {
                service.snapshot_event(event, SnapshotReasonType::AgentRequestEnd);
            }
            Ok(())
        });
        self.event_subscriptions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .extend([start, end]);

        self.ensure_adapter(self.host.auth());
    }

    fn snapshot_event(&self, event: &Value, reason_type: SnapshotReasonType) {
        let Some(request_id) = event
            .get("requestId")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return;
        };
        let adapter = self
            .adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let Some(adapter) = adapter else {
            return;
        };
        let trigger = CodebaseSnapshotTrigger::new(
            Arc::new(move || {
                let adapter = Arc::clone(&adapter);
                Some(Arc::new(move |reason: SnapshotReason| {
                    adapter
                        .snapshot(serde_json::json!({
                            "type": reason.reason_type.as_str(),
                            "requestId": reason.request_id,
                        }))
                        .map_err(|error| error.to_string())
                }))
            }),
            Arc::new(|_| {}),
            Arc::new(|_, _| {}),
        );
        trigger.handle(SnapshotReason {
            reason_type,
            request_id,
        });
    }

    fn ensure_adapter(self: &Arc<Self>, credentials: Option<TelemetryAuth>) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let previous = self
            .adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        if let Some(previous) = previous {
            let _ = previous.close();
        }
        if credentials.is_none()
            || self.stopping.load(Ordering::Acquire)
            || generation != self.generation.load(Ordering::Acquire)
        {
            return;
        }
        let credentials = credentials.expect("checked");
        let created = match self.host.create_adapter(credentials) {
            Ok(adapter) => adapter,
            Err(error) => {
                (self.logger)(&format!(
                    "error: Failed to start Codebase Telemetry adapter: {error}"
                ));
                return;
            }
        };
        if self.stopping.load(Ordering::Acquire)
            || generation != self.generation.load(Ordering::Acquire)
        {
            let _ = created.close();
            return;
        }
        if let Err(error) = created.set_desired_codebases(self.host.desired_codebases()) {
            (self.logger)(&format!(
                "error: Failed to start Codebase Telemetry adapter: {error}"
            ));
            let _ = created.close();
            return;
        }
        *self
            .adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(Arc::clone(&created));
        self.monitor_adapter(created);
    }

    fn monitor_adapter(self: &Arc<Self>, adapter: Arc<CsnapsCodebaseTelemetryAdapter>) {
        let weak: Weak<Self> = Arc::downgrade(self);
        let _ = thread::Builder::new()
            .name("codebase-telemetry-adapter-monitor".into())
            .spawn(move || loop {
                thread::sleep(Duration::from_millis(50));
                let Some(service) = weak.upgrade() else {
                    return;
                };
                if service.stopping.load(Ordering::Acquire) {
                    return;
                }
                if adapter.terminal_failure().is_none() {
                    continue;
                }
                let current = service
                    .adapter
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .clone();
                if !current
                    .as_ref()
                    .is_some_and(|value| Arc::ptr_eq(value, &adapter))
                {
                    return;
                }
                service.schedule_restart(adapter);
                return;
            });
    }

    fn schedule_restart(self: &Arc<Self>, failed: Arc<CsnapsCodebaseTelemetryAdapter>) {
        if self.stopping.load(Ordering::Acquire)
            || self.restart_in_flight.swap(true, Ordering::SeqCst)
        {
            return;
        }
        let current = self
            .adapter
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        if !current
            .as_ref()
            .is_some_and(|value| Arc::ptr_eq(value, &failed))
        {
            self.restart_in_flight.store(false, Ordering::SeqCst);
            return;
        }

        let weak = Arc::downgrade(self);
        let delay = self.restart_delay;
        let _ = thread::Builder::new()
            .name("codebase-telemetry-restart".into())
            .spawn(move || {
                let Some(service) = weak.upgrade() else {
                    return;
                };
                service.ensure_adapter(None);
                if service.stopping.load(Ordering::Acquire) {
                    service.restart_in_flight.store(false, Ordering::SeqCst);
                    return;
                }
                thread::sleep(delay);
                if !service.stopping.load(Ordering::Acquire) {
                    service.ensure_adapter(service.host.auth());
                }
                service.restart_in_flight.store(false, Ordering::SeqCst);
            });
    }
}

impl Drop for CodebaseTelemetryService {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[allow(dead_code)]
fn _failure_mode_anchor() -> HostEventFailureMode {
    HostEventFailureMode::Continue
}

#[allow(dead_code)]
fn _close_result_anchor(value: AdapterCloseResult) -> AdapterCloseResult {
    value
}
