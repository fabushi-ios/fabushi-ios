use crate::telemetry_record::{metadata, TelemetryRecord};

pub const TURN_EMPTY_DELIVERY_EVENT: &str = "sand.turn.empty_delivery";

pub struct TurnEmptyDeliveryReport<'a> {
    pub conversation_id: &'a str,
    pub request_id: Option<&'a str>,
    pub source: &'a str,
    pub request_source: Option<&'a str>,
    pub reply_nudge_attempts: Option<i64>,
    pub redrive_attempts: Option<i64>,
    pub tool_call_count: i64,
    pub stream_output_produced: bool,
    pub duration_ms: f64,
    pub ack_outstanding: bool,
}

pub fn turn_empty_delivery_telemetry(r: TurnEmptyDeliveryReport<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: "warn",
        event: Some(TURN_EMPTY_DELIVERY_EVENT),
        message: None,
        metadata: metadata([
            ("conversation_id", Some(r.conversation_id.into())),
            ("request_id", r.request_id.map(str::to_owned)),
            ("source", Some(r.source.into())),
            ("request_source", r.request_source.map(str::to_owned)),
            ("reply_nudge_attempts", r.reply_nudge_attempts.map(|v| v.to_string())),
            ("redrive_attempts", r.redrive_attempts.map(|v| v.to_string())),
            ("tool_call_count", Some(r.tool_call_count.to_string())),
            ("stream_output_produced", Some(r.stream_output_produced.to_string())),
            ("duration_ms", Some((r.duration_ms.round() as i64).to_string())),
            ("ack_outstanding", Some(r.ack_outstanding.to_string())),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_delivery_is_warn_and_keeps_retry_metadata() {
        let record = turn_empty_delivery_telemetry(TurnEmptyDeliveryReport {
            conversation_id: "c", request_id: None, source: "direct",
            request_source: Some("user"), reply_nudge_attempts: Some(1),
            redrive_attempts: None, tool_call_count: 0,
            stream_output_produced: false, duration_ms: 10.6, ack_outstanding: true,
        });
        assert_eq!(record.level, "warn");
        assert_eq!(record.metadata["duration_ms"], "11");
        assert_eq!(record.metadata["ack_outstanding"], "true");
    }
}
