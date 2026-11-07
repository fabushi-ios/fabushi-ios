use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use crate::extensions::plugin_sdk::channel_entry::{
    BundledChannelEntryContract, PluginRegistrationMode,
};
use super::capability_catalog::*;
use super::capability_catalog_channels::*;
use super::capability_catalog_core::*;
use super::capability_catalog_providers::*;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BundledPluginContractSnapshot {
    pub plugin_id: String,
    pub cli_backend_ids: Vec<String>,
    pub provider_ids: Vec<String>,
    pub provider_env_vars: std::collections::HashMap<String, Vec<String>>,
    pub worker_provider_ids: Vec<String>,
    pub storage_provider_ids: Vec<String>,
    pub embedding_provider_ids: Vec<String>,
    pub speech_provider_ids: Vec<String>,
    pub realtime_transcription_provider_ids: Vec<String>,
    pub realtime_voice_provider_ids: Vec<String>,
    pub media_understanding_provider_ids: Vec<String>,
    pub transcript_source_provider_ids: Vec<String>,
    pub document_extractor_ids: Vec<String>,
    pub image_generation_provider_ids: Vec<String>,
    pub video_generation_provider_ids: Vec<String>,
    pub music_generation_provider_ids: Vec<String>,
    pub web_content_extractor_ids: Vec<String>,
    pub web_fetch_provider_ids: Vec<String>,
    pub web_search_provider_ids: Vec<String>,
    pub migration_provider_ids: Vec<String>,
    pub tool_names: Vec<String>,
}

impl BundledPluginContractSnapshot {
    pub fn has_capabilities(&self) -> bool {
        !self.cli_backend_ids.is_empty()
            || !self.provider_ids.is_empty()
            || !self.worker_provider_ids.is_empty()
            || !self.storage_provider_ids.is_empty()
            || !self.embedding_provider_ids.is_empty()
            || !self.speech_provider_ids.is_empty()
            || !self.realtime_transcription_provider_ids.is_empty()
            || !self.realtime_voice_provider_ids.is_empty()
            || !self.media_understanding_provider_ids.is_empty()
            || !self.transcript_source_provider_ids.is_empty()
            || !self.document_extractor_ids.is_empty()
            || !self.image_generation_provider_ids.is_empty()
            || !self.video_generation_provider_ids.is_empty()
            || !self.music_generation_provider_ids.is_empty()
            || !self.web_content_extractor_ids.is_empty()
            || !self.web_fetch_provider_ids.is_empty()
            || !self.web_search_provider_ids.is_empty()
            || !self.migration_provider_ids.is_empty()
            || !self.tool_names.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Bundled Channel Config Metadata, Compat & Dir Resolvers
pub struct BundledCompatResolver;

impl BundledCompatResolver {
    pub fn normalize_plugin_id(id: &str) -> String {
        let trimmed = id.trim();
        match trimmed {
            "legacy-telegram" => "telegram".to_string(),
            "legacy-discord" => "discord".to_string(),
            "legacy-slack" => "slack".to_string(),
            other => other.to_string(),
        }
    }
}

pub struct BundledDirResolver;

impl BundledDirResolver {
    pub const CANDIDATE_DIRS: &'static [&'static str] = &[
        "bundled/plugins",
        "dist/bundled/plugins",
        "extensions",
    ];

    pub fn resolve_relative_path(base_dir: &str, relative_path: &str) -> Result<String, String> {
        if relative_path.contains("..") {
            return Err(format!(
                "Path traversal detected in bundled directory path: '{}'",
                relative_path
            ));
        }
        let clean_base = base_dir.trim_end_matches('/');
        let clean_rel = relative_path.trim_start_matches('/');
        Ok(format!("{}/{}", clean_base, clean_rel))
    }
}

pub struct BundledPluginScanner;

impl BundledPluginScanner {
    pub const RUNTIME_SIDECAR_ARTIFACTS: &'static [&'static str] = &[
        "helper-api.js",
        "light-runtime-api.js",
        "qa-runner-api.js",
        "runtime-api.js",
        "runtime-setter-api.js",
        "thread-bindings-runtime.js",
    ];

    pub const PUBLIC_SURFACE_SOURCE_EXTENSIONS: &'static [&'static str] = &[
        ".ts", ".mts", ".js", ".mjs", ".cts", ".cjs",
    ];

    pub fn rewrite_bundled_plugin_entry_to_built_path(entry: &str) -> String {
        let stripped = entry.strip_prefix("./").or_else(|| entry.strip_prefix(".\\")).unwrap_or(entry);
        if let Some(dot_idx) = stripped.rfind('.') {
            format!("{}.js", &stripped[..dot_idx])
        } else {
            format!("{}.js", stripped)
        }
    }

    pub fn derive_bundled_plugin_id_hint(
        entry_path: &str,
        manifest_id: &str,
        package_name: Option<&str>,
        has_multiple_extensions: bool,
    ) -> String {
        if !has_multiple_extensions {
            return manifest_id.to_string();
        }
        let base = std::path::Path::new(entry_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(entry_path);

        match package_name.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(pkg) => {
                let unscoped = pkg.rsplit('/').next().unwrap_or(pkg);
                format!("{}/{}", unscoped, base)
            }
            None => format!("{}/{}", manifest_id, base),
        }
    }

    pub fn is_top_level_public_surface_source(name: &str) -> bool {
        let ext = match name.rfind('.') {
            Some(idx) => &name[idx..],
            None => return false,
        };
        if !Self::PUBLIC_SURFACE_SOURCE_EXTENSIONS.contains(&ext) {
            return false;
        }
        if name.starts_with('.') || name.starts_with("test-") || name.contains(".test-") {
            return false;
        }
        if name.ends_with(".d.ts") {
            return false;
        }
        if name.starts_with("config-api.") {
            return false;
        }
        if name.contains(".test.") || name.contains(".spec.") {
            return false;
        }
        true
    }

    pub fn collect_bundled_plugin_runtime_sidecar_artifacts(
        public_surfaces: &[String],
    ) -> Vec<String> {
        let sidecar_set: HashSet<&str> = Self::RUNTIME_SIDECAR_ARTIFACTS.iter().copied().collect();
        public_surfaces
            .iter()
            .filter(|artifact| sidecar_set.contains(artifact.as_str()))
            .cloned()
            .collect()
    }

    pub fn has_bundled_plugin_startup_manifest(
        plugin_id: &str,
        manifest_content: Option<&str>,
    ) -> bool {
        let content = match manifest_content {
            Some(c) => c,
            None => return false,
        };
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(id) = val.get("id").and_then(|v| v.as_str()) {
                return id == plugin_id;
            }
        }
        false
    }
}

pub struct BundledSourceOverlayResolver;

impl BundledSourceOverlayResolver {
    pub fn parse_linux_mount_info_mount_points(mount_info: &str) -> HashSet<String> {
        let mut mount_points = HashSet::new();
        for line in mount_info.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let fields: Vec<&str> = trimmed.split_whitespace().collect();
            if fields.len() > 4 {
                mount_points.insert(fields[4].to_string());
            }
        }
        mount_points
    }

    pub fn source_overlays_disabled(env_val: Option<&str>) -> bool {
        match env_val {
            Some(v) => {
                let lower = v.trim().to_ascii_lowercase();
                lower == "1" || lower == "true"
            }
            None => false,
        }
    }

    pub fn is_bundled_source_overlay_path(
        source_path: &str,
        mount_points: &HashSet<String>,
    ) -> bool {
        mount_points.contains(source_path)
    }

    pub fn list_bundled_source_overlay_dirs(
        bundled_root: &str,
        legacy_root: &str,
        peer_dirs: &[String],
        mount_points: &HashSet<String>,
        env_disabled: bool,
    ) -> Vec<String> {
        if env_disabled || bundled_root.is_empty() || legacy_root.is_empty() {
            return Vec::new();
        }
        let legacy_root_mounted = Self::is_bundled_source_overlay_path(legacy_root, mount_points);
        let mut overlay_dirs = Vec::new();

        for dir in peer_dirs {
            let source_dir = format!("{}/{}", legacy_root.trim_end_matches('/'), dir);
            if legacy_root_mounted || Self::is_bundled_source_overlay_path(&source_dir, mount_points) {
                overlay_dirs.push(source_dir);
            }
        }
        overlay_dirs.sort();
        overlay_dirs
    }
}

pub struct BundledPluginNamingAndArtworkValidator;

impl BundledPluginNamingAndArtworkValidator {
    pub const PLUGIN_CATEGORY_SLUGS: &'static [&'static str] = &[
        "channels",
        "models",
        "agent-runtimes",
        "memory",
        "context",
        "voice",
        "web",
        "computer-use",
        "media",
        "security",
        "integrations",
        "developer-tools",
        "infrastructure",
        "documents-files",
        "inbox-collaboration",
        "productivity",
        "scheduling",
        "finance-payments",
        "sales-marketing",
        "data-analytics",
        "agent-orchestration",
        "research",
        "other",
    ];

    pub const ALLOWED_PACKAGE_SUFFIXES: &'static [&'static str] = &[
        "",
        "-provider",
        "-plugin",
        "-speech",
        "-sandbox",
        "-media-understanding",
    ];

    pub fn validate_bundled_plugin_categories(
        categories: Option<&[String]>,
    ) -> Result<String, String> {
        let cats = categories.ok_or_else(|| "expected exactly one purpose category".to_string())?;
        if cats.len() != 1 {
            return Err("expected exactly one purpose category".to_string());
        }
        let category = &cats[0];
        if !Self::PLUGIN_CATEGORY_SLUGS.contains(&category.as_str()) {
            return Err(format!("contains unknown category '{}'", category));
        }
        Ok(category.clone())
    }

    pub fn validate_bundled_plugin_icon_png(bytes: &[u8]) -> Result<(), String> {
        const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
        if bytes.len() < 24 {
            return Err("File too short for valid PNG header".to_string());
        }
        if bytes[0..8] != PNG_SIG {
            return Err("Invalid PNG signature".to_string());
        }
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        if width != 512 || height != 512 {
            return Err(format!(
                "Icon must be 512x512 pixels, got {}x{}",
                width, height
            ));
        }
        Ok(())
    }

    pub fn validate_bundled_plugin_naming(
        dir_name: &str,
        manifest_id: &str,
        package_name: &str,
    ) -> Result<(), String> {
        let expected_manifest_id = if dir_name == "kimi-coding" {
            "kimi"
        } else {
            dir_name
        };

        if manifest_id != expected_manifest_id {
            return Err(format!(
                "Manifest id '{}' does not match expected '{}' for directory '{}'",
                manifest_id, expected_manifest_id, dir_name
            ));
        }

        if !package_name.starts_with("@openclaw/") {
            return Err(format!(
                "Package name '{}' must start with '@openclaw/'",
                package_name
            ));
        }

        let pkg_suffix = &package_name["@openclaw/".len()..];
        let matches_suffix = Self::ALLOWED_PACKAGE_SUFFIXES.iter().any(|suffix| {
            let expected_pkg = format!("{}{}", expected_manifest_id, suffix);
            pkg_suffix == expected_pkg
        });

        if !matches_suffix {
            return Err(format!(
                "Package name '{}' does not conform to expected prefix '@openclaw/{}' with allowed suffixes",
                package_name, expected_manifest_id
            ));
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Bundled Plugin Sources & Resolvers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundledPluginSource {
    pub plugin_id: String,
    pub local_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub npm_spec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
    pub requires_config: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundledPluginLookup<'a> {
    PluginId(&'a str),
    NpmSpec(&'a str),
    LocalPath(&'a str),
}

pub struct BundledPluginSourceResolver;
impl BundledPluginSourceResolver {
    pub fn schema_has_required_fields(schema: &serde_json::Value) -> bool {
        if let Some(obj) = schema.as_object() {
            if let Some(req) = obj.get("required").and_then(|v| v.as_array()) {
                return req.iter().any(|item| item.is_string());
            }
        }
        false
    }

    pub fn find_in_map<'a>(
        bundled: &'a HashMap<String, BundledPluginSource>,
        lookup: BundledPluginLookup<'_>,
    ) -> Option<&'a BundledPluginSource> {
        match lookup {
            BundledPluginLookup::PluginId(id) => {
                let id = id.trim();
                if id.is_empty() {
                    None
                } else {
                    bundled.get(id)
                }
            }
            BundledPluginLookup::NpmSpec(spec) => {
                let spec = spec.trim();
                if spec.is_empty() {
                    None
                } else {
                    bundled.values().find(|s| s.npm_spec.as_deref() == Some(spec))
                }
            }
            BundledPluginLookup::LocalPath(p) => {
                let p = p.trim();
                if p.is_empty() {
                    None
                } else {
                    let canonical_target = std::path::Path::new(p);
                    bundled.values().find(|s| {
                        let source_p = std::path::Path::new(&s.local_path);
                        source_p == canonical_target
                    })
                }
            }
        }
    }

    pub fn resolve_install_command_hint(source: Option<&BundledPluginSource>) -> Option<String> {
        source.map(|s| format!("openclaw plugins install {}", s.local_path))
    }
}

// ---------------------------------------------------------------------------
// Candidate Install Owner & Immutable Identity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateInstallOwner {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_owner: Option<String>,
    #[serde(default)]
    pub ambiguous: bool,
}

pub struct CandidateInstallOwnerResolver;
impl CandidateInstallOwnerResolver {
    pub fn resolve_immutable_install_identity(
        origin: &str,
        install_owner: Option<&str>,
        source: Option<&str>,
        install_path: Option<&str>,
        spec: Option<&str>,
        resolved_spec: Option<&str>,
        integrity: Option<&str>,
        installed_at: Option<&str>,
    ) -> Option<String> {
        if origin != "global" || source != Some("npm") {
            return None;
        }
        let integrity = integrity?;
        let owner = install_owner?;
        let effective_spec = resolved_spec.or(spec).unwrap_or("");
        let arr = serde_json::json!([
            owner,
            install_path.unwrap_or(""),
            effective_spec,
            integrity,
            installed_at.unwrap_or("")
        ]);
        Some(arr.to_string())
    }
}

// ---------------------------------------------------------------------------
// Capability Catalog Families & Descriptors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityCatalogFamily {
    SpeechProviders,
    RealtimeTranscriptionProviders,
    RealtimeVoiceProviders,
}

impl CapabilityCatalogFamily {
    pub const ALL: &'static [CapabilityCatalogFamily] = &[
        CapabilityCatalogFamily::SpeechProviders,
        CapabilityCatalogFamily::RealtimeTranscriptionProviders,
        CapabilityCatalogFamily::RealtimeVoiceProviders,
    ];

    pub fn family_name(&self) -> &'static str {
        match self {
            CapabilityCatalogFamily::SpeechProviders => "speechProviders",
            CapabilityCatalogFamily::RealtimeTranscriptionProviders => {
                "realtimeTranscriptionProviders"
            }
            CapabilityCatalogFamily::RealtimeVoiceProviders => "realtimeVoiceProviders",
        }
    }

    pub fn required_method(&self) -> &'static str {
        match self {
            CapabilityCatalogFamily::SpeechProviders => "synthesize",
            CapabilityCatalogFamily::RealtimeTranscriptionProviders => "createSession",
            CapabilityCatalogFamily::RealtimeVoiceProviders => "createBridge",
        }
    }

    pub fn from_family_name(name: &str) -> Option<CapabilityCatalogFamily> {
        match name {
            "speechProviders" => Some(CapabilityCatalogFamily::SpeechProviders),
            "realtimeTranscriptionProviders" => {
                Some(CapabilityCatalogFamily::RealtimeTranscriptionProviders)
            }
            "realtimeVoiceProviders" => Some(CapabilityCatalogFamily::RealtimeVoiceProviders),
            _ => None,
        }
    }
}

pub struct CapabilityCatalogValidator;
impl CapabilityCatalogValidator {
    pub fn validate_catalog_shape(catalog_json: &serde_json::Value) -> Result<(), String> {
        let obj = catalog_json.as_object().ok_or_else(|| {
            "default export must synchronously provide a capability descriptor collection"
                .to_string()
        })?;

        for (family_str, providers_val) in obj {
            let family = CapabilityCatalogFamily::from_family_name(family_str)
                .ok_or_else(|| format!("unknown capability catalog family: {}", family_str))?;

            let providers = providers_val.as_array().ok_or_else(|| {
                format!("{} must contain complete provider descriptors", family_str)
            })?;

            let req_method = family.required_method();
            for p in providers {
                let p_obj = p.as_object().ok_or_else(|| {
                    format!("{} must contain complete provider descriptors", family_str)
                })?;

                let id = p_obj
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if id.is_empty() {
                    return Err(format!(
                        "{} must contain complete provider descriptors",
                        family_str
                    ));
                }

                if p_obj.get("label").and_then(|v| v.as_str()).is_none() {
                    return Err(format!(
                        "{} must contain complete provider descriptors",
                        family_str
                    ));
                }

                if !p_obj.contains_key("isConfigured") || !p_obj.contains_key(req_method) {
                    return Err(format!(
                        "{} must contain complete provider descriptors",
                        family_str
                    ));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Plugin Accepted Declared Surface & Capability Consent
// ---------------------------------------------------------------------------

