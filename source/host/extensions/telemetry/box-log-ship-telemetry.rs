use crate::telemetry_record::{metadata, TelemetryRecord};

pub const BOX_LOG_SHIP_EVENT: &str = "sand.box.log_ship";

pub enum BoxLogShipReport<'a> {
    Progress {
        bytes_written: u64,
        bytes_delivered: u64,
        pending_window_count: u64,
        oldest_pending_window_age_ms: u64,
    },
    SaveFailed {
        error_class: &'a str,
        failure_count: u64,
    },
    SaveRecovered {
        error_class: &'a str,
        failure_count: u64,
    },
}

pub fn box_log_ship_telemetry(r: BoxLogShipReport<'_>) -> TelemetryRecord {
    match r {
        BoxLogShipReport::Progress {
            bytes_written, bytes_delivered, pending_window_count,
            oldest_pending_window_age_ms,
        } => TelemetryRecord {
            level: "info",
            event: None,
            message: Some(BOX_LOG_SHIP_EVENT),
            metadata: metadata([
                ("kind", Some("progress".into())),
                ("bytes_written", Some(bytes_written.to_string())),
                ("bytes_delivered", Some(bytes_delivered.to_string())),
                ("pending_window_count", Some(pending_window_count.to_string())),
                ("oldest_pending_window_age_ms", Some(oldest_pending_window_age_ms.to_string())),
            ]),
        },
        BoxLogShipReport::SaveFailed { error_class, failure_count } => TelemetryRecord {
            level: "warn",
            event: None,
            message: Some(BOX_LOG_SHIP_EVENT),
            metadata: metadata([
                ("kind", Some("save_failed".into())),
                ("error_class", Some(error_class.into())),
                ("failure_count", Some(failure_count.to_string())),
            ]),
        },
        BoxLogShipReport::SaveRecovered { error_class, failure_count } => TelemetryRecord {
            level: "info",
            event: None,
            message: Some(BOX_LOG_SHIP_EVENT),
            metadata: metadata([
                ("kind", Some("save_recovered".into())),
                ("error_class", Some(error_class.into())),
                ("failure_count", Some(failure_count.to_string())),
            ]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_ship_warns_and_recovery_is_info() {
        assert_eq!(
            box_log_ship_telemetry(BoxLogShipReport::SaveFailed {
                error_class: "io", failure_count: 2
            }).level,
            "warn"
        );
        assert_eq!(
            box_log_ship_telemetry(BoxLogShipReport::SaveRecovered {
                error_class: "io", failure_count: 2
            }).level,
            "info"
        );
    }
}
