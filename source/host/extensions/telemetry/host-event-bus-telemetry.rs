use crate::telemetry_record::{metadata, TelemetryRecord};

pub const HOST_EVENT_BUS_EVENT: &str = "sand.host.event_bus";

pub struct HostEventBusReport<'a> {
    pub kind: &'a str,
    pub topic: &'a str,
    pub error_class: &'a str,
}

pub fn host_event_bus_telemetry(report: HostEventBusReport<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: "error",
        event: Some(HOST_EVENT_BUS_EVENT),
        message: None,
        metadata: metadata([
            ("kind", Some(report.kind.into())),
            ("topic", Some(report.topic.into())),
            ("error_class", Some(report.error_class.into())),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_event_bus_failure_to_error_telemetry() {
        let record = host_event_bus_telemetry(HostEventBusReport {
            kind: "subscriber_failed", topic: "turn", error_class: "Error",
        });
        assert_eq!(record.level, "error");
        assert_eq!(record.event, Some(HOST_EVENT_BUS_EVENT));
        assert_eq!(record.metadata["topic"], "turn");
    }
}
