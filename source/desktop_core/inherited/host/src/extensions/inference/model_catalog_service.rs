//! Model Catalog Service: Remote Overlay, Wire Bundle Projection, Store & Manifest Planning
//!
//! Mirrors upstream OpenClaw `src/model-catalog/`:
//! - `bundled-catalog-stamp.ts`: Bundled build stamp discovery and parsing.
//! - `remote-config.ts`: Remote catalog URL resolution and active source checks.
//! - `remote-bundle.ts`: Wire bundle V1/V2/V3 validation, normalization, and projection.
//! - `remote-store.ts` & `remote-refresh.ts`: TTL, cache freshness, store keys, and snapshot parsing.
//! - `manifest-planner.ts` & `index.ts`: Catalog row merging, trusted transport overriding,
//!   suppression evaluation, and conflict detection.

use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};

pub const DEFAULT_REMOTE_MODEL_CATALOG_URL: &str =
    "https://catalog.openclaw.ai/models/v2/catalog.json";
pub const RETIRED_DEFAULT_REMOTE_MODEL_CATALOG_URL: &str =
    "https://catalog.openclaw.ai/models/v1/catalog.json";

pub const REMOTE_MODEL_CATALOG_STATE_KEY: &str = "modelCatalog.remote.v3";
pub const PREVIOUS_REMOTE_MODEL_CATALOG_STATE_KEY: &str = "modelCatalog.remote.v2";
pub const LEGACY_REMOTE_MODEL_CATALOG_STATE_KEY: &str = "modelCatalog.remote";

pub const REMOTE_MODEL_CATALOG_TTL_MS: u64 = 6 * 60 * 60 * 1000; // 6 hours
pub const REMOTE_MODEL_CATALOG_TIMEOUT_MS: u64 = 15_000; // 15 seconds
pub const REMOTE_MODEL_CATALOG_MAX_BYTES: usize = 4 * 1024 * 1024; // 4 MB

/// Whether remote catalog refreshing is enabled in configuration.
pub fn is_remote_model_catalog_refresh_enabled(enabled: Option<bool>) -> bool {
    enabled.unwrap_or(true)
}

/// Resolves effective remote catalog URL from configuration.
pub fn resolve_remote_catalog_url(configured: Option<&str>) -> String {
    match configured.map(str::trim).filter(|s| !s.is_empty()) {
        Some(url) => url.to_string(),
        None => DEFAULT_REMOTE_MODEL_CATALOG_URL.to_string(),
    }
}

/// Whether a catalog downloaded from `source_url` may serve the configured catalog source.
pub fn is_remote_catalog_source_active(configured: Option<&str>, source_url: &str) -> bool {
    let active_url = resolve_remote_catalog_url(configured);
    source_url == active_url
        || (active_url == DEFAULT_REMOTE_MODEL_CATALOG_URL
            && source_url == RETIRED_DEFAULT_REMOTE_MODEL_CATALOG_URL)
}

/// Normalizes provider id to lowercase trimmed string.
pub fn normalize_model_catalog_provider_id(provider: &str) -> String {
    provider.trim().to_lowercase()
}

/// Builds unique catalog merge key: `<provider>/<model_id>`.
pub fn build_model_catalog_merge_key(provider: &str, model_id: &str) -> String {
    format!(
        "{}/{}",
        normalize_model_catalog_provider_id(provider),
        model_id.trim().to_lowercase()
    )
}

/// Checks whether stored snapshot is still fresh according to TTL.
pub fn is_remote_catalog_fresh(checked_at_ms: u64, current_time_ms: u64) -> bool {
    current_time_ms.saturating_sub(checked_at_ms) < REMOTE_MODEL_CATALOG_TTL_MS
}

/// Cost metrics for a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogCost {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// Price entry with explicit flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModelCatalogPrice {
    pub cost: ModelCatalogCost,
    pub explicit: bool,
}

/// Upstream rate entry for passthrough providers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamRate {
    pub source: String,
    pub cost: ModelCatalogCost,
}

/// Upstream pricing entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModelCatalogUpstreamPrice {
    pub rates: Vec<UpstreamRate>,
    pub passthrough_only: bool,
}

/// Model declaration inside a provider catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogModelDef {
    pub id: String,
    pub name: Option<String>,
    pub context_window: Option<u64>,
    pub context_tokens: Option<u64>,
    pub reasoning: Option<bool>,
    pub base_url: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
    pub api: Option<String>,
    pub cost: Option<ModelCatalogCost>,
}

/// Provider declaration in model catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogProviderDef {
    pub id: String,
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub api: Option<String>,
    pub models: Vec<ModelCatalogModelDef>,
    pub recommended_models: Option<Vec<String>>,
}

/// Wire bundle versions supported by OpenClaw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "schemaVersion")]
pub enum RemoteModelCatalogWireBundle {
    #[serde(rename = "1")]
    V1 {
        providers: BTreeMap<String, ModelCatalogProviderDef>,
        pricing: Option<BTreeMap<String, ModelCatalogCost>>,
        min_version: Option<String>,
        generated_at: u64,
    },
    #[serde(rename = "2")]
    V2 {
        providers: BTreeMap<String, ModelCatalogProviderDef>,
        models: Vec<ModelCatalogModelDef>,
        min_version: Option<String>,
        generated_at: u64,
    },
    #[serde(rename = "3")]
    V3 {
        providers: BTreeMap<String, ModelCatalogProviderDef>,
        models: Vec<ModelCatalogModelDef>,
        min_version: Option<String>,
        generated_at: u64,
    },
}

impl RemoteModelCatalogWireBundle {
    pub fn generated_at(&self) -> u64 {
        match self {
            Self::V1 { generated_at, .. } => *generated_at,
            Self::V2 { generated_at, .. } => *generated_at,
            Self::V3 { generated_at, .. } => *generated_at,
        }
    }

    pub fn min_version(&self) -> Option<&str> {
        match self {
            Self::V1 { min_version, .. } => min_version.as_deref(),
            Self::V2 { min_version, .. } => min_version.as_deref(),
            Self::V3 { min_version, .. } => min_version.as_deref(),
        }
    }
}

/// Projected remote model catalog.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RemoteModelCatalogProjection {
    pub providers: BTreeMap<String, ModelCatalogProviderDef>,
    pub pricing: BTreeMap<String, RemoteModelCatalogPrice>,
    pub upstream_pricing: BTreeMap<String, RemoteModelCatalogUpstreamPrice>,
    pub recommendations: BTreeMap<String, Vec<String>>,
}

/// Projects a wire bundle into authoritative provider, pricing, and recommendation tables.
pub fn project_remote_model_catalog(bundle: &RemoteModelCatalogWireBundle) -> RemoteModelCatalogProjection {
    let mut projection = RemoteModelCatalogProjection::default();
    match bundle {
        RemoteModelCatalogWireBundle::V1 { providers, pricing, .. } => {
            projection.providers = providers.clone();
            if let Some(p) = pricing {
                for (k, cost) in p {
                    projection.pricing.insert(
                        k.clone(),
                        RemoteModelCatalogPrice {
                            cost: cost.clone(),
                            explicit: false,
                        },
                    );
                }
            }
            for (p_id, p_def) in providers {
                if let Some(rec) = &p_def.recommended_models {
                    projection.recommendations.insert(p_id.clone(), rec.clone());
                }
            }
        }
        RemoteModelCatalogWireBundle::V2 { providers, models, .. }
        | RemoteModelCatalogWireBundle::V3 { providers, models, .. } => {
            for (p_id, p_def) in providers {
                let mut stripped = p_def.clone();
                stripped.models = Vec::new();
                projection.providers.insert(p_id.clone(), stripped);
                if let Some(rec) = &p_def.recommended_models {
                    projection.recommendations.insert(p_id.clone(), rec.clone());
                }
            }
            for model in models {
                for (p_id, p_def) in projection.providers.iter_mut() {
                    p_def.models.push(model.clone());
                    let key = build_model_catalog_merge_key(p_id, &model.id);
                    if let Some(cost) = &model.cost {
                        projection.pricing.insert(
                            key,
                            RemoteModelCatalogPrice {
                                cost: cost.clone(),
                                explicit: true,
                            },
                        );
                    }
                }
            }
        }
    }
    projection
}

/// Stored state row in database / machine state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteModelCatalogStoreRow {
    pub id: u64,
    pub bundle_json: String,
    pub generated_at: u64,
    pub min_version: Option<String>,
    pub source_url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub checked_at: u64,
}

/// Normalized planned row in effective catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedModelCatalogRow {
    pub provider: String,
    pub model_id: String,
    pub name: Option<String>,
    pub context_tokens: u64,
    pub base_url: Option<String>,
    pub api: Option<String>,
}

/// Catalog conflict detection result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestModelCatalogConflict {
    pub provider: String,
    pub model_id: String,
}

/// Manifest suppression entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestModelCatalogSuppressionEntry {
    pub plugin_id: String,
    pub provider: String,
    pub model: String,
    pub merge_key: String,
    pub reason: Option<String>,
}

/// Plan output from manifest planner.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ManifestModelCatalogPlan {
    pub rows: Vec<NormalizedModelCatalogRow>,
    pub conflicts: Vec<ManifestModelCatalogConflict>,
    pub suppressions: Vec<ManifestModelCatalogSuppressionEntry>,
}

/// Merges remote model row with trusted transport fields.
/// Trusted transport (local config or provider plugin) overrides untrusted remote base_url/headers.
pub fn merge_remote_model_with_trusted_transport(
    remote: &ModelCatalogModelDef,
    trusted: Option<&ModelCatalogModelDef>,
    effective_base_url: Option<&str>,
    effective_api: Option<&str>,
) -> ModelCatalogModelDef {
    let mut merged = remote.clone();
    if let Some(t) = trusted {
        if t.base_url.is_some() {
            merged.base_url = t.base_url.clone();
        }
        if t.headers.is_some() {
            merged.headers = t.headers.clone();
        }
        if t.api.is_some() {
            merged.api = t.api.clone();
        }
    }
    if merged.base_url.is_none() {
        merged.base_url = effective_base_url.map(ToString::to_string);
    }
    if merged.api.is_none() {
        merged.api = effective_api.map(ToString::to_string);
    }
    merged
}

/// Plans effective model catalog rows, resolving provider filters, suppressions, and conflicts.
pub fn plan_manifest_model_catalog_rows(
    providers: &[ModelCatalogProviderDef],
    trusted_overrides: Option<&BTreeMap<String, ModelCatalogProviderDef>>,
    provider_filters: Option<&[String]>,
    suppressions: &[ManifestModelCatalogSuppressionEntry],
) -> ManifestModelCatalogPlan {
    let mut plan = ManifestModelCatalogPlan::default();
    let mut seen_keys = BTreeSet::new();

    let suppressed_keys: BTreeSet<String> = suppressions
        .iter()
        .map(|s| s.merge_key.to_lowercase())
        .collect();

    for prov in providers {
        let p_norm = normalize_model_catalog_provider_id(&prov.id);
        if let Some(filters) = provider_filters {
            if !filters.iter().any(|f| f.eq_ignore_ascii_case(&p_norm)) {
                continue;
            }
        }

        let trusted_prov = trusted_overrides.and_then(|t| t.get(&p_norm));

        for model in &prov.models {
            let key = build_model_catalog_merge_key(&p_norm, &model.id);
            if suppressed_keys.contains(&key) {
                continue;
            }

            if !seen_keys.insert(key) {
                plan.conflicts.push(ManifestModelCatalogConflict {
                    provider: p_norm.clone(),
                    model_id: model.id.clone(),
                });
                continue;
            }

            let trusted_model = trusted_prov.and_then(|tp| {
                tp.models.iter().find(|tm| tm.id.eq_ignore_ascii_case(&model.id))
            });

            let effective = merge_remote_model_with_trusted_transport(
                model,
                trusted_model,
                trusted_prov.and_then(|tp| tp.base_url.as_deref()).or(prov.base_url.as_deref()),
                trusted_prov.and_then(|tp| tp.api.as_deref()).or(prov.api.as_deref()),
            );

            plan.rows.push(NormalizedModelCatalogRow {
                provider: p_norm.clone(),
                model_id: effective.id,
                name: effective.name,
                context_tokens: effective.context_tokens.or(effective.context_window).unwrap_or(8192),
                base_url: effective.base_url,
                api: effective.api,
            });
        }
    }

    plan.suppressions = suppressions.to_vec();
    plan
}
