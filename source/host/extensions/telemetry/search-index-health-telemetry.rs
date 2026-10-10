use crate::telemetry_record::{metadata, TelemetryRecord};

pub const SEARCH_INDEX_HEALTH_EVENT: &str = "sand.search_index.health";

fn is_warn_kind(kind: &str) -> bool {
    matches!(kind, "dispose_drain_cut" | "worker_terminate_failed" | "job_retry")
}

pub struct SearchIndexHealthReport<'a> {
    pub kind: &'a str,
    pub stage: Option<&'a str>,
    pub error_class: Option<&'a str>,
    pub count: Option<i64>,
}

pub fn search_index_health_telemetry(r: SearchIndexHealthReport<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: if is_warn_kind(r.kind) { "warn" } else { "error" },
        event: Some(SEARCH_INDEX_HEALTH_EVENT),
        message: None,
        metadata: metadata([
            ("kind", Some(r.kind.into())),
            ("stage", r.stage.map(str::to_owned)),
            ("error_class", r.error_class.map(str::to_owned)),
            ("count", r.count.map(|value| value.to_string())),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_recovery_kinds_warn_and_unknown_kinds_error() {
        assert_eq!(
            search_index_health_telemetry(SearchIndexHealthReport {
                kind: "job_retry", stage: None, error_class: None, count: Some(2),
            }).level,
            "warn"
        );
        assert_eq!(
            search_index_health_telemetry(SearchIndexHealthReport {
                kind: "corrupt", stage: None, error_class: None, count: None,
            }).level,
            "error"
        );
    }
}
