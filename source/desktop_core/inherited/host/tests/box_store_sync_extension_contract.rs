use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::box_store_sync::box_store_diagnostics::report_box_store_diagnostic;
use mahayana_host_runtime::extensions::box_store_sync::extension::{
    BOX_STORE_DB_DEBOUNCE_MS, BOX_STORE_MANIFEST_RETRY_ATTEMPTS,
    BOX_STORE_MANIFEST_RETRY_DELAY_MS, BOX_STORE_SYNC_DEPENDENCIES,
    BOX_STORE_SYNC_EXTENSION_ID, BOX_STORE_SYNC_INTERVAL_MS,
    BoxStoreScheduling, BoxStoreSyncExtensionDeps, BoxStoreSyncService,
    BoxStoreSyncServiceFactory, CHROME_SESSION_CHANGE_DEBOUNCE_MS,
    DebouncePolicyConfig, PollingPolicyConfig, RetryPolicyConfig, RetryPredicate,
    start_box_store_sync_extension,
};
use mahayana_host_runtime::extensions::extension_ids_generated::HostExtensionId;
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Api(&'static str);

struct FakeService {
    started: Arc<AtomicUsize>,
    disposed: Arc<AtomicUsize>,
}

impl BoxStoreSyncService for FakeService {
    type Api = Api;

    fn api(&self) -> Self::Api {
        Api("box-store-api")
    }

    fn start(&self) {
        self.started.fetch_add(1, Ordering::SeqCst);
    }

    fn dispose(&self) {
        self.disposed.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeFactory {
    seen: Arc<Mutex<Option<BoxStoreScheduling>>>,
    idle_value: Arc<AtomicBool>,
    started: Arc<AtomicUsize>,
    disposed: Arc<AtomicUsize>,
}

impl BoxStoreSyncServiceFactory for FakeFactory {
    type Service = FakeService;

    fn create(&self, deps: BoxStoreSyncExtensionDeps) -> Self::Service {
        *self.seen.lock().unwrap() = Some(deps.scheduling.clone());
        assert_eq!((deps.is_idle)(), self.idle_value.load(Ordering::SeqCst));
        assert_eq!((deps.resolve_store_id)().unwrap(), "source-map-store");
        (deps.log)("box store factory created");
        FakeService {
            started: Arc::clone(&self.started),
            disposed: Arc::clone(&self.disposed),
        }
    }
}

#[test]
fn frozen_extension_identity_dependencies_and_scheduling_are_preserved() {
    assert_eq!(BOX_STORE_SYNC_EXTENSION_ID, HostExtensionId::BoxStoreSync);
    assert_eq!(
        BOX_STORE_SYNC_DEPENDENCIES,
        [
            HostExtensionId::Auth,
            HostExtensionId::Mcp,
            HostExtensionId::SourceMap,
            HostExtensionId::Telemetry,
        ]
    );

    let scheduling = BoxStoreScheduling::default();
    assert_eq!(
        scheduling.polling,
        PollingPolicyConfig {
            name: "box-store-sync-cycle",
            interval_ms: BOX_STORE_SYNC_INTERVAL_MS,
        }
    );
    assert_eq!(
        scheduling.store_db_debounce,
        DebouncePolicyConfig {
            name: "box-store-sync-store-db",
            delay_ms: BOX_STORE_DB_DEBOUNCE_MS,
        }
    );
    assert_eq!(
        scheduling.chrome_session_debounce,
        DebouncePolicyConfig {
            name: "box-store-sync-chrome-session",
            delay_ms: CHROME_SESSION_CHANGE_DEBOUNCE_MS,
        }
    );
    assert_eq!(
        scheduling.manifest_retry,
        RetryPolicyConfig {
            name: "box-store-sync-manifest-cas",
            max_attempts: BOX_STORE_MANIFEST_RETRY_ATTEMPTS,
            initial_delay_ms: BOX_STORE_MANIFEST_RETRY_DELAY_MS,
            max_delay_ms: BOX_STORE_MANIFEST_RETRY_DELAY_MS,
            predicate: RetryPredicate::CanonicalWriteConflict,
        }
    );
    assert_eq!(
        scheduling.chrome_stage_retry.predicate,
        RetryPredicate::ChromeSessionStageRetryable
    );
}

#[test]
fn extension_pins_diagnostics_starts_service_projects_api_and_disposes() {
    let seen = Arc::new(Mutex::new(None));
    let idle_value = Arc::new(AtomicBool::new(true));
    let started = Arc::new(AtomicUsize::new(0));
    let disposed = Arc::new(AtomicUsize::new(0));
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let diagnostics = Arc::new(Mutex::new(Vec::<Map<String, Value>>::new()));

    let factory = FakeFactory {
        seen: Arc::clone(&seen),
        idle_value: Arc::clone(&idle_value),
        started: Arc::clone(&started),
        disposed: Arc::clone(&disposed),
    };
    let log = {
        let logs = Arc::clone(&logs);
        Arc::new(move |message: &str| logs.lock().unwrap().push(message.to_string()))
    };
    let reporter = {
        let diagnostics = Arc::clone(&diagnostics);
        Arc::new(move |diagnostic: &Map<String, Value>| {
            diagnostics.lock().unwrap().push(diagnostic.clone())
        })
    };

    let extension = start_box_store_sync_extension(
        &factory,
        BoxStoreSyncExtensionDeps {
            native_credentials: None,
            is_idle: Arc::new(|| true),
            resolve_store_id: Arc::new(|| Ok("source-map-store".to_string())),
            backend_url: None,
            get_access_token: None,
            get_machine_id: None,
            log,
            report_host_extension_diagnostic: reporter,
            report_box_store_sync_cycle: Arc::new(|_, _| {}),
            report_box_store_db_capture: Arc::new(|_, _| {}),
            report_box_store_manifest_conflict: Arc::new(|_, _| {}),
            report_chrome_session_stage: Arc::new(|_, _| {}),
            scheduling: BoxStoreScheduling::default(),
        },
    );

    assert_eq!(extension.api(), Api("box-store-api"));
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(disposed.load(Ordering::SeqCst), 0);
    assert_eq!(logs.lock().unwrap().as_slice(), ["box store factory created"]);
    assert!(seen.lock().unwrap().is_some());

    let mut diagnostic = Map::new();
    diagnostic.insert("phase".into(), Value::String("snapshot".into()));
    report_box_store_diagnostic(&diagnostic);
    assert_eq!(
        diagnostics.lock().unwrap()[0].get("phase").and_then(Value::as_str),
        Some("snapshot")
    );

    drop(extension);
    assert_eq!(disposed.load(Ordering::SeqCst), 1);

    report_box_store_diagnostic(&diagnostic);
    assert_eq!(
        diagnostics.lock().unwrap().len(),
        1,
        "extension teardown must unpin the process-global reporter"
    );
}
