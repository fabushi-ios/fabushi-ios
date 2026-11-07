use std::collections::BTreeMap;
use serde_json::{json, Map, Value};

use crate::agents::agent_roster::{
    apply_implicit_agent_roster_defaults, can_write_doctor_include, collect_agent_entries,
    pin_survivor_workspace_for_roster_collapse, try_resolve_default_agent_id,
};
use crate::extensions::secrets::audit_manager::SecretsAuditManager;
use crate::host_diagnostics::report_diagnostic_event;

use super::settings_service::{
    is_valid_iana_time_zone, normalize_sand_auto_review_instructions,
    normalize_sand_local_tool_permission, SettingsService, INFERENCE_PROVIDERS,
    REDACTED_SECRET_SENTINEL,
};
use super::settings_service_migration::{
    apply_account_scope, clear_account_sensitive_settings, force_model_max_mode,
    is_model_selection, normalize_disabled_tools, normalize_legacy_instructions,
    normalize_server_instructions, normalize_sidebar_sections, string_list_map_value,
    string_map_value, unique_strings,
};

pub fn report_secret_audit_findings(settings: &Map<String, Value>) {
    let manager = SecretsAuditManager::new();
    let findings = manager.audit_json_value(&Value::Object(settings.clone()), "settings");
    for finding in findings {
        report_diagnostic_event(
            "security.secrets.finding",
            json!({
                "keyPath": finding.key_path,
                "findingKind": finding.finding_kind,
                "message": finding.message,
                "suggestedSecretRef": finding.suggested_secret_ref,
            }),
        );
    }
}

pub fn apply_skill_config_update(settings: &mut Map<String, Value>, update: &Value) {
    let Some(skill_update) = update.as_object() else {
        return;
    };
    let Some(skill_entries) = skill_update.get("entries").and_then(Value::as_object) else {
        return;
    };
    let skills = settings
        .entry("skills")
        .or_insert_with(|| json!({}));
    if !skills.is_object() {
        *skills = json!({});
    }
    let skills_object = skills.as_object_mut().expect("skills object");
    let entries = skills_object
        .entry("entries")
        .or_insert_with(|| json!({}));
    if !entries.is_object() {
        *entries = json!({});
    }
    let entries_object = entries.as_object_mut().expect("skills entries object");

    for (skill_key, requested) in skill_entries {
        let Some(requested) = requested.as_object() else {
            continue;
        };
        let current = entries_object
            .entry(skill_key.clone())
            .or_insert_with(|| json!({}));
        if !current.is_object() {
            *current = json!({});
        }
        let current_object = current.as_object_mut().expect("skill entry object");

        if let Some(enabled) = requested.get("enabled").and_then(Value::as_bool) {
            current_object.insert("enabled".into(), Value::Bool(enabled));
        }
        if let Some(api_key) = requested.get("apiKey").and_then(Value::as_str) {
            let api_key = api_key.trim();
            if api_key != REDACTED_SECRET_SENTINEL {
                if api_key.is_empty() {
                    current_object.remove("apiKey");
                } else {
                    current_object.insert("apiKey".into(), Value::String(api_key.into()));
                }
            }
        }
        if let Some(requested_env) = requested.get("env").and_then(Value::as_object) {
            let env = current_object
                .entry("env")
                .or_insert_with(|| json!({}));
            if !env.is_object() {
                *env = json!({});
            }
            let env_object = env.as_object_mut().expect("skill env object");
            for (key, value) in requested_env {
                let Some(value) = value.as_str() else {
                    continue;
                };
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                let value = value.trim();
                if value == REDACTED_SECRET_SENTINEL {
                    continue;
                }
                if value.is_empty() {
                    env_object.remove(key);
                } else {
                    env_object.insert(key.into(), Value::String(value.into()));
                }
            }
        }
    }
}

impl SettingsService {
    pub fn get_host_settings(&self) -> Value {
        let stored = self.load();
        let mut object = Map::new();
        object.insert("version".into(), Value::from(1));
        if self.is_read_only() {
            object.insert("readOnly".into(), Value::Bool(true));
        }
        object.insert("notifications".into(), self.get_notification_config());
        if let Some(agents) = stored.get("agents") {
            object.insert("agents".into(), agents.clone());
        }
        if let Some(skills) = stored.get("skills") {
            object.insert("skills".into(), skills.clone());
        }
        object.insert(
            "mcpCustomInstructions".into(),
            string_map_value(self.get_mcp_custom_instructions()),
        );
        object.insert(
            "mcpCustomInstructionsByServerId".into(),
            string_map_value(self.get_mcp_custom_instructions_by_server_id()),
        );
        object.insert(
            "mcpDisabledToolsByServerId".into(),
            string_list_map_value(self.get_mcp_disabled_tools_by_server_id()),
        );
        if let Some(scope) = self.get_mcp_custom_instructions_account_scope() {
            object.insert(
                "mcpCustomInstructionsAccountScope".into(),
                Value::String(scope),
            );
        }
        object.insert(
            "mcpBoxServers".into(),
            Value::Array(
                self.get_mcp_box_servers()
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        let auto_review = self.get_auto_review_instructions();
        object.insert(
            "autoReviewInstructions".into(),
            json!({
                "isEnabled": auto_review.is_enabled,
                "allowInstructions": auto_review.allow_instructions,
                "blockInstructions": auto_review.block_instructions
            }),
        );
        object.insert(
            "localToolPermission".into(),
            Value::String(self.get_local_tool_permission().as_str().into()),
        );
        object.insert(
            "webauthnProxyEnabled".into(),
            Value::Bool(self.get_webauthn_proxy_enabled()),
        );
        object.insert(
            "inferenceProvider".into(),
            Value::String(self.get_inference_provider()),
        );
        object.insert(
            "inferenceRouterUsage".into(),
            self.get_inference_router_usage(),
        );
        if let Some(value) = self.get_detected_user_time_zone() {
            object.insert("userTimeZone".into(), Value::String(value));
        }
        if let Some(value) = self.get_user_time_zone_override() {
            object.insert("userTimeZoneOverride".into(), Value::String(value));
        }
        if let Some(value) = self.get_agent_default_model() {
            object.insert("agentDefaultModel".into(), value);
        }
        if let Some(value) = self.get_computer_use_model() {
            object.insert("computerUseModel".into(), value);
        }
        if let Some(value) = self.get_pinned_agent_ids() {
            object.insert(
                "pinnedAgentIds".into(),
                Value::Array(value.into_iter().map(Value::String).collect()),
            );
        }
        object.insert(
            "sidebarSections".into(),
            Value::Array(self.get_sidebar_sections().unwrap_or_default()),
        );
        if let Some(value) = self.get_has_seen_onboarding() {
            object.insert("hasSeenOnboarding".into(), Value::Bool(value));
        }
        Value::Object(object)
    }

    pub fn set_host_settings(&self, update: &Value) -> Result<Value, String> {
        if self.is_read_only() {
            return Err("Config is externally managed or marked readOnly; mutations are not allowed".into());
        }
        let Some(update) = update.as_object() else {
            return Ok(self.get_host_settings());
        };
        let _guard = self.write_lock.lock().unwrap();
        let previous_zone = self.get_user_time_zone();
        let mut settings = self.load();
        let changed_fields = update.keys().cloned().collect::<Vec<_>>();

        if let Some(agent_update) = update.get("agents") {
            let parsed = Value::Object(settings.clone());
            let mut resolved = parsed.clone();
            if let Some(object) = resolved.as_object_mut() {
                if agent_update.is_null() {
                    object.remove("agents");
                } else {
                    object.insert("agents".into(), agent_update.clone());
                }
            }
            let current_roster_is_writable =
                can_write_doctor_include(&parsed, &resolved, false);
            let target_roster_is_writable =
                can_write_doctor_include(&resolved, &resolved, false);
            if !current_roster_is_writable || !target_roster_is_writable {
                return Err(
                    "agent roster is include-owned and cannot be overwritten by this config owner"
                        .into(),
                );
            }
        }
        if let Some(value) = update.get("skills") {
            apply_skill_config_update(&mut settings, value);
        }

        settings.insert("notifications".into(), json!({ "isEnabled": false }));

        if let Some(value) = update.get("mcpCustomInstructionsAccountScope") {
            match value {
                Value::Null => {
                    settings.remove("mcpCustomInstructionsAccountScope");
                    clear_account_sensitive_settings(&mut settings);
                }
                Value::String(scope) if !scope.trim().is_empty() => {
                    if settings
                        .get("mcpCustomInstructionsAccountScope")
                        .and_then(Value::as_str)
                        != Some(scope.trim())
                    {
                        apply_account_scope(&mut settings, scope.trim());
                    }
                }
                _ => {}
            }
        }
        if let Some(value) = update.get("mcpCustomInstructions") {
            settings.insert(
                "mcpCustomInstructions".into(),
                string_map_value(normalize_legacy_instructions(Some(value))),
            );
        }
        if let Some(value) = update.get("mcpCustomInstructionsByServerId") {
            settings.insert(
                "mcpCustomInstructionsByServerId".into(),
                string_map_value(normalize_server_instructions(Some(value))),
            );
        }
        if let Some(value) = update.get("mcpDisabledToolsByServerId") {
            settings.insert(
                "mcpDisabledToolsByServerId".into(),
                string_list_map_value(normalize_disabled_tools(Some(value))),
            );
        }
        if let Some(value) = update.get("mcpBoxServers") {
            settings.insert(
                "mcpBoxServers".into(),
                Value::Array(
                    unique_strings(Some(value))
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        if let Some(value) = update.get("agents") {
            if value.is_null() {
                settings.remove("agents");
            } else if value.is_object() {
                let previous = Value::Object(settings.clone());
                let candidate = json!({ "agents": value.clone() });
                let normalized = apply_implicit_agent_roster_defaults(&candidate);
                let mut target = previous.clone();
                if let Some(normalized_agents) = normalized.get("agents") {
                    target
                        .as_object_mut()
                        .expect("settings object")
                        .insert("agents".into(), normalized_agents.clone());
                }

                let source_workspace = try_resolve_default_agent_id(&previous)
                    .and_then(|active_id| {
                        collect_agent_entries(&previous, None)
                            .into_iter()
                            .find(|entry| entry.id == active_id)
                            .and_then(|entry| {
                                entry
                                    .entry
                                    .get("workspace")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                    });
                let (pinned, _) = pin_survivor_workspace_for_roster_collapse(
                    &previous,
                    &target,
                    source_workspace.as_deref(),
                );
                settings.insert(
                    "agents".into(),
                    pinned
                        .get("agents")
                        .cloned()
                        .unwrap_or_else(|| value.clone()),
                );
            }
        }
        for field in ["userTimeZone", "userTimeZoneOverride"] {
            if let Some(Value::String(zone)) = update.get(field) {
                let trimmed = zone.trim();
                if trimmed.is_empty() {
                    settings.remove(field);
                } else if is_valid_iana_time_zone(trimmed) {
                    settings.insert(field.into(), Value::String(trimmed.into()));
                }
            }
        }
        if let Some(value) = update.get("agentDefaultModel") {
            if value.is_null() {
                settings.remove("agentDefaultModel");
            } else if is_model_selection(value) {
                settings.insert(
                    "agentDefaultModel".into(),
                    force_model_max_mode(value.clone()),
                );
            }
        }
        if let Some(value) = update.get("computerUseModel") {
            if value.is_null() {
                settings.remove("computerUseModel");
            } else if is_model_selection(value) {
                settings.insert("computerUseModel".into(), value.clone());
            }
        }
        if let Some(value) = update.get("autoReviewInstructions") {
            let normalized = normalize_sand_auto_review_instructions(Some(value));
            settings.insert(
                "autoReviewInstructions".into(),
                json!({
                    "isEnabled": normalized.is_enabled,
                    "allowInstructions": normalized.allow_instructions,
                    "blockInstructions": normalized.block_instructions
                }),
            );
        }
        if let Some(value) = update.get("localToolPermission") {
            let normalized =
                normalize_sand_local_tool_permission(value.as_str());
            settings.insert(
                "localToolPermission".into(),
                Value::String(normalized.as_str().into()),
            );
        }
        if let Some(Value::Bool(value)) = update.get("webauthnProxyEnabled") {
            settings.insert("webauthnProxyEnabled".into(), Value::Bool(*value));
        }
        if let Some(value) = update.get("pinnedAgentIds") {
            settings.insert(
                "pinnedAgentIds".into(),
                Value::Array(
                    unique_strings(Some(value))
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        if let Some(Value::Array(value)) = update.get("sidebarSections") {
            let stored = settings
                .get("sidebarSections")
                .and_then(Value::as_array)
                .cloned();
            settings.insert(
                "sidebarSections".into(),
                Value::Array(normalize_sidebar_sections(value, stored.as_deref())),
            );
        }
        if let Some(Value::Bool(value)) = update.get("hasSeenOnboarding") {
            settings.insert("hasSeenOnboarding".into(), Value::Bool(*value));
        }
        if let Some(Value::String(value)) = update.get("inferenceProvider") {
            if INFERENCE_PROVIDERS.contains(&value.as_str()) {
                settings.insert(
                    "inferenceProvider".into(),
                    Value::String(value.clone()),
                );
            }
        }

        self.persist(&settings).map_err(|error| error.to_string())?;
        report_secret_audit_findings(&settings);

        let next_zone = self.get_user_time_zone();
        if next_zone != previous_zone {
            let listeners = self
                .user_time_zone_listeners
                .lock()
                .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            for listener in listeners {
                listener(next_zone.clone());
            }
        }
        if let Some(Value::Object(raw)) = update.get("featureFlagOverrides") {
            let overrides = raw
                .iter()
                .filter_map(|(key, value)| value.as_bool().map(|value| (key.clone(), value)))
                .collect::<BTreeMap<_, _>>();
            let listeners = self
                .feature_flag_override_listeners
                .lock()
                .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            for listener in listeners {
                listener(overrides.clone());
            }
        }
        self.notify_changes(changed_fields);
        Ok(self.get_host_settings())
    }
}
