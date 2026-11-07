use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginActivationSource {
    Disabled,
    Explicit,
    Auto,
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginActivationState {
    pub enabled: bool,
    pub activated: bool,
    pub explicitly_enabled: bool,
    pub source: PluginActivationSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntryConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub hook_timeouts: Option<HashMap<String, u64>>,
    #[serde(default)]
    pub models: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSlotsConfig {
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub context_engine: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedPluginsConfig {
    #[serde(default)]
    pub entries: HashMap<String, PluginEntryConfig>,
    #[serde(default)]
    pub slots: PluginSlotsConfig,
    #[serde(default)]
    pub allow: Vec<String>,
}

pub fn normalize_plugin_id(id: &str) -> String {
    id.trim().to_lowercase()
}

pub fn is_retired_plugin_id(id: &str) -> bool {
    matches!(
        normalize_plugin_id(id).as_str(),
        "openclaw-legacy-plugin" | "deprecated-worker-plugin"
    )
}

pub fn is_explicit_plugin_disable_marker(val: &Value) -> bool {
    if let Some(b) = val.as_bool() {
        return !b;
    }
    if let Some(s) = val.as_str() {
        return s.eq_ignore_ascii_case("disabled") || s.eq_ignore_ascii_case("off");
    }
    false
}

// --- Path Pattern Contract Matching ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginConfigContractMatch {
    pub path: String,
    pub value: Value,
}

pub fn normalize_path_pattern(path_pattern: &str) -> Vec<String> {
    path_pattern
        .split('.')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn collect_plugin_config_contract_matches(
    root: &Value,
    path_pattern: &str,
) -> Vec<PluginConfigContractMatch> {
    let pattern = normalize_path_pattern(path_pattern);
    if pattern.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    traverse_matches(root, &pattern, 0, String::new(), &mut matches);
    matches
}

fn traverse_matches(
    current: &Value,
    pattern: &[String],
    segment_idx: usize,
    current_path: String,
    matches: &mut Vec<PluginConfigContractMatch>,
) {
    if segment_idx == pattern.len() {
        matches.push(PluginConfigContractMatch {
            path: current_path,
            value: current.clone(),
        });
        return;
    }

    let segment = &pattern[segment_idx];

    if segment == "*" {
        if let Some(obj) = current.as_object() {
            for (k, v) in obj {
                let next_path = if current_path.is_empty() {
                    k.clone()
                } else {
                    format!("{current_path}.{k}")
                };
                traverse_matches(v, pattern, segment_idx + 1, next_path, matches);
            }
        } else if let Some(arr) = current.as_array() {
            for (idx, v) in arr.iter().enumerate() {
                let next_path = if current_path.is_empty() {
                    idx.to_string()
                } else {
                    format!("{current_path}.{idx}")
                };
                traverse_matches(v, pattern, segment_idx + 1, next_path, matches);
            }
        }
    } else if let Some(obj) = current.as_object() {
        if let Some(v) = obj.get(segment) {
            let next_path = if current_path.is_empty() {
                segment.clone()
            } else {
                format!("{current_path}.{segment}")
            };
            traverse_matches(v, pattern, segment_idx + 1, next_path, matches);
        }
    } else if let Some(arr) = current.as_array() {
        if let Ok(idx) = segment.parse::<usize>() {
            if let Some(v) = arr.get(idx) {
                let next_path = if current_path.is_empty() {
                    idx.to_string()
                } else {
                    format!("{current_path}.{idx}")
                };
                traverse_matches(v, pattern, segment_idx + 1, next_path, matches);
            }
        }
    }
}

pub fn has_plugin_config_migration_source(
    root: &Value,
    path_patterns: &[String],
    touched_paths: Option<&[Vec<String>]>,
) -> bool {
    for pattern_str in path_patterns {
        let pattern = normalize_path_pattern(pattern_str);
        let touched = match touched_paths {
            None => true,
            Some(paths) => paths.iter().any(|parts| {
                if parts.len() > pattern.len() {
                    return false;
                }
                pattern[0..parts.len()]
                    .iter()
                    .zip(parts.iter())
                    .all(|(seg, part)| seg == "*" || seg == part)
            }),
        };
        if touched && !collect_plugin_config_contract_matches(root, pattern_str).is_empty() {
            return true;
        }
    }
    false
}

// --- Activation State Resolution ---

pub fn resolve_plugin_activation_state(
    id: &str,
    origin: &str,
    config: &NormalizedPluginsConfig,
    enabled_by_default: bool,
) -> PluginActivationState {
    let norm_id = normalize_plugin_id(id);

    // 1. Explicit disable
    if let Some(entry) = config.entries.get(&norm_id) {
        if entry.enabled == Some(false) {
            return PluginActivationState {
                enabled: false,
                activated: false,
                explicitly_enabled: false,
                source: PluginActivationSource::Disabled,
                reason: Some("explicitly disabled in plugin entries".to_string()),
            };
        }
    }

    // 2. Explicit enable
    if let Some(entry) = config.entries.get(&norm_id) {
        if entry.enabled == Some(true) {
            return PluginActivationState {
                enabled: true,
                activated: true,
                explicitly_enabled: true,
                source: PluginActivationSource::Explicit,
                reason: Some("enabled in config entries".to_string()),
            };
        }
    }

    // 3. Slot selections
    if config.slots.memory.as_deref() == Some(&norm_id) {
        return PluginActivationState {
            enabled: true,
            activated: true,
            explicitly_enabled: true,
            source: PluginActivationSource::Explicit,
            reason: Some("selected memory slot".to_string()),
        };
    }
    if config.slots.context_engine.as_deref() == Some(&norm_id) {
        return PluginActivationState {
            enabled: true,
            activated: true,
            explicitly_enabled: true,
            source: PluginActivationSource::Explicit,
            reason: Some("selected context engine slot".to_string()),
        };
    }

    // 4. Allowlist for external plugins
    if origin != "bundled" && config.allow.contains(&norm_id) {
        return PluginActivationState {
            enabled: true,
            activated: true,
            explicitly_enabled: true,
            source: PluginActivationSource::Explicit,
            reason: Some("selected in allowlist".to_string()),
        };
    }

    // 5. Default or Auto
    if enabled_by_default {
        PluginActivationState {
            enabled: true,
            activated: true,
            explicitly_enabled: false,
            source: PluginActivationSource::Default,
            reason: Some("enabled by default".to_string()),
        }
    } else {
        PluginActivationState {
            enabled: false,
            activated: false,
            explicitly_enabled: false,
            source: PluginActivationSource::Auto,
            reason: None,
        }
    }
}

// --- Memory Slot Decision Resolution ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySlotDecision {
    pub selected_plugin_id: Option<String>,
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn resolve_memory_slot_decision(
    config: &NormalizedPluginsConfig,
    available_memory_plugins: &[String],
) -> MemorySlotDecision {
    if let Some(selected) = &config.slots.memory {
        let norm_sel = normalize_plugin_id(selected);
        if is_retired_plugin_id(&norm_sel) {
            return MemorySlotDecision {
                selected_plugin_id: None,
                disabled: true,
                reason: Some(format!("retired memory plugin: {selected}")),
            };
        }
        return MemorySlotDecision {
            selected_plugin_id: Some(norm_sel),
            disabled: false,
            reason: Some("explicit memory slot configured".to_string()),
        };
    }

    if available_memory_plugins.len() == 1 {
        let single = normalize_plugin_id(&available_memory_plugins[0]);
        return MemorySlotDecision {
            selected_plugin_id: Some(single),
            disabled: false,
            reason: Some("single memory provider available".to_string()),
        };
    }

    MemorySlotDecision {
        selected_plugin_id: None,
        disabled: false,
        reason: None,
    }
}
