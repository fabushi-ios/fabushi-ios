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
use super::capability_catalog_providers::*;
use super::capability_catalog_validation::*;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAcceptedDeclaredSurface {
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub contracts: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: Vec<String>,
    #[serde(default, rename = "cliCommands")]
    pub cli_commands: Vec<String>,
    #[serde(default, rename = "cliBackends")]
    pub cli_backends: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default, rename = "dangerousConfigFlags")]
    pub dangerous_config_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCapabilityConsentReview {
    pub plugin_id: String,
    pub review_token: String,
    pub declared: PluginAcceptedDeclaredSurface,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_declared: Option<PluginAcceptedDeclaredSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widened: Option<HashMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_at: Option<String>,
}

pub struct CapabilityConsentManager;
impl CapabilityConsentManager {
    pub fn compute_declared_surface_hash(surface: &PluginAcceptedDeclaredSurface) -> String {
        let mut hasher = Sha256::new();
        let canonical_json = serde_json::to_string(surface).unwrap_or_default();
        hasher.update(canonical_json.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    pub fn resolve_plugin_install_record_integrity(
        integrity: Option<&str>,
        npm_integrity: Option<&str>,
        clawpack_sha256: Option<&str>,
        git_commit: Option<&str>,
    ) -> Option<(String, &'static str)> {
        if let Some(i) = integrity.filter(|s| !s.is_empty()) {
            return Some((i.to_string(), "ssri"));
        }
        if let Some(i) = npm_integrity.filter(|s| !s.is_empty()) {
            return Some((i.to_string(), "ssri"));
        }
        if let Some(i) = clawpack_sha256.filter(|s| !s.is_empty()) {
            return Some((i.to_string(), "sha256"));
        }
        if let Some(i) = git_commit.filter(|s| !s.is_empty()) {
            return Some((i.to_string(), "git-commit"));
        }
        None
    }

    pub fn resolve_accepted_surface_current(
        record_accepted_surface: Option<&PluginAcceptedDeclaredSurface>,
        record_accepted_surface_hash: Option<&str>,
        record_integrity: Option<&str>,
        record_accepted_surface_integrity: Option<&str>,
        current_surface: &PluginAcceptedDeclaredSurface,
    ) -> bool {
        let accepted = match record_accepted_surface {
            Some(a) => a,
            None => return false,
        };
        if accepted != current_surface {
            return false;
        }
        let expected_hash = Self::compute_declared_surface_hash(current_surface);
        if record_accepted_surface_hash != Some(expected_hash.as_str()) {
            return false;
        }
        if let Some(accepted_integrity) = record_accepted_surface_integrity {
            match record_integrity {
                Some(cur_integrity) if cur_integrity == accepted_integrity => {}
                _ => return false,
            }
        }
        true
    }

    pub fn diff_declared_surface_widening(
        previous: &PluginAcceptedDeclaredSurface,
        next: &PluginAcceptedDeclaredSurface,
    ) -> (HashMap<String, Vec<String>>, bool) {
        let mut widened = HashMap::new();

        fn diff_field(prev: &[String], nxt: &[String]) -> Vec<String> {
            let prev_set: HashSet<&str> = prev.iter().map(|s| s.as_str()).collect();
            let mut added: Vec<String> = nxt
                .iter()
                .filter(|s| !prev_set.contains(s.as_str()))
                .cloned()
                .collect();
            added.sort();
            added
        }

        let tools = diff_field(&previous.tools, &next.tools);
        if !tools.is_empty() {
            widened.insert("tools".to_string(), tools);
        }
        let channels = diff_field(&previous.channels, &next.channels);
        if !channels.is_empty() {
            widened.insert("channels".to_string(), channels);
        }
        let providers = diff_field(&previous.providers, &next.providers);
        if !providers.is_empty() {
            widened.insert("providers".to_string(), providers);
        }
        let contracts = diff_field(&previous.contracts, &next.contracts);
        if !contracts.is_empty() {
            widened.insert("contracts".to_string(), contracts);
        }
        let hooks = diff_field(&previous.hooks, &next.hooks);
        if !hooks.is_empty() {
            widened.insert("hooks".to_string(), hooks);
        }
        let mcp = diff_field(&previous.mcp_servers, &next.mcp_servers);
        if !mcp.is_empty() {
            widened.insert("mcpServers".to_string(), mcp);
        }
        let cli_cmds = diff_field(&previous.cli_commands, &next.cli_commands);
        if !cli_cmds.is_empty() {
            widened.insert("cliCommands".to_string(), cli_cmds);
        }
        let cli_backends = diff_field(&previous.cli_backends, &next.cli_backends);
        if !cli_backends.is_empty() {
            widened.insert("cliBackends".to_string(), cli_backends);
        }
        let skills = diff_field(&previous.skills, &next.skills);
        if !skills.is_empty() {
            widened.insert("skills".to_string(), skills);
        }
        let flags = diff_field(&previous.dangerous_config_flags, &next.dangerous_config_flags);
        if !flags.is_empty() {
            widened.insert("dangerousConfigFlags".to_string(), flags);
        }

        let has_widening = !widened.is_empty();
        (widened, has_widening)
    }

    pub fn redact_source_spec(spec: &str) -> String {
        if let Some(stripped) = spec.strip_prefix("git:") {
            if let Ok(mut parsed) = url::Url::parse(stripped) {
                if !parsed.username().is_empty() || parsed.password().is_some() {
                    let _ = parsed.set_username("***");
                    let _ = parsed.set_password(Some("***"));
                }
                let pairs: Vec<(String, String)> = parsed
                    .query_pairs()
                    .map(|(k, v)| {
                        if k == "token" || k == "key" || k == "secret" {
                            (k.to_string(), "***".to_string())
                        } else {
                            (k.to_string(), v.to_string())
                        }
                    })
                    .collect();
                parsed.query_pairs_mut().clear().extend_pairs(
                    pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
                );
                return format!("git:{}", parsed);
            }
        }
        spec.to_string()
    }
}

// ---------------------------------------------------------------------------
// Capability Lease
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PluginRuntimeCapabilityLease {
    owner: String,
    inner: Arc<Mutex<CapabilityLeaseState>>,
}

struct CapabilityLeaseState {
    active: bool,
    cleanups: Vec<Box<dyn FnOnce() + Send + 'static>>,
}

impl PluginRuntimeCapabilityLease {
    pub fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            inner: Arc::new(Mutex::new(CapabilityLeaseState {
                active: true,
                cleanups: Vec::new(),
            })),
        }
    }

    pub fn is_active(&self) -> bool {
        self.inner.lock().unwrap().active
    }

    pub fn assert_active(&self, capability: &str) -> Result<(), String> {
        if !self.is_active() {
            Err(format!("{} {} is no longer active", self.owner, capability))
        } else {
            Ok(())
        }
    }

    pub fn retain<F>(&self, cleanup: F) -> Result<(), String>
    where
        F: FnOnce() + Send + 'static,
    {
        let mut state = self.inner.lock().unwrap();
        if !state.active {
            cleanup();
            return Err(format!("{} capability lease is no longer active", self.owner));
        }
        state.cleanups.push(Box::new(cleanup));
        Ok(())
    }

    pub fn revoke(&self) {
        let cleanups_to_run = {
            let mut state = self.inner.lock().unwrap();
            if !state.active {
                return;
            }
            state.active = false;
            std::mem::take(&mut state.cleanups)
        };
        for cleanup in cleanups_to_run {
            cleanup();
        }
    }
}

// ---------------------------------------------------------------------------
// Operator Grants & Summary Builder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookGrant {
    pub effective: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginHookGrants {
    pub allow_prompt_injection: PluginHookGrant,
    pub allow_conversation_access: PluginHookGrant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginOperatorGrants {
    pub hooks: PluginHookGrants,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<serde_json::Value>,
}

pub struct CapabilitySummaryBuilder;
impl CapabilitySummaryBuilder {
    pub fn merge_declared_surfaces(
        surfaces: &[PluginAcceptedDeclaredSurface],
    ) -> PluginAcceptedDeclaredSurface {
        let mut merged = PluginAcceptedDeclaredSurface::default();
        let mut channels = HashSet::new();
        let mut providers = HashSet::new();
        let mut tools = HashSet::new();
        let mut contracts = HashSet::new();
        let mut hooks = HashSet::new();
        let mut mcp = HashSet::new();
        let mut cli_cmds = HashSet::new();
        let mut cli_backends = HashSet::new();
        let mut skills = HashSet::new();
        let mut flags = HashSet::new();

        for s in surfaces {
            channels.extend(s.channels.iter().cloned());
            providers.extend(s.providers.iter().cloned());
            tools.extend(s.tools.iter().cloned());
            contracts.extend(s.contracts.iter().cloned());
            hooks.extend(s.hooks.iter().cloned());
            mcp.extend(s.mcp_servers.iter().cloned());
            cli_cmds.extend(s.cli_commands.iter().cloned());
            cli_backends.extend(s.cli_backends.iter().cloned());
            skills.extend(s.skills.iter().cloned());
            flags.extend(s.dangerous_config_flags.iter().cloned());
        }

        fn to_sorted(set: HashSet<String>) -> Vec<String> {
            let mut v: Vec<String> = set.into_iter().collect();
            v.sort();
            v
        }

        merged.channels = to_sorted(channels);
        merged.providers = to_sorted(providers);
        merged.tools = to_sorted(tools);
        merged.contracts = to_sorted(contracts);
        merged.hooks = to_sorted(hooks);
        merged.mcp_servers = to_sorted(mcp);
        merged.cli_commands = to_sorted(cli_cmds);
        merged.cli_backends = to_sorted(cli_backends);
        merged.skills = to_sorted(skills);
        merged.dangerous_config_flags = to_sorted(flags);
        merged
    }

    pub fn build_operator_grants(
        origin: &PluginOrigin,
        allow_prompt_injection: Option<bool>,
        allow_conversation_access: Option<bool>,
    ) -> PluginOperatorGrants {
        let effective_prompt_injection = allow_prompt_injection.unwrap_or(false);
        let effective_conv_access = match origin {
            PluginOrigin::Bundled => allow_conversation_access.unwrap_or(true),
            _ => allow_conversation_access.unwrap_or(false),
        };

        PluginOperatorGrants {
            hooks: PluginHookGrants {
                allow_prompt_injection: PluginHookGrant {
                    effective: effective_prompt_injection,
                    configured: allow_prompt_injection,
                },
                allow_conversation_access: PluginHookGrant {
                    effective: effective_conv_access,
                    configured: allow_conversation_access,
                },
            },
            llm: None,
            subagent: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Captured Plugin Registration
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct CapturedPluginRegistration {
    pub providers: Vec<String>,
    pub cli_commands: Vec<String>,
    pub speech_providers: Vec<String>,
    pub realtime_transcription_providers: Vec<String>,
    pub realtime_voice_providers: Vec<String>,
    pub media_understanding_providers: Vec<String>,
    pub transcript_source_providers: Vec<String>,
    pub image_generation_providers: Vec<String>,
    pub video_generation_providers: Vec<String>,
    pub music_generation_providers: Vec<String>,
    pub web_fetch_providers: Vec<String>,
    pub web_search_providers: Vec<String>,
    pub worker_providers: Vec<String>,
    pub storage_providers: Vec<String>,
    pub migration_providers: Vec<String>,
    pub embedding_providers: Vec<String>,
    pub tools: Vec<String>,
    pub session_scheduler_jobs: Vec<String>,
    pub session_actions: Vec<String>,
}

impl CapturedPluginRegistration {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_capability_provider(
        &mut self,
        family: &str,
        provider_id: &str,
    ) -> Result<(), String> {
        match family {
            "speechProviders" => self.speech_providers.push(provider_id.to_string()),
            "realtimeTranscriptionProviders" => self
                .realtime_transcription_providers
                .push(provider_id.to_string()),
            "realtimeVoiceProviders" => self.realtime_voice_providers.push(provider_id.to_string()),
            "mediaUnderstandingProviders" => {
                self.media_understanding_providers.push(provider_id.to_string())
            }
            "transcriptSourceProviders" => self
                .transcript_source_providers
                .push(provider_id.to_string()),
            "imageGenerationProviders" => self
                .image_generation_providers
                .push(provider_id.to_string()),
            "videoGenerationProviders" => self
                .video_generation_providers
                .push(provider_id.to_string()),
            "musicGenerationProviders" => self
                .music_generation_providers
                .push(provider_id.to_string()),
            "webFetchProviders" => self.web_fetch_providers.push(provider_id.to_string()),
            "webSearchProviders" => self.web_search_providers.push(provider_id.to_string()),
            "embeddingProviders" => self.embedding_providers.push(provider_id.to_string()),
            _ => return Err(format!("Unknown capability family: {}", family)),
        }
        Ok(())
    }

    pub fn total_registered_capabilities(&self) -> usize {
        self.speech_providers.len()
            + self.realtime_transcription_providers.len()
            + self.realtime_voice_providers.len()
            + self.media_understanding_providers.len()
            + self.transcript_source_providers.len()
            + self.image_generation_providers.len()
            + self.video_generation_providers.len()
            + self.music_generation_providers.len()
            + self.web_fetch_providers.len()
            + self.web_search_providers.len()
            + self.embedding_providers.len()
    }
}

// ---------------------------------------------------------------------------
// Capability Acquisition Scope & Lifetime
// ---------------------------------------------------------------------------

pub struct PluginCapabilityAcquisitionScope {
    lease: PluginRuntimeCapabilityLease,
    generation: u64,
    current_generation: Arc<std::sync::atomic::AtomicU64>,
}

impl PluginCapabilityAcquisitionScope {
    pub fn new(owner: &str, current_generation: Arc<std::sync::atomic::AtomicU64>) -> Self {
        let generation_id = current_generation.load(std::sync::atomic::Ordering::SeqCst);
        Self {
            lease: PluginRuntimeCapabilityLease::new(owner),
            generation: generation_id,
            current_generation,
        }
    }

    pub fn assert_open(&self) -> Result<(), String> {
        self.lease.assert_active("acquisition")?;
        let cur = self
            .current_generation
            .load(std::sync::atomic::Ordering::SeqCst);
        if cur != self.generation {
            return Err("The provider setup changed while preparing this request. Retry with the current provider setup.".to_string());
        }
        Ok(())
    }

    pub fn release(&self) {
        self.lease.revoke();
    }
}

// ---------------------------------------------------------------------------
// Catalog Discovery Codec & Identity Join
// ---------------------------------------------------------------------------

pub struct CatalogIconRegistry {
    urls: Mutex<std::collections::VecDeque<String>>,
    lookup: Mutex<HashSet<String>>,
    capacity: usize,
}

impl CatalogIconRegistry {
    pub const DEFAULT_MAX_CATALOG_ICON_URLS: usize = 1024;

    pub fn new(capacity: usize) -> Self {
        Self {
            urls: Mutex::new(std::collections::VecDeque::new()),
            lookup: Mutex::new(HashSet::new()),
            capacity,
        }
    }

    pub fn normalize_catalog_icon_url(value: &str) -> Option<String> {
        if value.is_empty() || value.len() > 2048 {
            return None;
        }
        let parsed = url::Url::parse(value).ok()?;
        if parsed.scheme() != "https" {
            return None;
        }
        if parsed.host_str().is_none() {
            return None;
        }
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return None;
        }
        Some(parsed.to_string())
    }

    pub fn register_icon_urls<'a, I>(&self, values: I)
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut urls = self.urls.lock().unwrap();
        let mut lookup = self.lookup.lock().unwrap();
        for val in values {
            if let Some(norm) = Self::normalize_catalog_icon_url(val) {
                if !lookup.contains(&norm) {
                    if urls.len() >= self.capacity {
                        if let Some(oldest) = urls.pop_front() {
                            lookup.remove(&oldest);
                        }
                    }
                    urls.push_back(norm.clone());
                    lookup.insert(norm);
                }
            }
        }
    }

    pub fn resolve_icon_url(&self, value: &str) -> Option<String> {
        let norm = Self::normalize_catalog_icon_url(value)?;
        let lookup = self.lookup.lock().unwrap();
        if lookup.contains(&norm) {
            Some(norm)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
