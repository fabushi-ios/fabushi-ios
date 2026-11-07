use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::codebase_telemetry::codebase_telemetry_adapter::{
    CsnapsCodebaseTelemetryAdapter, CsnapsHandle,
};
use mahayana_host_runtime::extensions::codebase_telemetry::codebase_telemetry_host::{
    AuthListener,
};
use mahayana_host_runtime::extensions::codebase_telemetry::codebase_telemetry_service::{
    CodebaseTelemetryHostApi, CodebaseTelemetryService, ServiceSubscription,
};
use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_process::CsnapsProcessError;
use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_protocol::{
    CsnapsRejectedCodebase, CsnapsServiceState, CsnapsTrackedCodebase,
};
use mahayana_host_runtime::extensions::codebase_telemetry::privacy_mode::TelemetryAuth;
use mahayana_host_runtime::host_event_bus::{HostEventFailureMode, SandHostEventBus};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct FakeCsnaps {
    snapshots: Mutex<Vec<Value>>,
    flushes: AtomicUsize,
    closes: AtomicUsize,
    terminal: Mutex<Option<CsnapsProcessError>>,
}

impl CsnapsHandle for FakeCsnaps {
    fn apply_codebase_specs(
        &self,
        _codebases: Vec<Value>,
    ) -> Result<(CsnapsServiceState, Vec<CsnapsRejectedCodebase>), CsnapsProcessError> {
        Ok((
            CsnapsServiceState {
                tracked: vec![CsnapsTrackedCodebase {
                    codebase_uuid: "123e4567-e89b-42d3-a456-426614174000".into(),
                    path: "/workspace".into(),
                    kind: 1.0,
                    environment: 1.0,
                }],
            },
            vec![],
        ))
    }

    fn snapshot(&self, reason: Value) -> Result<(), CsnapsProcessError> {
        self.snapshots.lock().unwrap().push(reason);
        Ok(())
    }

    fn trigger_upload(&self, _credentials: Map<String, Value>) -> Result<(), CsnapsProcessError> {
        Ok(())
    }

    fn flush_pending_uploads(
        &self,
        _credentials: Map<String, Value>,
    ) -> Result<(), CsnapsProcessError> {
        self.flushes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn close(&self) -> Result<(), CsnapsProcessError> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn terminal_failure(&self) -> Option<CsnapsProcessError> {
        self.terminal.lock().unwrap().clone()
    }
}

struct NoopSubscription;
impl Drop for NoopSubscription {
    fn drop(&mut self) {}
}

struct FakeHost {
    auth: Mutex<Option<TelemetryAuth>>,
    listeners: Mutex<Vec<AuthListener>>,
    adapters: Mutex<VecDeque<Arc<CsnapsCodebaseTelemetryAdapter>>>,
    creates: AtomicUsize,
    disposed: AtomicBool,
}

impl CodebaseTelemetryHostApi for FakeHost {
    fn auth(&self) -> Option<TelemetryAuth> {
        self.auth.lock().unwrap().clone()
    }

    fn subscribe_auth(&self, listener: AuthListener) -> Box<dyn ServiceSubscription> {
        self.listeners.lock().unwrap().push(listener);
        Box::new(NoopSubscription)
    }

    fn desired_codebases(&self) -> Vec<Value> {
        vec![json!({"path":"/workspace","kind":1})]
    }

    fn create_adapter(
        &self,
        _auth: TelemetryAuth,
    ) -> Result<Arc<CsnapsCodebaseTelemetryAdapter>, mahayana_host_runtime::extensions::codebase_telemetry::codebase_telemetry_adapter::CsnapsAdapterError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        self.adapters
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| mahayana_host_runtime::extensions::codebase_telemetry::codebase_telemetry_adapter::CsnapsAdapterError::new("no adapter"))
    }

    fn dispose_host(&self) {
        self.disposed.store(true, Ordering::SeqCst);
    }
}

fn adapter(handle: Arc<FakeCsnaps>) -> Arc<CsnapsCodebaseTelemetryAdapter> {
    CsnapsCodebaseTelemetryAdapter::from_handle(
        handle,
        "auth-user",
        CsnapsServiceState { tracked: vec![] },
        Arc::new(|| Ok(Map::new())),
        Duration::from_secs(60),
    )
}

fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("condition did not become true");
}

#[test]
fn service_starts_snapshots_flushes_restarts_and_disposes() {
    let first_handle = Arc::new(FakeCsnaps::default());
    let second_handle = Arc::new(FakeCsnaps::default());
    let first_adapter = adapter(Arc::clone(&first_handle));
    let second_adapter = adapter(Arc::clone(&second_handle));
    let host = Arc::new(FakeHost {
        auth: Mutex::new(Some(TelemetryAuth {
            auth_id: "auth-user".into(),
            auth_token: "token".into(),
        })),
        listeners: Mutex::new(Vec::new()),
        adapters: Mutex::new(VecDeque::from([
            first_adapter,
            second_adapter,
        ])),
        creates: AtomicUsize::new(0),
        disposed: AtomicBool::new(false),
    });
    let events = SandHostEventBus::default();
    let service = CodebaseTelemetryService::start(
        host.clone(),
        events.clone(),
        Duration::from_millis(10),
        Duration::from_secs(1),
        Arc::new(|_| {}),
    );

    wait_until(|| host.creates.load(Ordering::SeqCst) == 1);
    service.flush_pending_uploads().unwrap();
    assert_eq!(first_handle.flushes.load(Ordering::SeqCst), 1);

    events
        .emit_topic(
            "transcript.run-started",
            &json!({"requestId":"req-1"}),
            HostEventFailureMode::Reject,
        )
        .unwrap();
    wait_until(|| !first_handle.snapshots.lock().unwrap().is_empty());
    assert_eq!(
        first_handle.snapshots.lock().unwrap()[0]["type"],
        "AGENT_REQUEST_START"
    );

    *first_handle.terminal.lock().unwrap() = Some(CsnapsProcessError::new("boom"));
    wait_until(|| host.creates.load(Ordering::SeqCst) >= 2);
    wait_until(|| service.current_adapter().is_some());

    service.dispose();
    assert!(host.disposed.load(Ordering::SeqCst));
    assert!(first_handle.closes.load(Ordering::SeqCst) >= 1);
    assert!(second_handle.closes.load(Ordering::SeqCst) >= 1);
}
