use std::collections::BTreeMap;

use super::HostTelemetryProjection;

pub const HOST_EVENT_BUS_EVENT: &str = "sand.host.event_bus";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEventBusReport {
    pub kind: String,
    pub topic: Option<String>,
    pub error_class: String,
}

pub fn host_event_bus_telemetry(report: &HostEventBusReport) -> HostTelemetryProjection {
    let mut metadata = BTreeMap::from([
        ("kind".into(), report.kind.clone()),
        ("error_class".into(), report.error_class.clone()),
    ]);
    if let Some(topic) = report.topic.as_ref() {
        metadata.insert("topic".into(), topic.clone());
    }
    HostTelemetryProjection {
        level: Some("error"),
        event: Some(HOST_EVENT_BUS_EVENT),
        metadata,
    }
}
