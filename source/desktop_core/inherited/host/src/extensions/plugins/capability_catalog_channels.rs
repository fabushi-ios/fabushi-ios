use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use crate::extensions::plugin_sdk::channel_entry::{
    BundledChannelEntryContract, PluginRegistrationMode,
};
use super::capability_catalog::*;
use super::capability_catalog_core::*;
use super::capability_catalog_providers::*;
use super::capability_catalog_validation::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelCatalogCandidate {
    pub id_hint: Option<String>,
    pub bundled_manifest_id: Option<String>,
    pub package_plugin_id: Option<String>,
    pub package_name: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub origin: PluginOrigin,
    pub root_dir: String,
    pub is_trusted_official: bool,
}

pub struct ChannelCatalogResolver;
impl ChannelCatalogResolver {
    pub fn resolve_channel_catalog_entries(
        candidates: &[ChannelCatalogCandidate],
        origin_filter: Option<&PluginOrigin>,
    ) -> Vec<ChannelCatalogEntry> {
        candidates
            .iter()
            .filter_map(|c| {
                if let Some(req_origin) = origin_filter {
                    if &c.origin != req_origin {
                        return None;
                    }
                }
                let ch_id = c.channel_id.as_deref()?.trim();
                if ch_id.is_empty() {
                    return None;
                }
                let plugin_id = c
                    .bundled_manifest_id
                    .as_deref()
                    .or(c.package_plugin_id.as_deref())
                    .or(c.id_hint.as_deref())?
                    .trim();
                if plugin_id.is_empty() {
                    return None;
                }

                Some(ChannelCatalogEntry {
                    plugin_id: plugin_id.to_string(),
                    origin: c.origin.clone(),
                    package_name: c.package_name.clone(),
                    channel_name: ch_id.to_string(),
                    trusted_official_install: c.is_trusted_official,
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Channel Plugin Metadata Validation & Normalization
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPluginMeta {
    pub id: String,
    pub label: String,
    pub selection_label: String,
    pub docs_path: String,
    pub blurb: String,
    pub aliases: Vec<String>,
}

impl Default for ChannelPluginMeta {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            selection_label: String::new(),
            docs_path: String::new(),
            blurb: String::new(),
            aliases: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPluginConfigHandlers {
    pub has_list_accounts: bool,
    pub has_resolve_account: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPluginDefinition {
    pub id: String,
    pub meta: ChannelPluginMeta,
    pub config: Option<ChannelPluginConfigHandlers>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelValidationDiagnostic {
    pub level: String, // "warn" or "error"
    pub plugin_id: String,
    pub message: String,
}

pub struct ChannelPluginValidator;
impl ChannelPluginValidator {
    pub fn validate_and_normalize(
        registered_id: &str,
        mut meta: ChannelPluginMeta,
        config: Option<&ChannelPluginConfigHandlers>,
        plugin_id: &str,
    ) -> Result<(ChannelPluginDefinition, Vec<ChannelValidationDiagnostic>), ChannelValidationDiagnostic> {
        let mut diagnostics = Vec::new();

        // 1. Check required config helpers
        match config {
            Some(c) if c.has_list_accounts && c.has_resolve_account => {}
            _ => {
                return Err(ChannelValidationDiagnostic {
                    level: "error".to_string(),
                    plugin_id: plugin_id.to_string(),
                    message: format!(
                        "channel \"{}\" registration missing required config helpers",
                        registered_id
                    ),
                });
            }
        }

        // 2. Mismatched meta.id
        if !meta.id.is_empty() && meta.id != registered_id {
            diagnostics.push(ChannelValidationDiagnostic {
                level: "warn".to_string(),
                plugin_id: plugin_id.to_string(),
                message: format!(
                    "channel \"{}\" meta.id mismatch (\"{}\"); using registered channel id",
                    registered_id, meta.id
                ),
            });
            meta.id = registered_id.to_string();
        } else if meta.id.is_empty() {
            meta.id = registered_id.to_string();
        }

        // 3. Incomplete metadata filling
        let mut missing_fields = Vec::new();
        if meta.label.is_empty() {
            meta.label = registered_id.to_string();
            missing_fields.push("label");
        }
        if meta.selection_label.is_empty() {
            meta.selection_label = registered_id.to_string();
            missing_fields.push("selectionLabel");
        }
        if meta.docs_path.is_empty() {
            meta.docs_path = format!("/channels/{}", registered_id);
            missing_fields.push("docsPath");
        }
        if meta.blurb.is_empty() {
            if registered_id == "telegram" {
                meta.blurb = "Telegram bot channel".to_string();
            }
            missing_fields.push("blurb");
        }
        if !missing_fields.is_empty() {
            diagnostics.push(ChannelValidationDiagnostic {
                level: "warn".to_string(),
                plugin_id: plugin_id.to_string(),
                message: format!(
                    "channel \"{}\" registered incomplete metadata; filled missing {}",
                    registered_id,
                    missing_fields.join(", ")
                ),
            });
        }

        // 4. Fill official external channel aliases if omitted
        if registered_id == "openclaw-weixin" && meta.aliases.is_empty() {
            meta.aliases = vec!["weixin".to_string(), "wechat".to_string(), "微信".to_string()];
        }

        Ok((
            ChannelPluginDefinition {
                id: registered_id.to_string(),
                meta,
                config: config.cloned(),
                capabilities: vec!["direct".to_string()],
            },
            diagnostics,
        ))
    }
}

// ---------------------------------------------------------------------------
// Channel Registry State & Adoption
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivePluginChannelRegistration {
    pub plugin_id: String,
    pub channel_id: String,
    pub origin: PluginOrigin,
    pub source_digest: String,
    pub accepting_calls: bool,
    pub borrowed_runtime_record: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeChannelSource {
    pub registry_id: String,
    pub is_current: bool,
}

pub struct ChannelRegistryAdoption;
impl ChannelRegistryAdoption {
    pub fn adopt_runtime_channel_registrations(
        target_registry_id: &str,
        target_channels: &[ActivePluginChannelRegistration],
        source: Option<&RuntimeChannelSource>,
        target_env_digest: &str,
        donor_env_digest: &str,
        target_gateway_resolver_id: &str,
        donor_gateway_resolver_id: &str,
        target_retired: bool,
    ) -> Result<Vec<ActivePluginChannelRegistration>, String> {
        let Some(src) = source else {
            return Ok(target_channels.to_vec());
        };
        if target_registry_id == src.registry_id || target_channels.is_empty() {
            return Ok(target_channels.to_vec());
        }
        if !src.is_current {
            return Err("Channel runtime owner changed during prepared registry admission".to_string());
        }
        if target_env_digest != donor_env_digest
            || target_gateway_resolver_id != donor_gateway_resolver_id
            || target_retired
        {
            return Ok(target_channels.to_vec());
        }

        let adopted = target_channels
            .iter()
            .map(|ch| {
                if ch.accepting_calls {
                    let mut cloned = ch.clone();
                    cloned.borrowed_runtime_record = Some(format!("{}:borrowed", ch.plugin_id));
                    cloned
                } else {
                    ch.clone()
                }
            })
            .collect();

        Ok(adopted)
    }
}

// ---------------------------------------------------------------------------
// Channel Startup Policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedPluginConfigEntry {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NormalizedPluginsConfig {
    pub enabled: bool,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub entries: HashMap<String, NormalizedPluginConfigEntry>,
}

pub struct ChannelStartupPolicy;
impl ChannelStartupPolicy {
    pub fn can_start_configured_channel_plugin(
        plugin_id: &str,
        origin: &PluginOrigin,
        channel_ids: &[String],
        plugins_config: &NormalizedPluginsConfig,
        explicitly_configured_channels: &[String],
    ) -> bool {
        // 1. Blocked check
        let in_deny = plugins_config.deny.iter().any(|d| d.eq_ignore_ascii_case(plugin_id));
        if in_deny || !plugins_config.enabled {
            return false;
        }

        let not_in_allowlist = !plugins_config.allow.is_empty()
            && !plugins_config.allow.iter().any(|a| a.eq_ignore_ascii_case(plugin_id));

        if not_in_allowlist {
            let has_explicit_channel = channel_ids.iter().any(|ch| {
                explicitly_configured_channels.iter().any(|c| c.eq_ignore_ascii_case(ch))
            });
            if origin != &PluginOrigin::Bundled || !has_explicit_channel {
                return false;
            }
        }

        if origin == &PluginOrigin::Bundled {
            return true;
        }

        // For non-bundled plugins: requires explicit activation
        if let Some(entry) = plugins_config.entries.get(plugin_id) {
            entry.enabled
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Channel Presence Policy & ID Resolution
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelPresenceSource {
    ExplicitConfig,
    Env,
    ManifestEnv,
    PersistedAuth,
    Cli,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredChannelPresenceEntry {
    pub channel_id: String,
    pub sources: Vec<ChannelPresenceSource>,
    pub effective: bool,
    pub plugin_ids: Vec<String>,
    pub blocked_reasons: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ChannelManifestRecord {
    pub plugin_id: String,
    pub origin: PluginOrigin,
    pub channels: Vec<String>,
    pub enabled_by_default: bool,
    pub required_env: Vec<String>,
}

pub struct ChannelPresencePolicy;
impl ChannelPresencePolicy {
    pub fn has_explicit_channel_config(
        configured_channels: &HashMap<String, serde_json::Value>,
        channel_id: &str,
    ) -> bool {
        let channel_id_lower = channel_id.to_ascii_lowercase();
        for (k, v) in configured_channels {
            if k.to_ascii_lowercase() == channel_id_lower {
                if let Some(obj) = v.as_object() {
                    if let Some(enabled) = obj.get("enabled").and_then(|e| e.as_bool()) {
                        if !enabled {
                            return false;
                        }
                    }
                    return true;
                }
            }
        }
        false
    }

    pub fn list_explicit_configured_channel_ids(
        configured_channels: &HashMap<String, serde_json::Value>,
    ) -> Vec<String> {
        let mut ids = Vec::new();
        for (k, v) in configured_channels {
            let trimmed = k.trim();
            if trimmed.is_empty() || trimmed.starts_with('_') {
                continue;
            }
            if Self::has_explicit_channel_config(configured_channels, trimmed) {
                ids.push(trimmed.to_ascii_lowercase());
            }
        }
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn resolve_channel_presence_policy(
        configured_channels: &HashMap<String, serde_json::Value>,
        env_vars: &HashMap<String, String>,
        manifests: &[ChannelManifestRecord],
        plugins_config: &NormalizedPluginsConfig,
        suppress_ambient_env: bool,
    ) -> Vec<ConfiguredChannelPresenceEntry> {
        let explicit_ids = Self::list_explicit_configured_channel_ids(configured_channels);
        let mut channel_sources: HashMap<String, HashSet<ChannelPresenceSource>> = HashMap::new();

        for ch in &explicit_ids {
            channel_sources
                .entry(ch.clone())
                .or_default()
                .insert(ChannelPresenceSource::ExplicitConfig);
        }

        // Check manifests for env-based presence
        for manifest in manifests {
            for ch in &manifest.channels {
                let ch_lower = ch.to_ascii_lowercase();
                if !manifest.required_env.is_empty()
                    && manifest.required_env.iter().all(|var| env_vars.contains_key(var))
                {
                    channel_sources
                        .entry(ch_lower.clone())
                        .or_default()
                        .insert(ChannelPresenceSource::ManifestEnv);
                }
            }
        }

        // Check for explicitly disabled channels
        let mut disabled_channels: HashSet<String> = HashSet::new();
        for (k, v) in configured_channels {
            if let Some(obj) = v.as_object() {
                if obj.get("enabled").and_then(|e| e.as_bool()) == Some(false) {
                    disabled_channels.insert(k.to_ascii_lowercase());
                }
            }
        }
        for dis in &disabled_channels {
            channel_sources.remove(dis);
        }

        // Suppress ambient-only if requested
        if suppress_ambient_env {
            channel_sources.retain(|_, sources| {
                !sources.iter().all(|s| *s == ChannelPresenceSource::Env || *s == ChannelPresenceSource::ManifestEnv)
            });
        }

        let mut sorted_channels: Vec<String> = channel_sources.keys().cloned().collect();
        sorted_channels.sort();

        let mut entries = Vec::new();
        for ch in sorted_channels {
            let sources = channel_sources.get(&ch).cloned().unwrap_or_default();
            let owning_manifests: Vec<&ChannelManifestRecord> = manifests
                .iter()
                .filter(|m| m.channels.iter().any(|c| c.eq_ignore_ascii_case(&ch)))
                .collect();

            let mut effective_plugin_ids = Vec::new();
            let mut blocked_reasons = Vec::new();

            if owning_manifests.is_empty() {
                blocked_reasons.push("no-channel-owner".to_string());
            } else {
                for m in owning_manifests {
                    let can_start = ChannelStartupPolicy::can_start_configured_channel_plugin(
                        &m.plugin_id,
                        &m.origin,
                        &m.channels,
                        plugins_config,
                        &explicit_ids,
                    );
                    if can_start {
                        effective_plugin_ids.push(m.plugin_id.clone());
                    } else if plugins_config.deny.iter().any(|d| d.eq_ignore_ascii_case(&m.plugin_id)) {
                        blocked_reasons.push("blocked-by-denylist".to_string());
                    } else if !plugins_config.enabled {
                        blocked_reasons.push("plugins-disabled".to_string());
                    } else {
                        blocked_reasons.push("not-in-allowlist".to_string());
                    }
                }
            }

            let effective = !effective_plugin_ids.is_empty();
            let mut sources_vec: Vec<ChannelPresenceSource> = sources.into_iter().collect();
            sources_vec.sort_by_key(|s| format!("{:?}", s));
            effective_plugin_ids.sort();
            effective_plugin_ids.dedup();
            blocked_reasons.sort();
            blocked_reasons.dedup();

            entries.push(ConfiguredChannelPresenceEntry {
                channel_id: ch,
                sources: sources_vec,
                effective,
                plugin_ids: effective_plugin_ids,
                blocked_reasons,
            });
        }

        entries
    }

    pub fn list_configured_announce_channel_ids(
        configured_channels: &HashMap<String, serde_json::Value>,
        env_vars: &HashMap<String, String>,
        manifests: &[ChannelManifestRecord],
        plugins_config: &NormalizedPluginsConfig,
    ) -> Vec<String> {
        let policy = Self::resolve_channel_presence_policy(
            configured_channels,
            env_vars,
            manifests,
            plugins_config,
            false,
        );
        let mut announced = Vec::new();
        for entry in policy {
            if entry.effective {
                announced.push(entry.channel_id);
            }
        }
        announced.sort();
        announced.dedup();
        announced
    }

    pub fn resolve_configured_channel_plugin_ids(
        configured_channels: &HashMap<String, serde_json::Value>,
        env_vars: &HashMap<String, String>,
        manifests: &[ChannelManifestRecord],
        plugins_config: &NormalizedPluginsConfig,
    ) -> Vec<String> {
        if !plugins_config.enabled {
            return Vec::new();
        }
        let policy = Self::resolve_channel_presence_policy(
            configured_channels,
            env_vars,
            manifests,
            plugins_config,
            false,
        );
        let mut plugin_ids = Vec::new();
        for entry in policy {
            if entry.effective {
                plugin_ids.extend(entry.plugin_ids);
            }
        }
        plugin_ids.sort();
        plugin_ids.dedup();
        plugin_ids
    }
}






// (from src/plugins/bundled-channel-config-metadata.ts, bundled-compat.ts, bundled-dir.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledChannelMetadata {
    pub channel_id: String,
    pub stream_mode: Option<String>,
    pub reply_to_mode: Option<String>,
    pub history_limit: Option<u32>,
    pub media_max_mb: Option<u32>,
    pub provider_skills: Vec<String>,
}

pub struct BundledChannelMetadataValidator;

impl BundledChannelMetadataValidator {
    pub fn validate(meta: &BundledChannelMetadata) -> Result<(), String> {
        if meta.channel_id.is_empty() {
            return Err("Channel ID cannot be empty".to_string());
        }
        if let Some(limit) = meta.history_limit {
            if limit == 0 || limit > 100_000 {
                return Err(format!("history_limit {} out of valid range (1..=100000)", limit));
            }
        }
        if let Some(max_mb) = meta.media_max_mb {
            if max_mb == 0 || max_mb > 2048 {
                return Err(format!("media_max_mb {} out of valid range (1..=2048)", max_mb));
            }
        }
        Ok(())
    }
}

