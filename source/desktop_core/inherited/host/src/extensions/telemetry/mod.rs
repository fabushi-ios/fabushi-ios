use std::collections::BTreeMap;

pub mod agent_error_telemetry;
pub mod agent_open_telemetry;
pub mod invariant_violation_telemetry;
pub mod auto_review_approval_telemetry;
pub mod disk_pressure_telemetry;
pub mod host_diagnostic_telemetry;
pub mod host_event_bus_telemetry;
pub mod gateway_command_telemetry;
pub mod search_index_health_telemetry;
pub mod send_trace_sampler;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTelemetryProjection {
    pub level: Option<&'static str>,
    pub event: Option<&'static str>,
    pub metadata: BTreeMap<String, String>,
}

pub mod automation_shadow_prune_telemetry;
pub mod box_log_ship_telemetry;
pub mod box_copy_in_telemetry;
pub mod box_recreate_decided_telemetry;
pub mod experiments_diagnostic_telemetry;
pub mod host_extension_diagnostic_telemetry;
pub mod revival_telemetry_mappers;
pub mod session_diagnostic_telemetry;
pub mod turn_empty_delivery_telemetry;

pub mod automation_fire_telemetry;
pub mod automation_lifecycle_telemetry;
pub mod bot_block_telemetry;
pub mod conversation_gc_telemetry;
pub mod local_exec_telemetry;

pub mod openclaw_logging_core;
pub mod openclaw_secret_redactor;
pub mod openclaw_diagnostic_telemetry;
pub mod queue_telemetry_mappers;

pub mod sand_error_tags;
pub mod memory_synthesis_telemetry;
pub mod journal_outcome_telemetry;
pub mod mcp_discovery_telemetry;
pub mod connector_auth_telemetry;
pub mod webauthn_proxy_telemetry;

pub mod analytics_service;
pub mod extension;
pub mod host_telemetry_service;
pub mod structured_log_telemetry;
pub mod host_lifecycle_progress;

pub mod model_experiment_exposure;
pub mod host_crash_marker;
pub mod event_loop_telemetry;
pub mod turn_telemetry_mappers;
pub mod desktop_health_forwarder;
pub mod lifecycle_telemetry;
pub mod pressure_cpu_profiler;
pub mod host_tracing;
pub mod box_log_shipper;
