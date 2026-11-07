use std::cell::{Cell, RefCell};
use std::fs;
use std::path::PathBuf;

use mahayana_host_runtime::extensions::telemetry::host_crash_marker::{
    DeleteIfUnchangedResult, FileHostCrashMarkerStore, ForwardHostCrashMarkerResult,
    HostCrashErrorClass, HostCrashMarkerDelete, HostCrashMarkerRead, HostCrashMarkerStore,
    delete_if_unchanged, forward_host_crash_marker_with, host_crash_marker_metadata,
    parse_host_crash_marker,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::HostTelemetryService;

fn signal_marker_raw() -> &'static str {
    r#"{"schemaVersion":1,"errorClass":"signal_exit","exitSignal":"SIGSEGV","startedAtMs":1000.2,"crashedAtMs":2400.7,"uptimeMs":1400.5}"#
}

#[test]
fn parses_frozen_marker_variants_and_rejects_invalid_combinations() {
    let marker = parse_host_crash_marker(signal_marker_raw()).unwrap();
    assert_eq!(marker.error_class, HostCrashErrorClass::SignalExit);
    assert_eq!(marker.exit_signal, "SIGSEGV");
    assert_eq!(marker.started_at_ms, Some(1000.2));
    assert_eq!(marker.crashed_at_ms, 2400.7);
    assert_eq!(marker.uptime_ms, Some(1400.5));

    let nonzero = parse_host_crash_marker(
        r#"{"schemaVersion":1,"errorClass":"nonzero_exit","exitSignal":"none","startedAtMs":1,"crashedAtMs":2,"uptimeMs":1}"#,
    )
    .unwrap();
    assert_eq!(nonzero.error_class, HostCrashErrorClass::NonzeroExit);

    let unobserved = parse_host_crash_marker(
        r#"{"schemaVersion":1,"errorClass":"unobserved_exit","exitSignal":"unknown","crashedAtMs":42}"#,
    )
    .unwrap();
    assert_eq!(unobserved.error_class, HostCrashErrorClass::UnobservedExit);
    assert_eq!(unobserved.started_at_ms, None);
    assert_eq!(unobserved.uptime_ms, None);

    assert!(parse_host_crash_marker(
        r#"{"schemaVersion":1,"errorClass":"signal_exit","exitSignal":"none","startedAtMs":1,"crashedAtMs":2,"uptimeMs":1}"#
    )
    .is_none());
    assert!(parse_host_crash_marker(
        r#"{"schemaVersion":1,"errorClass":"unobserved_exit","exitSignal":"unknown","crashedAtMs":-1}"#
    )
    .is_none());
    assert!(parse_host_crash_marker("not-json").is_none());
}

#[test]
fn projects_frozen_metadata_with_rounded_millisecond_values() {
    let marker = parse_host_crash_marker(signal_marker_raw()).unwrap();
    let metadata = host_crash_marker_metadata(&marker);
    assert_eq!(metadata.get("kind").map(String::as_str), Some("process_exit"));
    assert_eq!(metadata.get("error_class").map(String::as_str), Some("signal_exit"));
    assert_eq!(metadata.get("exit_signal").map(String::as_str), Some("SIGSEGV"));
    assert_eq!(metadata.get("started_at_ms").map(String::as_str), Some("1000"));
    assert_eq!(metadata.get("crashed_at_ms").map(String::as_str), Some("2401"));
    assert_eq!(metadata.get("uptime_ms").map(String::as_str), Some("1401"));
}

fn temp_marker_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "fabushi-host-crash-marker-{}-{name}.json",
        std::process::id()
    ))
}

#[test]
fn file_store_distinguishes_absent_present_and_delete() {
    let path = temp_marker_path("file-store");
    let _ = fs::remove_file(&path);
    let store = FileHostCrashMarkerStore::new(&path);
    assert_eq!(store.read(), HostCrashMarkerRead::Absent);

    fs::write(&path, signal_marker_raw()).unwrap();
    assert_eq!(
        store.read(),
        HostCrashMarkerRead::Present(signal_marker_raw().into())
    );
    assert_eq!(store.delete(), HostCrashMarkerDelete::Deleted);
    assert_eq!(store.read(), HostCrashMarkerRead::Absent);
}

struct MemoryStore {
    reads: RefCell<Vec<HostCrashMarkerRead>>,
    delete_result: HostCrashMarkerDelete,
    delete_calls: Cell<usize>,
}

impl MemoryStore {
    fn new(reads: Vec<HostCrashMarkerRead>, delete_result: HostCrashMarkerDelete) -> Self {
        Self {
            reads: RefCell::new(reads),
            delete_result,
            delete_calls: Cell::new(0),
        }
    }
}

impl HostCrashMarkerStore for MemoryStore {
    fn read(&self) -> HostCrashMarkerRead {
        let mut reads = self.reads.borrow_mut();
        if reads.is_empty() {
            return HostCrashMarkerRead::Absent;
        }
        reads.remove(0)
    }

    fn delete(&self) -> HostCrashMarkerDelete {
        self.delete_calls.set(self.delete_calls.get() + 1);
        self.delete_result
    }
}

#[test]
fn unchanged_delete_is_race_safe() {
    let store = MemoryStore::new(
        vec![HostCrashMarkerRead::Present("new-marker".into())],
        HostCrashMarkerDelete::Deleted,
    );
    assert_eq!(
        delete_if_unchanged(&store, "old-marker"),
        DeleteIfUnchangedResult::Changed
    );
    assert_eq!(store.delete_calls.get(), 0);
}

#[test]
fn forwarding_preserves_defer_parse_dedupe_and_changed_marker_semantics() {
    let unavailable = MemoryStore::new(
        vec![HostCrashMarkerRead::Unavailable],
        HostCrashMarkerDelete::Deleted,
    );
    assert_eq!(
        forward_host_crash_marker_with(&unavailable, |_| false, |_| {}, |_| true),
        ForwardHostCrashMarkerResult::Deferred
    );

    let parse_error = MemoryStore::new(
        vec![
            HostCrashMarkerRead::Present("bad".into()),
            HostCrashMarkerRead::Present("bad".into()),
        ],
        HostCrashMarkerDelete::Deleted,
    );
    let marked = Cell::new(false);
    assert_eq!(
        forward_host_crash_marker_with(
            &parse_error,
            |_| false,
            |_| marked.set(true),
            |_| panic!("invalid marker must not emit"),
        ),
        ForwardHostCrashMarkerResult::ParseError
    );
    assert!(marked.get());

    let delivery_deferred = MemoryStore::new(
        vec![HostCrashMarkerRead::Present(signal_marker_raw().into())],
        HostCrashMarkerDelete::Deleted,
    );
    assert_eq!(
        forward_host_crash_marker_with(&delivery_deferred, |_| false, |_| {}, |_| false),
        ForwardHostCrashMarkerResult::Deferred
    );
    assert_eq!(delivery_deferred.delete_calls.get(), 0);

    let delivered = MemoryStore::new(
        vec![
            HostCrashMarkerRead::Present(signal_marker_raw().into()),
            HostCrashMarkerRead::Present(signal_marker_raw().into()),
        ],
        HostCrashMarkerDelete::Deleted,
    );
    let marked = Cell::new(false);
    assert_eq!(
        forward_host_crash_marker_with(
            &delivered,
            |_| false,
            |_| marked.set(true),
            |_| true,
        ),
        ForwardHostCrashMarkerResult::Delivered
    );
    assert!(marked.get());
    assert_eq!(delivered.delete_calls.get(), 1);

    let changed = MemoryStore::new(
        vec![
            HostCrashMarkerRead::Present(signal_marker_raw().into()),
            HostCrashMarkerRead::Present("replacement".into()),
        ],
        HostCrashMarkerDelete::Deleted,
    );
    assert_eq!(
        forward_host_crash_marker_with(&changed, |_| true, |_| {}, |_| true),
        ForwardHostCrashMarkerResult::Pending
    );
    assert_eq!(changed.delete_calls.get(), 0);
}


const HOST_TELEMETRY_SERVICE_SOURCE: &str =
    include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
const TELEMETRY_EXTENSION_SOURCE: &str =
    include_str!("../src/extensions/telemetry/extension.rs");

#[test]
fn host_telemetry_service_owns_crash_marker_forwarding_start_stop_and_rollback_boundary() {
    let marker_path = temp_marker_path("service-owned-forward");
    let records_path = temp_marker_path("service-owned-forward-records");
    let _ = fs::remove_file(&marker_path);
    let _ = fs::remove_file(&records_path);
    fs::write(&marker_path, signal_marker_raw()).expect("write crash marker");

    let service = HostTelemetryService::open(&records_path).expect("telemetry service");
    service.start().expect("telemetry service start");
    let store = FileHostCrashMarkerStore::new(&marker_path);

    assert!(
        service
            .start_host_crash_marker_forwarding(store.clone())
            .expect("start crash-marker forwarding"),
        "first service-owned start must install the runtime"
    );
    assert_eq!(store.read(), HostCrashMarkerRead::Absent);
    assert!(
        !service
            .start_host_crash_marker_forwarding(store.clone())
            .expect("idempotent crash-marker forwarding start"),
        "second start must not create a parallel crash-marker runtime"
    );
    assert!(
        service
            .stop_host_crash_marker_forwarding()
            .expect("stop crash-marker forwarding"),
        "service stop must settle the owned forwarding runtime"
    );
    assert!(
        !service
            .stop_host_crash_marker_forwarding()
            .expect("idempotent crash-marker forwarding stop"),
        "second stop must observe no parallel forwarding runtime"
    );

    let records = fs::read_to_string(&records_path).expect("telemetry records");
    let record: serde_json::Value =
        serde_json::from_str(records.lines().next().expect("crash record"))
            .expect("crash telemetry JSON");
    assert_eq!(record["channel"], "structured_log");
    assert_eq!(record["event"], "sand.host.crash");
    assert_eq!(record["payload"]["level"], "error");
    assert_eq!(record["payload"]["metadata"]["kind"], "process_exit");
    assert_eq!(record["payload"]["metadata"]["error_class"], "signal_exit");
    assert_eq!(record["payload"]["metadata"]["error_code"], "SAND-E0001");
    assert_eq!(record["payload"]["metadata"]["error_domain"], "registry");
    assert_eq!(record["payload"]["metadata"]["error_retryable"], "false");

    service.dispose().expect("telemetry service dispose");
    assert!(
        service
            .start_host_crash_marker_forwarding(store)
            .is_err(),
        "disposed service must reject a new crash-marker forwarding runtime"
    );

    let identity = HOST_TELEMETRY_SERVICE_SOURCE
        .find("pub fn set_host_bundle_identity(&self")
        .expect("service identity boundary");
    let identity_source = &HOST_TELEMETRY_SERVICE_SOURCE[identity..];
    let start = identity_source
        .find("HostCrashMarkerForwarder::start(self.logs.clone())")
        .expect("service-owned identity start");
    let shipper = identity_source
        .find("if let Err(error) = shipper.start()")
        .expect("identity-gated box-log shipper start");
    let rollback = identity_source
        .find("runtime.crash_marker_forwarder.take();")
        .expect("crash-marker rollback");
    assert!(
        start < shipper && shipper < rollback,
        "identity must start crash forwarding before the shipper and roll it back on shipper failure"
    );

    assert!(
        !HOST_TELEMETRY_SERVICE_SOURCE.contains("pub fn forward_host_crash_marker_to_logs("),
        "low-level crash-marker forwarding helper must not remain a public parallel API"
    );
    for forbidden in [
        "HostCrashMarkerForwarder",
        "forward_host_crash_marker_to_logs",
    ] {
        assert!(
            !TELEMETRY_EXTENSION_SOURCE.contains(forbidden),
            "extension must stay a thin assembly boundary and not own {forbidden}"
        );
    }

    let _ = fs::remove_file(marker_path);
    let _ = fs::remove_file(records_path);
}
