use crate::telemetry_record::{metadata, TelemetryRecord};

pub struct DiskPressureReport<'a> {
    pub level: &'a str,
    pub volume: &'a str,
    pub trigger: &'a str,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
}

pub fn telemetry_level(level: &str) -> &'static str {
    match level {
        "hard" => "error",
        "soft" => "warn",
        _ => "info",
    }
}

pub fn disk_pressure_telemetry(r: DiskPressureReport<'_>) -> TelemetryRecord {
    TelemetryRecord {
        level: telemetry_level(r.level),
        event: None,
        message: None,
        metadata: metadata([
            ("volume", Some(r.volume.into())),
            ("pressure_level", Some(r.level.into())),
            ("trigger", Some(r.trigger.into())),
            ("total_bytes", Some(r.total_bytes.to_string())),
            ("available_bytes", Some(r.available_bytes.to_string())),
            ("used_percent", Some(format!("{:.1}", r.used_percent))),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hard_soft_and_other_levels_map_exactly() {
        assert_eq!(telemetry_level("hard"), "error");
        assert_eq!(telemetry_level("soft"), "warn");
        assert_eq!(telemetry_level("none"), "info");
    }
}
