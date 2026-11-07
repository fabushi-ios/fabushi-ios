use std::collections::BTreeMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::box_lifecycle::RecreateSandBoxResponse;
use mahayana_host_runtime::extensions::forever_box::{
    ForeverBoxLifecycle, ForeverBoxService, HostBox,
    is_host_bundle_auto_update_enabled, is_image_auto_update_enabled,
};
use mahayana_host_runtime::extensions::forever_box::forever_box_service::{
    decode_computer_use_screenshot_base64, encode_computer_use_screenshot_request,
};
use mahayana_host_runtime::r#box::production::ProductionBoxEnvironment;
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::HostTelemetryService;
use mahayana_host_runtime::extensions::telemetry::lifecycle_telemetry::DaemonPingReport;

#[derive(Default)]
struct FakeLifecycle {
    image_update_available: bool,
    recreates: Mutex<Vec<(bool, Option<bool>)>>,
}

impl ForeverBoxLifecycle for FakeLifecycle {
    fn fetch_image_update_available(&self) -> Result<bool, String> {
        Ok(self.image_update_available)
    }

    fn recreate_in_box(
        &self,
        preserve_data: bool,
        force: Option<bool>,
    ) -> Result<RecreateSandBoxResponse, String> {
        self.recreates
            .lock()
            .expect("recreate mutex")
            .push((preserve_data, force));
        Ok(RecreateSandBoxResponse {
            started: true,
            reason: None,
        })
    }
}

#[test]
fn forever_box_options_match_frozen_store_and_auto_update_gates() {
    let mut environment = BTreeMap::new();
    assert!(!is_image_auto_update_enabled(&environment));
    assert!(is_host_bundle_auto_update_enabled(&environment));

    environment.insert("SAND_BOX_STORE_SYNC".into(), "true".into());
    environment.insert("SAND_BOX_STORE_COPY_IN".into(), "1".into());
    assert!(is_image_auto_update_enabled(&environment));

    environment.insert("SAND_BOX_AUTO_UPDATE".into(), "false".into());
    assert!(!is_image_auto_update_enabled(&environment));
    assert!(!is_host_bundle_auto_update_enabled(&environment));
}

#[test]
fn forever_box_service_composes_host_box_and_box_lifecycle_without_parallel_runtime() {
    let lifecycle = Arc::new(FakeLifecycle {
        image_update_available: true,
        recreates: Mutex::new(Vec::new()),
    });
    let box_ = HostBox::new(ProductionBoxEnvironment::new(
        "127.0.0.1",
        9,
        "test-token",
    ));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let observed_for_listener = Arc::clone(&observed);
    box_.subscribe(Arc::new(move |status| {
        observed_for_listener
            .lock()
            .expect("status mutex")
            .push(status.clone());
    }));

    let initial = box_.get_status("agent-a");
    assert_eq!(initial.state, "absent");
    assert_eq!(initial.image_update_available, None);

    let service = ForeverBoxService::new(
        box_.clone(),
        lifecycle.clone(),
        true,
        false,
        true,
    );
    assert!(service.refresh_image_update_available().expect("image state"));
    assert_eq!(service.get_status("agent-a").image_update_available, Some(true));

    let reset = service.reset("agent-a").expect("reset request");
    assert_eq!(reset.state, "running");
    assert_eq!(reset.pull_percent, Some(0));

    let update = service.update("agent-a", Some(true)).expect("update request");
    assert_eq!(update.state, "running");
    assert_eq!(
        lifecycle.recreates.lock().expect("recreate mutex").as_slice(),
        &[(false, None), (true, Some(true))]
    );
    assert!(
        observed
            .lock()
            .expect("status mutex")
            .iter()
            .any(|status| status.image_update_available == Some(true))
    );
}

#[test]
fn forever_box_auto_update_preserves_reference_skip_reasons() {
    let lifecycle = Arc::new(FakeLifecycle {
        image_update_available: true,
        recreates: Mutex::new(Vec::new()),
    });
    let outside = ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", 9, "token")),
        lifecycle.clone(),
        true,
        false,
        false,
    );
    assert_eq!(
        outside.auto_update_now().expect("outside result").reason.as_deref(),
        Some("not-in-box")
    );

    let disabled = ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", 9, "token")),
        lifecycle,
        false,
        false,
        true,
    );
    assert_eq!(
        disabled
            .auto_update_now()
            .expect("disabled result")
            .reason
            .as_deref(),
        Some("auto-update-disabled")
    );
}


#[test]
fn forever_box_screenshot_codec_matches_generated_computer_use_contract() {
    assert_eq!(
        encode_computer_use_screenshot_request("t"),
        vec![0x0a, 0x01, b't', 0x12, 0x02, 0x52, 0x00]
    );

    let encoded_result = vec![
        0x0a, 0x06, // ComputerUseResult.success
        0x1a, 0x04, b'Y', b'W', b'J', b'j', // ComputerUseSuccess.screenshot
    ];
    assert_eq!(
        decode_computer_use_screenshot_base64(&encoded_result).as_deref(),
        Some("YWJj")
    );
    assert_eq!(decode_computer_use_screenshot_base64(&[0x12, 0x00]), None);
}


#[test]
fn forever_box_image_check_routes_real_outcomes_to_unique_host_owner() {
    let root = std::env::temp_dir().join(format!(
        "fabushi-forever-box-image-check-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let telemetry = HostTelemetryService::open(root.join("telemetry.jsonl"))
        .expect("telemetry");

    let lifecycle = Arc::new(FakeLifecycle {
        image_update_available: true,
        recreates: Mutex::new(Vec::new()),
    });
    let inside = ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", 9, "token")),
        lifecycle,
        true,
        false,
        true,
    )
    .with_recreate_telemetry(telemetry.logs.clone());
    assert!(
        inside
            .refresh_image_update_available_for("seed")
            .expect("image check")
    );

    let outside = ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", 9, "token")),
        Arc::new(FakeLifecycle::default()),
        true,
        false,
        false,
    )
    .with_recreate_telemetry(telemetry.logs.clone());
    assert!(
        !outside
            .refresh_image_update_available_for("manual")
            .expect("outside skip")
    );

    let text = fs::read_to_string(telemetry.records_path()).expect("telemetry jsonl");
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"event\":\"sand.box.image_check\""));
    assert!(lines[0].contains("\"trigger\":\"seed\""));
    assert!(lines[0].contains("\"outcome\":\"answered\""));
    assert!(lines[1].contains("\"trigger\":\"manual\""));
    assert!(lines[1].contains("\"outcome\":\"skipped\""));
    assert!(lines[1].contains("\"skip_reason\":\"outside_box\""));

    let _ = fs::remove_dir_all(root);
}


#[test]
fn shipping_daemon_ping_episode_uses_unique_host_structured_log_owner() {
    let extension = include_str!("../src/extensions/forever_box/extension.rs");
    assert!(extension.contains("environment.loopback().set_telemetry(Arc::new(move |report|"));
    assert!(extension.contains("daemon_logs.report_daemon_ping(report)"));

    let producer = include_str!("../src/box/loopback_sand_box.rs");
    assert!(producer.contains("self.report_daemon_ping(DaemonPingReport {"));
    assert!(producer.contains("daemon_watchdog_loop(weak, endpoint, interval)"));
    assert!(producer.contains("report_from_shared(&shared, &report)"));

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_daemon_ping(&self, report: &DaemonPingReport)"));
    assert!(owner.contains("self.report_projection(&daemon_ping_telemetry(report))"));

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(!coordinator.contains("reportDaemonPing"));
    assert!(!coordinator.contains("sand.box.daemon_ping"));

    let electron =
        include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(!electron.contains("reportDaemonPing"));
    assert!(!electron.contains("sand.box.daemon_ping"));

    let root = std::env::temp_dir().join(format!(
        "fabushi-daemon-ping-owner-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let telemetry = HostTelemetryService::open(root.join("telemetry.jsonl"))
        .expect("telemetry");
    telemetry
        .logs
        .report_daemon_ping(&DaemonPingReport {
            outcome: "ok".into(),
            attempts: 3,
            duration_ms: 40,
            unready_duration_ms: 35,
            readiness_state: "ready_after_retry".into(),
            target: "127.0.0.1:1337".into(),
            cause_summary: None,
        })
        .expect("daemon ping telemetry");
    let text = fs::read_to_string(telemetry.records_path()).expect("telemetry jsonl");
    assert!(text.contains("\"event\":\"sand.box.daemon_ping\""));
    assert!(text.contains("\"level\":\"warn\""));
    assert!(text.contains("\"attempts\":\"3\""));
    assert!(text.contains("\"readiness_state\":\"ready_after_retry\""));
    assert!(!text.contains("\"cause\""));
    let _ = fs::remove_dir_all(root);
}
