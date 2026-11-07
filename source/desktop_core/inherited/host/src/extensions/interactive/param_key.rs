//! Parameter Key Normalization & Snake-Case Resolution
//!
//! Mirrors upstream OpenClaw `src/param-key.ts`:
//! Normalizes parameter keys from camelCase to snake_case while preserving
//! user-visible whitespace.

use std::collections::BTreeMap;
use regex::Regex;
use std::sync::LazyLock;

static RE_ACRONYM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([A-Z]+)([A-Z][a-z])").unwrap());
static RE_CAMEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([a-z0-9])([A-Z])").unwrap());

/// Converts camelCase key to snake_case key while preserving surrounding whitespace.
pub fn to_snake_case_key(key: &str) -> String {
    let step1 = RE_ACRONYM.replace_all(key, "${1}_${2}");
    let step2 = RE_CAMEL.replace_all(&step1, "${1}_${2}");
    step2.to_lowercase()
}

/// Resolves parameter key from a map, checking direct match first, then snake_case variant.
pub fn resolve_snake_case_param_key<'a, V>(
    params: &'a BTreeMap<String, V>,
    key: &str,
) -> Option<&'a str> {
    if params.contains_key(key) {
        return params.get_key_value(key).map(|(k, _)| k.as_str());
    }
    let snake = to_snake_case_key(key);
    if snake != key && params.contains_key(&snake) {
        return params.get_key_value(&snake).map(|(k, _)| k.as_str());
    }
    None
}

/// Reads parameter value from a map, checking direct match first, then snake_case variant.
pub fn read_snake_case_param_raw<'a, V>(
    params: &'a BTreeMap<String, V>,
    key: &str,
) -> Option<&'a V> {
    if let Some(val) = params.get(key) {
        return Some(val);
    }
    let snake = to_snake_case_key(key);
    if snake != key {
        return params.get(&snake);
    }
    None
}
