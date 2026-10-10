use crate::telemetry_record::{metadata, TelemetryRecord};

pub struct AutoReviewApprovalReport<'a> {
    pub event_type: &'a str,
    pub conversation_id: &'a str,
    pub approval_id: &'a str,
    pub surface: &'a str,
    pub status: &'a str,
    pub age_ms: f64,
    pub ttl_ms: Option<f64>,
    pub cause: Option<&'a str>,
}

fn nonnegative_rounded(value: f64) -> i64 {
    value.max(0.0).round() as i64
}

pub fn auto_review_approval_telemetry(r: AutoReviewApprovalReport<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: "info",
        event: Some("sand.auto_review.approval"),
        message: None,
        metadata: metadata([
            ("event_type", Some(r.event_type.into())),
            ("conversation_id", Some(r.conversation_id.into())),
            ("approval_id", Some(r.approval_id.into())),
            ("surface", Some(r.surface.into())),
            ("status", Some(r.status.into())),
            ("age_ms", Some(nonnegative_rounded(r.age_ms).to_string())),
            ("ttl_ms", r.ttl_ms.map(|v| nonnegative_rounded(v).to_string())),
            ("cause", r.cause.map(str::to_owned)),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamps_negative_age_and_preserves_optional_fields() {
        let record = auto_review_approval_telemetry(AutoReviewApprovalReport {
            event_type: "expired", conversation_id: "c", approval_id: "a",
            surface: "composer", status: "pending", age_ms: -5.0,
            ttl_ms: Some(1000.4), cause: Some("timeout"),
        });
        assert_eq!(record.metadata["age_ms"], "0");
        assert_eq!(record.metadata["ttl_ms"], "1000");
        assert_eq!(record.metadata["cause"], "timeout");
    }
}
