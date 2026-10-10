use crate::telemetry_record::{metadata, TelemetryRecord};

pub const EXPERIMENTS_DIAGNOSTIC_EVENT: &str = "sand.experiments.diagnostic";

fn info_kind(kind: &str) -> bool {
    matches!(
        kind,
        "bootstrap_resolved"
            | "bootstrap_anonymous"
            | "bootstrap_discarded_auth_changed"
            | "exposure_flush_failed"
            | "shutdown_failed"
    )
}

fn warn_kind(kind: &str) -> bool {
    matches!(kind, "bootstrap_config_unparseable" | "bootstrap_cache_read_failed")
}

pub struct ExperimentsDiagnostic<'a> {
    pub kind: &'a str,
    pub stage: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub error_class: Option<&'a str>,
    pub gates_on_count: Option<i64>,
    pub authenticated: Option<bool>,
}

pub fn level_for(d: &ExperimentsDiagnostic<'_>) -> &'static str {
    if d.kind == "config_not_applied" {
        return if d.reason == Some("identity_unhydrated") { "info" } else { "warn" };
    }
    if info_kind(d.kind) { "info" }
    else if warn_kind(d.kind) { "warn" }
    else { "error" }
}

pub fn experiments_diagnostic_telemetry(d: ExperimentsDiagnostic<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: level_for(&d),
        event: Some(EXPERIMENTS_DIAGNOSTIC_EVENT),
        message: None,
        metadata: metadata([
            ("kind", Some(d.kind.into())),
            ("stage", d.stage.map(str::to_owned)),
            ("reason", d.reason.map(str::to_owned)),
            ("error_class", d.error_class.map(str::to_owned)),
            ("gates_on_count", d.gates_on_count.map(|v| v.to_string())),
            ("authenticated", d.authenticated.map(|v| v.to_string())),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn level_mapping_matches_reference_sets() {
        let info = ExperimentsDiagnostic {
            kind: "config_not_applied", stage: None, reason: Some("identity_unhydrated"),
            error_class: None, gates_on_count: None, authenticated: None,
        };
        assert_eq!(level_for(&info), "info");
        let warn = ExperimentsDiagnostic { kind: "bootstrap_cache_read_failed", reason: None, ..info };
        assert_eq!(level_for(&warn), "warn");
        let error = ExperimentsDiagnostic { kind: "unknown", reason: None, ..warn };
        assert_eq!(level_for(&error), "error");
    }
}
