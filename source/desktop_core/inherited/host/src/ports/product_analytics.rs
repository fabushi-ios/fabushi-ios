use std::collections::BTreeMap;

use serde_json::Value;

pub trait SandProductAnalytics: Send + Sync {
    fn track_event(&self, name: &str, properties: Option<&BTreeMap<String, Value>>);
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopSandProductAnalytics;

impl SandProductAnalytics for NoopSandProductAnalytics {
    fn track_event(&self, _name: &str, _properties: Option<&BTreeMap<String, Value>>) {}
}

pub fn create_noop_sand_product_analytics() -> NoopSandProductAnalytics {
    NoopSandProductAnalytics
}
