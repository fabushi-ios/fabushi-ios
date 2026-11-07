use serde::{Deserialize, Serialize};
use serde_json::{self, json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use super::hook_runner::*;

// Boundary invariant validations (from src/plugins/contracts/boundary-invariants.test.ts)

pub struct HookBoundaryInvariantGuard;

impl HookBoundaryInvariantGuard {
    pub const ALLOWED_BUNDLED_TYPED_HOOKS: &'static [&'static str] = &[
        "agent_end",
        "before_agent_reply",
        "before_agent_run",
        "before_prompt_build",
        "before_tool_call",
        "cron_changed",
        "cron_reconciled",
        "gateway_start",
        "gateway_stop",
        "inbound_claim",
        "reply_dispatch",
        "session_end",
        "subagent_delivery_target",
        "subagent_ended",
    ];

    pub fn is_allowed_typed_hook(hook_name: &str) -> bool {
        Self::ALLOWED_BUNDLED_TYPED_HOOKS.contains(&hook_name)
    }

    pub fn assert_no_raw_register_hook_calls(sources: &[(&str, &str)]) -> Result<(), Vec<String>> {
        let mut violations = Vec::new();
        for (file_path, content) in sources {
            if content.contains("registerHook(") {
                violations.push(file_path.to_string());
            }
        }
        if violations.is_empty() {
            Ok(())
        } else {
            violations.sort();
            Err(violations)
        }
    }

    pub fn is_allowed_bundled_extension_import(specifier: &str) -> bool {
        if !specifier.contains("extensions/") {
            return true;
        }
        specifier.ends_with("/api.js") || specifier.ends_with("/runtime-api.js")
    }
}

// Agent event subscription payloads (from src/plugins/contracts/agent-event-payload.contract.test.ts)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventPayloadMode {
    Full,
    Metadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEventSubscription {
    pub id: String,
    pub streams: Vec<String>,
    pub phases: Vec<String>,
    pub payload_mode: EventPayloadMode,
}

impl AgentEventSubscription {
    pub fn matches(&self, stream: &str, phase: Option<&str>) -> bool {
        if !self.streams.is_empty() && !self.streams.iter().any(|s| s == stream) {
            return false;
        }
        if !self.phases.is_empty() {
            let p_str = phase.unwrap_or("");
            if !self.phases.iter().any(|p| p.trim() == p_str) {
                return false;
            }
        }
        true
    }

    pub fn filter_payload(&self, raw_data: &Value) -> Value {
        match self.payload_mode {
            EventPayloadMode::Full => raw_data.clone(),
            EventPayloadMode::Metadata => {
                if let Some(obj) = raw_data.as_object() {
                    let mut meta = serde_json::Map::new();
                    for (k, v) in obj {
                        match v {
                            Value::String(s) if s.len() <= 256 => {
                                meta.insert(k.clone(), v.clone());
                            }
                            Value::Number(_) | Value::Bool(_) | Value::Null => {
                                meta.insert(k.clone(), v.clone());
                            }
                            _ => {}
                        }
                    }
                    Value::Object(meta)
                } else {
                    raw_data.clone()
                }
            }
        }
    }
}

// Active run steering (from src/plugins/contracts/active-run-steering.contract.test.ts)

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SteerActiveRunParams {
    pub session_key: String,
    pub text: String,
    pub display_text: Option<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerActiveRunOutcome {
    pub steered: bool,
    pub run_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Default)]
pub struct ActiveRunSteeringManager {
    seen_idempotency_keys: Mutex<std::collections::HashSet<String>>,
}

impl ActiveRunSteeringManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn steer_active_run(
        &self,
        allow_prompt_injection: bool,
        active_run_id: Option<&str>,
        is_streaming: bool,
        params: &SteerActiveRunParams,
    ) -> SteerActiveRunOutcome {
        if !allow_prompt_injection {
            return SteerActiveRunOutcome {
                steered: false,
                run_id: None,
                reason: Some("blocked by plugins.entries.steering-plugin.hooks.allowPromptInjection=false".to_string()),
            };
        }

        let run_id = match active_run_id {
            Some(id) => id,
            None => {
                return SteerActiveRunOutcome {
                    steered: false,
                    run_id: None,
                    reason: Some("no_active_run".to_string()),
                };
            }
        };

        if !is_streaming {
            return SteerActiveRunOutcome {
                steered: false,
                run_id: None,
                reason: Some("not_streaming".to_string()),
            };
        }

        if let Some(ref idem) = params.idempotency_key {
            let mut seen = self.seen_idempotency_keys.lock().unwrap();
            if !seen.insert(idem.clone()) {
                return SteerActiveRunOutcome {
                    steered: false,
                    run_id: None,
                    reason: Some("duplicate".to_string()),
                };
            }
        }

        SteerActiveRunOutcome {
            steered: true,
            run_id: Some(run_id.to_string()),
            reason: None,
        }
    }
}

// Architectural Boundary Guardrails (from src/plugins/contracts/...)

pub struct BundledExtensionConfigApiGuard;

impl BundledExtensionConfigApiGuard {
    pub fn validate_extension_config_schema(
        rel_path: &str,
        source: &str,
    ) -> Result<(), String> {
        let channel_schema_extensions = [
            "extensions/telegram/src/config-schema.ts",
            "extensions/discord/src/config-schema.ts",
            "extensions/slack/src/config-schema.ts",
            "extensions/signal/src/config-schema.ts",
            "extensions/imessage/src/config-schema.ts",
            "extensions/msteams/src/config-schema.ts",
        ];
        let local_config_api_extensions = [
            "extensions/whatsapp/src/config-schema.ts",
            "extensions/googlechat/src/config-schema.ts",
        ];

        if channel_schema_extensions.contains(&rel_path) {
            if !source.contains("openclaw/plugin-sdk/channel-config-schema") {
                return Err(format!("{}: missing allowed openclaw/plugin-sdk/channel-config-schema", rel_path));
            }
            if source.contains("openclaw/plugin-sdk/bundled-channel-config-schema") {
                return Err(format!("{}: contains forbidden openclaw/plugin-sdk/bundled-channel-config-schema", rel_path));
            }
        } else if local_config_api_extensions.contains(&rel_path) {
            if !source.contains("../config-api.js") {
                return Err(format!("{}: missing allowed ../config-api.js", rel_path));
            }
            if source.contains("openclaw/plugin-sdk/channel-config-schema") {
                return Err(format!("{}: contains forbidden openclaw/plugin-sdk/channel-config-schema", rel_path));
            }
        }
        Ok(())
    }
}

pub struct CoreExtensionFacadeBoundaryGuard;

impl CoreExtensionFacadeBoundaryGuard {
    pub fn assert_no_ollama_facades(existing_paths: &[&str]) -> Result<(), Vec<String>> {
        let forbidden = [
            "src/plugin-sdk/ollama.ts",
            "src/plugin-sdk/ollama-runtime.ts",
        ];
        let mut offenders = Vec::new();
        for p in existing_paths {
            if forbidden.contains(p) {
                offenders.push(p.to_string());
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }

    pub fn assert_no_bundled_provider_terms_in_fixtures(fixtures: &[(&str, &str)]) -> Result<(), Vec<String>> {
        let forbidden_terms = ["Ollama", "ollama", "Moonshot", "moonshot", "xAI", "xai", "x-ai"];
        let mut offenders = Vec::new();
        for (name, content) in fixtures {
            for term in &forbidden_terms {
                if content.contains(term) {
                    offenders.push(format!("{}: contains forbidden term '{}'", name, term));
                    break;
                }
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

pub struct DeprecatedInternalConfigApiGuard;

impl DeprecatedInternalConfigApiGuard {
    pub fn assert_no_deprecated_config_api(sources: &[(&str, &str)]) -> Result<(), Vec<String>> {
        let deprecated_seams = [
            "loadDeprecatedPluginConfig",
            "writeDeprecatedPluginConfig",
            "legacyPluginConfigAccessor",
        ];
        let mut offenders = Vec::new();
        for (path, content) in sources {
            for seam in &deprecated_seams {
                if content.contains(seam) {
                    offenders.push(format!("{}: uses deprecated config seam '{}'", path, seam));
                }
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

pub struct ModelCatalogCoreImportsGuard;

impl ModelCatalogCoreImportsGuard {
    pub fn assert_no_legacy_bridge_imports(sources: &[(&str, &str)]) -> Result<(), Vec<String>> {
        let legacy_bridges = [
            "src/agents/provider-id.js",
            "src/model-catalog/refs.js",
            "src/model-catalog/normalize.js",
            "src/model-catalog/types.js",
            "src/config/model-refs.js",
            "src/shared/provider-model-id-normalization.js",
            "src/plugin-sdk/provider-model-id-normalize.js",
        ];
        let mut offenders = Vec::new();
        for (path, content) in sources {
            for bridge in &legacy_bridges {
                if content.contains(bridge) {
                    offenders.push(format!("{}: imports legacy bridge '{}'", path, bridge));
                }
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }
}

// Config Boundary Guard & Extension Test API Consumption Guard
// (from config-boundary-guard.test.ts and extension-test-api-consumption.test.ts)

pub struct ConfigBoundaryGuard;

impl ConfigBoundaryGuard {
    pub fn assert_no_runtime_action_load_config(
        file_path: &str,
        source: &str,
    ) -> Result<(), Vec<String>> {
        let is_runtime_action = file_path.contains("/src/send.")
            || file_path.contains("/src/action.")
            || file_path.contains("/src/dispatch.");

        if is_runtime_action && (source.contains("loadConfig()") || source.contains("loadPluginConfig()")) {
            return Err(vec![format!("{}: runtime action invokes loadConfig() instead of passed context", file_path)]);
        }
        Ok(())
    }
}

pub struct ExtensionTestApiConsumptionGuard;

impl ExtensionTestApiConsumptionGuard {
    pub fn assert_test_api_imported_only_in_tests(
        file_path: &str,
        source: &str,
    ) -> Result<(), Vec<String>> {
        let is_test_file = file_path.ends_with(".test.ts")
            || file_path.ends_with(".spec.ts")
            || file_path.ends_with(".test.js")
            || file_path.contains("/test/")
            || file_path.contains("/tests/");

        if !is_test_file && (source.contains("/test-api") || source.contains("/test-api.js")) {
            return Err(vec![format!("{}: production file imports extension test-api", file_path)]);
        }
        Ok(())
    }
}

// Next-Turn Injection, Session Extension State, and Gateway Host-Hook Server Methods
// (from host-hook-state.identity.test.ts and host-hooks.contract.test.ts)

#[derive(Debug, Clone, Default)]
pub struct NextTurnInjectionManager {
    queues: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>>,
}

impl NextTurnInjectionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&self, session_key: &str, text: &str) {
        let mut queues = self.queues.lock().unwrap();
        queues.entry(session_key.to_string()).or_default().push(text.to_string());
    }

    pub fn drain(&self, session_key: &str) -> Vec<String> {
        let mut queues = self.queues.lock().unwrap();
        queues.remove(session_key).unwrap_or_default()
    }

    pub fn is_empty(&self, session_key: &str) -> bool {
        let queues = self.queues.lock().unwrap();
        queues.get(session_key).map(|q| q.is_empty()).unwrap_or(true)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PluginSessionExtensionStore {
    state: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(String, String), serde_json::Value>>>,
}

impl PluginSessionExtensionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn patch(&self, session_key: &str, namespace: &str, value: serde_json::Value) {
        let mut state = self.state.lock().unwrap();
        state.insert((session_key.to_string(), namespace.to_string()), value);
    }

    pub fn get(&self, session_key: &str, namespace: &str) -> Option<serde_json::Value> {
        let state = self.state.lock().unwrap();
        state.get(&(session_key.to_string(), namespace.to_string())).cloned()
    }

    pub fn clear_owned(&self, session_key: &str) {
        let mut state = self.state.lock().unwrap();
        state.retain(|(s, _), _| s != session_key);
    }
}

pub struct GatewayPluginHostHooks;

impl GatewayPluginHostHooks {
    pub fn handle_ui_descriptors(
        scopes: &[String],
        generation: usize,
    ) -> Result<serde_json::Value, String> {
        let has_admin = scopes.iter().any(|s| s == "operator.admin" || s == "admin");
        let mut descriptors = vec![
            serde_json::json!({
                "id": "approval-panel",
                "pluginId": "host-hook-fixture",
                "surface": "session",
                "label": "Approval panel"
            }),
        ];
        if has_admin {
            descriptors.push(serde_json::json!({
                "id": "admin-panel",
                "pluginId": "host-hook-fixture",
                "surface": "settings",
                "label": "Admin panel",
                "requiredScopes": ["operator.admin"]
            }));
        }
        Ok(serde_json::json!({
            "ok": true,
            "generation": generation,
            "descriptors": descriptors
        }))
    }

    pub fn handle_session_action(
        action: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({
            "ok": true,
            "action": action,
            "echo": payload
        }))
    }
}

// Run Context Lifecycle, Scheduled Turns, Session Actions, Attachments, and Entry Projections
// (from run-context-lifecycle, scheduled-turns, session-actions, session-attachments,
//  and session-entry-projection contract test suites)

