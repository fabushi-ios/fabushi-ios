use std::collections::{BTreeMap, BTreeSet};
use serde_json::{json, Map, Value};

use super::settings_service::{
    SettingsService, INFERENCE_PROVIDERS, MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH,
};

pub fn clamp_mcp_custom_instruction(value: &str) -> String {
    value.chars().take(MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH).collect()
}

pub fn default_mcp_custom_instruction(name: &str) -> &'static str {
    if name.trim().eq_ignore_ascii_case("hex") {
        "When using Hex, get the underlying numbers as data: download/export the results as CSV or use the data the connector returns, and analyze those raw values directly. Don't read rendered charts or graphs from screenshots (computer-use chart reading is unreliable) — work from the actual data."
    } else {
        ""
    }
}

pub fn is_server_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn normalize_legacy_instructions(
    value: Option<&Value>,
) -> BTreeMap<String, String> {
    let Some(object) = value.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    object
        .iter()
        .filter_map(|(name, value)| {
            let value = clamp_mcp_custom_instruction(value.as_str()?);
            if value.trim().is_empty() && default_mcp_custom_instruction(name).is_empty() {
                None
            } else {
                Some((name.clone(), value))
            }
        })
        .collect()
}

pub fn normalize_server_instructions(
    value: Option<&Value>,
) -> BTreeMap<String, String> {
    let Some(object) = value.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    object
        .iter()
        .filter(|(id, _)| is_server_id(id))
        .filter_map(|(id, value)| {
            value.as_str().map(|value| {
                (id.clone(), clamp_mcp_custom_instruction(value))
            })
        })
        .collect()
}

pub fn normalize_disabled_tools(
    value: Option<&Value>,
) -> BTreeMap<String, Vec<String>> {
    let Some(object) = value.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    let mut result = BTreeMap::new();
    for (id, tools) in object {
        if !is_server_id(id) {
            continue;
        }
        let tools = unique_strings(Some(tools));
        if !tools.is_empty() {
            result.insert(id.clone(), tools);
        }
    }
    result
}

pub fn dedupe_non_empty(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for value in values {
        if !value.is_empty() && seen.insert(value.clone()) {
            result.push(value);
        }
    }
    result
}

pub fn unique_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            dedupe_non_empty(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string),
            )
        })
        .unwrap_or_default()
}

pub fn string_map_value(values: BTreeMap<String, String>) -> Value {
    Value::Object(
        values
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect(),
    )
}

pub fn string_list_map_value(values: BTreeMap<String, Vec<String>>) -> Value {
    Value::Object(
        values
            .into_iter()
            .map(|(key, values)| {
                (
                    key,
                    Value::Array(values.into_iter().map(Value::String).collect()),
                )
            })
            .collect(),
    )
}

pub fn apply_account_scope(settings: &mut Map<String, Value>, account_scope: &str) {
    let previous_scope = settings
        .get("mcpCustomInstructionsAccountScope")
        .and_then(Value::as_str)
        .map(str::to_string);
    let seen = settings.get("hasSeenOnboarding").and_then(Value::as_bool);
    let seen_owner = settings
        .get("hasSeenOnboardingAccountScope")
        .and_then(Value::as_str)
        .map(str::to_string);

    settings.remove("hasSeenOnboarding");
    settings.remove("hasSeenOnboardingAccountScope");
    if let Some(seen) = seen {
        if seen_owner.as_deref().is_none() || seen_owner.as_deref() == Some(account_scope) {
            settings.insert("hasSeenOnboarding".into(), Value::Bool(seen));
            settings.insert(
                "hasSeenOnboardingAccountScope".into(),
                Value::String(account_scope.into()),
            );
        }
    }

    if previous_scope.is_some() && previous_scope.as_deref() != Some(account_scope) {
        clear_account_sensitive_settings(settings);
    }
    settings.insert(
        "mcpCustomInstructionsAccountScope".into(),
        Value::String(account_scope.into()),
    );
}

pub fn clear_account_sensitive_settings(settings: &mut Map<String, Value>) {
    for field in [
        "autoReviewInstructions",
        "agentDefaultModel",
        "computerUseModel",
        "localToolPermission",
        "localToolPermissionCeiling",
    ] {
        settings.remove(field);
    }
    settings.insert("mcpCustomInstructions".into(), json!({}));
    settings.insert("mcpCustomInstructionsByServerId".into(), json!({}));
    settings.insert("mcpDisabledToolsByServerId".into(), json!({}));
}

pub fn is_model_selection(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object
        .get("modelId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .is_none()
        || object.get("maxMode").and_then(Value::as_bool).is_none()
    {
        return false;
    }
    object
        .get("parameters")
        .and_then(Value::as_array)
        .is_some_and(|parameters| {
            parameters.iter().all(|parameter| {
                parameter.as_object().is_some_and(|parameter| {
                    parameter
                        .get("id")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .is_some()
                        && parameter.get("value").and_then(Value::as_str).is_some()
                })
            })
        })
}

pub fn force_model_max_mode(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("maxMode".into(), Value::Bool(true));
    }
    value
}

pub fn normalize_sidebar_sections(
    sections: &[Value],
    stored: Option<&[Value]>,
) -> Vec<Value> {
    let mut folds = BTreeMap::<String, bool>::new();
    for source in stored.into_iter().flatten().chain(sections.iter()) {
        let Some(object) = source.as_object() else {
            continue;
        };
        let Some(id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(collapsed) = object.get("isCollapsed").and_then(Value::as_bool) {
            folds.insert(id.trim().to_string(), collapsed);
        }
    }
    let mut seen_sections = BTreeSet::new();
    let mut claimed_agents = BTreeSet::new();
    let mut result = Vec::new();
    for section in sections {
        let Some(object) = section.as_object() else {
            continue;
        };
        let Some(raw_id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        let id = raw_id.trim();
        if id.is_empty() || id == "__agents__" || !seen_sections.insert(id.to_string()) {
            continue;
        }
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut agent_ids = Vec::new();
        if let Some(items) = object.get("agentIds").and_then(Value::as_array) {
            for item in items {
                let Some(agent_id) = item.as_str() else {
                    continue;
                };
                if !agent_id.is_empty() && claimed_agents.insert(agent_id.to_string()) {
                    agent_ids.push(Value::String(agent_id.to_string()));
                }
            }
        }
        result.push(json!({
            "id": id,
            "name": name,
            "agentIds": agent_ids,
            "isCollapsed": folds.get(id).copied().unwrap_or(false)
        }));
    }
    if !result.is_empty() {
        result.push(json!({
            "id": "__agents__",
            "name": "Unassigned",
            "agentIds": [],
            "isCollapsed": folds.get("__agents__").copied().unwrap_or(false)
        }));
    }
    result
}

pub fn empty_inference_usage() -> Value {
    let empty = || {
        json!({
            "requests": 0,
            "inputTokens": 0,
            "outputTokens": 0,
            "cacheReadTokens": 0,
            "cacheWriteTokens": 0,
            "lastUsedAt": null
        })
    };
    json!({
        "schemaVersion": 1,
        "providers": {
            "fabushi": empty(),
            "cursor": empty(),
            "claude-code": empty(),
            "codex": empty(),
            "openrouter": empty()
        }
    })
}

pub fn normalize_inference_usage(value: Option<&Value>) -> Value {
    let mut usage = empty_inference_usage();
    let Some(raw_providers) = value
        .and_then(|value| value.get("providers"))
        .and_then(Value::as_object)
    else {
        return usage;
    };
    for provider in INFERENCE_PROVIDERS {
        let Some(raw) = raw_providers.get(provider).and_then(Value::as_object) else {
            continue;
        };
        let target = usage
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut(provider))
            .and_then(Value::as_object_mut)
            .expect("empty usage has provider");
        for key in [
            "requests",
            "inputTokens",
            "outputTokens",
            "cacheReadTokens",
            "cacheWriteTokens",
        ] {
            let count = raw.get(key).and_then(Value::as_u64).unwrap_or(0);
            target.insert(key.into(), Value::from(count));
        }
        if let Some(last_used_at) = raw.get("lastUsedAt").and_then(Value::as_str) {
            target.insert(
                "lastUsedAt".into(),
                Value::String(last_used_at.to_string()),
            );
        }
    }
    usage
}

pub fn safe_usage(value: Option<f64>) -> u64 {
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.round().min(u64::MAX as f64) as u64)
        .unwrap_or(0)
}

pub fn increment_counter(object: &mut Map<String, Value>, key: &str, by: u64) {
    let current = object.get(key).and_then(Value::as_u64).unwrap_or(0);
    object.insert(key.into(), Value::from(current.saturating_add(by)));
}

pub fn disabled_notification_config() -> Value {
    json!({
        "isEnabled": false,
        "allowedApps": [],
        "minIntervalMs": 5_000,
        "maxPerWindow": 10,
        "windowMs": 5 * 60_000
    })
}

impl SettingsService {
    pub fn get_mcp_custom_instructions_by_server_id(&self) -> BTreeMap<String, String> {
        normalize_server_instructions(
            self.load().get("mcpCustomInstructionsByServerId"),
        )
    }

    pub fn set_mcp_custom_instructions_by_server_id(
        &self,
        value: BTreeMap<String, String>,
    ) -> Result<bool, String> {
        let normalized = value
            .into_iter()
            .filter(|(id, _)| is_server_id(id))
            .map(|(id, value)| {
                (id, Value::String(clamp_mcp_custom_instruction(&value)))
            })
            .collect::<Map<String, Value>>();
        self.set_json_field(
            "mcpCustomInstructionsByServerId",
            Some(Value::Object(normalized)),
        )
    }

    pub fn get_mcp_disabled_tools_by_server_id(
        &self,
    ) -> BTreeMap<String, Vec<String>> {
        normalize_disabled_tools(self.load().get("mcpDisabledToolsByServerId"))
    }

    pub fn set_mcp_disabled_tools_by_server_id(
        &self,
        value: BTreeMap<String, Vec<String>>,
    ) -> Result<bool, String> {
        let mut normalized = Map::new();
        for (id, tools) in value {
            if !is_server_id(&id) {
                continue;
            }
            let tools = dedupe_non_empty(tools);
            if !tools.is_empty() {
                normalized.insert(
                    id,
                    Value::Array(tools.into_iter().map(Value::String).collect()),
                );
            }
        }
        self.set_json_field(
            "mcpDisabledToolsByServerId",
            Some(Value::Object(normalized)),
        )
    }

    pub fn get_mcp_custom_instructions_account_scope(&self) -> Option<String> {
        self.load()
            .get("mcpCustomInstructionsAccountScope")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }

    pub fn scope_to_account(&self, account_scope: &str) -> Result<bool, String> {
        let account_scope = account_scope.trim();
        if account_scope.is_empty() {
            return self.clear_account_scope();
        }
        let mut settings = self.load();
        if settings
            .get("mcpCustomInstructionsAccountScope")
            .and_then(Value::as_str)
            == Some(account_scope)
        {
            return Ok(false);
        }
        apply_account_scope(&mut settings, account_scope);
        self.persist(&settings).map_err(|error| error.to_string())?;
        self.notify_changes(vec!["mcpCustomInstructionsAccountScope".into()]);
        Ok(true)
    }

    pub fn clear_account_scope(&self) -> Result<bool, String> {
        let mut settings = self.load();
        let before = settings.clone();
        settings.remove("mcpCustomInstructionsAccountScope");
        clear_account_sensitive_settings(&mut settings);
        if before == settings {
            return Ok(false);
        }
        self.persist(&settings).map_err(|error| error.to_string())?;
        self.notify_changes(vec!["mcpCustomInstructionsAccountScope".into()]);
        Ok(true)
    }

    pub fn get_raw_mcp_custom_instruction(&self, name: &str) -> Option<String> {
        self.get_mcp_custom_instructions().get(name).cloned()
    }

    pub fn get_raw_mcp_custom_instruction_by_server_id(
        &self,
        id: &str,
    ) -> Option<String> {
        self.get_mcp_custom_instructions_by_server_id()
            .get(id)
            .cloned()
    }

    pub fn migrate_mcp_custom_instruction_to_server_id(
        &self,
        server_id: &str,
        display_name: &str,
    ) -> Result<bool, String> {
        if !is_server_id(server_id) {
            return Ok(false);
        }
        let mut settings = self.load();
        let mut by_id =
            normalize_server_instructions(settings.get("mcpCustomInstructionsByServerId"));
        if by_id.contains_key(server_id) {
            return Ok(false);
        }
        let legacy = normalize_legacy_instructions(settings.get("mcpCustomInstructions"));
        let Some(value) = legacy.get(display_name).cloned() else {
            return Ok(false);
        };
        by_id.insert(server_id.into(), value);
        settings.insert(
            "mcpCustomInstructionsByServerId".into(),
            string_map_value(by_id),
        );
        self.persist(&settings).map_err(|error| error.to_string())?;
        self.notify_changes(vec!["mcpCustomInstructionsByServerId".into()]);
        Ok(true)
    }

    pub fn set_mcp_custom_instruction_by_server_id(
        &self,
        server_id: &str,
        display_name: &str,
        value: &str,
        mirror_legacy_name: bool,
    ) -> Result<bool, String> {
        if !is_server_id(server_id) {
            return Ok(false);
        }
        let value = clamp_mcp_custom_instruction(value);
        let mut settings = self.load();
        let mut by_id =
            normalize_server_instructions(settings.get("mcpCustomInstructionsByServerId"));
        let mut legacy =
            normalize_legacy_instructions(settings.get("mcpCustomInstructions"));
        by_id.insert(server_id.into(), value.clone());
        if mirror_legacy_name {
            if value.trim().is_empty()
                && default_mcp_custom_instruction(display_name).is_empty()
            {
                legacy.remove(display_name);
            } else {
                legacy.insert(display_name.into(), value);
            }
        } else {
            legacy.remove(display_name);
        }
        settings.insert(
            "mcpCustomInstructionsByServerId".into(),
            string_map_value(by_id),
        );
        settings.insert("mcpCustomInstructions".into(), string_map_value(legacy));
        self.persist(&settings).map_err(|error| error.to_string())?;
        self.notify_changes(vec![
            "mcpCustomInstructionsByServerId".into(),
            "mcpCustomInstructions".into(),
        ]);
        Ok(true)
    }

    pub fn delete_mcp_custom_instruction_by_server_id(
        &self,
        server_id: &str,
        display_name: &str,
        delete_legacy_name: bool,
    ) -> Result<bool, String> {
        let mut settings = self.load();
        let mut by_id =
            normalize_server_instructions(settings.get("mcpCustomInstructionsByServerId"));
        let mut legacy =
            normalize_legacy_instructions(settings.get("mcpCustomInstructions"));
        let mut changed = by_id.remove(server_id).is_some();
        if delete_legacy_name {
            changed |= legacy.remove(display_name).is_some();
        }
        if !changed {
            return Ok(false);
        }
        settings.insert(
            "mcpCustomInstructionsByServerId".into(),
            string_map_value(by_id),
        );
        settings.insert("mcpCustomInstructions".into(), string_map_value(legacy));
        self.persist(&settings).map_err(|error| error.to_string())?;
        self.notify_changes(vec![
            "mcpCustomInstructionsByServerId".into(),
            "mcpCustomInstructions".into(),
        ]);
        Ok(true)
    }
}
