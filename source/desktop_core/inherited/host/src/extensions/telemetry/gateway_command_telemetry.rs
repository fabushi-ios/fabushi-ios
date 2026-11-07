use std::collections::BTreeMap;

use crate::gateway_server::GatewayCommandReport;

use super::HostTelemetryProjection;

pub const GATEWAY_COMMAND_ERROR_EVENT: &str = "sand.gateway_command_error";
pub const GATEWAY_COMMAND_TIMING_EVENT: &str = "sand.gateway_command_timing";

fn trace_ids(traceparent: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(value) = traceparent else {
        return (None, None);
    };
    let mut parts = value.split('-');
    let _version = parts.next();
    let trace_id = parts.next().filter(|value| !value.is_empty()).map(str::to_string);
    let span_id = parts.next().filter(|value| !value.is_empty()).map(str::to_string);
    (trace_id, span_id)
}

fn base_metadata(report: &GatewayCommandReport) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::from([
        ("method".into(), report.method.clone()),
        ("duration_ms".into(), report.duration_ms.to_string()),
    ]);
    if let Some(value) = report.request_id.as_deref() {
        metadata.insert("request_id".into(), value.into());
    }
    let (trace_id, span_id) = trace_ids(report.traceparent.as_deref());
    if let Some(value) = trace_id {
        metadata.insert("trace_id".into(), value);
    }
    if let Some(value) = span_id {
        metadata.insert("span_id".into(), value);
    }
    metadata
}

pub fn gateway_command_timing_telemetry(report: &GatewayCommandReport) -> HostTelemetryProjection {
    HostTelemetryProjection {
        level: Some("info"),
        event: Some(GATEWAY_COMMAND_TIMING_EVENT),
        metadata: base_metadata(report),
    }
}

pub fn gateway_command_error_telemetry(report: &GatewayCommandReport) -> HostTelemetryProjection {
    let mut metadata = base_metadata(report);
    if let Some(value) = report.reason.as_deref() {
        metadata.insert("reason".into(), value.into());
    }
    if let Some(value) = report.error_class.as_deref() {
        metadata.insert("error_class".into(), value.into());
    }
    if let Some(value) = report.errno.as_deref() {
        metadata.insert("errno".into(), value.into());
    }
    HostTelemetryProjection {
        level: Some("error"),
        event: Some(GATEWAY_COMMAND_ERROR_EVENT),
        metadata,
    }
}
