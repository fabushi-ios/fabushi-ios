use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mahayana_host_runtime::extensions::codebase_telemetry::codebase_telemetry_adapter::{
    AdapterCloseResult, CsnapsCodebaseTelemetryAdapter, CsnapsHandle,
    CODEBASE_ENVIRONMENT_SAND_BOX, to_adapter_state, to_codebase,
};
use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_process::{
    CsnapsProcessError,
};
use mahayana_host_runtime::extensions::codebase_telemetry::csnaps_protocol::{
    CsnapsRejectedCodebase, CsnapsServiceState, CsnapsTrackedCodebase,
};
use serde_json::{Map, Value, json};

#[derive(Default)]
struct FakeCsnaps {
    apply_result: Mutex<Option<(CsnapsServiceState, Vec<CsnapsRejectedCodebase>)>>,
    snapshots: Mutex<Vec<Value>>,
    uploads: AtomicUsize,
    flushes: AtomicUsize,
    closes: AtomicUsize,
    terminal: Mutex<Option<CsnapsProcessError>>,
}

impl CsnapsHandle for FakeCsnaps {
    fn apply_codebase_specs(
        &self,
        _codebases: Vec<Value>,
    ) -> Result<(CsnapsServiceState, Vec<CsnapsRejectedCodebase>), CsnapsProcessError> {
        self.apply_result
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| CsnapsProcessError::new("missing apply result"))
    }

    fn snapshot(&self, reason: Value) -> Result<(), CsnapsProcessError> {
        self.snapshots.lock().unwrap().push(reason);
        Ok(())
    }

    fn trigger_upload(&self, _credentials: Map<String, Value>) -> Result<(), CsnapsProcessError> {
        self.uploads.fetch_add(1, Ordering::SeqCst);
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

fn tracked(path: &str, kind: f64, environment: f64) -> CsnapsTrackedCodebase {
    CsnapsTrackedCodebase {
        codebase_uuid: "123e4567-e89b-42d3-a456-426614174000".into(),
        path: path.into(),
        kind,
        environment,
    }
}

fn wait_until(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("condition did not become true");
}

#[test]
fn state_projection_rejects_non_sand_or_unknown_kinds() {
    let valid = tracked("/workspace", 1.0, CODEBASE_ENVIRONMENT_SAND_BOX);
    assert_eq!(to_codebase("auth", &valid).unwrap().auth_id, "auth");
    assert!(to_codebase("auth", &tracked("/x", 3.0, 1.0)).is_err());
    assert!(to_codebase("auth", &tracked("/x", 1.0, 2.0)).is_err());
    let projected = to_adapter_state(
        "auth",
        &CsnapsServiceState { tracked: vec![valid] },
    )
    .unwrap();
    assert!(projected.is_within_storage_budget);
    assert_eq!(projected.tracked_codebases.len(), 1);
}

#[test]
fn desired_codebases_reconcile_state_start_upload_and_flush() {
    let fake = Arc::new(FakeCsnaps::default());
    *fake.apply_result.lock().unwrap() = Some((
        CsnapsServiceState {
            tracked: vec![tracked("/workspace", 1.0, 1.0)],
        },
        vec![],
    ));
    let adapter = CsnapsCodebaseTelemetryAdapter::from_handle(
        fake.clone(),
        "auth",
        CsnapsServiceState { tracked: vec![] },
        Arc::new(|| Ok(Map::from_iter([("authToken".into(), json!("secret"))]))),
        Duration::from_millis(10),
    );
    adapter
        .set_desired_codebases(vec![json!({"path":"/workspace","kind":1})])
        .unwrap();
    assert_eq!(adapter.state().tracked_codebases.len(), 1);
    wait_until(|| fake.uploads.load(Ordering::SeqCst) > 0);
    adapter.flush_pending_uploads().unwrap();
    assert_eq!(fake.flushes.load(Ordering::SeqCst), 1);
    adapter.snapshot(json!({"type":"AGENT_REQUEST_START"})).unwrap();
    assert_eq!(fake.snapshots.lock().unwrap().len(), 1);
    assert_eq!(adapter.close(), AdapterCloseResult::Closed);
    assert_eq!(fake.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn rejected_required_codebase_does_not_publish_state() {
    let fake = Arc::new(FakeCsnaps::default());
    *fake.apply_result.lock().unwrap() = Some((
        CsnapsServiceState { tracked: vec![] },
        vec![CsnapsRejectedCodebase {
            path: "/workspace".into(),
            kind: 1.0,
        }],
    ));
    let adapter = CsnapsCodebaseTelemetryAdapter::from_handle(
        fake,
        "auth",
        CsnapsServiceState { tracked: vec![] },
        Arc::new(|| Ok(Map::new())),
        Duration::from_secs(60),
    );
    let error = adapter
        .set_desired_codebases(vec![json!({"path":"/workspace","kind":1})])
        .unwrap_err();
    assert!(error.to_string().contains("rejected a required codebase"));
    assert!(adapter.state().tracked_codebases.is_empty());
}

#[test]
fn terminal_failure_degrades_state_and_is_visible_to_service_owner() {
    let fake = Arc::new(FakeCsnaps::default());
    let adapter = CsnapsCodebaseTelemetryAdapter::from_handle(
        fake.clone(),
        "auth",
        CsnapsServiceState {
            tracked: vec![tracked("/workspace", 1.0, 1.0)],
        },
        Arc::new(|| Ok(Map::new())),
        Duration::from_secs(60),
    );
    *fake.terminal.lock().unwrap() = Some(CsnapsProcessError::new("boom"));
    wait_until(|| adapter.terminal_failure().is_some());
    assert!(!adapter.state().is_within_storage_budget);
    assert!(adapter.state().tracked_codebases.is_empty());
}
