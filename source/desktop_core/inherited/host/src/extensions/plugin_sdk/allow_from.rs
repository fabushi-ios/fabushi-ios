use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasicAllowlistResolutionEntry {
    pub input: String,
    pub resolved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowlistGroupOverride {
    pub label: String,
    pub entries: Vec<String>,
}

pub fn format_allow_from_lowercase(allow_from: &[String], strip_prefix: Option<&str>) -> Vec<String> {
    allow_from
        .iter()
        .map(|entry| {
            let lower = entry.trim().to_lowercase();
            let s = if let Some(prefix) = strip_prefix {
                let prefix_lower = prefix.to_lowercase();
                if let Some(stripped) = lower.strip_prefix(&prefix_lower) {
                    stripped.to_string()
                } else {
                    lower
                }
            } else {
                lower
            };
            s
        })
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn format_normalized_allow_from_entries<F>(allow_from: &[String], mut normalize: F) -> Vec<String>
where
    F: FnMut(&str) -> Option<String>,
{
    allow_from
        .iter()
        .filter_map(|entry| normalize(entry.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn parse_allow_from_entries<F>(raw: &str, mut parse_entry: F) -> Result<Vec<String>, String>
where
    F: FnMut(&str) -> Result<String, String>,
{
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for token in raw.split(|c| c == '\n' || c == ',' || c == ';') {
        let entry = token.trim();
        if entry.is_empty() {
            continue;
        }
        if entry == "*" {
            if seen.insert("*".to_string()) {
                entries.push("*".to_string());
            }
            continue;
        }

        match parse_entry(entry) {
            Ok(val) => {
                let trimmed = val.trim().to_string();
                if !trimmed.is_empty() && seen.insert(trimmed.clone()) {
                    entries.push(trimmed);
                }
            }
            Err(e) => return Err(e),
        }
    }

    Ok(entries)
}

pub fn is_normalized_sender_allowed(
    sender_id: &str,
    allow_from: &[String],
    strip_prefix: Option<&str>,
) -> bool {
    let normalized = format_allow_from_lowercase(allow_from, strip_prefix);
    if normalized.is_empty() {
        return false;
    }
    if normalized.iter().any(|e| e == "*") {
        return true;
    }
    let sender_lower = sender_id.trim().to_lowercase();
    if sender_lower.is_empty() {
        return false;
    }
    normalized.contains(&sender_lower)
}

// --- Configuration Paths & Adapters ---

pub const DM_ALLOWLIST_READ_PATHS: &[&str] = &["allowFrom"];
pub const DM_ALLOWLIST_WRITE_PATH: &[&str] = &["allowFrom"];

pub const GROUP_ALLOWLIST_READ_PATHS: &[&str] = &["groupAllowFrom"];
pub const GROUP_ALLOWLIST_WRITE_PATH: &[&str] = &["groupAllowFrom"];

pub const LEGACY_DM_ALLOWLIST_READ_PATHS: &[&[&str]] = &[&["allowFrom"], &["dm", "allowFrom"]];
pub const LEGACY_DM_ALLOWLIST_WRITE_PATH: &[&str] = &["allowFrom"];

pub fn resolve_dm_group_allowlist_config_paths(scope: &str) -> (&'static [&'static str], &'static [&'static str]) {
    if scope == "dm" {
        (DM_ALLOWLIST_READ_PATHS, DM_ALLOWLIST_WRITE_PATH)
    } else {
        (GROUP_ALLOWLIST_READ_PATHS, GROUP_ALLOWLIST_WRITE_PATH)
    }
}

pub fn resolve_legacy_dm_allowlist_config_paths(scope: &str) -> Option<(&'static [&'static [&'static str]], &'static [&'static str])> {
    if scope == "dm" {
        Some((LEGACY_DM_ALLOWLIST_READ_PATHS, LEGACY_DM_ALLOWLIST_WRITE_PATH))
    } else {
        None
    }
}

pub fn collect_allowlist_overrides_from_value(
    record: Option<&Value>,
    label_fn: impl Fn(&str, &Value) -> String,
    entries_fn: impl Fn(&Value) -> Option<Vec<String>>,
) -> Vec<AllowlistGroupOverride> {
    let mut overrides = Vec::new();
    if let Some(Value::Object(map)) = record {
        for (key, val) in map {
            if let Some(entries) = entries_fn(val) {
                if !entries.is_empty() {
                    overrides.push(AllowlistGroupOverride {
                        label: label_fn(key, val),
                        entries,
                    });
                }
            }
        }
    }
    overrides
}

pub fn apply_account_scoped_allowlist_config_edit(
    root: &mut Value,
    channel_id: &str,
    account_id: Option<&str>,
    write_path: &[&str],
    entries: Vec<String>,
) {
    if !root.is_object() {
        *root = Value::Object(Default::default());
    }
    let channels = root
        .as_object_mut()
        .unwrap()
        .entry("channels")
        .or_insert_with(|| Value::Object(Default::default()));

    if !channels.is_object() {
        *channels = Value::Object(Default::default());
    }
    let channel = channels
        .as_object_mut()
        .unwrap()
        .entry(channel_id)
        .or_insert_with(|| Value::Object(Default::default()));

    let target = match account_id {
        Some(acc) if acc != "default" => {
            if !channel.is_object() {
                *channel = Value::Object(Default::default());
            }
            let accounts = channel
                .as_object_mut()
                .unwrap()
                .entry("accounts")
                .or_insert_with(|| Value::Object(Default::default()));
            if !accounts.is_object() {
                *accounts = Value::Object(Default::default());
            }
            accounts
                .as_object_mut()
                .unwrap()
                .entry(acc)
                .or_insert_with(|| Value::Object(Default::default()))
        }
        _ => channel,
    };

    if !target.is_object() {
        *target = Value::Object(Default::default());
    }

    let mut current = target;
    for &key in &write_path[..write_path.len() - 1] {
        current = current
            .as_object_mut()
            .unwrap()
            .entry(key)
            .or_insert_with(|| Value::Object(Default::default()));
    }

    let leaf = write_path.last().unwrap();
    let arr = Value::Array(entries.into_iter().map(Value::String).collect());
    current.as_object_mut().unwrap().insert(leaf.to_string(), arr);
}
