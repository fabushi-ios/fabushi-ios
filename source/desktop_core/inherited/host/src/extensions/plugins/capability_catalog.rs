use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use crate::extensions::plugin_sdk::channel_entry::{
    BundledChannelEntryContract, PluginRegistrationMode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub const CAPABILITY_FAMILY_SPEECH_PROVIDERS: &str = "speechProviders";
pub const CAPABILITY_FAMILY_TRANSCRIPTION_PROVIDERS: &str = "realtimeTranscriptionProviders";
pub const CAPABILITY_FAMILY_VOICE_PROVIDERS: &str = "realtimeVoiceProviders";
/// Bundled worker-provider metadata family. Entries in this family describe
/// discoverability only; provider lifecycle and readiness remain owned by the
/// relevant worker owner.
pub const CAPABILITY_FAMILY_WORKER_PROVIDERS: &str = "workerProviders";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginOrigin {
    Bundled,
    Installed,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityProviderEntry {
    pub family: String,
    pub provider_id: String,
    pub plugin_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelCatalogEntry {
    pub plugin_id: String,
    pub origin: PluginOrigin,
    pub package_name: Option<String>,
    pub channel_name: String,
    pub trusted_official_install: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuthChoice {
    pub provider: String,
    pub method: String,
    pub choice_id: String,
    pub choice_label: String,
    pub onboarding_scopes: Vec<String>,
    pub cli_flag: Option<String>,
    pub option_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginContractRegistryEntry {
    pub plugin_id: String,
    pub provider_ids: Vec<String>,
    pub worker_provider_ids: Vec<String>,
    pub web_fetch_provider_ids: Vec<String>,
    pub web_search_provider_ids: Vec<String>,
    pub speech_provider_ids: Vec<String>,
    pub realtime_voice_provider_ids: Vec<String>,
    pub realtime_transcription_provider_ids: Vec<String>,
    pub image_generation_provider_ids: Vec<String>,
}

#[derive(Clone, Default)]
pub struct PluginCapabilityCatalog {
    capabilities: Arc<Mutex<HashMap<(String, String), CapabilityProviderEntry>>>,
    channels: Arc<Mutex<HashMap<String, ChannelCatalogEntry>>>,
    canonical_aliases: Arc<Mutex<HashMap<String, Vec<String>>>>,
    auth_choices: Arc<Mutex<Vec<ProviderAuthChoice>>>,
}

impl PluginCapabilityCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_capability(&self, entry: CapabilityProviderEntry) {
        let key = (entry.family.clone(), entry.provider_id.to_ascii_lowercase());
        let mut caps = self.capabilities.lock().unwrap();
        caps.insert(key, entry);
    }

    pub fn get_capability(&self, family: &str, provider_id: &str) -> Option<CapabilityProviderEntry> {
        let key = (family.to_string(), provider_id.to_ascii_lowercase());
        let caps = self.capabilities.lock().unwrap();
        caps.get(&key).cloned()
    }

    pub fn list_capabilities_for_family(&self, family: &str) -> Vec<CapabilityProviderEntry> {
        let caps = self.capabilities.lock().unwrap();
        caps.values()
            .filter(|e| e.family == family)
            .cloned()
            .collect()
    }

    pub fn matches_provider_id_or_alias(&self, family: &str, candidate: &str) -> bool {
        let candidate_lower = candidate.to_ascii_lowercase();
        let caps = self.capabilities.lock().unwrap();
        caps.values().any(|e| {
            e.family == family
                && (e.provider_id.eq_ignore_ascii_case(&candidate_lower)
                    || e.aliases.iter().any(|a| a.eq_ignore_ascii_case(&candidate_lower)))
        })
    }

    pub fn register_channel(&self, entry: ChannelCatalogEntry) {
        let key = entry.channel_name.to_ascii_lowercase();
        let mut channels = self.channels.lock().unwrap();
        channels.insert(key, entry);
    }

    /// Registers one bundled channel entry through the existing catalog owner.
    ///
    /// The entry handler remains responsible for the mode-specific registration
    /// work; this catalog owns the resulting discoverability metadata. No second
    /// channel registry or channel state is created by this bridge.
    pub fn register_bundled_channel_entry(
        &self,
        entry: &BundledChannelEntryContract,
        origin: PluginOrigin,
        package_name: Option<String>,
        mode: PluginRegistrationMode,
    ) -> Result<Vec<String>, String> {
        let channel_id = entry.id.trim().to_ascii_lowercase();
        if channel_id.is_empty() {
            return Err("channel capability registration requires a channel id".into());
        }
        let mut channels = self.channels.lock().unwrap();
        if channels.contains_key(&channel_id) {
            return Err(format!("channel capability already registered: {channel_id}"));
        }
        let registration = entry.register(mode);
        channels.insert(channel_id, ChannelCatalogEntry {
            plugin_id: entry.id.clone(),
            origin: origin.clone(),
            package_name,
            channel_name: entry.id.clone(),
            trusted_official_install: origin == PluginOrigin::Bundled,
        });
        Ok(registration)
    }

    pub fn get_channel(&self, channel_name: &str) -> Option<ChannelCatalogEntry> {
        let key = channel_name.to_ascii_lowercase();
        let channels = self.channels.lock().unwrap();
        channels.get(&key).cloned()
    }

    pub fn unregister_channel(&self, channel_name: &str) -> Option<ChannelCatalogEntry> {
        let key = channel_name.trim().to_ascii_lowercase();
        if key.is_empty() {
            return None;
        }
        let mut channels = self.channels.lock().unwrap();
        channels.remove(&key)
    }

    pub fn register_canonical_aliases(&self, key: &str, aliases: Vec<String>) {
        let mut map = self.canonical_aliases.lock().unwrap();
        map.insert(key.to_string(), aliases);
    }

    pub fn get_canonical_aliases(&self, key: &str) -> Option<Vec<String>> {
        let map = self.canonical_aliases.lock().unwrap();
        map.get(key).cloned()
    }

    pub fn register_auth_choice(&self, choice: ProviderAuthChoice) {
        let mut list = self.auth_choices.lock().unwrap();
        list.push(choice);
    }

    pub fn build_provider_method_choice(provider_id: &str, method_id: &str) -> String {
        format!("{}:{}", provider_id, method_id)
    }

    pub fn resolve_preferred_provider_for_auth_choice(&self, choice: &str) -> Option<String> {
        let list = self.auth_choices.lock().unwrap();
        for entry in list.iter() {
            if entry.choice_id == choice
                || Self::build_provider_method_choice(&entry.provider, &entry.method) == choice
            {
                return Some(entry.provider.clone());
            }
        }
        None
    }

    pub fn filter_text_onboarding_auth_choices(&self) -> Vec<ProviderAuthChoice> {
        let list = self.auth_choices.lock().unwrap();
        list.iter()
            .filter(|choice| {
                // If onboarding scopes are strictly limited to image-generation or video, exclude from text onboarding
                if choice.onboarding_scopes.len() == 1 && choice.onboarding_scopes[0] == "image-generation" {
                    return false;
                }
                true
            })
            .cloned()
            .collect()
    }

    pub fn validate_unique_provider_ids(entries: &[PluginContractRegistryEntry]) -> Result<(), String> {
        let mut seen_providers = HashSet::new();
        for entry in entries {
            for pid in &entry.provider_ids {
                if !seen_providers.insert(pid.clone()) {
                    return Err(format!("Duplicate provider id: {}", pid));
                }
            }
        }
        Ok(())
    }

    pub fn resolve_web_search_providers(
        bundled: &[String],
        activation_scoped: &[String],
    ) -> Vec<String> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for id in bundled {
            if !activation_scoped.contains(id) && seen.insert(id.clone()) {
                result.push(id.clone());
            }
        }
        for id in activation_scoped {
            if seen.insert(id.clone()) {
                result.push(id.clone());
            }
        }
        result.sort();
        result
    }

    pub fn resolve_provider_contract_providers_for_plugin_ids(
        &self,
        plugin_ids: &[&str],
        public_artifacts: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for plugin_id in plugin_ids {
            if let Some(providers) = public_artifacts.get(*plugin_id) {
                for p in providers {
                    if seen.insert(p.clone()) {
                        result.push(p.clone());
                    }
                }
            } else {
                let caps = self.capabilities.lock().unwrap();
                for entry in caps.values() {
                    if entry.plugin_id == *plugin_id && seen.insert(entry.provider_id.clone()) {
                        result.push(entry.provider_id.clone());
                    }
                }
            }
        }
        result
    }

    pub fn resolve_web_search_providers_for_plugin_id(
        &self,
        plugin_id: &str,
        public_artifacts: &HashMap<String, Vec<String>>,
    ) -> Vec<String> {
        if let Some(providers) = public_artifacts.get(plugin_id) {
            providers.clone()
        } else {
            let caps = self.capabilities.lock().unwrap();
            caps.values()
                .filter(|e| e.plugin_id == plugin_id && e.family == "webSearch")
                .map(|e| e.provider_id.clone())
                .collect()
        }
    }
}

// Shared upstream model catalog validations (from src/plugins/contracts/shared-upstream-model.contract.test.ts)


// Sibling submodule re-exports
pub use super::capability_catalog_channels::*;
pub use super::capability_catalog_core::*;
pub use super::capability_catalog_providers::*;
pub use super::capability_catalog_validation::*;
