use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::telemetry::box_log_shipper::{
    BoxLogShipper, BoxLogShipperConfig, BoxTelemetryRecord, DeliverySettlement,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::HostTelemetryService;

static TEMP_ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn temp_root() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = TEMP_ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "fabushi-box-log-shipper-{}-{suffix}-{sequence}",
        std::process::id()
    ))
}

fn write(path: &Path, value: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, value).expect("write");
}

#[test]
fn shipping_box_log_owner_scans_filters_maps_and_persists_offsets() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let host_log = root.join("sand-host.log");
    let infra_log = root.join("sand-box-telemetry.log");
    let excluded_log = root.join("sand-notify-injector-worker.log");
    let browser_log = root.join("sand-window-7").join("browser.log");
    let offsets = root.join("shipper-offsets.json");
    let records_path = root.join("host-events.jsonl");

    write(&host_log, "must not recursively ship\n");
    write(
        &infra_log,
        concat!(
            "{\"kind\":\"boot_stage\",\"stage\":\"ready\",\"durationMs\":42}\n",
            "{\"kind\":\"boot_failure\",\"stage\":\"desktop\",\"reason\":\"x_display_timeout\",\"durationMs\":43}\n",
            "{\"kind\":\"egress_tunnel\",\"outcome\":\"restart\",\"attempt\":2,\"exitStatus\":1,\"runtimeS\":9}\n",
            "{\"kind\":\"host_boot_fetch\",\"outcome\":\"fallback\",\"reason\":\"download_failed\",\"durationMs\":44,\"swapMs\":3,\"fromVersion\":\"0123456\",\"toVersion\":\"abcdef0\"}\n",
            "{\"kind\":\"exec_daemon_restart\",\"restartAttempt\":1,\"runtimeS\":10,\"cause\":\"daemon_exited\",\"exitStatus\":1}\n",
            "{\"kind\":\"supervisor_restart\",\"restartAttempt\":1,\"runtimeS\":11,\"cause\":\"supervisor_exited\",\"exitStatus\":1}\n",
        ),
    );
    write(&excluded_log, "must be excluded\n");
    write(&browser_log, "browser started\n");

    let telemetry = HostTelemetryService::open(&records_path).expect("telemetry");
    let logs_for_batch = telemetry.logs.clone();
    let logs_for_ship = telemetry.logs.clone();
    let shipper = BoxLogShipper::new(
        BoxLogShipperConfig {
            log_dir: root.clone(),
            skip_paths: vec![host_log.clone()],
            offsets_path: Some(offsets.clone()),
            ..BoxLogShipperConfig::default()
        },
        Arc::new(move |records: &[BoxTelemetryRecord]| {
            records
                .iter()
                .map(|record| {
                    if logs_for_batch.report_box_log_record(record).is_ok() {
                        DeliverySettlement::Delivered
                    } else {
                        DeliverySettlement::Dropped
                    }
                })
                .collect()
        }),
        Arc::new(move |report| {
            if logs_for_ship.report_box_log_ship(report).is_ok() {
                DeliverySettlement::Delivered
            } else {
                DeliverySettlement::Dropped
            }
        }),
    );

    assert_eq!(shipper.poll_once().expect("poll"), 7);
    assert_eq!(
        shipper.offset_for(&infra_log),
        Some(fs::metadata(&infra_log).expect("infra metadata").len())
    );
    assert_eq!(
        shipper.offset_for(&browser_log),
        Some(fs::metadata(&browser_log).expect("browser metadata").len())
    );
    assert_eq!(shipper.offset_for(&host_log), None);
    assert_eq!(shipper.offset_for(&excluded_log), None);
    shipper.checkpoint_offsets().expect("checkpoint");

    let raw = fs::read_to_string(&records_path).expect("records");
    assert!(raw.contains("\"event\":\"sand.box.boot_stage\""));
    assert!(raw.contains("\"event\":\"sand.box.boot_failure\""));
    assert!(raw.contains("\"event\":\"sand.box.egress_tunnel\""));
    assert!(raw.contains("\"event\":\"sand.box.host_boot_fetch\""));
    assert!(raw.contains("\"event\":\"sand.box.exec_daemon_restart\""));
    assert!(raw.contains("\"event\":\"sand.box.supervisor_restart\""));
    assert!(raw.contains("\"event\":\"sand.box.log\""));
    assert!(raw.contains("\"source\":\"sand-window-7/browser\""));
    assert!(raw.contains("\"event\":\"sand.box.log_ship\""));

    let reloaded = BoxLogShipper::new(
        BoxLogShipperConfig {
            log_dir: root.clone(),
            offsets_path: Some(offsets),
            ..BoxLogShipperConfig::default()
        },
        Arc::new(|records| vec![DeliverySettlement::Delivered; records.len()]),
        Arc::new(|_| DeliverySettlement::Delivered),
    );
    assert_eq!(
        reloaded.offset_for(&infra_log),
        Some(fs::metadata(&infra_log).expect("infra metadata").len())
    );
    assert_eq!(
        reloaded.offset_for(&browser_log),
        Some(fs::metadata(&browser_log).expect("browser metadata").len())
    );

    drop(reloaded);
    drop(shipper);
    drop(telemetry);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn dropped_delivery_does_not_advance_the_file_offset() {
    let root = temp_root();
    fs::create_dir_all(&root).expect("root");
    let log = root.join("worker.log");
    let offsets = root.join("shipper-offsets.json");
    write(&log, "one line\n");

    let dropped = BoxLogShipper::new(
        BoxLogShipperConfig {
            log_dir: root.clone(),
            offsets_path: Some(offsets.clone()),
            ..BoxLogShipperConfig::default()
        },
        Arc::new(|records| vec![DeliverySettlement::Dropped; records.len()]),
        Arc::new(|_| DeliverySettlement::Delivered),
    );
    assert_eq!(dropped.poll_once().expect("poll"), 1);
    assert_eq!(dropped.offset_for(&log), None);

    let delivered = BoxLogShipper::new(
        BoxLogShipperConfig {
            log_dir: root.clone(),
            offsets_path: Some(offsets),
            ..BoxLogShipperConfig::default()
        },
        Arc::new(|records| vec![DeliverySettlement::Delivered; records.len()]),
        Arc::new(|_| DeliverySettlement::Delivered),
    );
    assert_eq!(delivered.poll_once().expect("poll"), 1);
    assert_eq!(
        delivered.offset_for(&log),
        Some(fs::metadata(&log).expect("metadata").len())
    );

    drop(delivered);
    drop(dropped);
    let _ = fs::remove_dir_all(root);
}


#[test]
fn infrastructure_schema_is_strict_and_fail_closed() {
    use mahayana_host_runtime::extensions::telemetry::box_log_shipper::parse_box_infrastructure_event;
    use mahayana_host_runtime::extensions::telemetry::lifecycle_telemetry::BoxInfrastructureEvent;
    use serde_json::json;

    assert_eq!(
        parse_box_infrastructure_event(&json!({
            "kind": "process_crash",
            "binary": "chrome",
            "signal": "sigsegv",
            "count": 2
        })),
        Some(BoxInfrastructureEvent::ProcessCrash {
            binary: "chrome".into(),
            signal: "sigsegv".into(),
            count: 2,
        })
    );
    assert!(parse_box_infrastructure_event(&json!({
        "kind": "process_crash",
        "binary": "not-a-frozen-binary",
        "signal": "sigsegv",
        "count": 2
    })).is_none());
    assert!(parse_box_infrastructure_event(&json!({
        "kind": "host_boot_fetch",
        "outcome": "applied",
        "reason": "applied",
        "durationMs": 7,
        "fromVersion": "not-a-sha"
    })).is_none());
}
