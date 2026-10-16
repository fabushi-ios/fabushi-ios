use crate::telemetry_record::{metadata, TelemetryRecord};

pub const HOST_DIAGNOSTIC_EVENT: &str = "sand.host.diagnostic";

pub struct HostDiagnosticTelemetryInput<'a> {
    pub kind: &'a str,
    pub stage: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub error_class: Option<&'a str>,
}

pub fn host_diagnostic_telemetry(d: HostDiagnosticTelemetryInput<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: if d.kind == "send_ledger_degraded" { "error" } else { "warn" },
        event: Some(HOST_DIAGNOSTIC_EVENT),
        message: None,
        metadata: metadata([
            ("kind", Some(d.kind.into())),
            ("stage", d.stage.map(str::to_owned)),
            ("agent_id", d.agent_id.map(str::to_owned)),
            ("reason", d.reason.map(str::to_owned)),
            ("error_class", d.error_class.map(str::to_owned)),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn send_ledger_degradation_is_error_other_diagnostics_warn() {
        let degraded = host_diagnostic_telemetry(HostDiagnosticTelemetryInput {
            kind: "send_ledger_degraded", stage: None, agent_id: None, reason: None, error_class: None,
        });
        assert_eq!(degraded.level, "error");
        let other = host_diagnostic_telemetry(HostDiagnosticTelemetryInput {
            kind: "other", stage: None, agent_id: None, reason: None, error_class: None,
        });
        assert_eq!(other.level, "warn");
    }
}
