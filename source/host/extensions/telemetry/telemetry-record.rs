use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryRecord {
    pub level: &'static str,
    pub event: Option<&'static str>,
    pub message: Option<&'static str>,
    pub metadata: BTreeMap<String, String>,
}

pub fn metadata<const N: usize>(entries: [(&str, Option<String>); N]) -> BTreeMap<String, String> {
    entries
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value)))
        .collect()
}
