use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::extensions::experiments::HostExperimentsExtension;
use crate::extensions::inference::production::ProductionInferenceExtension;
use crate::ports::telemetry::resolve_sand_box_identity_tags_from;
use crate::r#box::box_store_backend_policy::{
    BoxStoreBackendKind, resolve_box_store_backend_policy,
};

use super::analytics_service::AutomationRunAnalyticsTelemetry;
use super::host_telemetry_service::{
    HostBundleIdentity, HostProductAnalytics, HostStructuredLogTelemetry, HostTelemetryApi,
    HostTelemetryRuntimeHooks, HostTelemetryService,
};

pub use super::host_telemetry_service::{
    DESKTOP_HEALTH_EVENT, DESKTOP_HEALTH_HEARTBEAT_MS, DesktopHealthForwardState,
    FATAL_TELEMETRY_FLUSH_TIMEOUT, HOST_CRASH_MARKER_FORWARD_INTERVAL,
    TELEMETRY_FLUSH_TICK, forward_desktop_health_file_to_logs,
};

pub const TELEMETRY_EXTENSION_ID: &str = "telemetry";
pub const TELEMETRY_DEPENDENCIES: &[HostExtensionId] = &[
    HostExtensionId::Auth,
    HostExtensionId::Experiments,
    HostExtensionId::Inference,
];

fn box_store_backend_kind_tag(kind: BoxStoreBackendKind) -> &'static str {
    match kind {
        BoxStoreBackendKind::Native => "fabushi-native",
        BoxStoreBackendKind::LocalFs => "local-fs",
        BoxStoreBackendKind::SandBoxStoreV2 => "sand-box-store-v2",
        BoxStoreBackendKind::AgentStore => "agent-store",
    }
}

pub fn telemetry_identity_tags_from(
    environment: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut tags = resolve_sand_box_identity_tags_from(environment);
    tags.insert(
        "store_backend".into(),
        box_store_backend_kind_tag(resolve_box_store_backend_policy(environment).kind).into(),
    );
    tags
}

#[derive(Clone)]
pub struct HostTelemetryExtension {
    pub logs: HostStructuredLogTelemetry,
    pub analytics: HostProductAnalytics,
    pub brain: Arc<AutomationRunAnalyticsTelemetry>,
    records_path: PathBuf,
    api: HostTelemetryApi,
}

impl HostTelemetryExtension {
    pub fn records_path(&self) -> &Path {
        &self.records_path
    }

    pub fn dispose(&self) {
        let _ = self.api.service_dispose();
    }

    pub fn flush_for_fatal_exit(&self) {
        self.api.flush_for_fatal_exit();
    }

    pub fn api(&self) -> HostTelemetryApi {
        self.api.clone()
    }
}

pub fn start_host_telemetry_extension(
    app_data_dir: &Path,
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
    inference: Arc<ProductionInferenceExtension>,
) -> io::Result<HostTelemetryExtension> {
    let environment = std::env::vars().collect::<BTreeMap<_, _>>();
    let service = Arc::new(HostTelemetryService::open_production(
        app_data_dir.join("telemetry").join("host-events.jsonl"),
        telemetry_identity_tags_from(&environment),
        backend_url,
        auth,
        experiments,
        inference,
        HostTelemetryRuntimeHooks::default(),
    )?);
    service.start()?;

    let api = service.api();
    let host_bundle_version = environment
        .get("SAND_HOST_BUNDLE_VERSION")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| Some(env!("CARGO_PKG_VERSION").to_string()));
    let box_store_id = environment
        .get("SAND_BOX_STORE_ID")
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Err(error) = api.set_host_bundle_identity(HostBundleIdentity {
        host_bundle_version,
        box_store_id,
    }) {
        let _ = service.dispose();
        return Err(error);
    }

    Ok(HostTelemetryExtension {
        logs: service.logs.clone(),
        analytics: service.analytics.clone(),
        brain: Arc::clone(&service.brain),
        records_path: service.records_path().to_path_buf(),
        api,
    })
}
