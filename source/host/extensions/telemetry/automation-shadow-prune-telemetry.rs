use crate::telemetry_record::{metadata, TelemetryRecord};

pub const AUTOMATION_SHADOW_PRUNE_EVENT: &str = "sand.automation.shadow_prune";

pub struct AutomationShadowPruneReport<'a> {
    pub outcome: &'a str,
    pub conversation_id: &'a str,
    pub automation_id: &'a str,
    pub local_definition_state: &'a str,
    pub local_definition_count: i64,
    pub desired_count: i64,
    pub remote_shadow_count: i64,
    pub box_uptime_ms: Option<i64>,
}

pub fn automation_shadow_prune_telemetry(r: AutomationShadowPruneReport<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: if r.outcome == "failed" { "warn" } else { "info" },
        event: Some(AUTOMATION_SHADOW_PRUNE_EVENT),
        message: None,
        metadata: metadata([
            ("conversation_id", Some(r.conversation_id.into())),
            ("automation_id", Some(r.automation_id.into())),
            ("outcome", Some(r.outcome.into())),
            ("local_definition_state", Some(r.local_definition_state.into())),
            ("local_definition_count", Some(r.local_definition_count.to_string())),
            ("desired_count", Some(r.desired_count.to_string())),
            ("remote_shadow_count", Some(r.remote_shadow_count.to_string())),
            ("box_uptime_ms", r.box_uptime_ms.map(|v| v.to_string())),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_prune_warns_and_serializes_counts() {
        let record = automation_shadow_prune_telemetry(AutomationShadowPruneReport {
            outcome: "failed", conversation_id: "c", automation_id: "a",
            local_definition_state: "present", local_definition_count: 1,
            desired_count: 2, remote_shadow_count: 3, box_uptime_ms: None,
        });
        assert_eq!(record.level, "warn");
        assert_eq!(record.metadata["remote_shadow_count"], "3");
        assert!(!record.metadata.contains_key("box_uptime_ms"));
    }
}
