use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::extensions::auth::auth_service::HostAuthRenewalEvent;
use crate::extensions::auth::credential_renewer::{
    RenewalOutcome, SAND_CLIENT_TYPE, sand_client_version,
};
use crate::extensions::auth::extension::HostAuthExtension;
use crate::extensions::browser_ua::extension::StopSubscription;
use crate::extensions::experiments::{
    HostExperimentsExtension, pin_experiments_diagnostics_reporter,
};
use crate::extensions::inference::production::ProductionInferenceExtension;
use crate::extensions::inference::sand_model_experiment::SandModelExperimentArm;
use crate::host_paths::get_host_crash_marker_path;
use crate::host_invariant::SandInvariantReport;
use crate::gateway_server::GatewayCommandReport;
use crate::process_spawn_diagnostics::SpawnCountRecord;
use crate::ports::sand_analytics_types::sand_message_length_bucket;
use crate::ports::telemetry::resolve_sand_box_identity_tags;
use crate::send_trace_host::set_turn_trace_host_bundle_version;

use super::HostTelemetryProjection;
use super::analytics_service::{
    AnalyticsClient, AutomationRunAnalyticsTelemetry, ProductionAnalyticsRuntime, TelemetryService,
    product_analytics_event,
};
use super::agent_error_telemetry::{
    AgentErrorReport, agent_error_detail_telemetry, agent_error_telemetry,
};
use super::agent_open_telemetry::{AgentOpenReport, agent_open_telemetry};
use super::invariant_violation_telemetry::invariant_violation_telemetry;
use super::journal_outcome_telemetry::{
    JournalOutcomeReport, journal_outcome_telemetry,
};
use super::auto_review_approval_telemetry::{
    AutoReviewApprovalReport, auto_review_approval_telemetry,
};
use super::automation_fire_telemetry::{
    AutomationFireDroppedReport, AutomationRunReport, automation_fire_dropped_telemetry,
    automation_run_telemetry,
};
use super::automation_lifecycle_telemetry::automation_lifecycle_telemetry;
use super::bot_block_telemetry::{
    BotBlockReport, bot_block_detail_telemetry, bot_block_telemetry,
};
use crate::extensions::transcript::automation_runtime::AutomationLifecycleEvent;
use super::box_log_ship_telemetry::{BoxLogShipReport, box_log_ship_telemetry};
use super::box_log_shipper::{
    BoxLogShipper, BoxLogShipperConfig, BoxTelemetryRecord, DeliverySettlement,
    SUPERVISOR_LOG_PATH, is_box_log_shipping_enabled,
};
use super::desktop_health_forwarder::{DesktopHealthForwardResult, forward_desktop_health_with};
use super::event_loop_telemetry::{
    EventLoopTelemetryRuntime, EventLoopTrigger, EventLoopWindowReport, event_loop_window_telemetry,
};
use super::experiments_diagnostic_telemetry::{
    ExperimentsDiagnostic, experiments_diagnostic_telemetry,
};
use super::host_crash_marker::{
    FileHostCrashMarkerStore, ForwardHostCrashMarkerResult, HostCrashMarkerStore,
    forward_host_crash_marker_with, host_crash_marker_metadata,
};
use super::host_lifecycle_progress::{HostLifecycleProgress, HostLifecycleReport, WatchdogArm};
use super::host_tracing::{HostTracing, init_production_host_tracing};
use super::lifecycle_telemetry::{
    BoxImageCheckReport, DaemonPingReport, box_image_check_telemetry, box_infrastructure_telemetry,
    daemon_ping_telemetry, host_lifecycle_telemetry, host_startup_telemetry,
};
use super::conversation_gc_telemetry::{ConversationGcReport, conversation_gc_telemetry};
use super::host_diagnostic_telemetry::{HostDiagnostic, host_diagnostic_telemetry};
use super::host_event_bus_telemetry::{HostEventBusReport, host_event_bus_telemetry};
use super::gateway_command_telemetry::{gateway_command_error_telemetry, gateway_command_timing_telemetry};
use super::disk_pressure_telemetry::{DiskPressureReport, disk_pressure_telemetry};
use super::session_diagnostic_telemetry::{SessionTelemetryDiagnostic, session_diagnostic_telemetry};
use super::local_exec_telemetry::{
    LocalExecFailedReport, LocalExecProviderReport, LocalExecRefusedReport,
    local_exec_failed_telemetry, local_exec_provider_telemetry, local_exec_refused_telemetry,
};
use super::memory_synthesis_telemetry::{MemorySynthesisReport, memory_synthesis_telemetry};
use super::mcp_discovery_telemetry::{
    McpDiscoveryFailedReport, mcp_discovery_failed_telemetry,
};
use super::connector_auth_telemetry::{
    ConnectorAuthReport, connector_auth_telemetry,
};
use super::box_copy_in_telemetry::box_copy_in_telemetry;
use super::box_recreate_decided_telemetry::box_recreate_decided_telemetry;
use super::model_experiment_exposure::{
    ModelExperimentExposureAnalytics, ModelExperimentExposureExperiments,
    ModelExperimentExposureLatch, SandModelExperimentState as ExposureModelExperimentState,
};
use super::pressure_cpu_profiler::{
    PressureCpuProfiler, SandProfilerCaptureError, create_production_pressure_cpu_profiler,
};
use super::queue_telemetry_mappers::{
    AckObligationReport, PendingWakeReport, QueueAcceptedReport, QueueDequeuedReport,
    QueueWatchdogReport, SendDispatchReport, ack_obligation_telemetry, pending_wake_telemetry,
    queue_accepted_telemetry, queue_dequeued_telemetry, queue_watchdog_telemetry,
    send_dispatch_telemetry,
};
use super::revival_telemetry_mappers::{
    ShellRevivalReport, SubagentRevivalReport, shell_revival_telemetry, subagent_revival_telemetry,
};
use super::sand_error_tags::{SandErrorPayloadValue, SandErrorValue, sand_error_tags};
use super::structured_log_telemetry::{
    BOX_HELP_EVENT, CursorStructuredLogBackend, HOST_BUILT_AT_MS, ProductionStructuredLogTransport,
    StructuredLogBackend, box_help_telemetry, level_from_str,
};
use super::turn_empty_delivery_telemetry::{
    TurnEmptyDeliveryReport, turn_empty_delivery_telemetry,
};
use super::turn_telemetry_mappers::{
    ClosingSendNudgeFields, ComputerUseUsageFields, TtftFields, TurnAwaitFields,
    TurnInterruptFields, TurnRetryFields, TurnUsageFields, UserMessageReceivedFields,
    closing_send_nudge_telemetry, computer_use_usage_telemetry, ttft_telemetry,
    turn_await_telemetry, turn_interrupt_telemetry, turn_retry_telemetry,
    turn_usage_telemetry, user_message_received_telemetry,
};
use super::webauthn_proxy_telemetry::{WebAuthnProxyReport, webauthn_proxy_telemetry};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedHostTelemetryRecord {
    pub channel: String,
    pub event: String,
    pub payload: Value,
}

struct JsonlHostTelemetrySink {
    path: PathBuf,
    file: Mutex<File>,
}

impl JsonlHostTelemetrySink {
    fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
        })
    }

    fn emit(&self, record: &PersistedHostTelemetryRecord) -> io::Result<()> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| io::Error::other("Host telemetry sink mutex poisoned"))?;
        serde_json::to_writer(&mut *file, record).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.flush()
    }

    fn flush(&self) -> io::Result<()> {
        self.file
            .lock()
            .map_err(|_| io::Error::other("Host telemetry sink mutex poisoned"))?
            .flush()
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone)]
pub struct HostStructuredLogTelemetry {
    sink: Arc<JsonlHostTelemetrySink>,
    identity_tags: Arc<Mutex<BTreeMap<String, String>>>,
    production_transport: Option<Arc<ProductionStructuredLogTransport>>,
    production_backend: Option<Arc<dyn StructuredLogBackend>>,
    active_turns: Arc<Mutex<BTreeMap<String, String>>>,
}

pub const TURN_START_EVENT: &str = "sand.turn.start";
pub const TURN_OUTCOME_EVENT: &str = "sand.turn.outcome";
pub const TURN_OUTCOME_DETAIL_EVENT: &str = "sand.turn.outcome_detail";
const MAX_ERROR_DETAIL_MESSAGE_LENGTH: usize = 1_024;
const MAX_ERROR_DETAIL_STACK_LENGTH: usize = 4_096;

pub struct HostStructuredLogTurnTelemetry {
    logs: HostStructuredLogTelemetry,
    token: String,
    conversation_id: String,
    turn_type: String,
    model: Option<String>,
    request_id: Option<String>,
    started_at: Instant,
    start_emitted: bool,
    finalized: bool,
    retry_count: u64,
    backoff_total_ms: u64,
    retry_cause: Option<String>,
}

impl HostStructuredLogTurnTelemetry {
    fn base_tags(&self) -> BTreeMap<String, String> {
        let mut metadata = BTreeMap::from([
            ("turn_type".into(), self.turn_type.clone()),
            ("conversation_id".into(), self.conversation_id.clone()),
        ]);
        if let Some(request_id) = self.request_id.as_ref() {
            metadata.insert("request_id".into(), request_id.clone());
        }
        if let Some(model) = self.model.as_ref() {
            metadata.insert("model_intent".into(), model.clone());
        }
        metadata
    }

    fn emit_start(&mut self) {
        if self.start_emitted {
            return;
        }
        self.start_emitted = true;
        let _ = self.logs.report_projection(&HostTelemetryProjection {
            level: Some("info"),
            event: Some(TURN_START_EVENT),
            metadata: self.base_tags(),
        });
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        if model.is_empty() {
            return;
        }
        self.model = Some(model);
        self.emit_start();
    }

    pub fn set_request_id(&mut self, request_id: impl Into<String>) {
        if self.request_id.is_some() {
            return;
        }
        let request_id = request_id.into();
        if !request_id.is_empty() {
            self.request_id = Some(request_id);
        }
    }

    pub fn note_retry(&mut self, delay_ms: Option<u64>, cause: Option<&str>) {
        if self.finalized {
            return;
        }
        self.retry_count = self.retry_count.saturating_add(1);
        self.backoff_total_ms = self
            .backoff_total_ms
            .saturating_add(delay_ms.unwrap_or_default());
        if let Some(cause) = cause.filter(|value| !value.is_empty()) {
            self.retry_cause = Some(cause.to_string());
        }
    }

    pub fn finalize(
        &mut self,
        outcome: &str,
        error: Option<&SandErrorValue>,
        detail: Option<(&str, Option<&str>)>,
    ) {
        if self.finalized {
            return;
        }
        self.finalized = true;
        self.emit_start();
        let mut metadata = self.base_tags();
        metadata.insert(
            "outcome".into(),
            if error.is_some() { "error" } else { outcome }.into(),
        );
        metadata.insert(
            "duration_ms".into(),
            self.started_at.elapsed().as_millis().to_string(),
        );
        if self.retry_count > 0 {
            metadata.insert("retry_count".into(), self.retry_count.to_string());
            metadata.insert("backoff_total_ms".into(), self.backoff_total_ms.to_string());
            if let Some(cause) = self.retry_cause.as_ref() {
                metadata.insert("retry_cause".into(), cause.clone());
            }
        }
        if let Some(error) = error {
            metadata.extend(sand_error_tags(error));
        }
        let _ = self.logs.report_projection(&HostTelemetryProjection {
            level: Some("info"),
            event: Some(TURN_OUTCOME_EVENT),
            metadata,
        });
        if let (Some(error), Some((message, stack))) = (error, detail) {
            let mut metadata = self.base_tags();
            metadata.extend(sand_error_tags(error));
            metadata.insert(
                "error_message".into(),
                message
                    .chars()
                    .take(MAX_ERROR_DETAIL_MESSAGE_LENGTH)
                    .collect(),
            );
            if let Some(stack) = stack {
                metadata.insert(
                    "error_stack".into(),
                    stack.chars().take(MAX_ERROR_DETAIL_STACK_LENGTH).collect(),
                );
            }
            let _ = self.logs.report_projection(&HostTelemetryProjection {
                level: Some("info"),
                event: Some(TURN_OUTCOME_DETAIL_EVENT),
                metadata,
            });
        }
        if let Ok(mut active) = self.logs.active_turns.lock()
            && active.get(&self.conversation_id) == Some(&self.token)
        {
            active.remove(&self.conversation_id);
        }
    }
}

impl HostStructuredLogTelemetry {
    pub fn start_turn(
        &self,
        conversation_id: impl Into<String>,
        turn_type: impl Into<String>,
        model: Option<String>,
    ) -> HostStructuredLogTurnTelemetry {
        let conversation_id = conversation_id.into();
        let token = uuid::Uuid::new_v4().to_string();
        self.active_turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(conversation_id.clone(), token.clone());
        let mut turn = HostStructuredLogTurnTelemetry {
            logs: self.clone(),
            token,
            conversation_id,
            turn_type: turn_type.into(),
            model,
            request_id: None,
            started_at: Instant::now(),
            start_emitted: false,
            finalized: false,
            retry_count: 0,
            backoff_total_ms: 0,
            retry_cause: None,
        };
        if turn.model.is_some() {
            turn.emit_start();
        }
        turn
    }

    pub fn flush(&self) -> io::Result<()> {
        self.sink.flush()?;
        if let Some(transport) = self.production_transport.as_ref() {
            let _ = transport.flush();
        }
        Ok(())
    }

    pub fn report_projection(&self, projection: &HostTelemetryProjection) -> io::Result<()> {
        let mut metadata = self
            .identity_tags
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        metadata.extend(projection.metadata.clone());
        let event = projection
            .event
            .unwrap_or("sand.host.telemetry")
            .to_string();
        let level = projection.level.unwrap_or("info");
        self.sink.emit(&PersistedHostTelemetryRecord {
            channel: "structured_log".into(),
            event: event.clone(),
            payload: json!({
                "level": level,
                "metadata": metadata,
            }),
        })?;
        if let Some(transport) = self.production_transport.as_ref() {
            let _ = transport.enqueue(level_from_str(level), event, projection.metadata.clone());
        }
        Ok(())
    }

    fn report_raw(&self, event: &str, report: &Value) -> io::Result<()> {
        self.sink.emit(&PersistedHostTelemetryRecord {
            channel: "structured_log".into(),
            event: event.to_string(),
            payload: report.clone(),
        })?;
        if let Some(transport) = self.production_transport.as_ref() {
            let mut metadata = BTreeMap::new();
            if let Some(object) = report.as_object() {
                for (key, value) in object {
                    match value {
                        Value::String(value) => {
                            metadata.insert(key.clone(), value.clone());
                        }
                        Value::Bool(value) => {
                            metadata.insert(key.clone(), value.to_string());
                        }
                        Value::Number(value) => {
                            metadata.insert(key.clone(), value.to_string());
                        }
                        _ => {}
                    }
                }
            }
            let _ = transport.enqueue(level_from_str("info"), event.to_string(), metadata);
        }
        Ok(())
    }

    pub fn ship_confirmed_projection(&self, projection: &HostTelemetryProjection) -> bool {
        let Some(backend) = self.production_backend.as_ref() else {
            return self.report_projection(projection).is_ok();
        };
        let mut metadata = self
            .identity_tags
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        metadata.extend(projection.metadata.clone());
        self.production_transport.as_ref().is_some_and(|transport| {
            transport.ship_confirmed(
                backend.as_ref(),
                level_from_str(projection.level.unwrap_or("info")),
                projection.event.unwrap_or("sand.host.telemetry"),
                metadata,
            )
        })
    }

    pub fn report_box_log_record(&self, record: &BoxTelemetryRecord) -> io::Result<()> {
        match record {
            BoxTelemetryRecord::Log { source, line } => {
                let text = line.chars().take(2_048).collect::<String>();
                self.report_projection(&HostTelemetryProjection {
                    level: Some("info"),
                    event: Some("sand.box.log"),
                    metadata: BTreeMap::from([
                        ("source".into(), source.clone()),
                        ("text".into(), text),
                    ]),
                })
            }
            BoxTelemetryRecord::Infrastructure { event } => {
                self.report_projection(&box_infrastructure_telemetry(event))
            }
        }
    }

    pub fn report_box_log_ship(&self, report: &BoxLogShipReport) -> io::Result<()> {
        self.report_projection(&box_log_ship_telemetry(report))
    }

    pub fn report_auto_review_approval(
        &self,
        report: &AutoReviewApprovalReport,
    ) -> io::Result<()> {
        self.report_projection(&auto_review_approval_telemetry(report))
    }

    pub fn report_auto_review_display_recheck_failed(
        &self,
        conversation_id: &str,
    ) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some("info"),
            event: Some("sand.auto_review.display_recheck_failed"),
            metadata: BTreeMap::from([
                ("conversation_id".into(), conversation_id.to_string()),
                ("surface".into(), "computer".into()),
            ]),
        })
    }

    pub fn report_auto_review_expire_sweep_failed(
        &self,
        stage: &str,
        error_class: &str,
    ) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some("warn"),
            event: Some("sand.auto_review.expire_sweep_failed"),
            metadata: BTreeMap::from([
                ("stage".into(), stage.to_string()),
                ("error_class".into(), error_class.to_string()),
            ]),
        })
    }

    pub fn report_tool_call_started(
        &self,
        conversation_id: &str,
        request_id: Option<&str>,
        tool_name: &str,
        tool_call_id: &str,
        surface: &str,
    ) -> io::Result<()> {
        let mut metadata = BTreeMap::from([
            ("conversation_id".into(), conversation_id.to_string()),
            ("tool_name".into(), tool_name.to_string()),
            ("tool_call_id".into(), tool_call_id.to_string()),
            ("surface".into(), surface.to_string()),
        ]);
        if let Some(request_id) = request_id.filter(|value| !value.is_empty()) {
            metadata.insert("request_id".into(), request_id.to_string());
        }
        self.report_projection(&HostTelemetryProjection {
            level: Some("info"),
            event: Some("sand.tool_call.started"),
            metadata,
        })
    }

    pub fn report_tool_call_error(
        &self,
        conversation_id: &str,
        request_id: Option<&str>,
        tool_name: &str,
        tool_call_id: &str,
        error_class: &str,
        duration_ms: u64,
        connector: &str,
    ) -> io::Result<()> {
        let mut metadata = BTreeMap::from([
            ("conversation_id".into(), conversation_id.to_string()),
            ("tool_name".into(), tool_name.to_string()),
            ("tool_call_id".into(), tool_call_id.to_string()),
            ("error_class".into(), error_class.to_string()),
            ("duration_ms".into(), duration_ms.min(86_400_000).to_string()),
            ("connector".into(), connector.to_string()),
        ]);
        if let Some(request_id) = request_id.filter(|value| !value.is_empty()) {
            metadata.insert("request_id".into(), request_id.to_string());
        }
        self.report_projection(&HostTelemetryProjection {
            level: Some("error"),
            event: Some("sand.tool_call.error"),
            metadata,
        })
    }

    pub fn report_tool_call_stalled(
        &self,
        conversation_id: &str,
        request_id: Option<&str>,
        tool_name: &str,
        tool_call_id: &str,
        connector: &str,
        elapsed_ms: u64,
    ) -> io::Result<()> {
        let mut metadata = BTreeMap::from([
            ("conversation_id".into(), conversation_id.to_string()),
            ("tool_name".into(), tool_name.to_string()),
            ("tool_call_id".into(), tool_call_id.to_string()),
            ("connector".into(), connector.to_string()),
            ("elapsed_ms".into(), elapsed_ms.min(86_400_000).to_string()),
        ]);
        if let Some(request_id) = request_id.filter(|value| !value.is_empty()) {
            metadata.insert("request_id".into(), request_id.to_string());
        }
        self.report_projection(&HostTelemetryProjection {
            level: Some("warn"),
            event: Some("sand.tool_call.stalled"),
            metadata,
        })
    }

    pub fn report_agent_error(&self, report: &AgentErrorReport) -> io::Result<()> {
        self.report_projection(&agent_error_telemetry(report))?;
        if let Some(detail) = agent_error_detail_telemetry(report) {
            self.report_projection(&detail)?;
        }
        Ok(())
    }

    pub fn report_automation_fire_dropped(
        &self,
        report: &AutomationFireDroppedReport,
    ) -> io::Result<()> {
        self.report_projection(&automation_fire_dropped_telemetry(report))
    }

    pub fn report_local_exec_refused(&self, report: &LocalExecRefusedReport) -> io::Result<()> {
        self.report_projection(&local_exec_refused_telemetry(report))
    }

    pub fn report_local_exec_provider(&self, report: &LocalExecProviderReport) -> io::Result<()> {
        self.report_projection(&local_exec_provider_telemetry(report))
    }

    pub fn report_local_exec_failed(&self, report: &LocalExecFailedReport) -> io::Result<()> {
        self.report_projection(&local_exec_failed_telemetry(report))
    }

    pub fn report_webauthn_proxy(&self, report: &WebAuthnProxyReport) -> io::Result<()> {
        self.report_projection(&webauthn_proxy_telemetry(report))
    }

    pub fn report_memory_synthesis(&self, report: &MemorySynthesisReport) -> io::Result<()> {
        self.report_projection(&memory_synthesis_telemetry(report))
    }

    pub fn report_host_startup(&self, metadata: BTreeMap<String, String>) -> io::Result<()> {
        self.report_projection(&host_startup_telemetry(metadata, HOST_BUILT_AT_MS))
    }

    pub fn report_subagent_revival(&self, report: &SubagentRevivalReport) -> io::Result<()> {
        self.report_projection(&subagent_revival_telemetry(report))
    }

    pub fn report_shell_revival(&self, report: &ShellRevivalReport) -> io::Result<()> {
        self.report_projection(&shell_revival_telemetry(report))
    }

    pub fn report_pending_wake(&self, report: &PendingWakeReport) -> io::Result<()> {
        self.report_projection(&pending_wake_telemetry(report))
    }

    pub fn report_box_disk_pressure(&self, report: &DiskPressureReport) -> io::Result<()> {
        let mut projection = disk_pressure_telemetry(report);
        projection.event = Some("sand.box.disk_pressure");
        self.report_projection(&projection)
    }

    pub fn report_host_diagnostic(&self, report: &HostDiagnostic) -> io::Result<()> {
        self.report_projection(&host_diagnostic_telemetry(report))
    }

    pub fn report_host_crash(&self, kind: &str) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some("error"),
            event: Some("sand.host.crash"),
            metadata: BTreeMap::from([("kind".into(), kind.to_string())]),
        })
    }

    pub fn report_host_event_bus_failure(&self, report: &HostEventBusReport) -> io::Result<()> {
        self.report_projection(&host_event_bus_telemetry(report))
    }

    pub fn report_gateway_command_timing(&self, report: &GatewayCommandReport) -> io::Result<()> {
        self.report_projection(&gateway_command_timing_telemetry(report))
    }

    pub fn report_gateway_command_error(&self, report: &GatewayCommandReport) -> io::Result<()> {
        self.report_projection(&gateway_command_error_telemetry(report))
    }

    pub fn report_session_diagnostic(
        &self,
        report: &SessionTelemetryDiagnostic,
    ) -> io::Result<()> {
        self.report_projection(&session_diagnostic_telemetry(report))
    }

    pub fn report_conversation_gc(&self, report: &ConversationGcReport) -> io::Result<()> {
        self.report_projection(&conversation_gc_telemetry(report))
    }

    pub fn report_send_dispatch(&self, report: &SendDispatchReport) -> io::Result<()> {
        self.report_projection(&send_dispatch_telemetry(report))
    }

    pub fn report_ack_obligation(&self, report: &AckObligationReport) -> io::Result<()> {
        self.report_projection(&ack_obligation_telemetry(report))
    }

    pub fn report_queue_accepted(&self, report: &QueueAcceptedReport) -> io::Result<()> {
        self.report_projection(&queue_accepted_telemetry(report))
    }

    pub fn report_queue_dequeued(&self, report: &QueueDequeuedReport) -> io::Result<()> {
        self.report_projection(&queue_dequeued_telemetry(report))
    }

    pub fn report_queue_watchdog(&self, report: &QueueWatchdogReport) -> io::Result<()> {
        self.report_projection(&queue_watchdog_telemetry(report))
    }

    pub fn report_turn_empty_delivery(&self, report: &TurnEmptyDeliveryReport) -> io::Result<()> {
        self.report_projection(&turn_empty_delivery_telemetry(report))
    }

    pub fn report_turn_interrupt(&self, fields: &TurnInterruptFields) -> io::Result<()> {
        self.report_projection(&turn_interrupt_telemetry(fields))
    }

    pub fn report_turn_await(&self, fields: &TurnAwaitFields) -> io::Result<()> {
        self.report_projection(&turn_await_telemetry(fields))
    }

    pub fn report_turn_retry(&self, fields: &TurnRetryFields) -> io::Result<()> {
        self.report_projection(&turn_retry_telemetry(fields))
    }

    pub fn report_user_message_received(
        &self,
        fields: &UserMessageReceivedFields,
    ) -> io::Result<()> {
        self.report_projection(&user_message_received_telemetry(fields))
    }

    pub fn report_closing_send_nudge(
        &self,
        fields: &ClosingSendNudgeFields,
    ) -> io::Result<()> {
        self.report_projection(&closing_send_nudge_telemetry(fields))
    }

    pub fn report_ttft(&self, fields: &TtftFields) -> io::Result<()> {
        self.report_projection(&ttft_telemetry(fields))
    }

    pub fn report_turn_usage(&self, fields: &TurnUsageFields) -> io::Result<()> {
        self.report_projection(&turn_usage_telemetry(fields))
    }

    pub fn report_computer_use_usage(
        &self,
        fields: &ComputerUseUsageFields,
    ) -> io::Result<()> {
        self.report_projection(&computer_use_usage_telemetry(fields))
    }

    pub fn report_host_upgrade(&self, metadata: BTreeMap<String, String>) -> io::Result<()> {
        let level = if metadata.get("outcome").map(String::as_str) == Some("failed") {
            "warn"
        } else {
            "info"
        };
        self.report_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some("sand.host.upgrade"),
            metadata,
        })
    }

    pub fn report_host_upgrade_confirmed(&self, metadata: BTreeMap<String, String>) -> bool {
        let level = if metadata.get("outcome").map(String::as_str) == Some("failed") {
            "warn"
        } else {
            "info"
        };
        self.ship_confirmed_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some("sand.host.upgrade"),
            metadata,
        })
    }

    pub fn report_desktop_health(
        &self,
        level: &'static str,
        metadata: BTreeMap<String, String>,
    ) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some(DESKTOP_HEALTH_EVENT),
            metadata,
        })
    }

    pub fn report_box_help(&self, report: &Value) -> io::Result<()> {
        let projection = box_help_telemetry(report);
        debug_assert_eq!(projection.event, Some(BOX_HELP_EVENT));
        self.report_projection(&projection)
    }

    pub fn report_box_store_sync_cycle(
        &self,
        level: &str,
        metadata: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        let level = match level {
            "warn" => "warn",
            "error" => "error",
            _ => "info",
        };
        self.report_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some("sand.box_store_sync"),
            metadata: metadata.clone(),
        })
    }

    pub fn report_box_store_db_capture(
        &self,
        level: &str,
        metadata: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        let level = match level {
            "warn" => "warn",
            "error" => "error",
            _ => "info",
        };
        self.report_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some("sand.box_store_db_capture"),
            metadata: metadata.clone(),
        })
    }

    pub fn report_box_store_manifest_conflict(
        &self,
        level: &str,
        metadata: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        let level = match level {
            "warn" => "warn",
            "error" => "error",
            _ => "info",
        };
        self.report_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some("sand.box_store_manifest_conflict"),
            metadata: metadata.clone(),
        })
    }

    pub fn report_chrome_session_stage(
        &self,
        level: &str,
        metadata: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        let level = if level == "warn" { "warn" } else { "info" };
        self.report_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some("sand.chrome_session_stage"),
            metadata: metadata.clone(),
        })
    }

    pub fn report_mcp_discovery_failed(
        &self,
        report: &McpDiscoveryFailedReport,
    ) -> io::Result<()> {
        self.report_projection(&mcp_discovery_failed_telemetry(report))
    }

    pub fn report_connector_auth(&self, report: &ConnectorAuthReport) -> io::Result<()> {
        self.report_projection(&connector_auth_telemetry(report, "host"))
    }

    pub fn report_box_copy_in(
        &self,
        level: &'static str,
        metadata: BTreeMap<String, String>,
    ) -> io::Result<()> {
        self.report_projection(&box_copy_in_telemetry(level, metadata))
    }

    pub fn report_box_recreate_decided(
        &self,
        metadata: BTreeMap<String, String>,
    ) -> io::Result<()> {
        self.report_projection(&box_recreate_decided_telemetry(metadata))
    }

    pub fn report_automation_lifecycle(
        &self,
        report: &AutomationLifecycleEvent,
    ) -> io::Result<()> {
        self.report_projection(&automation_lifecycle_telemetry(report))
    }

    pub fn report_agent_open(&self, report: &AgentOpenReport) -> io::Result<()> {
        self.report_projection(&agent_open_telemetry(report))
    }

    pub fn report_invariant_violation(&self, report: &SandInvariantReport) -> io::Result<()> {
        self.report_projection(&invariant_violation_telemetry(report))
    }

    pub fn report_journal_outcome(&self, report: &JournalOutcomeReport) -> io::Result<()> {
        self.report_projection(&journal_outcome_telemetry(report))
    }

    pub fn report_automation_run(&self, report: &AutomationRunReport) -> io::Result<()> {
        self.report_projection(&automation_run_telemetry(report))
    }

    pub fn report_daemon_ping(&self, report: &DaemonPingReport) -> io::Result<()> {
        self.report_projection(&daemon_ping_telemetry(report))
    }

    pub fn report_box_image_check(&self, report: &BoxImageCheckReport) -> io::Result<()> {
        self.report_projection(&box_image_check_telemetry(report))
    }

    pub fn report_bot_block(&self, report: &BotBlockReport) -> io::Result<()> {
        self.report_projection(&bot_block_telemetry(report))?;
        self.report_projection(&bot_block_detail_telemetry(report))
    }

    pub fn report_mcp_auth_cleanup(&self, outcome: &str, removed_count: usize) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some(if outcome == "error" { "warn" } else { "info" }),
            event: Some("sand.mcp_auth_cleanup"),
            metadata: BTreeMap::from([
                ("outcome".into(), outcome.to_string()),
                ("removed_count".into(), removed_count.to_string()),
            ]),
        })
    }

    pub fn report_local_tool_permission_stranded_retirement(&self) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some("warn"),
            event: Some("sand.client_resource"),
            metadata: BTreeMap::from([
                ("domain".into(), "permissions".into()),
                ("operation".into(), "resolveLocalToolPermission".into()),
                ("state".into(), "failed".into()),
                ("failure_code".into(), "permissions/stranded-ask-retired".into()),
                ("boundary".into(), "host".into()),
                ("retry_owner".into(), "none".into()),
            ]),
        })
    }

    pub fn report_skill_publish_edge_failed(&self, stage: &str, error_class: &str) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some("warn"),
            event: Some("sand.skill_publish.edge_failed"),
            metadata: BTreeMap::from([
                ("stage".into(), stage.to_string()),
                ("error_class".into(), error_class.to_string()),
            ]),
        })
    }

    pub fn report_plugin_skills_sync(
        &self,
        trigger: &str,
        outcome: &str,
        changed: bool,
        skill_count: usize,
        error_class: Option<&str>,
        duration_ms: i64,
    ) -> io::Result<()> {
        let mut metadata = BTreeMap::from([
            ("trigger".into(), trigger.to_string()),
            ("outcome".into(), outcome.to_string()),
            ("changed".into(), changed.to_string()),
            ("skill_count".into(), skill_count.to_string()),
            ("duration_ms".into(), duration_ms.to_string()),
        ]);
        if let Some(error_class) = error_class {
            metadata.insert("error_class".into(), error_class.to_string());
        }
        self.report_projection(&HostTelemetryProjection {
            level: Some(if outcome == "failed" { "warn" } else { "info" }),
            event: Some("sand.plugin_skills.sync"),
            metadata,
        })
    }

    pub fn report_teach_recording_cap_stop_failed(&self, report: &Value) -> io::Result<()> {
        let mut metadata = BTreeMap::new();
        if let Some(error_class) = report.get("errorClass").and_then(Value::as_str) {
            metadata.insert("error_class".into(), error_class.to_string());
        }
        self.report_projection(&HostTelemetryProjection {
            level: Some("warn"),
            event: Some("sand.teach.cap_stop_failed"),
            metadata,
        })
    }

    pub fn report_teach_recording_start_failed(&self, report: &Value) -> io::Result<()> {
        let mut metadata = BTreeMap::new();
        if let Some(kind) = report.get("kind").and_then(Value::as_str) {
            metadata.insert("kind".into(), kind.to_string());
        }
        if let Some(error_class) = report.get("errorClass").and_then(Value::as_str) {
            metadata.insert("error_class".into(), error_class.to_string());
        }
        if let Some(window_index) = report.get("windowIndex").and_then(Value::as_u64) {
            metadata.insert("window_index".into(), window_index.to_string());
        }
        if let Some(entry_point) = report.get("entryPoint").and_then(Value::as_str) {
            metadata.insert("entry_point".into(), entry_point.to_string());
        }
        self.report_projection(&HostTelemetryProjection {
            level: Some("warn"),
            event: Some("sand.teach.recording_start_failed"),
            metadata,
        })
    }

    pub fn report_search_index_health(&self, report: &Value) -> io::Result<()> {
        self.sink.emit(&PersistedHostTelemetryRecord {
            channel: "structured_log".into(),
            event: "sand.search_index_health".into(),
            payload: report.clone(),
        })
    }

    pub fn report_host_extension_diagnostic(&self, diagnostic: &Value) -> io::Result<()> {
        self.sink.emit(&PersistedHostTelemetryRecord {
            channel: "structured_log".into(),
            event: "sand.host_extension_diagnostic".into(),
            payload: diagnostic.clone(),
        })
    }

    pub fn report_child_process_spawn_sample(
        &self,
        sample: &SpawnCountRecord,
    ) -> io::Result<()> {
        self.report_projection(&HostTelemetryProjection {
            level: Some("info"),
            event: Some("diagnostic.child_process.spawn"),
            metadata: BTreeMap::from([
                ("family".into(), sample.family.clone()),
                ("operation".into(), sample.operation.clone()),
                ("count".into(), sample.count.to_string()),
            ]),
        })
    }

    pub fn report_experiments_diagnostic(
        &self,
        diagnostic: &ExperimentsDiagnostic,
    ) -> io::Result<()> {
        self.report_projection(&experiments_diagnostic_telemetry(diagnostic))
    }

    pub fn report_automation_shadow_prune(&self, report: &Value) -> io::Result<()> {
        self.sink.emit(&PersistedHostTelemetryRecord {
            channel: "structured_log".into(),
            event: "sand.automation_shadow_prune".into(),
            payload: report.clone(),
        })
    }

    pub fn report_box_boot_stage_confirmed(&self, stage: &str, duration_ms: u64) -> bool {
        let mut metadata = BTreeMap::new();
        metadata.insert("stage".into(), stage.to_string());
        metadata.insert("duration_ms".into(), duration_ms.to_string());
        self.ship_confirmed_projection(&HostTelemetryProjection {
            level: Some("info"),
            event: Some("sand.box.boot_stage_confirmed"),
            metadata,
        })
    }
}

pub const REPORT_CONNECTOR_AUTH_GATEWAY_METHOD: &str = "reportConnectorAuth";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorAuthGatewayError {
    BadRequest(String),
    Internal(String),
}

fn connector_auth_optional_string(
    args: &Value,
    key: &str,
) -> Result<Option<String>, ConnectorAuthGatewayError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ConnectorAuthGatewayError::BadRequest(format!(
            "reportConnectorAuth requires string {key} when present"
        ))),
    }
}

fn connector_auth_error_from_json(value: &Value) -> Result<SandErrorValue, ConnectorAuthGatewayError> {
    let object = value.as_object().ok_or_else(|| {
        ConnectorAuthGatewayError::BadRequest(
            "reportConnectorAuth requires object error when present".into(),
        )
    })?;
    let code = object
        .get("code")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ConnectorAuthGatewayError::BadRequest(
                "reportConnectorAuth error requires non-empty code".into(),
            )
        })?;
    let mut error = SandErrorValue::new(code);
    for (key, value) in object {
        if key == "code" {
            continue;
        }
        let payload = match value {
            Value::String(value) => Some(SandErrorPayloadValue::String(value.clone())),
            Value::Number(value) => value
                .as_f64()
                .filter(|value| value.is_finite())
                .map(SandErrorPayloadValue::Number),
            Value::Bool(value) => Some(SandErrorPayloadValue::Bool(*value)),
            _ => None,
        };
        if let Some(payload) = payload {
            error.payload.insert(key.clone(), payload);
        }
    }
    Ok(error)
}

pub fn dispatch_connector_auth_gateway(
    logs: &HostStructuredLogTelemetry,
    method: &str,
    args: &Value,
) -> Option<Result<Value, ConnectorAuthGatewayError>> {
    if method != REPORT_CONNECTOR_AUTH_GATEWAY_METHOD {
        return None;
    }
    let phase = match args.get("phase").and_then(Value::as_str) {
        Some(value) => value.to_string(),
        None => {
            return Some(Err(ConnectorAuthGatewayError::BadRequest(
                "reportConnectorAuth requires string phase".into(),
            )));
        }
    };
    let outcome = match args.get("outcome").and_then(Value::as_str) {
        Some(value) => value.to_string(),
        None => {
            return Some(Err(ConnectorAuthGatewayError::BadRequest(
                "reportConnectorAuth requires string outcome".into(),
            )));
        }
    };
    let server_name = match connector_auth_optional_string(args, "serverName") {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };
    let server_id = match connector_auth_optional_string(args, "serverId") {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };
    let reauth = match args.get("reauth") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => {
            return Some(Err(ConnectorAuthGatewayError::BadRequest(
                "reportConnectorAuth requires boolean reauth when present".into(),
            )));
        }
    };
    let error = match args.get("error") {
        None | Some(Value::Null) => None,
        Some(value) => match connector_auth_error_from_json(value) {
            Ok(value) => Some(value),
            Err(error) => return Some(Err(error)),
        },
    };
    Some(
        logs.report_connector_auth(&ConnectorAuthReport {
            phase,
            outcome,
            server_name,
            server_id,
            reauth,
            error,
        })
        .map(|_| json!({ "reported": true }))
        .map_err(|error| ConnectorAuthGatewayError::Internal(error.to_string())),
    )
}

pub const REPORT_MCP_DISCOVERY_FAILED_GATEWAY_METHOD: &str = "reportMcpDiscoveryFailed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpDiscoveryFailedGatewayError {
    BadRequest(String),
    Internal(String),
}

pub fn dispatch_mcp_discovery_failed_gateway(
    logs: &HostStructuredLogTelemetry,
    method: &str,
    args: &Value,
) -> Option<Result<Value, McpDiscoveryFailedGatewayError>> {
    if method != REPORT_MCP_DISCOVERY_FAILED_GATEWAY_METHOD {
        return None;
    }
    let error_class = match args
        .get("errorClass")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value.to_string(),
        None => {
            return Some(Err(McpDiscoveryFailedGatewayError::BadRequest(
                "reportMcpDiscoveryFailed requires non-empty errorClass".into(),
            )));
        }
    };
    let elapsed_ms = match args.get("elapsedMs").and_then(Value::as_f64) {
        Some(value) if value.is_finite() && value >= 0.0 => value,
        _ => {
            return Some(Err(McpDiscoveryFailedGatewayError::BadRequest(
                "reportMcpDiscoveryFailed requires finite non-negative elapsedMs".into(),
            )));
        }
    };
    let served_stale = match args.get("servedStale").and_then(Value::as_bool) {
        Some(value) => value,
        None => {
            return Some(Err(McpDiscoveryFailedGatewayError::BadRequest(
                "reportMcpDiscoveryFailed requires boolean servedStale".into(),
            )));
        }
    };
    Some(
        logs.report_mcp_discovery_failed(&McpDiscoveryFailedReport {
            error_class,
            elapsed_ms,
            served_stale,
        })
        .map(|_| json!({ "reported": true }))
        .map_err(|error| McpDiscoveryFailedGatewayError::Internal(error.to_string())),
    )
}

#[derive(Clone)]
pub struct HostProductAnalytics {
    sink: Arc<JsonlHostTelemetrySink>,
    runtime: Arc<Mutex<Option<Arc<ProductionAnalyticsRuntime>>>>,
}

impl HostProductAnalytics {
    pub fn attach_runtime(&self, runtime: Arc<ProductionAnalyticsRuntime>) {
        *self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(runtime);
    }

    pub fn detach_runtime(&self) {
        self.runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
    }

    pub fn mark_active(&self, reason: &str) {
        if let Some(runtime) = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            runtime.mark_active(reason);
        }
    }

    pub fn can_record_events(&self) -> bool {
        self.runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|runtime| runtime.can_record_events())
    }

    pub fn track_event(&self, name: &str, properties: &Value) -> io::Result<()> {
        let event = product_analytics_event(name, properties);
        self.sink.emit(&PersistedHostTelemetryRecord {
            channel: "product_analytics".into(),
            event: event.name.clone(),
            payload: serde_json::to_value(&event.properties).map_err(io::Error::other)?,
        })?;
        if let Some(runtime) = self
            .runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            runtime.track(event);
        }
        Ok(())
    }
}

impl AnalyticsClient for HostProductAnalytics {
    fn track_event(&self, name: &str, properties: &Value) {
        let _ = HostProductAnalytics::track_event(self, name, properties);
    }
}

impl TelemetryService for HostStructuredLogTelemetry {
    fn report(&self, method: &'static str, report: &Value) {
        let event = match method {
            "startTurn" => "sand.turn.start",
            "reportToolCallError" => "sand.tool_call.error",
            "reportToolCallStalled" => "sand.tool_call.stalled",
            "reportToolCallStarted" => "sand.tool_call.started",
            "reportAgentError" => "sand.agent.error",
            "reportBotBlock" => "sand.bot.block",
            "reportDaemonPing" => "sand.daemon.ping",
            "reportBoxBootStage" => "sand.box.boot_stage",
            "reportExecDaemonRestart" => "sand.exec_daemon.restart",
            "reportSupervisorRestart" => "sand.supervisor.restart",
            "reportTurnInterrupt" => "sand.turn.interrupt",
            "reportTurnAwait" => "sand.turn.await",
            "reportTurnRetry" => "sand.turn.retry",
            "reportUserMessageReceived" => "sand.user_message.received",
            "reportClosingSendNudge" => "sand.closing_send_nudge",
            "reportSubagentRevival" => "sand.subagent.revival",
            "reportShellRevival" => "sand.shell.revival",
            "reportComputerUseUsage" => "sand.computer_use.usage",
            "reportTtft" => "sand.turn.ttft",
            "reportSendDispatch" => "sand.send.dispatch",
            "reportQueueAccepted" => "sand.queue.accepted",
            "reportQueueDequeued" => "sand.queue.dequeued",
            "reportQueueWatchdog" => "sand.queue.watchdog",
            "reportAckObligation" => "sand.ack_obligation",
            "reportPendingWake" => "sand.pending_wake",
            "reportTurnUsage" => "sand.turn.usage",
            "reportTurnEmptyDelivery" => "sand.turn.empty_delivery",
            "reportJournalOutcome" => "sand.journal.outcome",
            "reportAutoReviewExpireSweepFailed" => "sand.auto_review.expire_sweep_failed",
            "reportAutomationLifecycle" => "sand.automation.lifecycle",
            "reportAutomationFireDropped" => "sand.automation.fire_dropped",
            "reportAutomationRun" => "sand.automation.run",
            _ => "sand.host.telemetry",
        };
        let _ = self.report_raw(event, report);
    }
}

pub const TELEMETRY_FLUSH_TICK: Duration = Duration::from_secs(3);
pub const FATAL_TELEMETRY_FLUSH_TIMEOUT: Duration = Duration::from_millis(2_000);
pub const HOST_CRASH_MARKER_FORWARD_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub const SAND_SUPERVISOR_DESKTOP_HEALTH_PATH: &str = "/tmp/sand-supervisor/desktop-health.json";
pub const DESKTOP_HEALTH_FORWARD_INTERVAL: Duration = Duration::from_secs(30);
pub const DESKTOP_HEALTH_HEARTBEAT_MS: u64 = 5 * 60 * 1_000;
pub const DESKTOP_HEALTH_EVENT: &str = "sand.box.desktop_health";
pub const HOST_CONSOLE_FORWARD_INTERVAL: Duration = Duration::from_secs(1);

struct HostConsoleForwarder {
    stop: Option<mpsc::Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl HostConsoleForwarder {
    fn start(logs: HostStructuredLogTelemetry, path: PathBuf) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("sand-host-console-forwarder".into())
            .spawn(move || {
                let mut offset = fs::metadata(&path)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0);
                loop {
                    match stop_rx.recv_timeout(HOST_CONSOLE_FORWARD_INTERVAL) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let Ok(mut file) = File::open(&path) else {
                                continue;
                            };
                            let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                            if size < offset {
                                offset = 0;
                            }
                            if file.seek(SeekFrom::Start(offset)).is_err() {
                                continue;
                            }
                            let mut buffer = String::new();
                            if file.read_to_string(&mut buffer).is_err() {
                                continue;
                            }
                            offset = file.stream_position().unwrap_or(size);
                            for line in buffer
                                .lines()
                                .map(str::trim)
                                .filter(|line| !line.is_empty())
                            {
                                let lower = line.to_ascii_lowercase();
                                let level = if lower.contains("error")
                                    || lower.contains("failed")
                                    || lower.contains("panic")
                                {
                                    "error"
                                } else if lower.contains("warn") {
                                    "warn"
                                } else {
                                    "info"
                                };
                                let _ = logs.report_host_log(level, line);
                            }
                        }
                    }
                }
            })
            .ok();
        Self {
            stop: Some(stop_tx),
            worker,
        }
    }

    fn dispose(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for HostConsoleForwarder {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostBundleIdentity {
    pub host_bundle_version: Option<String>,
    pub box_store_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageSentReport {
    pub agent_id: String,
    pub prompt: Option<String>,
    pub attachment_paths: Vec<String>,
    pub rich_text: Option<String>,
    pub is_fork: bool,
    pub source: Option<String>,
    pub is_group_room: bool,
}

impl HostStructuredLogTelemetry {
    pub fn set_host_bundle_identity(&self, identity: &HostBundleIdentity) {
        let mut tags = self
            .identity_tags
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(version) = identity
            .host_bundle_version
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            tags.insert("host_bundle_version".into(), version.to_string());
        }
        tags.insert("host_built_at_ms".into(), HOST_BUILT_AT_MS.into());
        if let Some(box_store_id) = identity
            .box_store_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            tags.insert("box_store_id".into(), box_store_id.to_string());
        }
        let snapshot = tags.clone();
        drop(tags);
        if let Some(transport) = self.production_transport.as_ref() {
            let _ = transport.set_identity_tags(snapshot);
        }
    }

    pub fn report_inference_credential_renewal(
        &self,
        event: &HostAuthRenewalEvent,
    ) -> io::Result<()> {
        let mut metadata = BTreeMap::from([
            (
                "outcome".into(),
                match event.result.outcome {
                    RenewalOutcome::Renewed => "renewed",
                    RenewalOutcome::Failed => "failed",
                }
                .to_string(),
            ),
            (
                "consecutive_failures".into(),
                event.result.consecutive_failures.to_string(),
            ),
            ("duration_ms".into(), event.result.duration_ms.to_string()),
        ]);
        if let Some(error_summary) = event.result.error_summary.as_deref() {
            metadata.insert("error_summary".into(), error_summary.to_string());
        }
        self.report_projection(&HostTelemetryProjection {
            level: Some(if event.result.outcome == RenewalOutcome::Failed {
                "error"
            } else {
                "info"
            }),
            event: Some("sand.inference.credential_renewal"),
            metadata,
        })
    }

    pub fn report_host_log(&self, level: &str, message: &str) -> io::Result<()> {
        let level = match level {
            "error" => "error",
            "warn" => "warn",
            _ => "info",
        };
        self.report_projection(&HostTelemetryProjection {
            level: Some(level),
            event: Some("sand.host.log"),
            metadata: BTreeMap::from([("message".into(), message.to_string())]),
        })
    }

    pub fn report_host_lifecycle(&self, report: HostLifecycleReport) -> io::Result<()> {
        self.report_projection(&host_lifecycle_telemetry(&report))
    }
}

impl ModelExperimentExposureExperiments for HostExperimentsExtension {
    fn has_hydrated_statsig_user_id(&self) -> bool {
        HostExperimentsExtension::has_hydrated_statsig_user_id(self)
    }

    fn wait_for_hydrated_statsig_user_id(&self, timeout_ms: u64) -> bool {
        HostExperimentsExtension::wait_for_hydrated_statsig_user_id(
            self,
            Duration::from_millis(timeout_ms),
        )
    }

    fn get_sand_model_experiment_state(&self) -> Option<ExposureModelExperimentState> {
        HostExperimentsExtension::get_sand_model_experiment_state(self).map(|state| {
            ExposureModelExperimentState {
                active: state.active,
                arm: match state.arm {
                    SandModelExperimentArm::Control => "control",
                    SandModelExperimentArm::Treatment => "treatment",
                }
                .into(),
            }
        })
    }

    fn log_sand_model_experiment_exposure(&self) -> bool {
        HostExperimentsExtension::log_sand_model_experiment_exposure(self)
    }
}

impl ModelExperimentExposureAnalytics for HostProductAnalytics {
    fn can_record_events(&self) -> bool {
        HostProductAnalytics::can_record_events(self)
    }

    fn track_event(&self, name: &str, arm: &str) {
        let _ = HostProductAnalytics::track_event(self, name, &json!({ "arm": arm }));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DesktopHealthForwardState {
    last_forwarded_revision: Option<u64>,
    last_forwarded_at_ms: Option<u64>,
}

pub fn forward_desktop_health_file_to_logs(
    path: &Path,
    logs: &HostStructuredLogTelemetry,
    state: &Mutex<DesktopHealthForwardState>,
    now_ms: u64,
) -> DesktopHealthForwardResult {
    let raw = fs::read_to_string(path).ok();
    let (last_revision, last_at_ms) = state
        .lock()
        .map(|state| (state.last_forwarded_revision, state.last_forwarded_at_ms))
        .unwrap_or((None, None));
    let mut next_state = None;
    let result = forward_desktop_health_with(
        raw.as_deref(),
        last_revision,
        last_at_ms,
        now_ms,
        DESKTOP_HEALTH_HEARTBEAT_MS,
        |level, metadata| {
            let _ = logs.report_desktop_health(level, metadata.clone());
        },
        |revision, at_ms| {
            next_state = Some((revision, at_ms));
        },
    );
    if let Some((revision, at_ms)) = next_state {
        if let Ok(mut state) = state.lock() {
            state.last_forwarded_revision = Some(revision);
            state.last_forwarded_at_ms = Some(at_ms);
        }
    }
    result
}

fn wall_clock_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn terminal_crash_marker_result(result: ForwardHostCrashMarkerResult) -> bool {
    matches!(
        result,
        ForwardHostCrashMarkerResult::Absent
            | ForwardHostCrashMarkerResult::Delivered
            | ForwardHostCrashMarkerResult::ParseError
    )
}

fn forward_host_crash_marker_to_logs(
    store: &impl HostCrashMarkerStore,
    logs: &HostStructuredLogTelemetry,
    last_handled: &Mutex<Option<String>>,
) -> ForwardHostCrashMarkerResult {
    forward_host_crash_marker_with(
        store,
        |raw| {
            last_handled
                .lock()
                .ok()
                .and_then(|value| value.clone())
                .as_deref()
                == Some(raw)
        },
        |raw| {
            if let Ok(mut value) = last_handled.lock() {
                *value = Some(raw.to_string());
            }
        },
        |marker| {
            let mut metadata = host_crash_marker_metadata(marker);
            metadata.insert("error_code".into(), "SAND-E0001".into());
            metadata.insert("error_domain".into(), "registry".into());
            metadata.insert("error_retryable".into(), "false".into());
            logs.ship_confirmed_projection(&HostTelemetryProjection {
                level: Some("error"),
                event: Some("sand.host.crash"),
                metadata,
            })
        },
    )
}

struct HostCrashMarkerForwarder {
    stop: mpsc::Sender<()>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl HostCrashMarkerForwarder {
    fn start(logs: HostStructuredLogTelemetry) -> Self {
        Self::start_with_store(
            logs,
            FileHostCrashMarkerStore::new(get_host_crash_marker_path()),
        )
    }

    fn start_with_store(logs: HostStructuredLogTelemetry, store: FileHostCrashMarkerStore) -> Self {
        let last_handled = Arc::new(Mutex::new(None));
        let initial = forward_host_crash_marker_to_logs(&store, &logs, &last_handled);
        let (stop_tx, stop_rx) = mpsc::channel();

        let worker = if terminal_crash_marker_result(initial) {
            None
        } else {
            Some(thread::spawn(move || {
                loop {
                    match stop_rx.recv_timeout(HOST_CRASH_MARKER_FORWARD_INTERVAL) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let result =
                                forward_host_crash_marker_to_logs(&store, &logs, &last_handled);
                            if terminal_crash_marker_result(result) {
                                break;
                            }
                        }
                    }
                }
            }))
        };

        Self {
            stop: stop_tx,
            worker: Mutex::new(worker),
        }
    }
}

impl Drop for HostCrashMarkerForwarder {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
    }
}

struct DesktopHealthForwarder {
    stop: mpsc::Sender<()>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl DesktopHealthForwarder {
    fn start(logs: HostStructuredLogTelemetry) -> Self {
        let path = PathBuf::from(SAND_SUPERVISOR_DESKTOP_HEALTH_PATH);
        let state = Arc::new(Mutex::new(DesktopHealthForwardState::default()));
        let _ = forward_desktop_health_file_to_logs(&path, &logs, &state, wall_clock_now_ms());

        let (stop_tx, stop_rx) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker = thread::spawn(move || {
            loop {
                match stop_rx.recv_timeout(DESKTOP_HEALTH_FORWARD_INTERVAL) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let _ = forward_desktop_health_file_to_logs(
                            &path,
                            &logs,
                            &worker_state,
                            wall_clock_now_ms(),
                        );
                    }
                }
            }
        });

        Self {
            stop: stop_tx,
            worker: Mutex::new(Some(worker)),
        }
    }
}

impl Drop for DesktopHealthForwarder {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
    }
}

pub struct StructuredLogFlushPolling {
    stop: mpsc::Sender<()>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl StructuredLogFlushPolling {
    pub fn start(
        logs: HostStructuredLogTelemetry,
        on_tick: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self::start_with_interval(logs, on_tick, TELEMETRY_FLUSH_TICK)
    }

    pub fn start_with_interval(
        logs: HostStructuredLogTelemetry,
        on_tick: Option<Arc<dyn Fn() + Send + Sync>>,
        interval: Duration,
    ) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("sand-host-structured-log-flush".into())
            .spawn(move || {
                loop {
                    match stop_rx.recv_timeout(interval) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let _ = logs.flush();
                            if let Some(on_tick) = on_tick.as_ref() {
                                on_tick();
                            }
                        }
                    }
                }
            })
            .ok();
        Self {
            stop: stop_tx,
            worker: Mutex::new(worker),
        }
    }
}

impl Drop for StructuredLogFlushPolling {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(handle) = worker.take() {
                let _ = handle.join();
            }
        }
    }
}

pub fn tick_pressure_cpu_profiler(
    profiler: Option<&Arc<Mutex<PressureCpuProfiler>>>,
    now_ms: i64,
) -> Result<Option<PathBuf>, SandProfilerCaptureError> {
    let Some(profiler) = profiler else {
        return Ok(None);
    };
    let Ok(mut profiler) = profiler.lock() else {
        return Ok(None);
    };
    profiler.on_tick(now_ms)
}

pub fn route_pressure_cpu_profiler_event(
    experiments: &HostExperimentsExtension,
    profiler: Option<&Arc<Mutex<PressureCpuProfiler>>>,
    report: EventLoopWindowReport,
    now_ms: i64,
) {
    if report.trigger != EventLoopTrigger::Pressure
        || !experiments.check_feature_gate("sand_enable_pressure_cpu_profiler")
    {
        return;
    }
    if let Some(profiler) = profiler {
        if let Ok(mut profiler) = profiler.lock() {
            profiler.on_pressure(now_ms);
        }
    }
}

#[derive(Clone, Default)]
pub struct HostTelemetryRuntimeHooks {
    pub tracing: Option<Arc<HostTracing>>,
    pub pressure_profiler: Option<Arc<Mutex<PressureCpuProfiler>>>,
}

#[derive(Clone)]
struct ProductionHostTelemetryDependencies {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    experiments: Arc<HostExperimentsExtension>,
    inference: Arc<ProductionInferenceExtension>,
}

#[derive(Default)]
struct HostTelemetryRuntimeState {
    auth_renewal_subscription: Option<u64>,
    console_forwarder: Option<HostConsoleForwarder>,
    inference_stop: Option<StopSubscription>,
    flush_polling: Option<StructuredLogFlushPolling>,
    crash_marker_forwarder: Option<HostCrashMarkerForwarder>,
    desktop_health_forwarder: Option<DesktopHealthForwarder>,
    event_loop_telemetry: Option<EventLoopTelemetryRuntime>,
    box_log_shipper: Option<Arc<BoxLogShipper>>,
    model_experiment_exposure: Option<Arc<ModelExperimentExposureLatch>>,
    host_bundle_identity_set: bool,
}

pub struct HostTelemetryService {
    pub logs: HostStructuredLogTelemetry,
    pub analytics: HostProductAnalytics,
    pub brain: Arc<AutomationRunAnalyticsTelemetry>,
    sink: Arc<JsonlHostTelemetrySink>,
    started: AtomicBool,
    disposed: AtomicBool,
    production: Option<ProductionHostTelemetryDependencies>,
    hooks: HostTelemetryRuntimeHooks,
    runtime: Mutex<HostTelemetryRuntimeState>,
    clock_origin: Instant,
}

impl HostTelemetryService {
    pub fn open(records_path: impl Into<PathBuf>) -> io::Result<Self> {
        Self::open_with_identity_tags(records_path, resolve_sand_box_identity_tags())
    }

    pub fn open_with_identity_tags(
        records_path: impl Into<PathBuf>,
        mut identity_tags: BTreeMap<String, String>,
    ) -> io::Result<Self> {
        Self::open_internal(
            records_path,
            &mut identity_tags,
            None,
            HostTelemetryRuntimeHooks::default(),
        )
    }

    pub fn open_production(
        records_path: impl Into<PathBuf>,
        mut identity_tags: BTreeMap<String, String>,
        backend_url: String,
        auth: Arc<HostAuthExtension>,
        experiments: Arc<HostExperimentsExtension>,
        inference: Arc<ProductionInferenceExtension>,
        mut hooks: HostTelemetryRuntimeHooks,
    ) -> io::Result<Self> {
        if hooks.tracing.is_none() {
            let auth_for_tracing = Arc::clone(&auth);
            hooks.tracing = init_production_host_tracing(
                backend_url.clone(),
                Arc::new(move || auth_for_tracing.service().peek_access_token()),
                Some(env!("CARGO_PKG_VERSION")),
            );
        }
        if hooks.pressure_profiler.is_none() {
            hooks.pressure_profiler = Some(create_production_pressure_cpu_profiler(Arc::clone(
                &experiments,
            )));
        }
        Self::open_internal(
            records_path,
            &mut identity_tags,
            Some(ProductionHostTelemetryDependencies {
                backend_url,
                auth,
                experiments,
                inference,
            }),
            hooks,
        )
    }

    fn open_internal(
        records_path: impl Into<PathBuf>,
        identity_tags: &mut BTreeMap<String, String>,
        production: Option<ProductionHostTelemetryDependencies>,
        hooks: HostTelemetryRuntimeHooks,
    ) -> io::Result<Self> {
        identity_tags.retain(|_, value| !value.is_empty());
        let sink = Arc::new(JsonlHostTelemetrySink::open(records_path)?);
        let initial_identity_tags = identity_tags.clone();
        let identity_tags = Arc::new(Mutex::new(initial_identity_tags.clone()));
        let (production_backend, production_transport) =
            if std::env::var("SAND_DISABLE_TELEMETRY").as_deref() == Ok("1") {
                (None, None)
            } else if let Some(production) = production.as_ref() {
                let mut platform_tags = initial_identity_tags.clone();
                platform_tags.insert("client".into(), SAND_CLIENT_TYPE.into());
                platform_tags.insert("client.type".into(), SAND_CLIENT_TYPE.into());
                platform_tags.insert("client_version".into(), sand_client_version());
                platform_tags.insert("app_version".into(), env!("CARGO_PKG_VERSION").into());
                platform_tags.insert("arch".into(), std::env::consts::ARCH.into());
                platform_tags.insert(
                    "platform".into(),
                    match std::env::consts::OS {
                        "macos" => "darwin",
                        other => other,
                    }
                    .into(),
                );
                let backend: Arc<dyn StructuredLogBackend> = Arc::new(
                    CursorStructuredLogBackend::new(
                        production.backend_url.clone(),
                        Arc::clone(&production.auth),
                        platform_tags,
                    )
                    .map_err(io::Error::other)?,
                );
                let pressure_tick = hooks.pressure_profiler.as_ref().map(|profiler| {
                    let profiler = Arc::clone(profiler);
                    Arc::new(move || {
                        let _ = tick_pressure_cpu_profiler(
                            Some(&profiler),
                            wall_clock_now_ms().min(i64::MAX as u64) as i64,
                        );
                    }) as Arc<dyn Fn() + Send + Sync>
                });
                let transport = ProductionStructuredLogTransport::start_with_tick(
                    Arc::clone(&backend),
                    true,
                    pressure_tick,
                );
                (Some(backend), Some(transport))
            } else {
                (None, None)
            };
        let logs = HostStructuredLogTelemetry {
            sink: Arc::clone(&sink),
            identity_tags,
            production_transport,
            production_backend,
            active_turns: Arc::new(Mutex::new(BTreeMap::new())),
        };
        let analytics = HostProductAnalytics {
            sink: Arc::clone(&sink),
            runtime: Arc::new(Mutex::new(None)),
        };
        let brain = Arc::new(AutomationRunAnalyticsTelemetry::new(
            Arc::new(logs.clone()),
            Arc::new(analytics.clone()),
        ));
        Ok(Self {
            logs,
            analytics,
            brain,
            sink,
            started: AtomicBool::new(false),
            disposed: AtomicBool::new(false),
            production,
            hooks,
            runtime: Mutex::new(HostTelemetryRuntimeState::default()),
            clock_origin: Instant::now(),
        })
    }

    pub fn start(&self) -> io::Result<()> {
        if self.disposed.load(Ordering::Acquire) {
            return Err(io::Error::other(
                "Host telemetry service is already disposed",
            ));
        }
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if let Err(error) = self.sink.flush() {
            self.started.store(false, Ordering::Release);
            return Err(error);
        }

        let Some(production) = self.production.as_ref().cloned() else {
            return Ok(());
        };
        let telemetry_enabled = std::env::var("SAND_DISABLE_TELEMETRY").as_deref() != Ok("1");
        let console_path = std::env::var("SAND_HOST_LOG_FILE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(SUPERVISOR_LOG_PATH));
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| io::Error::other("Host telemetry runtime mutex poisoned"))?;

        let renewal_logs = self.logs.clone();
        let renewal_id = production
            .auth
            .service()
            .subscribe_to_renewal(Arc::new(move |event| {
                let _ = renewal_logs.report_inference_credential_renewal(&event);
            }));
        if let Some(missed) = production.auth.service().get_last_renewal_event() {
            let _ = self.logs.report_inference_credential_renewal(&missed);
        }
        runtime.auth_renewal_subscription = Some(renewal_id);
        runtime.console_forwarder =
            Some(HostConsoleForwarder::start(self.logs.clone(), console_path));

        let analytics_runtime = ProductionAnalyticsRuntime::start(
            production.backend_url.clone(),
            Arc::clone(&production.auth),
            Arc::clone(&production.experiments),
        );
        self.analytics.attach_runtime(analytics_runtime);
        self.analytics.mark_active("host_startup");

        let exposure_experiments: Arc<dyn ModelExperimentExposureExperiments> =
            production.experiments.clone();
        let exposure_analytics: Arc<dyn ModelExperimentExposureAnalytics> =
            Arc::new(self.analytics.clone());
        let exposure = Arc::new(ModelExperimentExposureLatch::new(
            exposure_experiments,
            exposure_analytics,
            std::env::vars().collect(),
        ));
        let inference_exposure = Arc::clone(&exposure);
        runtime.inference_stop = Some(
            production
                .inference
                .on_model_experiment_applied(Arc::new(move || inference_exposure.note())),
        );
        runtime.model_experiment_exposure = Some(exposure);

        let diagnostic_logs = self.logs.clone();
        pin_experiments_diagnostics_reporter(Some(Arc::new(move |diagnostic| {
            let _ = diagnostic_logs.report_experiments_diagnostic(&diagnostic);
        })));

        if telemetry_enabled {
            runtime.desktop_health_forwarder =
                Some(DesktopHealthForwarder::start(self.logs.clone()));

            let event_logs = self.logs.clone();
            let spawn_diagnostic_logs = self.logs.clone();
            let pressure = self.hooks.pressure_profiler.clone();
            let experiments = Arc::clone(&production.experiments);
            runtime.event_loop_telemetry =
                Some(EventLoopTelemetryRuntime::start(Arc::new(move |report| {
                    let _ = event_logs.report_projection(&event_loop_window_telemetry(report));
                    for sample in crate::process_spawn_diagnostics::global_child_process_spawn_tracker()
                        .sample_counts(false)
                    {
                        let _ = spawn_diagnostic_logs.report_child_process_spawn_sample(&sample);
                    }
                    route_pressure_cpu_profiler_event(
                        &experiments,
                        pressure.as_ref(),
                        report,
                        wall_clock_now_ms().min(i64::MAX as u64) as i64,
                    );
                })));
        }

        if telemetry_enabled
            && is_box_log_shipping_enabled(
                std::env::var("SAND_HOST_IN_BOX").ok().as_deref(),
                std::env::var("SAND_BOX_LOG_SHIP_DISABLED").ok().as_deref(),
            )
        {
            let logs_for_batch = self.logs.clone();
            let logs_for_ship = self.logs.clone();
            let mut config = BoxLogShipperConfig::default();
            let host_log_file = std::env::var("SAND_HOST_LOG_FILE")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());
            config.skip_paths = vec![PathBuf::from(
                host_log_file.as_deref().unwrap_or(SUPERVISOR_LOG_PATH),
            )];
            runtime.box_log_shipper = Some(Arc::new(BoxLogShipper::new(
                config,
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
            )));
        }

        Ok(())
    }

    pub fn api(self: &Arc<Self>) -> HostTelemetryApi {
        HostTelemetryApi {
            service: Arc::clone(self),
        }
    }

    pub fn start_host_crash_marker_forwarding(
        &self,
        store: FileHostCrashMarkerStore,
    ) -> io::Result<bool> {
        if self.disposed.load(Ordering::Acquire) {
            return Err(io::Error::other(
                "Host telemetry service is already disposed",
            ));
        }
        if !self.started.load(Ordering::Acquire) {
            return Err(io::Error::other("Host telemetry service is not started"));
        }
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| io::Error::other("Host telemetry runtime mutex poisoned"))?;
        if runtime.crash_marker_forwarder.is_some() {
            return Ok(false);
        }
        runtime.crash_marker_forwarder = Some(HostCrashMarkerForwarder::start_with_store(
            self.logs.clone(),
            store,
        ));
        Ok(true)
    }

    pub fn stop_host_crash_marker_forwarding(&self) -> io::Result<bool> {
        let forwarder = {
            let mut runtime = self
                .runtime
                .lock()
                .map_err(|_| io::Error::other("Host telemetry runtime mutex poisoned"))?;
            runtime.crash_marker_forwarder.take()
        };
        let stopped = forwarder.is_some();
        drop(forwarder);
        Ok(stopped)
    }

    pub fn set_host_bundle_identity(&self, identity: HostBundleIdentity) -> io::Result<()> {
        if self.disposed.load(Ordering::Acquire) {
            return Err(io::Error::other(
                "Host telemetry service is already disposed",
            ));
        }
        set_turn_trace_host_bundle_version(identity.host_bundle_version.as_deref());
        self.logs.set_host_bundle_identity(&identity);
        if self.production.is_none() {
            return Ok(());
        }
        let mut runtime = self
            .runtime
            .lock()
            .map_err(|_| io::Error::other("Host telemetry runtime mutex poisoned"))?;
        if runtime.host_bundle_identity_set {
            return Ok(());
        }

        let started_crash_forwarder = runtime.crash_marker_forwarder.is_none();
        if started_crash_forwarder {
            runtime.crash_marker_forwarder =
                Some(HostCrashMarkerForwarder::start(self.logs.clone()));
        }
        if let Some(shipper) = runtime.box_log_shipper.as_ref() {
            if let Err(error) = shipper.start() {
                if started_crash_forwarder {
                    runtime.crash_marker_forwarder.take();
                }
                return Err(error);
            }
        }
        runtime.host_bundle_identity_set = true;
        Ok(())
    }

    pub fn report_message_sent(&self, report: MessageSentReport) {
        let text = report.prompt.as_deref().unwrap_or("").trim();
        let char_count = text.chars().count();
        self.analytics.mark_active("user_action");
        let _ = self.analytics.track_event(
            "sand.message.sent",
            &json!({
                "agent_id": report.agent_id,
                "char_count": char_count,
                "length_bucket": sand_message_length_bucket(char_count as f64).as_str(),
                "attachment_count": report.attachment_paths.len(),
                "has_rich_text": report.rich_text.as_deref().is_some_and(|value| !value.is_empty()),
                "is_fork": report.is_fork,
                "source": report.source.as_deref().unwrap_or("desktop"),
                "is_group_room": report.is_group_room,
            }),
        );
    }

    pub fn note_sand_model_experiment_active(&self) {
        let exposure = self
            .runtime
            .lock()
            .ok()
            .and_then(|runtime| runtime.model_experiment_exposure.clone());
        if let Some(exposure) = exposure {
            exposure.note();
        }
    }

    pub fn flush_tracing(&self) {
        if let Some(tracing) = self.hooks.tracing.as_ref() {
            tracing.flush();
        }
    }

    fn flush_for_fatal_exit_inner(&self) {
        let shipper = self
            .runtime
            .lock()
            .ok()
            .and_then(|mut runtime| runtime.box_log_shipper.take());
        if let Some(shipper) = shipper {
            shipper.stop_polling();
            let _ = shipper.checkpoint_offsets();
        }
        let _ = self.logs.flush();
        self.flush_tracing();
    }

    pub fn dispose(&self) -> io::Result<()> {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        pin_experiments_diagnostics_reporter(None);
        if let Ok(mut runtime) = self.runtime.lock() {
            if let Some(production) = self.production.as_ref() {
                if let Some(subscription) = runtime.auth_renewal_subscription.take() {
                    production
                        .auth
                        .service()
                        .unsubscribe_from_renewal(subscription);
                }
            }
            if let Some(stop) = runtime.inference_stop.take() {
                stop();
            }
            if let Some(shipper) = runtime.box_log_shipper.take() {
                shipper.stop_polling();
                let _ = shipper.checkpoint_offsets();
            }
            runtime.crash_marker_forwarder.take();
            if let Some(mut console_forwarder) = runtime.console_forwarder.take() {
                console_forwarder.dispose();
            }
            runtime.desktop_health_forwarder.take();
            runtime.event_loop_telemetry.take();
            runtime.flush_polling.take();
            runtime.model_experiment_exposure.take();
            runtime.host_bundle_identity_set = false;
        }

        if let Some(profiler) = self.hooks.pressure_profiler.as_ref() {
            if let Ok(mut profiler) = profiler.lock() {
                profiler.dispose();
            }
        }
        self.analytics.detach_runtime();
        if let Some(transport) = self.logs.production_transport.as_ref() {
            let _ = transport.dispose();
        }
        self.sink.flush()?;
        if let Some(tracing) = self.hooks.tracing.as_ref() {
            tracing.dispose();
        }
        Ok(())
    }

    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    pub fn is_disposed(&self) -> bool {
        self.disposed.load(Ordering::Acquire)
    }

    pub fn records_path(&self) -> &Path {
        self.sink.path()
    }

    fn monotonic_now_ms(&self) -> f64 {
        self.clock_origin.elapsed().as_secs_f64() * 1_000.0
    }
}

impl Drop for HostTelemetryService {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

#[derive(Clone)]
pub struct HostTelemetryApi {
    service: Arc<HostTelemetryService>,
}

impl HostTelemetryApi {
    pub fn service_dispose(&self) -> io::Result<()> {
        self.service.dispose()
    }

    pub fn set_host_bundle_identity(&self, identity: HostBundleIdentity) -> io::Result<()> {
        self.service.set_host_bundle_identity(identity)
    }

    pub fn flush_tracing(&self) {
        self.service.flush_tracing();
    }

    pub fn flush_for_fatal_exit(&self) {
        let service = Arc::clone(&self.service);
        let (done_tx, done_rx) = mpsc::channel();
        if thread::Builder::new()
            .name("sand-host-fatal-telemetry-flush".into())
            .spawn(move || {
                service.flush_for_fatal_exit_inner();
                let _ = done_tx.send(());
            })
            .is_ok()
        {
            let _ = done_rx.recv_timeout(FATAL_TELEMETRY_FLUSH_TIMEOUT);
        }
    }

    pub fn report_message_sent(&self, report: MessageSentReport) {
        self.service.report_message_sent(report);
    }

    pub fn note_sand_model_experiment_active(&self) {
        self.service.note_sand_model_experiment_active();
    }

    pub fn forward_console(&self, level: &str, message: &str) {
        let _ = self.service.logs.report_host_log(level, message);
    }

    pub fn monotonic_now_ms(&self) -> f64 {
        self.service.monotonic_now_ms()
    }

    pub fn create_host_lifecycle_progress(
        &self,
        started_at: f64,
        watchdog: WatchdogArm,
    ) -> HostLifecycleProgress {
        let service = Arc::clone(&self.service);
        let now = Arc::new(move || service.monotonic_now_ms());
        let logs = self.service.logs.clone();
        let report = Arc::new(move |report| {
            let _ = logs.report_host_lifecycle(report);
        });
        HostLifecycleProgress::new(started_at, now, report, watchdog)
    }
}
