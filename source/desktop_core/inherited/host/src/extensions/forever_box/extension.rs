use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;

use crate::host_diagnostics::{HostDiagnostic, report_host_diagnostic};
use crate::host_paths::get_sand_root_dir;
use crate::runner::box_reference_docs::provision_sand_box_prompt_artifacts;
use crate::extensions::telemetry::host_telemetry_service::HostStructuredLogTelemetry;
use crate::r#box::box_store_backend_policy::{
    is_box_store_copy_in_enabled, is_box_store_sync_enabled,
};
use crate::r#box::production::ProductionBoxEnvironment;

use super::disk_pressure::{DiskPressureWatchDeps, start_disk_pressure_watch};
use super::disk_pressure_guard::{DiskPressureLevel, DiskPressureTrigger};
use super::forever_box_service::{ForeverBoxLifecycle, ForeverBoxService};
use super::host_box::HostBox;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForeverBoxExtensionOptions {
    pub auto_update_enabled: bool,
    pub host_bundle_auto_update_enabled: bool,
    pub is_in_box: bool,
}

fn env_switch(environment: &BTreeMap<String, String>, name: &str) -> Option<bool> {
    environment
        .get(name)
        .map(|value| value.trim().to_ascii_lowercase())
        .map(|value| !matches!(value.as_str(), "0" | "false" | "no"))
}

pub fn is_image_auto_update_enabled(environment: &BTreeMap<String, String>) -> bool {
    is_box_store_sync_enabled(environment)
        && is_box_store_copy_in_enabled(environment)
        && env_switch(environment, "SAND_BOX_AUTO_UPDATE").unwrap_or(true)
}

pub fn is_host_bundle_auto_update_enabled(environment: &BTreeMap<String, String>) -> bool {
    env_switch(environment, "SAND_BOX_AUTO_UPDATE").unwrap_or(true)
}

impl ForeverBoxExtensionOptions {
    pub fn from_process_env() -> Self {
        let environment = env::vars().collect::<BTreeMap<_, _>>();
        Self {
            auto_update_enabled: is_image_auto_update_enabled(&environment),
            host_bundle_auto_update_enabled: is_host_bundle_auto_update_enabled(&environment),
            is_in_box: environment.get("SAND_HOST_IN_BOX").is_some_and(|value| value == "1"),
        }
    }
}

pub fn start_forever_box_extension(
    environment: ProductionBoxEnvironment,
    lifecycle: Arc<dyn ForeverBoxLifecycle>,
    options: ForeverBoxExtensionOptions,
) -> Arc<ForeverBoxService> {
    start_forever_box_extension_inner(environment, lifecycle, options, None)
}

pub fn start_forever_box_extension_with_telemetry(
    environment: ProductionBoxEnvironment,
    lifecycle: Arc<dyn ForeverBoxLifecycle>,
    options: ForeverBoxExtensionOptions,
    recreate_telemetry: HostStructuredLogTelemetry,
) -> Arc<ForeverBoxService> {
    start_forever_box_extension_inner(
        environment,
        lifecycle,
        options,
        Some(recreate_telemetry),
    )
}

fn start_forever_box_extension_inner(
    environment: ProductionBoxEnvironment,
    lifecycle: Arc<dyn ForeverBoxLifecycle>,
    options: ForeverBoxExtensionOptions,
    recreate_telemetry: Option<HostStructuredLogTelemetry>,
) -> Arc<ForeverBoxService> {
    if let Some(logs) = recreate_telemetry.as_ref() {
        let daemon_logs = logs.clone();
        environment.loopback().set_telemetry(Arc::new(move |report| {
            let _ = daemon_logs.report_daemon_ping(report);
        }));
    }
    let service = ForeverBoxService::new(
        HostBox::new(environment),
        lifecycle,
        options.auto_update_enabled,
        options.host_bundle_auto_update_enabled,
        options.is_in_box,
    );
    let service = match recreate_telemetry {
        Some(recreate_telemetry) => service.with_recreate_telemetry(recreate_telemetry),
        None => service,
    };
    let service = Arc::new(service);

    let report = Arc::new(|report: &super::disk_pressure_guard::DiskPressureReport| {
        let level = match report.level {
            DiskPressureLevel::Healthy => "healthy",
            DiskPressureLevel::Soft => "soft",
            DiskPressureLevel::Hard => "hard",
        };
        let trigger = match report.trigger {
            DiskPressureTrigger::Transition => "transition",
            DiskPressureTrigger::Heartbeat => "heartbeat",
        };
        let fields = serde_json::json!({
            "volume": report.volume,
            "deviceId": report.device_id,
            "totalBytes": report.total_bytes,
            "availableBytes": report.available_bytes,
            "level": level,
            "trigger": trigger,
            "usedPercent": report.used_percent,
        })
        .as_object()
        .cloned()
        .unwrap_or_default();
        report_host_diagnostic(&HostDiagnostic {
            kind: "disk_pressure".into(),
            fields,
        });
    });
    let log = Arc::new(|message: &str| {
        report_host_diagnostic(&HostDiagnostic {
            kind: "disk_pressure_log".into(),
            fields: serde_json::json!({ "message": message })
                .as_object()
                .cloned()
                .unwrap_or_default(),
        });
    });
    if options.is_in_box {
        if let Err(error) = provision_sand_box_prompt_artifacts() {
            report_host_diagnostic(&HostDiagnostic {
                kind: "box_reference_docs_provision_failed".into(),
                fields: serde_json::json!({ "error": error.to_string() })
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            });
        }
    }

    let watch = start_disk_pressure_watch(DiskPressureWatchDeps {
        is_in_box: options.is_in_box,
        root_dir: get_sand_root_dir(),
        polling_interval: Duration::from_secs(60),
        report,
        log,
    });
    service.install_disk_pressure_watch(watch);
    service.start();
    service
}
