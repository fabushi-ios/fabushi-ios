use serde_json::{Map, Value};

pub trait SandProductAnalytics: Send + Sync {
    fn track_event(&self, name: &str, properties: Option<&Map<String, Value>>);
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopSandProductAnalytics;

impl SandProductAnalytics for NoopSandProductAnalytics {
    fn track_event(&self, _name: &str, _properties: Option<&Map<String, Value>>) {}
}

pub fn create_noop_sand_product_analytics() -> NoopSandProductAnalytics {
    NoopSandProductAnalytics
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn noop_analytics_accepts_events_without_side_effects() {
        create_noop_sand_product_analytics().track_event("send", None);
    }
}
