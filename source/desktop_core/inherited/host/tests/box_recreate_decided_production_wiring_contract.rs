use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::box_lifecycle::RecreateSandBoxResponse;
use mahayana_host_runtime::extensions::forever_box::{
    ForeverBoxLifecycle, ForeverBoxService, HostBox,
};
use mahayana_host_runtime::extensions::telemetry::host_telemetry_service::HostTelemetryService;
use mahayana_host_runtime::r#box::production::ProductionBoxEnvironment;

#[derive(Clone, Copy)]
enum RecreateBehavior { Started, Declined, Error }

struct FakeLifecycle {
    image_update: Result<bool, &'static str>,
    recreate: RecreateBehavior,
    records_path: PathBuf,
    saw_decision_before_recreate: AtomicBool,
}

impl FakeLifecycle {
    fn new(image_update: Result<bool, &'static str>, recreate: RecreateBehavior, records_path: PathBuf) -> Self {
        Self { image_update, recreate, records_path, saw_decision_before_recreate: AtomicBool::new(false) }
    }
}

impl ForeverBoxLifecycle for FakeLifecycle {
    fn fetch_image_update_available(&self) -> Result<bool, String> {
        self.image_update.map_err(str::to_string)
    }
    fn recreate_in_box(&self, preserve_data: bool, force: Option<bool>) -> Result<RecreateSandBoxResponse, String> {
        assert!(preserve_data);
        assert_eq!(force, None);
        if fs::read_to_string(&self.records_path).unwrap_or_default().contains("sand.box.recreate_decided") {
            self.saw_decision_before_recreate.store(true, Ordering::Release);
        }
        match self.recreate {
            RecreateBehavior::Started => Ok(RecreateSandBoxResponse { started: true, reason: None }),
            RecreateBehavior::Declined => Ok(RecreateSandBoxResponse { started: false, reason: Some("declined-by-service".into()) }),
            RecreateBehavior::Error => Err("lifecycle unavailable".into()),
        }
    }
}

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("time").as_nanos();
    let root = std::env::temp_dir().join(format!("fabushi-box-recreate-decided-{label}-{}-{nonce}", std::process::id()));
    fs::create_dir_all(&root).expect("temp root");
    root
}

fn service_with_telemetry(root: &PathBuf, lifecycle: Arc<FakeLifecycle>, auto_update_enabled: bool, is_in_box: bool) -> (ForeverBoxService, HostTelemetryService) {
    let telemetry = HostTelemetryService::open_with_identity_tags(root.join("events.jsonl"), BTreeMap::new()).expect("telemetry");
    let service = ForeverBoxService::new(
        HostBox::new(ProductionBoxEnvironment::new("127.0.0.1", 9, "test-token")),
        lifecycle,
        auto_update_enabled,
        false,
        is_in_box,
    ).with_recreate_telemetry(telemetry.logs.clone());
    (service, telemetry)
}

#[test]
fn hibernation_decision_is_emitted_before_recreate_even_when_service_declines() {
    let root = temp_root("declined");
    let lifecycle = Arc::new(FakeLifecycle::new(Ok(true), RecreateBehavior::Declined, root.join("events.jsonl")));
    let (service, telemetry) = service_with_telemetry(&root, lifecycle.clone(), true, true);
    let result = service.auto_update_now().expect("decision result");
    assert!(!result.started);
    assert_eq!(result.reason.as_deref(), Some("declined-by-service"));
    assert!(lifecycle.saw_decision_before_recreate.load(Ordering::Acquire));
    let text = fs::read_to_string(telemetry.records_path()).expect("jsonl");
    assert_eq!(text.matches("sand.box.recreate_decided").count(), 1);
    assert!(text.contains(r#""trigger":"hibernation_auto_update""#));
    assert!(text.contains(r#""mode":"pod_recreate""#));
    assert!(text.contains(r#""preserved":"true""#));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn hibernation_recreate_transport_failure_occurs_after_the_decision() {
    let root = temp_root("transport-error");
    let lifecycle = Arc::new(FakeLifecycle::new(Ok(true), RecreateBehavior::Error, root.join("events.jsonl")));
    let (service, telemetry) = service_with_telemetry(&root, lifecycle.clone(), true, true);
    let result = service.auto_update_now().expect("mapped recreate failure");
    assert!(!result.started);
    assert_eq!(result.reason.as_deref(), Some("recreate-unavailable"));
    assert!(lifecycle.saw_decision_before_recreate.load(Ordering::Acquire));
    assert_eq!(fs::read_to_string(telemetry.records_path()).expect("jsonl").matches("sand.box.recreate_decided").count(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn frozen_skip_branches_do_not_emit_recreate_decided() {
    let cases = [
        ("outside", true, false, Ok(true), false, "not-in-box"),
        ("disabled", false, true, Ok(true), false, "auto-update-disabled"),
        ("no-update", true, true, Ok(false), false, "no-update-required"),
        ("staleness-failed", true, true, Err("image check failed"), false, "staleness-check-failed"),
        ("busy", true, true, Ok(true), true, "busy"),
    ];
    for (label, enabled, in_box, image, busy, expected_reason) in cases {
        let root = temp_root(label);
        let lifecycle = Arc::new(FakeLifecycle::new(image, RecreateBehavior::Started, root.join("events.jsonl")));
        let (service, telemetry) = service_with_telemetry(&root, lifecycle, enabled, in_box);
        if busy { service.set_busy(true); }
        let result = service.auto_update_now().expect("skip result");
        assert!(!result.started, "{label}");
        assert_eq!(result.reason.as_deref(), Some(expected_reason), "{label}");
        assert!(!fs::read_to_string(telemetry.records_path()).expect("jsonl").contains("sand.box.recreate_decided"), "{label}");
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn shipping_recreate_decision_uses_forever_box_producer_and_unique_host_owner() {
    let producer = include_str!("../src/extensions/forever_box/forever_box_service.rs");
    assert_eq!(producer.matches("self.report_recreate_decided(").count(), 2);
    assert!(producer.contains(r#"self.report_recreate_decided("hibernation_auto_update");"#));
    assert!(producer.contains(r#"self.report_recreate_decided("auto_update");"#));
    assert!(producer.contains(r#"("mode".into(), "pod_recreate".into())"#));
    assert!(producer.contains(r#"("preserved".into(), "true".into())"#));
    assert!(producer.contains("logs.report_box_recreate_decided("));

    let composition = include_str!("../src/host_production_extensions.rs");
    assert!(composition.contains("start_forever_box_extension_with_telemetry("));
    assert!(composition.contains("core.telemetry.logs.clone()"));

    let owner = include_str!("../src/extensions/telemetry/host_telemetry_service.rs");
    assert!(owner.contains("pub fn report_box_recreate_decided("));
    assert!(owner.contains("self.report_projection(&box_recreate_decided_telemetry(metadata))"));

    let mapper = include_str!("../src/extensions/telemetry/box_recreate_decided_telemetry.rs");
    assert_eq!(mapper.matches("sand.box.recreate_decided").count(), 1);
    assert!(mapper.contains(r#"level: Some("info")"#));
    assert!(mapper.contains("event: Some(BOX_RECREATE_DECIDED_EVENT)"));

    let coordinator = include_str!("../../node-agent-coordinator/src/main.rs");
    assert!(!coordinator.contains("reportBoxRecreateDecided"));
    assert!(!coordinator.contains("report_box_recreate_decided"));
    assert!(!coordinator.contains("sand.box.recreate_decided"));
    let electron = include_str!("../../electron-main/telemetry/desktop-structured-log-telemetry.ts");
    assert!(!electron.contains("reportBoxRecreateDecided"));
    assert!(!electron.contains("sand.box.recreate_decided"));
}

#[test]
fn frozen_decision_gates_precede_the_event_and_manual_recreate_has_no_event() {
    let producer = include_str!("../src/extensions/forever_box/forever_box_service.rs");
    let hibernation = producer.split("pub fn auto_update_now").nth(1)
        .and_then(|part| part.split("fn maybe_auto_update").next()).expect("hibernation body");
    let event = hibernation.find(r#"self.report_recreate_decided("hibernation_auto_update")"#).expect("event");
    for gate in ["if !self.is_in_box", "if !self.auto_update_enabled", "if self.busy.load", "if self.update_in_flight.swap", "self.refresh_image_update_available_for(\"pre_hibernation\")", "if !available"] {
        assert!(hibernation.find(gate).expect("gate") < event, "{gate}");
    }
    assert!(event < hibernation.find("self.lifecycle.recreate_in_box(true, None)").expect("recreate"));

    let automatic = producer.split("fn maybe_auto_update").nth(1)
        .and_then(|part| part.split("fn report_recreate_decided").next()).expect("auto body");
    let automatic_event = automatic.find(r#"self.report_recreate_decided("auto_update")"#).expect("auto event");
    for gate in ["!self.auto_update_enabled", "self.host_bundle_auto_update_enabled", "available != Some(true)", "self.busy.load", "self.update_in_flight.swap"] {
        assert!(automatic.find(gate).expect("auto gate") < automatic_event, "{gate}");
    }
    assert!(producer.contains("pub fn reset(&self, agent_id: &str) -> Result<BoxStatus, ForeverBoxServiceError> {\n        self.recreate(agent_id, false, None)\n    }"));
    assert!(producer.contains("self.recreate(agent_id, true, force)"));
}
