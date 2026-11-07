use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatus {
    Active,
    Disabled,
    Error(String),
    Uninstalled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginUninstallReceipt {
    pub plugin_id: String,
    pub uninstalled_at_ms: u64,
    pub files_removed: usize,
    pub retained_storage_bytes: u64,
}

#[derive(Clone, Default)]
pub struct PluginRuntimeManager {
    statuses: Arc<Mutex<HashMap<String, PluginStatus>>>,
    uninstall_receipts: Arc<Mutex<HashMap<String, PluginUninstallReceipt>>>,
}

impl PluginRuntimeManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_status(&self, plugin_id: &str, status: PluginStatus) {
        let mut statuses = self.statuses.lock().unwrap();
        statuses.insert(plugin_id.to_string(), status);
    }

    pub fn get_status(&self, plugin_id: &str) -> PluginStatus {
        let statuses = self.statuses.lock().unwrap();
        statuses
            .get(plugin_id)
            .cloned()
            .unwrap_or(PluginStatus::Disabled)
    }

    pub fn uninstall_plugin(&self, plugin_id: &str, files_removed: usize) -> PluginUninstallReceipt {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let receipt = PluginUninstallReceipt {
            plugin_id: plugin_id.to_string(),
            uninstalled_at_ms: now_ms,
            files_removed,
            retained_storage_bytes: 0,
        };

        let mut statuses = self.statuses.lock().unwrap();
        statuses.insert(plugin_id.to_string(), PluginStatus::Uninstalled);

        let mut receipts = self.uninstall_receipts.lock().unwrap();
        receipts.insert(plugin_id.to_string(), receipt.clone());

        receipt
    }

    pub fn get_uninstall_receipt(&self, plugin_id: &str) -> Option<PluginUninstallReceipt> {
        let receipts = self.uninstall_receipts.lock().unwrap();
        receipts.get(plugin_id).cloned()
    }
}

// Runtime Import Side-Effects & Seam Guards (from runtime-import-side-effects and runtime-seams contracts)

pub struct RuntimeImportSideEffectsGuard;

impl RuntimeImportSideEffectsGuard {
    pub fn assert_no_direct_channel_registry_imports(sources: &[(&str, &str)]) -> Result<(), Vec<String>> {
        let mut offenders = Vec::new();
        for (name, content) in sources {
            if content.contains("../../channels/plugins/registry")
                || content.contains("../channels/plugins/registry")
            {
                offenders.push(name.to_string());
            }
        }
        if offenders.is_empty() {
            Ok(())
        } else {
            offenders.sort();
            Err(offenders)
        }
    }

    pub fn resolve_markdown_table_mode(
        channel_id: &str,
        supports_block_tables: bool,
        configured_mode: Option<&str>,
    ) -> &'static str {
        if let Some(mode) = configured_mode {
            match mode {
                "block" => return "block",
                "bullets" => return "bullets",
                _ => {}
            }
        }
        if channel_id == "telegram" {
            "bullets"
        } else if supports_block_tables {
            "block"
        } else {
            "bullets"
        }
    }
}

pub struct RuntimeFacadeSeamManager;

impl RuntimeFacadeSeamManager {
    pub fn resolve_bundled_plugin_public_surface_access(
        plugin_id: &str,
        enabled: bool,
    ) -> bool {
        let _ = plugin_id;
        enabled
    }

    pub fn fetch_with_ssrf_guard(
        url: &str,
        is_private_ip: bool,
    ) -> Result<String, String> {
        if is_private_ip {
            return Err(format!("SSRF blocked access to private network: {}", url));
        }
        if !url.starts_with("https://") && !url.starts_with("http://") {
            return Err(format!("Unsupported protocol for guarded fetch: {}", url));
        }
        Ok(format!("fetched:{}", url))
    }
}

// ---------------------------------------------------------------------------
// Guarded Plugin API Lifecycle & Late-Callable Methods (from src/plugins/api-lifecycle.ts & api-builder.ts)
// ---------------------------------------------------------------------------

pub struct GuardedPluginApiLifecycle;

impl GuardedPluginApiLifecycle {
    pub const LATE_CALLABLE_METHODS: &'static [&'static str] = &[
        "clearRunContext",
        "emitAgentEvent",
        "enqueueNextTurnInjection",
        "steerActiveRun",
        "getRunContext",
        "sendSessionAttachment",
        "scheduleSessionTurn",
        "setRunContext",
        "unscheduleSessionTurnsByTag",
    ];

    pub fn is_late_callable_method(method: &str) -> bool {
        Self::LATE_CALLABLE_METHODS.contains(&method)
    }

    pub fn assert_method_callable_after_activation(method: &str) -> Result<(), String> {
        if Self::is_late_callable_method(method) {
            Ok(())
        } else {
            Err(format!(
                "Method '{}' cannot be invoked after plugin activation phase has closed",
                method
            ))
        }
    }

    pub fn assert_workspace_generation_match(
        caller_generation: &str,
        owner_generation: &str,
        plugin_id: &str,
    ) -> Result<(), String> {
        if caller_generation == owner_generation {
            Ok(())
        } else {
            Err(format!(
                "Plugin {} API registration does not belong to the calling workspace generation",
                plugin_id
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin Auth Profile Reference Rewriter (from src/plugins/auth-profile-config-refs.ts)
// ---------------------------------------------------------------------------

pub struct PluginAuthProfileRewriter;

impl PluginAuthProfileRewriter {
    pub fn rewrite_auth_profile_refs(
        value: &mut serde_json::Value,
        id_map: &HashMap<String, String>,
    ) -> bool {
        let mut changed = false;
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map.iter_mut() {
                    if (k == "authProfileId" || k == "defaultAuthProfileId") && v.is_string() {
                        let current = v.as_str().unwrap().trim();
                        if let Some(replacement) = id_map.get(current) {
                            if replacement != current {
                                *v = serde_json::Value::String(replacement.clone());
                                changed = true;
                            }
                        }
                    } else {
                        if Self::rewrite_auth_profile_refs(v, id_map) {
                            changed = true;
                        }
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr.iter_mut() {
                    if Self::rewrite_auth_profile_refs(item, id_map) {
                        changed = true;
                    }
                }
            }
            _ => {}
        }
        changed
    }
}

// ---------------------------------------------------------------------------
// Board Widget Content Kind Validation (from src/plugins/board-widget-content-kinds.ts)
// ---------------------------------------------------------------------------

pub struct BoardWidgetContentKindValidator;

impl BoardWidgetContentKindValidator {
    pub const RESERVED_CONTENT_KINDS: &'static [&'static str] = &["html", "mcp-app", "plugin"];

    pub fn validate(
        kind: &str,
        label: &str,
        surface: &str,
        paths: &[String],
    ) -> Result<(), String> {
        // 1. Kind validation: 1-32 chars, starts with lowercase letter, lowercase alphanumeric or hyphen
        if kind.is_empty()
            || kind.len() > 32
            || !kind.chars().next().unwrap().is_ascii_lowercase()
            || !kind.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!("kind '{}' is invalid", kind));
        }
        if Self::RESERVED_CONTENT_KINDS.contains(&kind) {
            return Err(format!("kind '{}' is reserved", kind));
        }

        // 2. Label: 1-80 chars
        if label.is_empty() || label.len() > 80 {
            return Err("label must contain 1-80 characters".to_string());
        }

        // 3. Surface: 1-64 chars, starts with lowercase letter, alphanumeric or . _ -
        if surface.is_empty()
            || surface.len() > 64
            || !surface.chars().next().unwrap().is_ascii_lowercase()
            || !surface.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-')
        {
            return Err(format!("resource surface '{}' is invalid", surface));
        }

        // 4. Paths: 1-8 items, each must start with "/" and not with "//" or "/\"
        if paths.is_empty() || paths.len() > 8 {
            return Err("paths must contain between 1 and 8 items".to_string());
        }
        for p in paths {
            if !p.starts_with('/') || p.starts_with("//") || p.starts_with("/\\") {
                return Err(format!("invalid gateway local resource path: '{}'", p));
            }
        }

        Ok(())
    }
}


