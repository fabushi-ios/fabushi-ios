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
use super::capability_catalog_validation::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCatalogRow {
    pub provider_id: String,
    pub model_id: String,
    pub upstream_model: Option<String>,
    pub code_mode: Option<String>,
}

impl ModelCatalogRow {
    pub fn ref_id(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }

    pub fn group_key(&self) -> String {
        let target = self.upstream_model.as_deref().unwrap_or(&self.model_id);
        if let Some(pos) = target.find('/') {
            target[pos + 1..].to_ascii_lowercase()
        } else {
            target.to_ascii_lowercase()
        }
    }
}

pub struct SharedUpstreamModelCatalog;

impl SharedUpstreamModelCatalog {
    pub fn validate_upstream_model_refs(rows: &[ModelCatalogRow]) -> Result<(), Vec<String>> {
        let refs: HashSet<String> = rows.iter().map(|r| r.ref_id()).collect();
        let mut unresolved = Vec::new();

        for row in rows {
            if let Some(upstream) = &row.upstream_model {
                if upstream == &row.ref_id() || !refs.contains(upstream) {
                    unresolved.push(format!("{} -> {}", row.ref_id(), upstream));
                }
            }
        }
        if unresolved.is_empty() {
            Ok(())
        } else {
            unresolved.sort();
            Err(unresolved)
        }
    }

    pub fn validate_code_mode_tier_consistency(rows: &[ModelCatalogRow]) -> Result<(), Vec<String>> {
        let mut groups: HashMap<String, Vec<&ModelCatalogRow>> = HashMap::new();
        for row in rows {
            groups.entry(row.group_key()).or_default().push(row);
        }

        let mut silent = Vec::new();
        for (group_key, group) in groups {
            if group.len() < 2 || !group.iter().any(|r| r.code_mode.is_some()) {
                continue;
            }
            let mut declared: Vec<String> = group
                .iter()
                .filter_map(|r| r.code_mode.as_ref().map(|cm| format!("{}={}", r.ref_id(), cm)))
                .collect();
            declared.sort();
            let declared_str = declared.join(", ");

            for r in group.iter().filter(|r| r.code_mode.is_none()) {
                silent.push(format!(
                    "{} (shared model {}; siblings declare {})",
                    r.ref_id(),
                    group_key,
                    declared_str
                ));
            }
        }

        if silent.is_empty() {
            Ok(())
        } else {
            silent.sort();
            Err(silent)
        }
    }
}

// Bundled provider catalog deprecation guard (from src/plugins/contracts/provider-catalog-deprecation.contract.test.ts)

pub struct ProviderCatalogDeprecationGuard;

impl ProviderCatalogDeprecationGuard {
    pub fn assert_no_deprecated_discovery_hooks(sources: &[(&str, &str)]) -> Result<(), Vec<String>> {
        let mut offenders = Vec::new();
        for (file_path, content) in sources {
            if (content.contains("registerProvider") || content.contains("ProviderPlugin"))
                && content.contains("discovery:")
            {
                offenders.push(file_path.to_string());
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

// Embedding provider registration (from src/plugins/contracts/embedding-provider.contract.test.ts)

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisteredEmbeddingProvider {
    pub provider_id: String,
    pub plugin_id: String,
}

pub struct EmbeddingProviderRegistry {
    core_providers: std::collections::HashSet<String>,
    providers: Vec<RegisteredEmbeddingProvider>,
    diagnostics: Vec<String>,
}

impl Default for EmbeddingProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingProviderRegistry {
    pub fn new() -> Self {
        let mut core = std::collections::HashSet::new();
        core.insert("openai-compatible".to_string());
        Self {
            core_providers: core,
            providers: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn is_public_sdk_mutation_allowed(method_name: &str) -> bool {
        let forbidden = [
            "registerEmbeddingProvider",
            "listRegisteredEmbeddingProviders",
            "clearEmbeddingProviders",
            "restoreEmbeddingProviders",
            "restoreRegisteredEmbeddingProviders",
        ];
        !forbidden.contains(&method_name)
    }

    pub fn register(
        &mut self,
        plugin_id: &str,
        provider_id: &str,
        declared_in_contracts: bool,
    ) -> Result<(), String> {
        if !declared_in_contracts {
            let msg = format!(
                "plugin must declare contracts.embeddingProviders for adapter: {}",
                provider_id
            );
            self.diagnostics.push(format!("{}: {}", plugin_id, msg));
            return Err(msg);
        }

        if self.core_providers.contains(provider_id) {
            let msg = format!(
                "embedding provider already registered: {} (owner: core)",
                provider_id
            );
            self.diagnostics.push(format!("{}: {}", plugin_id, msg));
            return Err(msg);
        }

        if let Some(existing) = self.providers.iter().find(|p| p.provider_id == provider_id) {
            let msg = format!(
                "embedding provider already registered: {} (owner: {})",
                provider_id, existing.plugin_id
            );
            self.diagnostics.push(format!("{}: {}", plugin_id, msg));
            return Err(msg);
        }

        self.providers.push(RegisteredEmbeddingProvider {
            provider_id: provider_id.to_string(),
            plugin_id: plugin_id.to_string(),
        });
        Ok(())
    }

    pub fn providers(&self) -> &[RegisteredEmbeddingProvider] {
        &self.providers
    }

    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

// Wizard choice resolution and model picker (from wizard.choice-resolution and wizard.model-picker contracts)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardAuthMethod {
    pub id: String,
    pub wizard: Option<String>,
    pub model_picker: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardProviderEntry {
    pub id: String,
    pub setup_wizard: Option<String>,
    pub model_picker: bool,
    pub auth: Vec<WizardAuthMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProviderChoice {
    pub provider_id: String,
    pub method_id: String,
    pub wizard_source: String, // "method" | "provider"
    pub wizard: Option<String>,
}

pub struct ProviderWizardChoiceResolver;

impl ProviderWizardChoiceResolver {
    pub fn resolve_choice(
        providers: &[WizardProviderEntry],
        choice: &str,
    ) -> Option<ResolvedProviderChoice> {
        let clean_choice = choice.strip_prefix("provider-plugin:").unwrap_or(choice);

        // Try exact colon separation: provider:method
        if let Some(idx) = clean_choice.find(':') {
            let p_id = &clean_choice[..idx];
            let m_id = &clean_choice[idx + 1..];
            if let Some(provider) = providers.iter().find(|p| p.id == p_id) {
                if let Some(method) = provider.auth.iter().find(|m| m.id == m_id) {
                    let (source, wizard) = if let Some(ref w) = method.wizard {
                        ("method", Some(w.clone()))
                    } else if let Some(ref w) = provider.setup_wizard {
                        ("provider", Some(w.clone()))
                    } else {
                        ("none", None)
                    };
                    return Some(ResolvedProviderChoice {
                        provider_id: provider.id.clone(),
                        method_id: method.id.clone(),
                        wizard_source: source.to_string(),
                        wizard,
                    });
                }
            }
        }

        // Try exact provider match (defaulting to first auth method)
        if let Some(provider) = providers.iter().find(|p| p.id == clean_choice) {
            if let Some(first_method) = provider.auth.first() {
                let (source, wizard) = if let Some(ref w) = first_method.wizard {
                    ("method", Some(w.clone()))
                } else if let Some(ref w) = provider.setup_wizard {
                    ("provider", Some(w.clone()))
                } else {
                    ("none", None)
                };
                return Some(ResolvedProviderChoice {
                    provider_id: provider.id.clone(),
                    method_id: first_method.id.clone(),
                    wizard_source: source.to_string(),
                    wizard,
                });
            }
        }

        // Try hyphen separation: provider-method
        for provider in providers {
            let prefix = format!("{}-", provider.id);
            if clean_choice.starts_with(&prefix) {
                let m_id = &clean_choice[prefix.len()..];
                if let Some(method) = provider.auth.iter().find(|m| m.id == m_id) {
                    let (source, wizard) = if let Some(ref w) = method.wizard {
                        ("method", Some(w.clone()))
                    } else if let Some(ref w) = provider.setup_wizard {
                        ("provider", Some(w.clone()))
                    } else {
                        ("none", None)
                    };
                    return Some(ResolvedProviderChoice {
                        provider_id: provider.id.clone(),
                        method_id: method.id.clone(),
                        wizard_source: source.to_string(),
                        wizard,
                    });
                }
            }
        }

        None
    }

    pub fn resolve_model_picker_entries(providers: &[WizardProviderEntry]) -> Vec<String> {
        let mut entries = Vec::new();
        for provider in providers {
            if provider.model_picker {
                entries.push(provider.id.clone());
            }
            for method in &provider.auth {
                if method.model_picker {
                    entries.push(format!("provider-plugin:{}:{}", provider.id, method.id));
                }
            }
        }
        entries.sort();
        entries
    }
}

// Bundled Plugin Contract Snapshot (from src/plugins/contracts/inventory/bundled-capability-metadata.ts)


pub struct BundledProviderCompatValidator;

impl BundledProviderCompatValidator {
    pub const BUNDLED_PROVIDER_COMPAT_CONTRACT_KEYS: &'static [&'static str] = &[
        "speechProviders",
        "externalAuthProviders",
        "embeddingProviders",
        "mediaUnderstandingProviders",
        "transcriptSourceProviders",
        "realtimeVoiceProviders",
        "realtimeTranscriptionProviders",
        "imageGenerationProviders",
        "videoGenerationProviders",
        "musicGenerationProviders",
        "webFetchProviders",
        "webSearchProviders",
        "workerProviders",
        "storageProviders",
        "usageProviders",
        "migrationProviders",
    ];

    pub fn is_bundled_provider_compat_contract(contract: &str) -> bool {
        Self::BUNDLED_PROVIDER_COMPAT_CONTRACT_KEYS.contains(&contract)
    }

    pub fn is_bundled_provider_compat_plugin(
        origin: &str,
        providers_len: usize,
        contract_counts: &HashMap<String, usize>,
    ) -> bool {
        if origin != "bundled" {
            return false;
        }
        if providers_len > 0 {
            return true;
        }
        Self::BUNDLED_PROVIDER_COMPAT_CONTRACT_KEYS.iter().any(|key| {
            contract_counts.get(*key).copied().unwrap_or(0) > 0
        })
    }
}


// ---------------------------------------------------------------------------

pub struct CatalogDiscoveryCodec;
impl CatalogDiscoveryCodec {
    pub const DISCOVERY_ID_PREFIX: &'static str = "ch_";
    pub const LOCAL_DISCOVERY_ID_PREFIX: &'static str = "local_";

    pub fn encode_discovery_id(prefix: &str, identity: &str) -> Result<String, String> {
        let normalized = identity.trim();
        if normalized.is_empty() {
            return Err("Cannot encode an empty plugin discovery identity.".to_string());
        }
        let encoded = URL_SAFE_NO_PAD.encode(normalized.as_bytes());
        Ok(format!("{}{}", prefix, encoded))
    }

    pub fn encode_plugin_discovery_id(package_name: &str) -> Result<String, String> {
        Self::encode_discovery_id(Self::DISCOVERY_ID_PREFIX, package_name)
    }

    pub fn encode_local_plugin_discovery_id(identity: &str) -> Result<String, String> {
        Self::encode_discovery_id(Self::LOCAL_DISCOVERY_ID_PREFIX, identity)
    }

    pub fn plugin_catalog_id(id: &str, clawhub_package: Option<&str>) -> Result<String, String> {
        if let Some(pkg) = clawhub_package {
            Self::encode_plugin_discovery_id(pkg)
        } else {
            Self::encode_local_plugin_discovery_id(id)
        }
    }

    pub fn resolve_plugin_discovery_identity(id: &str) -> Option<(&'static str, String)> {
        let (prefix, origin) = if id.starts_with(Self::DISCOVERY_ID_PREFIX) {
            (Self::DISCOVERY_ID_PREFIX, "clawhub")
        } else if id.starts_with(Self::LOCAL_DISCOVERY_ID_PREFIX) {
            (Self::LOCAL_DISCOVERY_ID_PREFIX, "local")
        } else {
            return None;
        };

        let payload = &id[prefix.len()..];
        if payload.is_empty() {
            return None;
        }

        if !payload
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }

        let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
        let identity = String::from_utf8(bytes).ok()?;
        let re_encoded = Self::encode_discovery_id(prefix, &identity).ok()?;
        if re_encoded == id {
            Some((origin, identity))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Catalog Icon Registry

// Catalog Search Ranking & Deduplication
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CatalogPackageSearchResult {
    pub name: String,
    pub family: String,
    pub score: f64,
}

pub struct CatalogSearchRanking;
impl CatalogSearchRanking {
    pub const DEFAULT_PLUGIN_SEARCH_LIMIT: usize = 20;
    pub const MAX_PLUGIN_SEARCH_LIMIT: usize = 100;
    pub const ALLOWED_FAMILIES: &'static [&'static str] = &["code-plugin", "bundle-plugin"];

    pub fn clamp_limit(limit: Option<usize>) -> usize {
        match limit {
            None | Some(0) => Self::DEFAULT_PLUGIN_SEARCH_LIMIT,
            Some(l) => l.clamp(1, Self::MAX_PLUGIN_SEARCH_LIMIT),
        }
    }

    pub fn deduplicate_and_rank(
        results: Vec<CatalogPackageSearchResult>,
        limit: Option<usize>,
    ) -> Vec<CatalogPackageSearchResult> {
        let effective_limit = Self::clamp_limit(limit);
        let mut by_name: HashMap<String, CatalogPackageSearchResult> = HashMap::new();

        for entry in results {
            if !Self::ALLOWED_FAMILIES.contains(&entry.family.as_str()) {
                continue;
            }
            match by_name.get(&entry.name) {
                Some(existing) if existing.score >= entry.score => {}
                _ => {
                    by_name.insert(entry.name.clone(), entry);
                }
            }
        }

        let mut sorted: Vec<CatalogPackageSearchResult> = by_name.into_values().collect();
        sorted.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });

        sorted.truncate(effective_limit);
        sorted
    }
}

// ---------------------------------------------------------------------------
// Channel Catalog Resolver
// ---------------------------------------------------------------------------

