use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{SecondsFormat, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use crate::agents::agent_roster::{
    apply_implicit_agent_roster_defaults, collect_agent_entries,
    can_write_doctor_include,
    pin_survivor_workspace_for_roster_collapse, try_resolve_default_agent_id,
};
use crate::extensions::secrets::audit_manager::SecretsAuditManager;
use crate::host_diagnostics::report_diagnostic_event;
use crate::host_paths::get_sand_root_dir;

pub const MCP_CUSTOM_INSTRUCTIONS_MAX_LENGTH: usize = 500;
pub const SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES: usize = 20;
pub const SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS: usize = 1_000;
pub(crate) const INFERENCE_PROVIDERS: [&str; 5] = ["fabushi", "cursor", "claude-code", "codex", "openrouter"];
pub(crate) const REDACTED_SECRET_SENTINEL: &str = "[REDACTED]";

pub use super::settings_service_migration::*;
pub use super::settings_service_ocrm::*;

// OCRM-RESP-CONFIG-WRITE-GUARD-001: set_host_settings
// OCRM-RESP-SKILLS-CONFIG-MUTATION-001: apply_skill_config_update
pub use super::settings_service_ocrm::apply_skill_config_update;

pub type UserTimeZoneListener = Arc<dyn Fn(Option<String>) + Send + Sync + 'static>;
pub type SettingsChangeListener = Arc<dyn Fn(Vec<String>) + Send + Sync + 'static>;
pub type FeatureFlagOverrideListener =
    Arc<dyn Fn(BTreeMap<String, bool>) + Send + Sync + 'static>;

pub fn is_valid_iana_time_zone(value: &str) -> bool {
    value.parse::<Tz>().is_ok()
}

/// Traverses a nested JSON Value along a dot-separated path (e.g. "foo.bar"),
/// blocking prototype-bearing keys like "__proto__", "prototype", and "constructor".
pub fn get_json_value_at_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = root;
    for part in path.split('.') {
        let trimmed = part.trim();
        if trimmed.is_empty() || trimmed == "__proto__" || trimmed == "prototype" || trimmed == "constructor" {
            return None;
        }
        cursor = cursor.as_object()?.get(trimmed)?;
    }
    Some(cursor)
}

/// Mutates a nested JSON Object along a dot-separated path, creating intermediate objects
/// and rejecting unsafe prototype keys.
pub fn set_json_value_at_path(root: &mut Value, path: &str, val: Value) -> Result<(), &'static str> {
    let parts: Vec<&str> = path.split('.').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err("Invalid path. Use non-empty dot notation (e.g. foo.bar)");
    }
    if parts.iter().any(|p| *p == "__proto__" || *p == "prototype" || *p == "constructor") {
        return Err("Unsafe path segment: prototype pollution prevented");
    }
    if !root.is_object() {
        *root = Value::Object(Map::new());
    }
    let mut cursor = root.as_object_mut().unwrap();
    let (leaf, ancestors) = parts.split_last().unwrap();
    for ancestor in ancestors {
        if !cursor.contains_key(*ancestor) || !cursor[*ancestor].is_object() {
            cursor.insert((*ancestor).to_string(), Value::Object(Map::new()));
        }
        cursor = cursor.get_mut(*ancestor).unwrap().as_object_mut().unwrap();
    }
    cursor.insert((*leaf).to_string(), val);
    Ok(())
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandAutoReviewInstructions {
    pub is_enabled: bool,
    pub allow_instructions: Vec<String>,
    pub block_instructions: Vec<String>,
}
impl Default for SandAutoReviewInstructions {
    fn default() -> Self {
        Self {
            is_enabled: true,
            allow_instructions: Vec::new(),
            block_instructions: Vec::new(),
        }
    }
}
fn normalize_auto_review_instruction_list(raw: Option<&Value>) -> Vec<String> {
    let Some(items) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for item in items {
        let Some(value) = item.as_str() else {
            continue;
        };
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        let clamped = trimmed
            .chars()
            .take(SAND_AUTO_REVIEW_INSTRUCTION_MAX_CHARS)
            .collect::<String>();
        if !result.contains(&clamped) {
            result.push(clamped);
        }
        if result.len() >= SAND_AUTO_REVIEW_INSTRUCTION_MAX_ENTRIES {
            break;
        }
    }
    result
}
pub fn normalize_sand_auto_review_instructions(
    raw: Option<&Value>,
) -> SandAutoReviewInstructions {
    let object = raw.and_then(Value::as_object);
    SandAutoReviewInstructions {
        is_enabled: object
            .and_then(|value| value.get("isEnabled"))
            .and_then(Value::as_bool)
            != Some(false),
        allow_instructions: normalize_auto_review_instruction_list(
            object.and_then(|value| value.get("allowInstructions")),
        ),
        block_instructions: normalize_auto_review_instruction_list(
            object.and_then(|value| value.get("blockInstructions")),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandLocalToolPermission {
    Always,
    Ask,
    Never,
}
pub const SAND_DEFAULT_LOCAL_TOOL_PERMISSION: SandLocalToolPermission =
    SandLocalToolPermission::Ask;
impl SandLocalToolPermission {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Ask => "ask",
            Self::Never => "never",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "always" => Some(Self::Always),
            "ask" => Some(Self::Ask),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
    const fn rank(self) -> u8 {
        match self {
            Self::Never => 0,
            Self::Ask => 1,
            Self::Always => 2,
        }
    }
}
pub fn normalize_sand_local_tool_permission(
    value: Option<&str>,
) -> SandLocalToolPermission {
    value
        .and_then(SandLocalToolPermission::parse)
        .unwrap_or(SAND_DEFAULT_LOCAL_TOOL_PERMISSION)
}
pub fn resolve_sand_local_tool_permission(
    choice: SandLocalToolPermission,
    ceiling: Option<SandLocalToolPermission>,
) -> SandLocalToolPermission {
    match ceiling {
        Some(ceiling) if choice.rank() > ceiling.rank() => ceiling,
        _ => choice,
    }
}

pub struct SettingsSubscription<T: ?Sized> {
    id: u64,
    listeners: Arc<Mutex<BTreeMap<u64, Arc<T>>>>,
    active: bool,
}
impl<T: ?Sized> SettingsSubscription<T> {
    pub fn unsubscribe(mut self) {
        if self.active {
            if let Ok(mut listeners) = self.listeners.lock() {
                listeners.remove(&self.id);
            }
            self.active = false;
        }
    }
}
impl<T: ?Sized> Drop for SettingsSubscription<T> {
    fn drop(&mut self) {
        if self.active {
            if let Ok(mut listeners) = self.listeners.lock() {
                listeners.remove(&self.id);
            }
            self.active = false;
        }
    }
}

pub struct SettingsService {
    pub(crate) settings_path: PathBuf,
    pub(crate) user_time_zone_listeners: Arc<Mutex<BTreeMap<u64, UserTimeZoneListener>>>,
    pub(crate) change_listeners: Arc<Mutex<BTreeMap<u64, SettingsChangeListener>>>,
    pub(crate) feature_flag_override_listeners:
        Arc<Mutex<BTreeMap<u64, FeatureFlagOverrideListener>>>,
    pub(crate) next_listener_id: AtomicU64,
    pub(crate) write_lock: Mutex<()>,
}
impl SettingsService {
    pub fn production() -> Self {
        Self::new(get_sand_root_dir().join("settings.json"))
    }
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            settings_path: path.into(),
            user_time_zone_listeners: Arc::new(Mutex::new(BTreeMap::new())),
            change_listeners: Arc::new(Mutex::new(BTreeMap::new())),
            feature_flag_override_listeners: Arc::new(Mutex::new(BTreeMap::new())),
            next_listener_id: AtomicU64::new(1),
            write_lock: Mutex::new(()),
        }
    }
    pub fn get_settings_path(&self) -> &Path {
        &self.settings_path
    }

    /// Returns secret-free provenance for the persisted settings source.
    pub fn get_config_source_metadata(&self) -> Value {
        let path = self.settings_path.to_string_lossy().into_owned();
        let Ok(raw) = fs::read(&self.settings_path) else {
            return json!({
                "path": path,
                "exists": self.settings_path.exists(),
                "valid": false,
                "rawHash": Value::Null,
            });
        };
        let valid = serde_json::from_slice::<Value>(&raw)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .and_then(|object| object.get("version").and_then(Value::as_i64))
            == Some(1);
        json!({
            "path": path,
            "exists": true,
            "valid": valid,
            "rawHash": crate::sha256::sha256_hex(raw),
        })
    }
    pub(crate) fn empty_settings() -> Map<String, Value> {
        Map::from_iter([
            ("version".into(), Value::from(1)),
            ("mcpBoxServers".into(), json!([])),
            ("webauthnProxyEnabled".into(), Value::Bool(true)),
            ("mcpCustomInstructions".into(), json!({})),
            ("mcpCustomInstructionsByServerId".into(), json!({})),
            ("mcpDisabledToolsByServerId".into(), json!({})),
        ])
    }
    pub(crate) fn load(&self) -> Map<String, Value> {
        let Ok(raw) = fs::read_to_string(&self.settings_path) else {
            return Self::empty_settings();
        };
        let Ok(Value::Object(mut value)) = serde_json::from_str::<Value>(&raw) else {
            return Self::empty_settings();
        };
        if value.get("version").and_then(Value::as_i64) != Some(1) {
            return Self::empty_settings();
        }
        for (key, default) in Self::empty_settings() {
            value.entry(key).or_insert(default);
        }
        value
    }
    pub fn is_read_only(&self) -> bool {
        if std::env::var("OPENCLAW_CONFIG_READONLY").map(|v| v == "1").unwrap_or(false)
            || std::env::var("OPENCLAW_NIX_MODE").map(|v| v == "1").unwrap_or(false)
        {
            return true;
        }
        self.load()
            .get("readOnly")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub(crate) fn persist(&self, value: &Map<String, Value>) -> io::Result<()> {
        if self.is_read_only() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Config is externally managed or marked readOnly; mutations are not allowed",
            ));
        }
        if let Some(parent) = self.settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut raw =
            serde_json::to_string_pretty(&Value::Object(value.clone())).map_err(io::Error::other)?;
        raw.push('\n');
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = PathBuf::from(format!(
            "{}.{}.{}.tmp",
            self.settings_path.display(),
            process::id(),
            nonce
        ));
        fs::write(&temp, raw)?;
        if let Err(error) = fs::rename(&temp, &self.settings_path) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn notify_changes(&self, fields: Vec<String>) {
        if fields.is_empty() {
            return;
        }
        let listeners = self
            .change_listeners
            .lock()
            .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for listener in listeners {
            listener(fields.clone());
        }
    }
    pub(crate) fn set_json_field(
        &self,
        field: &str,
        value: Option<Value>,
    ) -> Result<bool, String> {
        if self.is_read_only() {
            return Err("Config is externally managed or marked readOnly; mutations are not allowed".into());
        }
        let _guard = self.write_lock.lock().unwrap();
        let mut settings = self.load();
        let changed = match value {
            Some(value) => {
                if settings.get(field) == Some(&value) {
                    false
                } else {
                    settings.insert(field.to_string(), value);
                    true
                }
            }
            None => settings.remove(field).is_some(),
        };
        if !changed {
            return Ok(false);
        }
        self.persist(&settings).map_err(|error| error.to_string())?;
        self.notify_changes(vec![field.to_string()]);
        Ok(true)
    }

    pub fn get_detected_user_time_zone(&self) -> Option<String> {
        self.load()
            .get("userTimeZone")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }
    pub fn get_user_time_zone_override(&self) -> Option<String> {
        self.load()
            .get("userTimeZoneOverride")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }
    pub fn get_user_time_zone(&self) -> Option<String> {
        self.get_user_time_zone_override()
            .or_else(|| self.get_detected_user_time_zone())
    }
    pub fn set_user_time_zone(&self, value: Option<&str>) -> Result<bool, String> {
        self.set_zone_field("userTimeZone", value)
    }
    pub fn set_user_time_zone_override(
        &self,
        value: Option<&str>,
    ) -> Result<bool, String> {
        self.set_zone_field("userTimeZoneOverride", value)
    }
    fn set_zone_field(
        &self,
        field: &str,
        value: Option<&str>,
    ) -> Result<bool, String> {
        let trimmed = value.map(str::trim).filter(|value| !value.is_empty());
        if let Some(zone) = trimmed {
            if !is_valid_iana_time_zone(zone) {
                return Ok(false);
            }
        }
        let before = self.get_user_time_zone();
        let mut settings = self.load();
        let changed = match trimmed {
            Some(zone) => {
                if settings.get(field).and_then(Value::as_str) == Some(zone) {
                    false
                } else {
                    settings.insert(field.into(), Value::String(zone.into()));
                    true
                }
            }
            None => settings.remove(field).is_some(),
        };
        if !changed {
            return Ok(false);
        }
        self.persist(&settings).map_err(|error| error.to_string())?;
        let after = self.get_user_time_zone();
        if after != before {
            let listeners = self
                .user_time_zone_listeners
                .lock()
                .map(|listeners| listeners.values().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            for listener in listeners {
                listener(after.clone());
            }
        }
        self.notify_changes(vec![field.to_string()]);
        Ok(true)
    }

    pub fn get_auto_review_instructions(&self) -> SandAutoReviewInstructions {
        let settings = self.load();
        normalize_sand_auto_review_instructions(settings.get("autoReviewInstructions"))
    }
    pub fn set_auto_review_instructions(
        &self,
        value: &SandAutoReviewInstructions,
    ) -> Result<bool, String> {
        let normalized = SandAutoReviewInstructions {
            is_enabled: value.is_enabled,
            allow_instructions: normalize_auto_review_instruction_list(Some(&Value::Array(
                value
                    .allow_instructions
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ))),
            block_instructions: normalize_auto_review_instruction_list(Some(&Value::Array(
                value
                    .block_instructions
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ))),
        };
        if normalized == SandAutoReviewInstructions::default() {
            self.set_json_field("autoReviewInstructions", None)
        } else {
            let next = json!({
                "isEnabled": normalized.is_enabled,
                "allowInstructions": normalized.allow_instructions,
                "blockInstructions": normalized.block_instructions
            });
            self.set_json_field("autoReviewInstructions", Some(next))
        }
    }

    pub fn get_local_tool_permission_choice(&self) -> SandLocalToolPermission {
        normalize_sand_local_tool_permission(
            self.load()
                .get("localToolPermission")
                .and_then(Value::as_str),
        )
    }
    pub fn get_local_tool_permission_ceiling(&self) -> Option<SandLocalToolPermission> {
        self.load()
            .get("localToolPermissionCeiling")
            .and_then(Value::as_str)
            .and_then(SandLocalToolPermission::parse)
    }
    pub fn get_local_tool_permission(&self) -> SandLocalToolPermission {
        resolve_sand_local_tool_permission(
            self.get_local_tool_permission_choice(),
            self.get_local_tool_permission_ceiling(),
        )
    }
    pub fn set_local_tool_permission(
        &self,
        value: SandLocalToolPermission,
    ) -> Result<bool, String> {
        self.set_json_field(
            "localToolPermission",
            Some(Value::String(value.as_str().into())),
        )
    }
    pub fn set_local_tool_permission_ceiling(
        &self,
        value: Option<SandLocalToolPermission>,
    ) -> Result<bool, String> {
        self.set_json_field(
            "localToolPermissionCeiling",
            value.map(|value| Value::String(value.as_str().into())),
        )
    }

    pub fn get_webauthn_proxy_enabled(&self) -> bool {
        self.load()
            .get("webauthnProxyEnabled")
            .and_then(Value::as_bool)
            != Some(false)
    }
    pub fn set_webauthn_proxy_enabled(&self, value: bool) -> Result<bool, String> {
        self.set_json_field("webauthnProxyEnabled", Some(Value::Bool(value)))
    }

    pub fn get_agent_default_model(&self) -> Option<Value> {
        self.load()
            .get("agentDefaultModel")
            .filter(|value| is_model_selection(value))
            .cloned()
            .map(force_model_max_mode)
    }
    pub fn set_agent_default_model(
        &self,
        value: Option<&Value>,
    ) -> Result<bool, String> {
        if value.is_some_and(|value| !is_model_selection(value)) {
            return Ok(false);
        }
        self.set_json_field(
            "agentDefaultModel",
            value.cloned().map(force_model_max_mode),
        )
    }
    pub fn get_computer_use_model(&self) -> Option<Value> {
        self.load()
            .get("computerUseModel")
            .filter(|value| is_model_selection(value))
            .cloned()
    }
    pub fn set_computer_use_model(
        &self,
        value: Option<&Value>,
    ) -> Result<bool, String> {
        if value.is_some_and(|value| !is_model_selection(value)) {
            return Ok(false);
        }
        self.set_json_field("computerUseModel", value.cloned())
    }

    pub fn get_mcp_box_servers(&self) -> Vec<String> {
        unique_strings(self.load().get("mcpBoxServers"))
    }
    pub fn set_mcp_box_servers(&self, names: &[String]) -> Result<bool, String> {
        self.set_json_field(
            "mcpBoxServers",
            Some(Value::Array(
                dedupe_non_empty(names.iter().cloned())
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            )),
        )
    }
    pub fn get_mcp_custom_instructions(&self) -> BTreeMap<String, String> {
        normalize_legacy_instructions(self.load().get("mcpCustomInstructions"))
    }
    pub fn set_mcp_custom_instructions(
        &self,
        value: BTreeMap<String, String>,
    ) -> Result<bool, String> {
        let normalized = value
            .into_iter()
            .filter_map(|(name, value)| {
                let value = clamp_mcp_custom_instruction(&value);
                if value.trim().is_empty() && default_mcp_custom_instruction(&name).is_empty() {
                    None
                } else {
                    Some((name, Value::String(value)))
                }
            })
            .collect::<Map<String, Value>>();
        self.set_json_field(
            "mcpCustomInstructions",
            Some(Value::Object(normalized)),
        )
    }

    pub fn get_pinned_agent_ids(&self) -> Option<Vec<String>> {
        let settings = self.load();
        settings
            .get("pinnedAgentIds")
            .and_then(Value::as_array)
            .map(|_| unique_strings(settings.get("pinnedAgentIds")))
    }
    pub fn set_pinned_agent_ids(&self, ids: &[String]) -> Result<bool, String> {
        self.set_json_field(
            "pinnedAgentIds",
            Some(Value::Array(
                dedupe_non_empty(ids.iter().cloned())
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            )),
        )
    }
    pub fn get_sidebar_sections(&self) -> Option<Vec<Value>> {
        let settings = self.load();
        let raw = settings.get("sidebarSections")?.as_array()?;
        Some(normalize_sidebar_sections(raw, None))
    }
    pub fn set_sidebar_sections(&self, sections: &[Value]) -> Result<bool, String> {
        let stored = self.get_sidebar_sections();
        self.set_json_field(
            "sidebarSections",
            Some(Value::Array(normalize_sidebar_sections(
                sections,
                stored.as_deref(),
            ))),
        )
    }
    pub fn get_has_seen_onboarding(&self) -> Option<bool> {
        self.load().get("hasSeenOnboarding").and_then(Value::as_bool)
    }
    pub fn set_has_seen_onboarding(&self, value: bool) -> Result<bool, String> {
        let mut settings = self.load();
        let scope = settings
            .get("mcpCustomInstructionsAccountScope")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut changed =
            settings.get("hasSeenOnboarding").and_then(Value::as_bool) != Some(value);
        settings.insert("hasSeenOnboarding".into(), Value::Bool(value));
        settings.remove("hasSeenOnboardingAccountScope");
        if let Some(scope) = scope {
            settings.insert(
                "hasSeenOnboardingAccountScope".into(),
                Value::String(scope),
            );
            changed = true;
        }
        if !changed {
            return Ok(false);
        }
        self.persist(&settings).map_err(|error| error.to_string())?;
        self.notify_changes(vec!["hasSeenOnboarding".into()]);
        Ok(true)
    }

    pub fn get_inference_provider(&self) -> String {
        self.load()
            .get("inferenceProvider")
            .and_then(Value::as_str)
            .filter(|provider| INFERENCE_PROVIDERS.contains(provider))
            .unwrap_or("cursor")
            .to_string()
    }
    pub fn set_inference_provider(&self, value: &str) -> Result<bool, String> {
        if !INFERENCE_PROVIDERS.contains(&value) {
            return Ok(false);
        }
        self.set_json_field(
            "inferenceProvider",
            Some(Value::String(value.into())),
        )
    }
    pub fn get_inference_router_usage(&self) -> Value {
        normalize_inference_usage(self.load().get("inferenceRouterUsage"))
    }
    pub fn record_inference_usage(
        &self,
        provider: &str,
        input_tokens: Option<f64>,
        output_tokens: Option<f64>,
        cache_read_tokens: Option<f64>,
        cache_write_tokens: Option<f64>,
    ) -> Result<bool, String> {
        if !INFERENCE_PROVIDERS.contains(&provider) {
            return Ok(false);
        }
        let mut usage = self.get_inference_router_usage();
        let entry = usage
            .get_mut("providers")
            .and_then(Value::as_object_mut)
            .and_then(|providers| providers.get_mut(provider))
            .and_then(Value::as_object_mut)
            .expect("normalized inference usage provider");
        increment_counter(entry, "requests", 1);
        increment_counter(entry, "inputTokens", safe_usage(input_tokens));
        increment_counter(entry, "outputTokens", safe_usage(output_tokens));
        increment_counter(entry, "cacheReadTokens", safe_usage(cache_read_tokens));
        increment_counter(entry, "cacheWriteTokens", safe_usage(cache_write_tokens));
        entry.insert(
            "lastUsedAt".into(),
            Value::String(
                Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            ),
        );
        self.set_json_field("inferenceRouterUsage", Some(usage))
    }

    pub fn get_notification_config(&self) -> Value {
        let mut settings = self.load();
        let stored = json!({ "isEnabled": false });
        if settings.get("notifications") != Some(&stored) {
            settings.insert("notifications".into(), stored);
            let _ = self.persist(&settings);
        }
        disabled_notification_config()
    }
    pub fn set_notification_config(&self, _input: &Value) -> Result<bool, String> {
        self.set_json_field(
            "notifications",
            Some(json!({ "isEnabled": false })),
        )
    }

    pub fn subscribe_to_user_time_zone(
        &self,
        listener: UserTimeZoneListener,
    ) -> SettingsSubscription<dyn Fn(Option<String>) + Send + Sync + 'static> {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.user_time_zone_listeners
            .lock()
            .expect("settings timezone listeners poisoned")
            .insert(id, listener);
        SettingsSubscription {
            id,
            listeners: Arc::clone(&self.user_time_zone_listeners),
            active: true,
        }
    }
    pub fn subscribe_to_changes(
        &self,
        listener: SettingsChangeListener,
    ) -> SettingsSubscription<dyn Fn(Vec<String>) + Send + Sync + 'static> {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.change_listeners
            .lock()
            .expect("settings change listeners poisoned")
            .insert(id, listener);
        SettingsSubscription {
            id,
            listeners: Arc::clone(&self.change_listeners),
            active: true,
        }
    }
    pub fn subscribe_to_feature_flag_overrides(
        &self,
        listener: FeatureFlagOverrideListener,
    ) -> SettingsSubscription<dyn Fn(BTreeMap<String, bool>) + Send + Sync + 'static> {
        let id = self.next_listener_id.fetch_add(1, Ordering::Relaxed);
        self.feature_flag_override_listeners
            .lock()
            .expect("settings feature override listeners poisoned")
            .insert(id, listener);
        SettingsSubscription {
            id,
            listeners: Arc::clone(&self.feature_flag_override_listeners),
            active: true,
        }
    }
}
