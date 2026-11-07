use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::host_paths::get_sand_root_dir;
use crate::host_diagnostics::{
    HostDiagnostic as DomainHostDiagnostic, pin_host_diagnostics_reporter,
};
use crate::extensions::session::conversation_size_limits::{
    ConversationGcReport as DomainConversationGcReport, pin_conversation_gc_reporter,
};
use crate::extensions::session::session_diagnostics::{
    SessionDiagnostic as DomainSessionDiagnostic, pin_session_diagnostics_reporter,
};
use crate::extensions::action_audit::extension::{
    ActionAuditExtension, start_action_audit_extension,
};
use crate::extensions::attachments::attachments_service::AttachmentsService;
use crate::extensions::attachments::extension::{
    HostAttachmentsExtension, start_attachments_extension,
};
use crate::extensions::auto_review::auto_review_service::{
    AutoReviewExpireSweepFailedSink, AutoReviewTelemetrySink, AutoReviewUpdateSink,
};
use crate::extensions::auto_review::extension::{
    HostAutoReviewExtension, start_auto_review_extension_with_expire_sweep_telemetry,
};
use crate::extensions::auth::auth_service::HostAuthServiceOptions;
use crate::extensions::auth::extension::{
    HostAuthExtension, start_host_auth_extension_with_options,
};
use crate::extensions::auth::user_full_name_service::production_user_full_name_fetch;
use crate::extensions::automations::listener_connect_watcher_production::ListenerConnectedCallback;
use crate::extensions::automations::production::{FireDispatch, FirePollStateReader, ProductionLog, RelayEventSink, RelayListeners};
use crate::extensions::automations::sand_automation_cloud_sync::is_server_schedulable;
use crate::extensions::automations::listener_integrations::AgentChannelsReader;
use crate::extensions::automations::production_lifecycle::{
    CloudAgentIdsReader, CloudDefinitionsReader, CloudTimeZoneReader,
    ProductionAutomationsLifecycle,
};
use crate::extensions::box_lifecycle::box_lifecycle_service::BoxLifecycleService;
use crate::extensions::box_lifecycle::extension::start_box_lifecycle_extension;
use crate::extensions::box_lifecycle::production::{
    ProductionBoxLifecycleClient, ProductionBoxLifecycleClientFactory,
};
use crate::extensions::box_store_sync::extension::{
    BoxStoreScheduling, BoxStoreSyncExtension, BoxStoreSyncExtensionDeps,
    start_box_store_sync_extension,
};
use crate::extensions::box_store_sync::production::{
    ProductionBoxStoreSyncApi, ProductionBoxStoreSyncFactory, ProductionBoxStoreSyncService,
};
use crate::extensions::browser_ua::{
    BrowserUaExtensionRuntime, BrowserUaHostLog, start_browser_ua_extension,
};
use crate::extensions::cloud_agents::extension::{
    CloudAgentsExtension, start_cloud_agents_extension,
};
use crate::extensions::cloud_workers::extension::{
    CloudWorkerProvisioningExtension, start_cloud_worker_provisioning_extension,
};
use crate::extensions::content_search::extension::{
    ProductionContentSearchExtension, start_production_content_search_extension,
};
use crate::extensions::cross_user_sharing::production::{
    ProductionCrossUserRuntime, RemoteRequestedTurnRunner, SharedRoomTurnRunner,
};
use crate::extensions::codebase_telemetry::extension::{
    CodebaseTelemetryExtension, start_codebase_telemetry_extension,
};
use crate::extensions::experiments::{
    HostExperimentsExtension, ProductionStatsigBootstrapRuntime,
    start_authenticated_statsig_bootstrap, start_host_experiments_extension,
};
use crate::extensions::forever_box::{
    ForeverBoxExtensionOptions, ForeverBoxLifecycle, ForeverBoxService,
    start_forever_box_extension_with_telemetry,
};
use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::extensions::inference::production::{
    ProductionInferenceExtension, start_production_inference_extension,
};
use crate::extensions::host_upgrade::production::{
    ProductionHostUpgradeExtension, ProductionHostUpgradePeers,
    start_production_host_upgrade_extension,
};
use crate::extensions::managed_setup::extension::{
    ManagedSetupExtension, start_managed_setup_extension,
};
use crate::extensions::mcp::coordinator_relay::{
    CoordinatorMcpLifecycleRelay, CoordinatorPluginSkillsLoader,
};
use crate::extensions::mcp::extension::{
    AuthRenewalPort, McpExtensionRuntime, McpPluginSkillsService,
    PluginSkillsAuthenticatedStartup, PollingPort,
};
use crate::extensions::mcp::legacy_live_references::{
    PluginCatalogEntry, PluginSkillCatalogEntry, sweep_legacy_plugin_skill_references,
};
use crate::extensions::mcp::mcp_service::{
    McpHostService, McpManagerBackend, PluginSkillsPort,
};
use crate::extensions::mcp::plugin_skills::{
    PluginSkillsLoader, SandPluginSkillsService,
};
use crate::extensions::mcp::production::{
    LegacyMcpAuthCleanupOutcome, RealPluginSkillsPolling,
    cleanup_legacy_mcp_auth_credentials, create_production_skill_publish,
};
use crate::extensions::local_exec::extension::{
    HostLocalExecExtension, start_local_exec_extension,
};
use crate::extensions::plugins::compat::validate_all_records;
use crate::extensions::plugin_sdk::channel_entry::{
    BundledChannelEntryContract, PluginRegistrationMode,
};
use crate::extensions::plugins::capability_catalog::{
    CapabilityProviderEntry, PluginCapabilityCatalog, PluginOrigin,
    CAPABILITY_FAMILY_WORKER_PROVIDERS,
};
use crate::extensions::plugins::installed_index_store::BundledInstallGuard;
use crate::connectors::channel_delivery::{
    ChannelDeliveryPort, ChannelDeliveryRouter, HostChannelDelivery,
};
use crate::extensions::local_tool_permission::extension::{
    HostLocalToolPermissionExtension, start_local_tool_permission_extension,
};
use crate::extensions::memory::extension::HostMemoryExtension;
use crate::extensions::memory::production::{
    create_production_memory_synthesis, start_production_memory_extension,
};
use crate::extensions::notify_bus::extension::{
    HostNotifyBusExtension, start_notify_bus_extension,
};
use crate::extensions::notifications::extension::{
    HostNotificationsExtension, WindowFocusedAtSource, start_notifications_extension,
};
use crate::extensions::notifications::mobile_push_notifier::NotificationAgent;
use crate::extensions::source_map::extension::start_source_map_extension;
use crate::extensions::source_map::source_map_service::SandSourceMap;
use crate::extensions::secrets::extension::{
    HostSecretsExtension, start_secrets_extension,
};
use crate::extensions::session::box_handoff_service::{BoxHandoffDeps, BoxHandoffService};
use crate::extensions::session::extension::{SessionExtension, start_session_extension};
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::extensions::settings::extension::start_settings_extension;
use crate::extensions::settings::settings_service::{SettingsService, SettingsSubscription};
use crate::extensions::state_backstop::extension::{
    HostStateBackstopExtension, start_state_backstop_extension,
};
use crate::extensions::state_backstop::state_backstop_service::{
    StateBackstopObjectStore, StateBackstopOptions,
};
use crate::extensions::teach_recording::extension::{
    CAP_SLACK_MS, TeachRecordingExtension, TeachRecordingServiceDeps,
    create_teach_recording_extension, load_teach_queue_key,
};
use crate::extensions::teach_recording::teach_recording_service::{
    LEARN_SKILL_NAME, ProductionTeachRecordingRuntime, SAND_TEACH_MAX_DURATION_MS,
    SandTeachRecordingService, SandTeachRecordingServiceFactory, TeachRecordingApi,
    TeachRecordingRuntimePort,
};
use crate::extensions::telemetry::extension::{
    HostTelemetryExtension, start_host_telemetry_extension,
};
use crate::extensions::telemetry::memory_synthesis_telemetry::MemorySynthesisReport;
use crate::extensions::telemetry::conversation_gc_telemetry::ConversationGcReport;
use crate::extensions::telemetry::host_diagnostic_telemetry::HostDiagnostic;
use crate::extensions::telemetry::host_event_bus_telemetry::HostEventBusReport;
use crate::extensions::telemetry::disk_pressure_telemetry::DiskPressureReport;
use crate::extensions::telemetry::session_diagnostic_telemetry::{
    SessionDiagnosticFamily, SessionTelemetryDiagnostic,
};
use crate::extensions::telemetry::webauthn_proxy_telemetry::{
    WebAuthnProxyReport,
};
use crate::extensions::trays::extension::{
    HostTraysExtension, start_trays_extension,
};
use crate::extensions::transcript::extension::{
    TranscriptExtension, TranscriptExtensionDeps, TranscriptExtensionEventBridge,
    start_production_transcript_extension,
};
use crate::extensions::transcript::roster_emit::ProductionRosterEmit;
use crate::extensions::transcript::transcript_manager::TranscriptManager;
use crate::extensions::transcript::shared_rooms::SharedRooms;
use crate::extensions::turn_execution::extension::turn_execution_extension;
use crate::extensions::turn_execution::turn_execution_service::TurnExecutionRegistry;
use crate::extensions::webauthn_proxy::extension::{
    HostWebAuthnProxyExtension, start_webauthn_proxy_extension,
};
use crate::extensions::wallpaper::extension::{
    HostWallpaperExtension, start_wallpaper_extension,
};
use crate::host_event_bus::{SandHostEventBus, pin_host_event_failure_reporter};
use crate::production_binding_providers::{
    create_production_state_backstop_runtime, production_cloud_agent_trace_converter,
    production_secrets_log,
};
use crate::r#box::production::ProductionBoxEnvironment;
use crate::extensions::box_store_sync::box_object_store::BoxObjectStore;

/// Canonical Grok-shaped production extension registry for the Rust Host.
///
/// All frozen Grok 0.18 slots are backed by real production owners. Owners that
/// need transcript, Gateway, or Box dependencies are staged here and started
/// only when those dependencies become available.
pub const CURRENT_SHIPPING_PRODUCTION_EXTENSION_IDS: &[HostExtensionId] = &[
    HostExtensionId::Notifications,
    HostExtensionId::ContentSearch,
    HostExtensionId::Memory,
    HostExtensionId::CrossUserSharing,
    HostExtensionId::StateBackstop,
    HostExtensionId::SourceMap,
    HostExtensionId::Telemetry,
    HostExtensionId::Trays,
    HostExtensionId::Auth,
    HostExtensionId::Experiments,
    HostExtensionId::BrowserUa,
    HostExtensionId::Inference,
    HostExtensionId::LocalExec,
    HostExtensionId::LocalToolPermission,
    HostExtensionId::Attachments,
    HostExtensionId::ForeverBox,
    HostExtensionId::Secrets,
    HostExtensionId::TurnExecution,
    HostExtensionId::Transcript,
    HostExtensionId::Session,
    HostExtensionId::Automations,
    HostExtensionId::Settings,
    HostExtensionId::BoxLifecycle,
    HostExtensionId::ManagedSetup,
    HostExtensionId::Mcp,
    HostExtensionId::BoxStoreSync,
    HostExtensionId::CloudAgents,
    HostExtensionId::CloudWorkers,
    HostExtensionId::ActionAudit,
    HostExtensionId::HostUpgrade,
    HostExtensionId::AutoReview,
    HostExtensionId::CodebaseTelemetry,
    HostExtensionId::TeachRecording,
    HostExtensionId::WebauthnProxy,
    HostExtensionId::NotifyBus,
    HostExtensionId::Wallpaper,
];

pub struct ProductionBrowserUaLog;

impl BrowserUaHostLog for ProductionBrowserUaLog {
    fn log(&self, message: &str) {
        eprintln!("mahayana-host-browser-ua {message}");
    }
}

pub fn start_production_browser_ua(
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
) -> BrowserUaExtensionRuntime {
    start_browser_ua_extension(
        auth,
        experiments,
        Arc::new(ProductionBrowserUaLog),
        None,
        None,
    )
}

struct ProductionStateBackstopObjectStore {
    store: Result<Arc<dyn BoxObjectStore>, String>,
}

impl ProductionStateBackstopObjectStore {
    fn new(store: Result<Arc<dyn BoxObjectStore>, String>) -> Self {
        Self { store }
    }

    fn store(&self) -> Result<&Arc<dyn BoxObjectStore>, String> {
        self.store.as_ref().map_err(Clone::clone)
    }
}

impl StateBackstopObjectStore for ProductionStateBackstopObjectStore {
    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.store()?.put(path, bytes)
    }

    fn get(&self, path: &str) -> Result<Option<Vec<u8>>, String> {
        self.store()?.get(path)
    }
}

fn diagnostic_string(
    values: &impl DiagnosticValues,
    names: &[&str],
) -> Option<String> {
    names.iter().find_map(|name| values.string_value(name))
}

trait DiagnosticValues {
    fn string_value(&self, name: &str) -> Option<String>;
    fn integer_value(&self, name: &str) -> Option<i64>;
    fn float_value(&self, name: &str) -> Option<f64>;
}

impl DiagnosticValues for serde_json::Map<String, serde_json::Value> {
    fn string_value(&self, name: &str) -> Option<String> {
        self.get(name).and_then(|value| match value {
            serde_json::Value::String(value) => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            serde_json::Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
    }

    fn integer_value(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(serde_json::Value::as_i64)
    }

    fn float_value(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(serde_json::Value::as_f64)
    }
}

impl DiagnosticValues for BTreeMap<String, serde_json::Value> {
    fn string_value(&self, name: &str) -> Option<String> {
        self.get(name).and_then(|value| match value {
            serde_json::Value::String(value) => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            serde_json::Value::Bool(value) => Some(value.to_string()),
            _ => None,
        })
    }

    fn integer_value(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(serde_json::Value::as_i64)
    }

    fn float_value(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(serde_json::Value::as_f64)
}
}

pub fn project_box_disk_pressure(report: &DomainHostDiagnostic) -> Option<DiskPressureReport> {
    if report.kind != "disk_pressure" {
        return None;
    }
    Some(DiskPressureReport {
        level: diagnostic_string(&report.fields, &["level"] )?,
        volume: diagnostic_string(&report.fields, &["volume"] )?,
        trigger: diagnostic_string(&report.fields, &["trigger"] )?,
        total_bytes: report.fields.float_value("totalBytes")
            .or_else(|| report.fields.float_value("total_bytes"))?,
        available_bytes: report.fields.float_value("availableBytes")
            .or_else(|| report.fields.float_value("available_bytes"))?,
        used_percent: report.fields.float_value("usedPercent")
            .or_else(|| report.fields.float_value("used_percent"))?,
    })
}


pub fn project_host_event_bus_failure(report: &serde_json::Value) -> Option<HostEventBusReport> {
    let object = report.as_object()?;
    Some(HostEventBusReport {
        kind: object.get("kind")?.as_str()?.to_string(),
        topic: object.get("topic").and_then(serde_json::Value::as_str).map(str::to_string),
        error_class: object
            .get("errorClass")
            .or_else(|| object.get("error_class"))?
            .as_str()?
            .to_string(),
    })
}

pub fn project_host_diagnostic(report: &DomainHostDiagnostic) -> HostDiagnostic {
    HostDiagnostic {
        kind: report.kind.clone(),
        stage: diagnostic_string(&report.fields, &["stage"]),
        agent_id: diagnostic_string(&report.fields, &["agentId", "agent_id"]),
        reason: diagnostic_string(&report.fields, &["reason"]),
        error_class: diagnostic_string(&report.fields, &["errorClass", "error_class"]),
    }
}

pub fn project_session_diagnostic(
    report: &DomainSessionDiagnostic,
) -> Option<SessionTelemetryDiagnostic> {
    let family = match report.family.as_str() {
        "store_db" => SessionDiagnosticFamily::StoreDb,
        "maintenance" => SessionDiagnosticFamily::Maintenance,
        "materialize" => SessionDiagnosticFamily::Materialize,
        "summary_build" => SessionDiagnosticFamily::SummaryBuild,
        _ => return None,
    };
    Some(SessionTelemetryDiagnostic {
        family,
        kind: report.kind.clone(),
        agent_id: diagnostic_string(&report.metadata, &["agentId", "agent_id"]),
        error_class: diagnostic_string(&report.metadata, &["errorClass", "error_class"]),
        outcome: diagnostic_string(&report.metadata, &["outcome"]),
        quarantine: diagnostic_string(&report.metadata, &["quarantine"]),
        salvaged_kv: report.metadata.integer_value("salvagedKv")
            .or_else(|| report.metadata.integer_value("salvaged_kv")),
        salvaged_blobs: report.metadata.integer_value("salvagedBlobs")
            .or_else(|| report.metadata.integer_value("salvaged_blobs")),
        salvaged_transcript: report.metadata.integer_value("salvagedTranscript")
            .or_else(|| report.metadata.integer_value("salvaged_transcript")),
    })
}

pub fn project_conversation_gc(report: &DomainConversationGcReport) -> ConversationGcReport {
    match report.outcome.as_str() {
        "failed" => ConversationGcReport::Failed {
            trigger: report.trigger.clone(),
            agent_id: report.agent_id.clone(),
        },
        "skipped" => ConversationGcReport::Skipped {
            trigger: report.trigger.clone(),
            agent_id: report.agent_id.clone(),
            skip_reason: report.skip_reason.clone().unwrap_or_else(|| "unknown".into()),
            unresolved_proto_refs: report.unresolved_proto_refs.map(|value| value as f64),
        },
        _ => ConversationGcReport::Collected {
            trigger: report.trigger.clone(),
            agent_id: report.agent_id.clone(),
            still_over_cap: report.still_over_cap,
            deleted_rows: report.deleted_rows.unwrap_or_default() as f64,
            deleted_bytes: report.deleted_bytes.unwrap_or_default() as f64,
            live_rows: report.live_rows.unwrap_or_default() as f64,
            live_bytes: report.live_bytes.unwrap_or_default() as f64,
            vacuumed: report.vacuumed.unwrap_or(false),
        },
    }
}

fn pin_structured_log_domain_reporters(logs: crate::extensions::telemetry::host_telemetry_service::HostStructuredLogTelemetry) {
    let event_logs = logs.clone();
    pin_host_event_failure_reporter(Some(Arc::new(move |report| {
        if let Some(projected) = project_host_event_bus_failure(report) {
            let _ = event_logs.report_host_event_bus_failure(&projected);
        }
    })));
    let host_logs = logs.clone();
    pin_host_diagnostics_reporter(Some(Arc::new(move |report| {
        if let Some(disk_pressure) = project_box_disk_pressure(report) {
            let _ = host_logs.report_box_disk_pressure(&disk_pressure);
        } else {
            let _ = host_logs.report_host_diagnostic(&project_host_diagnostic(report));
        }
    })));
    let session_logs = logs.clone();
    pin_session_diagnostics_reporter(Some(Arc::new(move |report| {
        if let Some(projected) = project_session_diagnostic(report) {
            let _ = session_logs.report_session_diagnostic(&projected);
        }
    })));
    pin_conversation_gc_reporter(Some(Arc::new(move |report| {
        let _ = logs.report_conversation_gc(&project_conversation_gc(report));
    })));
}

fn unpin_structured_log_domain_reporters() {
    pin_host_event_failure_reporter(None);
    pin_conversation_gc_reporter(None);
    pin_session_diagnostics_reporter(None);
    pin_host_diagnostics_reporter(None);
}

pub struct ProductionHostExtensions {
    pub telemetry: HostTelemetryExtension,
    pub auth: Arc<HostAuthExtension>,
    pub settings: Arc<SettingsService>,
    pub experiments: Arc<HostExperimentsExtension>,
    pub browser_ua: Mutex<Option<BrowserUaExtensionRuntime>>,
    pub wallpaper: Mutex<Option<HostWallpaperExtension>>,
    pub inference: Arc<ProductionInferenceExtension>,
    pub content_search: Arc<ProductionContentSearchExtension>,
    pub local_tool_permission: Arc<HostLocalToolPermissionExtension>,
    pub local_exec: Arc<HostLocalExecExtension>,
    _settings_feature_override_subscription:
        SettingsSubscription<dyn Fn(BTreeMap<String, bool>) + Send + Sync + 'static>,
    pub codebase_telemetry: CodebaseTelemetryExtension,
    pub notify_bus: HostNotifyBusExtension,
    notifications: Mutex<Option<HostNotificationsExtension>>,
    pub memory: HostMemoryExtension,
    pub managed_setup: Arc<ManagedSetupExtension>,
    pub source_map: Arc<SandSourceMap>,
    pub trays: Arc<HostTraysExtension>,
    pub box_lifecycle:
        Arc<BoxLifecycleService<ProductionBoxLifecycleClient<HostAuthExtension>>>,
    pub webauthn_proxy: Arc<HostWebAuthnProxyExtension>,
    pub action_audit: ActionAuditExtension,
    pub cloud_agents: CloudAgentsExtension,
    pub cloud_workers: CloudWorkerProvisioningExtension,
    /// Canonical plugin/channel capability catalog shared by production registration paths.
    pub plugin_capability_catalog: Arc<PluginCapabilityCatalog>,
    /// Canonical channel delivery boundary consumed by Gateway and channel adapters.
    pub channel_delivery: HostChannelDelivery,
    /// Provider registration surface behind the canonical channel delivery boundary.
    channel_delivery_router: ChannelDeliveryRouter,
    pub turn_execution: Arc<Mutex<TurnExecutionRegistry>>,
    session: Mutex<Option<SessionExtension>>,
    auto_review: Mutex<Option<Arc<HostAutoReviewExtension>>>,
    transcript: Mutex<Option<TranscriptExtension>>,
    cross_user: Mutex<Option<Arc<ProductionCrossUserRuntime>>>,
    host_upgrade: Mutex<Option<Arc<ProductionHostUpgradeExtension>>>,
    teach_recording: Mutex<Option<TeachRecordingExtension<SandTeachRecordingService>>>,
    backend_url: String,
    mcp: Mutex<Option<McpExtensionRuntime>>,
    box_store_sync: Mutex<Option<BoxStoreSyncExtension<ProductionBoxStoreSyncService>>>,
    state_backstop: Mutex<Option<HostStateBackstopExtension>>,
    automations: Mutex<Option<Arc<ProductionAutomationsLifecycle>>>,
    // Keep the Experiments lifecycle owner last so dependent extensions settle first.
    _statsig_bootstrap: ProductionStatsigBootstrapRuntime,
}

/// Publishes bundled channel discoverability to the Host-owned catalog.
///
/// This is deliberately capability metadata only: it does not create a
/// transport, credentials, session state, or delivery provider. A channel
/// adapter must still register a real `ChannelDeliveryPort` before delivery
/// can succeed.
fn register_bundled_channel_capabilities(catalog: &PluginCapabilityCatalog) -> Result<(), String> {
    for channel_id in BundledInstallGuard::IMMUTABLE_BUNDLED_PLUGINS {
        let label = match *channel_id {
            "telegram" => "Telegram",
            "discord" => "Discord",
            "slack" => "Slack",
            "whatsapp" => "WhatsApp",
            "signal" => "Signal",
            "imessage" => "iMessage",
            _ => *channel_id,
        };
        let entry = BundledChannelEntryContract::new(
            *channel_id,
            label,
            "Bundled channel capability metadata",
        );
        catalog.register_bundled_channel_entry(
            &entry,
            PluginOrigin::Bundled,
            None,
            PluginRegistrationMode::Capabilities,
        )?;
    }
    Ok(())
}

/// Registers the bundled Crabbox worker-provider contract as capability
/// metadata only. This mirrors the upstream plugin manifest's
/// `workerProviders: ["crabbox"]` declaration without creating a provider
/// process, resolving credentials, enrolling a device, or changing worker
/// readiness. The Cloud Worker owner remains the sole lifecycle/state owner.
fn register_bundled_worker_capabilities(catalog: &PluginCapabilityCatalog) {
    catalog.register_capability(CapabilityProviderEntry {
        family: CAPABILITY_FAMILY_WORKER_PROVIDERS.into(),
        provider_id: "crabbox".into(),
        plugin_id: "crabbox".into(),
        label: Some("Crabbox Worker Provider".into()),
        aliases: Vec::new(),
    });
}

impl Drop for ProductionHostExtensions {
    fn drop(&mut self) {
        let _ = self.stop_teach_recording();
        let _ = self.stop_host_upgrade();
        let _ = self.stop_cross_user();
        let _ = self.stop_auto_review();
        // Stop global reporter callbacks before disposing their single Host owner.
        unpin_structured_log_domain_reporters();
        // Dependents settle while Auth / Experiments / Inference remain live.
        self.cloud_agents.stop();
        self.telemetry.dispose();
    }
}

pub fn start_production_host_extensions(
    app_data_dir: &Path,
    events: SandHostEventBus,
) -> Result<ProductionHostExtensions, String> {
    validate_all_records().map_err(|errors| {
        format!(
            "plugin compatibility registry failed production validation: {}",
            errors.join("; ")
        )
    })?;
    let auth_options = HostAuthServiceOptions::production(|message| {
        eprintln!("mahayana-host-auth {message}");
    })
    .map_err(|error| error.to_string())?;

    let backend_url = auth_options
        .backend_url
        .clone()
        .ok_or_else(|| "production Auth requires a configured backend URL".to_string())?;
    let auth = Arc::new(
        start_host_auth_extension_with_options(
            auth_options,
            production_user_full_name_fetch(backend_url.clone()),
        )
        .map_err(|error| error.to_string())?,
    );
    let settings = start_settings_extension();
    let experiments = Arc::new(start_host_experiments_extension());
    let statsig_bootstrap = start_authenticated_statsig_bootstrap(
        Arc::clone(&experiments),
        Arc::clone(&auth),
        backend_url.clone(),
        get_sand_root_dir(),
    )?;
    let browser_ua = start_production_browser_ua(
        Arc::clone(&auth),
        Arc::clone(&experiments),
    );
    let wallpaper = start_wallpaper_extension(
        Arc::clone(&settings),
        Arc::new(|message| eprintln!("mahayana-host-wallpaper {message}")),
    );
    let inference = Arc::new(start_production_inference_extension(
        Arc::clone(&auth),
        Arc::clone(&experiments),
        Arc::clone(&settings),
    ));
    let telemetry = start_host_telemetry_extension(
        app_data_dir,
        backend_url.clone(),
        Arc::clone(&auth),
        Arc::clone(&experiments),
        Arc::clone(&inference),
    )
    .map_err(|error| error.to_string())?;
    let telemetry_logs = telemetry.logs.clone();
    let content_search_logs = telemetry_logs.clone();
    let content_search = Arc::new(start_production_content_search_extension(
        Arc::clone(&experiments),
        Arc::new(move |health| {
            let payload = serde_json::json!({
                "kind": health.kind,
                "stage": health.stage,
                "count": health.count,
                "errorClass": health.error_class,
            });
            let _ = content_search_logs.report_search_index_health(&payload);
        }),
    ));
    let local_tool_permission = Arc::new(start_local_tool_permission_extension(Arc::clone(&settings)));
    let local_exec = Arc::new(start_local_exec_extension(telemetry.logs.clone()));
    let experiments_for_settings = Arc::clone(&experiments);
    let settings_feature_override_subscription =
        settings.subscribe_to_feature_flag_overrides(Arc::new(move |overrides| {
            experiments_for_settings.replace_feature_flag_overrides(overrides);
        }));
    let codebase_telemetry = start_codebase_telemetry_extension(
        Arc::clone(&auth),
        Arc::clone(&experiments),
        events,
        app_data_dir,
        backend_url.clone(),
    )?;
    let action_audit = start_action_audit_extension(
        backend_url.clone(),
        Arc::clone(&auth),
        Arc::clone(&experiments),
        telemetry_logs,
    );
    let cloud_agents = start_cloud_agents_extension(
        backend_url.clone(),
        Arc::clone(&auth),
        production_cloud_agent_trace_converter(),
    );
    let cloud_workers = start_cloud_worker_provisioning_extension(
        app_data_dir.join("cloud-workers").join("state.json"),
    )
    .map_err(|error| format!("failed to start CloudWorkers extension: {error}"))?;
    let notify_bus = start_notify_bus_extension(
        Arc::clone(&auth),
        Arc::clone(&experiments),
        Arc::new(|message| eprintln!("{message}")),
    )
    .map_err(|error| error.to_string())?;
    let memory = start_production_memory_extension();
    let memory_for_gate = memory.clone();
    let inference_for_memory = Arc::clone(&inference);
    let memory_data_dir = app_data_dir.to_path_buf();
    let memory_logs = telemetry.logs.clone();
    experiments.pin_gate_on_authenticated_bootstrap(
        "sand_memory_dreaming",
        Arc::new(move |enabled| {
            if !enabled {
                let _ = memory_logs.report_memory_synthesis(&MemorySynthesisReport::SkippedGate);
                return;
            }
            let synthesis = create_production_memory_synthesis(
                memory_for_gate.service(),
                Arc::clone(&inference_for_memory),
                memory_data_dir.clone(),
                memory_logs.clone(),
            );
            memory_for_gate.enable_memory_synthesis(synthesis);
        }),
    );
    let managed_setup = start_managed_setup_extension(
        backend_url.clone(),
        Arc::clone(&auth),
        app_data_dir,
    );

    let source_map = Arc::new(start_source_map_extension());
    let trays = Arc::new(start_trays_extension());

    let factory =
        ProductionBoxLifecycleClientFactory::from_process_env()
            .map_err(|error| error.to_string())?;
    let box_lifecycle =
        Arc::new(start_box_lifecycle_extension(Arc::clone(&auth), &factory));
    let webauthn_logs = telemetry.logs.clone();
    let webauthn_proxy = Arc::new(start_webauthn_proxy_extension(Arc::new(
        move |report: WebAuthnProxyReport| {
            if let Err(error) = webauthn_logs.report_webauthn_proxy(&report) {
                eprintln!("mahayana-host webauthn_proxy_telemetry_failed error={error}");
            }
        },
    )));

    let (_, turn_execution_registry) = turn_execution_extension();
    let turn_execution = Arc::new(Mutex::new(turn_execution_registry));
    let plugin_capability_catalog = Arc::new(PluginCapabilityCatalog::new());
    register_bundled_channel_capabilities(&plugin_capability_catalog)?;
    register_bundled_worker_capabilities(&plugin_capability_catalog);
    let channel_delivery_router = ChannelDeliveryRouter::new();
    let channel_delivery = HostChannelDelivery::new(Arc::new(channel_delivery_router.clone()));

    pin_structured_log_domain_reporters(telemetry.logs.clone());

    Ok(ProductionHostExtensions {
        telemetry,
        auth,
        settings,
        experiments,
        browser_ua: Mutex::new(Some(browser_ua)),
        wallpaper: Mutex::new(Some(wallpaper)),
        inference,
        content_search,
        local_tool_permission,
        local_exec,
        _settings_feature_override_subscription: settings_feature_override_subscription,
        codebase_telemetry,
        notify_bus,
        notifications: Mutex::new(None),
        memory,
        managed_setup,
        source_map,
        trays,
        box_lifecycle,
        webauthn_proxy,
        action_audit,
        cloud_agents,
        cloud_workers,
        plugin_capability_catalog,
        channel_delivery,
        channel_delivery_router,
        turn_execution,
        session: Mutex::new(None),
        auto_review: Mutex::new(None),
        transcript: Mutex::new(None),
        cross_user: Mutex::new(None),
        host_upgrade: Mutex::new(None),
        teach_recording: Mutex::new(None),
        backend_url,
        mcp: Mutex::new(None),
        box_store_sync: Mutex::new(None),
        state_backstop: Mutex::new(None),
        automations: Mutex::new(None),
        _statsig_bootstrap: statsig_bootstrap,
    })
}


impl ProductionHostExtensions {
    /// Registers a provider port only after the channel is present in the
    /// canonical plugin capability catalog. This keeps plugin discovery and
    /// delivery registration on one Host-owned boundary.
    pub fn register_channel_delivery_provider(
        &self,
        channel_id: &str,
        port: Arc<dyn ChannelDeliveryPort>,
    ) -> Result<(), String> {
        if self.plugin_capability_catalog.get_channel(channel_id).is_none() {
            return Err(format!("channel is not registered: {channel_id}"));
        }
        self.channel_delivery_router.register(channel_id, port)
    }

    pub fn replace_channel_delivery_provider(
        &self,
        channel_id: &str,
        port: Arc<dyn ChannelDeliveryPort>,
    ) -> Result<(), String> {
        if self.plugin_capability_catalog.get_channel(channel_id).is_none() {
            return Err(format!("channel is not registered: {channel_id}"));
        }
        self.channel_delivery_router.replace(channel_id, port)
    }

    pub fn unregister_channel_delivery_provider(&self, channel_id: &str) -> Result<bool, String> {
        if self.plugin_capability_catalog.get_channel(channel_id).is_none() {
            return Err(format!("channel is not registered: {channel_id}"));
        }
        Ok(self.channel_delivery_router.unregister(channel_id))
    }

    /// Revokes a bundled channel's provider and discovery metadata together.
    ///
    /// Provider revocation happens first so no new delivery can be admitted
    /// after the capability entry is removed. The catalog gate rejects unknown
    /// channels and prevents this lifecycle operation from becoming a second
    /// plugin registry.
    pub fn revoke_bundled_channel(&self, channel_id: &str) -> Result<bool, String> {
        if self.plugin_capability_catalog.get_channel(channel_id).is_none() {
            return Err(format!("channel is not registered: {channel_id}"));
        }
        let provider_removed = self.channel_delivery_router.unregister(channel_id);
        let catalog_removed = self
            .plugin_capability_catalog
            .unregister_channel(channel_id)
            .is_some();
        Ok(provider_removed || catalog_removed)
    }

    pub fn stop_structured_log_domain_reporters(&self) {
        unpin_structured_log_domain_reporters();
    }

    pub fn stop_cloud_agents(&self) {
        self.cloud_agents.stop();
    }

    pub fn start_cross_user(
        &self,
        attachments: Arc<AttachmentsService>,
        shared_rooms: Arc<SharedRooms>,
        run_remote_requested_turn: RemoteRequestedTurnRunner,
        run_shared_room_turn: SharedRoomTurnRunner,
    ) -> Result<Arc<ProductionCrossUserRuntime>, String> {
        let mut slot = self
            .cross_user
            .lock()
            .map_err(|_| "production CrossUserSharing runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production CrossUserSharing runtime is already started".into());
        }
        let runtime = ProductionCrossUserRuntime::new(
            Arc::clone(&self.auth),
            self.notify_bus.clone(),
            attachments,
            shared_rooms,
            run_remote_requested_turn,
            run_shared_room_turn,
        )?;
        *slot = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    pub fn start_cross_user_background_work(&self) -> Result<(), String> {
        let runtime = self
            .cross_user
            .lock()
            .map_err(|_| "production CrossUserSharing runtime lock poisoned".to_string())?
            .as_ref()
            .cloned()
            .ok_or_else(|| "production CrossUserSharing runtime is not started".to_string())?;
        runtime.start_background_work(Arc::clone(&self.experiments))
    }

    pub fn stop_cross_user(&self) -> Result<(), String> {
        let runtime = self
            .cross_user
            .lock()
            .map_err(|_| "production CrossUserSharing runtime lock poisoned".to_string())?
            .take();
        if let Some(runtime) = runtime {
            runtime.stop();
        }
        Ok(())
    }

    pub fn start_transcript(
        &self,
        root_dir: &Path,
        sessions: Arc<ProductionSessionWorkers>,
        attachments: Arc<crate::extensions::attachments::attachments_service::AttachmentsService>,
        events: SandHostEventBus,
    ) -> Result<(
        Arc<TranscriptManager>,
        Arc<ProductionRosterEmit>,
        TranscriptExtensionEventBridge,
        Option<String>,
    ), String> {
        let mut slot = self
            .transcript
            .lock()
            .map_err(|_| "production Transcript runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production Transcript runtime is already started".into());
        }
        let extension = start_production_transcript_extension(
            root_dir,
            sessions,
            TranscriptExtensionDeps {
                attachments,
                content_search: Arc::clone(&self.content_search),
                memory: self.memory.clone(),
                telemetry: self.telemetry.clone(),
                trays: Arc::clone(&self.trays),
                turn_execution: Arc::clone(&self.turn_execution),
                events,
            },
        );
        let profile_watch_error = extension.profile_watch_error().map(str::to_string);
        let manager = extension.manager();
        let roster = extension.roster_emit();
        let event_bridge = extension
            .event_bridge()
            .ok_or_else(|| "production Transcript extension did not install its event bridge".to_string())?;
        *slot = Some(extension);
        Ok((manager, roster, event_bridge, profile_watch_error))
    }

    pub fn stop_transcript(&self) -> Result<(), String> {
        let extension = self
            .transcript
            .lock()
            .map_err(|_| "production Transcript runtime lock poisoned".to_string())?
            .take();
        drop(extension);
        Ok(())
    }

    pub fn start_auto_review(
        &self,
        sessions: Arc<ProductionSessionWorkers>,
        host_generation: impl Into<String>,
        on_update: AutoReviewUpdateSink,
        telemetry: AutoReviewTelemetrySink,
        expire_sweep_failed: Option<AutoReviewExpireSweepFailedSink>,
    ) -> Result<Arc<HostAutoReviewExtension>, String> {
        let mut slot = self
            .auto_review
            .lock()
            .map_err(|_| "production AutoReview runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production AutoReview runtime is already started".into());
        }
        let extension = Arc::new(start_auto_review_extension_with_expire_sweep_telemetry(
            sessions,
            Arc::clone(&self.experiments),
            Arc::clone(&self.settings),
            host_generation,
            on_update,
            telemetry,
            expire_sweep_failed,
        ));
        *slot = Some(Arc::clone(&extension));
        Ok(extension)
    }

    pub fn stop_auto_review(&self) -> Result<(), String> {
        let extension = self
            .auto_review
            .lock()
            .map_err(|_| "production AutoReview runtime lock poisoned".to_string())?
            .take();
        if let Some(extension) = extension {
            extension.stop();
        }
        Ok(())
    }

    pub fn start_session(
        &self,
        store: Arc<ProductionSessionWorkers>,
        handoff_deps: BoxHandoffDeps,
    ) -> Result<(Arc<ProductionSessionWorkers>, BoxHandoffService), String> {
        let mut slot = self
            .session
            .lock()
            .map_err(|_| "production Session runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production Session runtime is already started".into());
        }
        let extension = start_session_extension(
            Arc::clone(&self.experiments),
            store,
            handoff_deps,
        );
        let workers = extension.store();
        let handoff = extension.handoff_service();
        *slot = Some(extension);
        Ok((workers, handoff))
    }

    pub fn shutdown_session(&self) -> Result<(), String> {
        let slot = self
            .session
            .lock()
            .map_err(|_| "production Session runtime lock poisoned".to_string())?;
        if let Some(extension) = slot.as_ref() {
            extension.shutdown();
        }
        Ok(())
    }

    pub fn start_host_upgrade(
        &self,
        peers: ProductionHostUpgradePeers,
    ) -> Result<Arc<ProductionHostUpgradeExtension>, String> {
        let mut slot = self
            .host_upgrade
            .lock()
            .map_err(|_| "production HostUpgrade runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production HostUpgrade runtime is already started".into());
        }
        let extension = start_production_host_upgrade_extension(
            peers,
            self.telemetry.logs.clone(),
            Arc::new(|| Ok(())),
        )?;
        *slot = Some(Arc::clone(&extension));
        Ok(extension)
    }

    pub fn stop_host_upgrade(&self) -> Result<(), String> {
        let extension = self
            .host_upgrade
            .lock()
            .map_err(|_| "production HostUpgrade runtime lock poisoned".to_string())?
            .take();
        if let Some(extension) = extension {
            extension.stop_background_work();
        }
        Ok(())
    }

    pub fn start_teach_recording(
        &self,
        forever_box: Arc<ForeverBoxService>,
        sessions: Arc<ProductionSessionWorkers>,
        send_learning_prompt: Arc<
            dyn Fn(&str, &str, &str, Option<&str>) -> Result<(), String> + Send + Sync
        >,
    ) -> Result<TeachRecordingApi, String> {
        let mut slot = self
            .teach_recording
            .lock()
            .map_err(|_| "production TeachRecording runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production TeachRecording runtime is already started".into());
        }
        let runtime: Arc<dyn TeachRecordingRuntimePort> =
            Arc::new(ProductionTeachRecordingRuntime::new(forever_box));
        let factory = SandTeachRecordingServiceFactory::new(runtime);
        let experiments = Arc::clone(&self.experiments);
        let managed_setup = Arc::clone(&self.managed_setup);
        let analytics_started = self.telemetry.analytics.clone();
        let analytics_stopped = self.telemetry.analytics.clone();
        let cap_logs = self.telemetry.logs.clone();
        let start_logs = self.telemetry.logs.clone();
        let deps = TeachRecordingServiceDeps {
            is_enabled: Arc::new(move || {
                experiments.check_feature_gate("sand_teach_by_demonstration")
            }),
            cap_delay_ms: SAND_TEACH_MAX_DURATION_MS + CAP_SLACK_MS,
            send_learning_prompt,
            list_agent_ids: Arc::new(move || sessions.list_agent_record_ids()),
            queue_signature_key: Arc::new(|| load_teach_queue_key(None)),
            ensure_learning_workflow: Arc::new(move || {
                Ok(managed_setup.ensure_managed_skill(LEARN_SKILL_NAME))
            }),
            track_recording_started: Arc::new(move |event| {
                let _ = analytics_started.track_event("sand.teach.recording_started", &event);
            }),
            track_recording_stopped: Arc::new(move |event| {
                let _ = analytics_stopped.track_event("sand.teach.recording_stopped", &event);
            }),
            report_cap_stop_failed: Arc::new(move |event| {
                let _ = cap_logs.report_teach_recording_cap_stop_failed(&event);
            }),
            report_start_failed: Arc::new(move |event| {
                let _ = start_logs.report_teach_recording_start_failed(&event);
            }),
        };
        let extension = create_teach_recording_extension(&factory, deps);
        let api = extension.api();
        let recovery_service = Arc::clone(extension.service());
        thread::Builder::new()
            .name("mahayana-teach-recording-recovery".into())
            .spawn(move || {
                if let Err(error) = recovery_service.recover_pending() {
                    eprintln!("teach-recording: pending delivery recovery failed: {error}");
                }
            })
            .map_err(|error| format!("could not start TeachRecording recovery worker: {error}"))?;
        *slot = Some(extension);
        Ok(api)
    }

    pub fn stop_teach_recording(&self) -> Result<(), String> {
        let extension = self
            .teach_recording
            .lock()
            .map_err(|_| "production TeachRecording runtime lock poisoned".to_string())?
            .take();
        drop(extension);
        Ok(())
    }

    pub fn start_notifications(
        &self,
        events: SandHostEventBus,
        initial_baseline: Option<Vec<NotificationAgent>>,
        window_focused_at: WindowFocusedAtSource,
    ) -> Result<(), String> {
        let mut slot = self
            .notifications
            .lock()
            .map_err(|_| "production Notifications runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production Notifications runtime is already started".into());
        }
        let extension = start_notifications_extension(
            Arc::clone(&self.auth),
            events,
            initial_baseline,
            window_focused_at,
        )?;
        *slot = Some(extension);
        Ok(())
    }

    pub fn stop_notifications(&self) -> Result<(), String> {
        let extension = self
            .notifications
            .lock()
            .map_err(|_| "production Notifications runtime lock poisoned".to_string())?
            .take();
        drop(extension);
        Ok(())
    }

    pub fn start_automations(
        &self,
        listeners: RelayListeners,
        relay_sink: RelayEventSink,
        fire_dispatch: FireDispatch,
        on_connected: ListenerConnectedCallback,
        agent_channels: AgentChannelsReader,
        cloud_definitions: CloudDefinitionsReader,
        cloud_agent_ids: CloudAgentIdsReader,
        cloud_time_zone: CloudTimeZoneReader,
        log: ProductionLog,
    ) -> Result<Arc<ProductionAutomationsLifecycle>, String> {
        let mut slot = self
            .automations
            .lock()
            .map_err(|_| "production Automations runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production Automations runtime is already started".into());
        }
        let fire_definitions = Arc::clone(&cloud_definitions);
        let fire_inference = Arc::clone(&self.inference);
        let fire_poll_state: FirePollStateReader = Arc::new(move || {
            let has_server_schedulable = fire_definitions()
                .map(|entries| {
                    entries.into_iter().any(|(_, automation)| {
                        automation.is_enabled && is_server_schedulable(&automation.trigger)
                    })
                })
                .unwrap_or(false);
            (fire_inference.is_ready(), has_server_schedulable)
        });
        let runtime = ProductionAutomationsLifecycle::start(
            self.backend_url.clone(),
            Arc::clone(&self.auth),
            self.notify_bus.clone(),
            listeners,
            relay_sink,
            fire_dispatch,
            on_connected,
            agent_channels,
            cloud_definitions,
            cloud_agent_ids,
            cloud_time_zone,
            fire_poll_state,
            Arc::clone(&self.trays),
            self.telemetry.logs.clone(),
            log,
        )?;
        *slot = Some(Arc::clone(&runtime));
        Ok(runtime)
    }

    pub fn stop_automations(&self) -> Result<(), String> {
        let runtime = self
            .automations
            .lock()
            .map_err(|_| "production Automations runtime lock poisoned".to_string())?
            .take();
        if let Some(runtime) = runtime {
            runtime.stop();
        }
        Ok(())
    }

    pub fn start_mcp(
        &self,
        sand_root_dir: &Path,
        relay: Arc<CoordinatorMcpLifecycleRelay>,
        backend: Arc<dyn McpManagerBackend>,
    ) -> Result<Arc<McpHostService>, String> {
        let mut slot = self.mcp.lock().map_err(|_| "production MCP runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production MCP runtime is already started".into());
        }

        let loader: Arc<dyn PluginSkillsLoader> =
            Arc::new(CoordinatorPluginSkillsLoader::new(relay));
        let plugin_sync_logs = self.telemetry.logs.clone();
        let plugin_skills = Arc::new(
            SandPluginSkillsService::new(sand_root_dir.to_path_buf(), loader).with_reporter(
                Arc::new(move |report| {
                    let _ = plugin_sync_logs.report_plugin_skills_sync(
                        &report.trigger,
                        &report.outcome,
                        report.changed,
                        report.skill_count,
                        report.error_class.as_deref(),
                        report.duration_ms,
                    );
                }),
            ),
        );
        let plugin_port: Arc<dyn PluginSkillsPort> = plugin_skills.clone();
        let backend_for_sweep = Arc::clone(&backend);
        let service = Arc::new(McpHostService::new(backend, Some(plugin_port)));

        let (cleanup_outcome, removed_credentials) =
            cleanup_legacy_mcp_auth_credentials(sand_root_dir);
        let cleanup_tag = if cleanup_outcome == LegacyMcpAuthCleanupOutcome::Error {
            "error"
        } else {
            "ok"
        };
        let _ = self
            .telemetry
            .logs
            .report_mcp_auth_cleanup(cleanup_tag, removed_credentials);
        if cleanup_outcome == LegacyMcpAuthCleanupOutcome::Error {
            eprintln!(
                "MCP legacy auth cleanup completed with errors after removing {removed_credentials} file(s)"
            );
        }

        let auth: Arc<dyn AuthRenewalPort> = self.auth.clone();
        let plugin_runtime: Arc<dyn McpPluginSkillsService> = plugin_skills.clone();
        let polling: Arc<dyn PollingPort> = Arc::new(RealPluginSkillsPolling::daily());
        let sweep_root = sand_root_dir.to_path_buf();
        let on_startup_sync_succeeded: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let Ok(catalog) = backend_for_sweep.list_catalog(false) else {
                eprintln!("MCP legacy live-reference sweep could not load the marketplace catalog");
                return;
            };
            let catalog = catalog
                .into_iter()
                .map(|plugin| PluginCatalogEntry {
                    plugin_id: plugin.id,
                    skills: plugin
                        .skills
                        .into_iter()
                        .map(|skill| PluginSkillCatalogEntry {
                            source_url: skill.source_url,
                        })
                        .collect(),
                })
                .collect::<Vec<_>>();
            let removed = sweep_legacy_plugin_skill_references(&sweep_root, &catalog);
            if removed > 0 {
                eprintln!(
                    "[sand:plugin-skills] retired {removed} legacy live-reference record(s)"
                );
            }
        });
        let startup = PluginSkillsAuthenticatedStartup::start(
            auth,
            plugin_runtime,
            polling,
            Some(on_startup_sync_succeeded),
        );
        let skill_edge_logs = self.telemetry.logs.clone();
        let skill_publish = Arc::new(
            create_production_skill_publish(
                sand_root_dir.to_path_buf(),
                self.backend_url.clone(),
                Arc::clone(&self.auth),
                plugin_skills,
            )
            .with_edge_reporter(Arc::new(move |failure| {
                let _ = skill_edge_logs
                    .report_skill_publish_edge_failed(failure.stage, &failure.error_class);
            })),
        );
        *slot = Some(McpExtensionRuntime::new(
            startup,
            service.clone(),
            skill_publish,
        ));
        Ok(service)
    }

    pub fn stop_mcp(&self) -> Result<(), String> {
        let runtime = self
            .mcp
            .lock()
            .map_err(|_| "production MCP runtime lock poisoned".to_string())?
            .take();
        runtime.map(|runtime| runtime.stop()).unwrap_or(Ok(()))
    }

    pub fn start_box_store_sync(
        &self,
        is_idle: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<(), String> {
        let mut slot = self
            .box_store_sync
            .lock()
            .map_err(|_| "production BoxStoreSync runtime lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("production BoxStoreSync runtime is already started".into());
        }
        if self
            .mcp
            .lock()
            .map_err(|_| "production MCP runtime lock poisoned".to_string())?
            .is_none()
        {
            return Err("production BoxStoreSync requires MCP to be started first".into());
        }
        let logs = self.telemetry.logs.clone();
        let diagnostic_logs = logs.clone();
        let box_store_sync_logs = logs.clone();
        let box_store_db_capture_logs = logs.clone();
        let box_store_manifest_conflict_logs = logs.clone();
        let chrome_session_stage_logs = logs.clone();
        let source_map = Arc::clone(&self.source_map);
        let token_auth = Arc::clone(&self.auth);
        let machine_auth = Arc::clone(&self.auth);
        let extension = start_box_store_sync_extension(
            &ProductionBoxStoreSyncFactory,
            BoxStoreSyncExtensionDeps {
                native_credentials: Some(self.auth.service().clone()),
                is_idle,
                resolve_store_id: Arc::new(move || {
                    source_map
                        .get_or_create_box_store()
                        .map(|entry| entry.source_id)
                        .map_err(|error| error.to_string())
                }),
                backend_url: Some(self.backend_url.clone()),
                get_access_token: Some(Arc::new(move || {
                    token_auth.get_access_token().map_err(|error| error.to_string())
                })),
                get_machine_id: Some(Arc::new(move || {
                    machine_auth.get_machine_id().map_err(|error| error.to_string())
                })),
                log: Arc::new(move |message| eprintln!("{message}")),
                report_host_extension_diagnostic: Arc::new(move |diagnostic| {
                    let _ = diagnostic_logs.report_host_extension_diagnostic(
                        &serde_json::Value::Object(diagnostic.clone()),
                    );
                }),
                report_box_store_sync_cycle: Arc::new(move |level, metadata| {
                    let _ = box_store_sync_logs.report_box_store_sync_cycle(level, metadata);
                }),
                report_box_store_db_capture: Arc::new(move |level, metadata| {
                    let _ = box_store_db_capture_logs.report_box_store_db_capture(level, metadata);
                }),
                report_box_store_manifest_conflict: Arc::new(move |level, metadata| {
                    let _ = box_store_manifest_conflict_logs
                        .report_box_store_manifest_conflict(level, metadata);
                }),
                report_chrome_session_stage: Arc::new(move |level, metadata| {
                    let _ = chrome_session_stage_logs.report_chrome_session_stage(level, metadata);
                }),
                scheduling: BoxStoreScheduling::default(),
            },
        );
        let box_api = extension.api();
        *slot = Some(extension);
        drop(slot);

        let mut backstop_slot = self
            .state_backstop
            .lock()
            .map_err(|_| "production StateBackstop runtime lock poisoned".to_string())?;
        if backstop_slot.is_some() {
            return Err("production StateBackstop runtime is already started".into());
        }
        let runtime = create_production_state_backstop_runtime();
        let store_api = box_api.clone();
        let provider = Arc::new(move |source_id: &str| -> Arc<dyn StateBackstopObjectStore> {
            Arc::new(ProductionStateBackstopObjectStore::new(
                store_api.object_store_for(source_id),
            ))
        });
        let source_map = Arc::clone(&self.source_map);
        let source_id_for_agent = Arc::new(move |agent_id: &str| {
            source_map
                .get_or_create(agent_id)
                .map(|entry| entry.source_id)
                .map_err(|error| error.to_string())
        });
        let options = StateBackstopOptions::new(
            provider,
            source_id_for_agent,
            runtime.agents_root_dir,
            runtime.read_db_bytes,
        );
        let state_backstop = start_state_backstop_extension(options);
        if state_backstop.is_enabled() {
            eprintln!("SandState S3 backstop enabled");
        }
        *backstop_slot = Some(state_backstop);
        Ok(())
    }

    pub fn box_store_sync_api(&self) -> Result<Option<ProductionBoxStoreSyncApi>, String> {
        let slot = self
            .box_store_sync
            .lock()
            .map_err(|_| "production BoxStoreSync runtime lock poisoned".to_string())?;
        Ok(slot.as_ref().map(|extension| extension.api()))
    }

    pub fn state_backstop_is_enabled(&self) -> Result<bool, String> {
        let slot = self
            .state_backstop
            .lock()
            .map_err(|_| "production StateBackstop runtime lock poisoned".to_string())?;
        Ok(slot.as_ref().is_some_and(HostStateBackstopExtension::is_enabled))
    }

    pub fn schedule_state_backstop_snapshot(&self, agent_id: &str) -> Result<(), String> {
        let slot = self
            .state_backstop
            .lock()
            .map_err(|_| "production StateBackstop runtime lock poisoned".to_string())?;
        if let Some(extension) = slot.as_ref() {
            extension.schedule_snapshot(agent_id.to_string());
        }
        Ok(())
    }

    pub fn stop_box_store_sync(&self) -> Result<(), String> {
        let state_backstop = self
            .state_backstop
            .lock()
            .map_err(|_| "production StateBackstop runtime lock poisoned".to_string())?
            .take();
        if let Some(state_backstop) = state_backstop {
            state_backstop.stop();
        }
        let extension = self
            .box_store_sync
            .lock()
            .map_err(|_| "production BoxStoreSync runtime lock poisoned".to_string())?
            .take();
        drop(extension);
        Ok(())
    }
}


/// Second production-extension stage. These frozen Grok slots require the
/// platform Box environment, so they are composed only after the core Host
/// extensions exist. Keeping this stage in the same composition module avoids
/// giving app/main.rs a second extension registry.
pub struct ProductionHostBoxExtensions {
    pub forever_box: Arc<ForeverBoxService>,
    pub attachments: HostAttachmentsExtension,
    pub secrets: Arc<HostSecretsExtension>,
}

impl ProductionHostBoxExtensions {
    pub fn stop(&mut self) {
        self.attachments.stop_lifecycle_worker();
        self.secrets.stop();
        self.forever_box.dispose();
    }
}

pub fn start_production_host_box_extensions(
    core: &ProductionHostExtensions,
    environment: ProductionBoxEnvironment,
) -> Result<ProductionHostBoxExtensions, String> {
    let lifecycle: Arc<dyn ForeverBoxLifecycle> = core.box_lifecycle.clone();
    let forever_box = start_forever_box_extension_with_telemetry(
        environment,
        lifecycle,
        ForeverBoxExtensionOptions::from_process_env(),
        core.telemetry.logs.clone(),
    );
    let attachment_logs = core.telemetry.logs.clone();
    let attachments = start_attachments_extension(
        Arc::clone(&core.auth),
        Arc::clone(&forever_box),
        Some(Arc::new(move |diagnostic| {
            let _ = attachment_logs.report_host_extension_diagnostic(&diagnostic);
        })),
    )?;
    let secrets = Arc::new(start_secrets_extension(
        Arc::clone(&forever_box),
        production_secrets_log(),
    ));
    Ok(ProductionHostBoxExtensions {
        forever_box,
        attachments,
        secrets,
    })
}
