use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaUnderstandingCapability {
    Image,
    Audio,
    Video,
}

impl MediaUnderstandingCapability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MediaUnderstandingProviderMetadata {
    pub id: String,
    pub capabilities: Vec<MediaUnderstandingCapability>,
    pub default_models: HashMap<String, String>,
    pub auto_priority: HashMap<String, i32>,
}

pub fn normalize_media_provider_id(provider_id: &str) -> String {
    let lower = provider_id.trim().to_ascii_lowercase();
    match lower.as_str() {
        "google" | "gemini" => "google".to_string(),
        "minimax" | "minimax-portal" | "minimax-cn" => "minimax".to_string(),
        _ => lower,
    }
}

pub fn resolve_default_media_model(
    provider_id: &str,
    capability: MediaUnderstandingCapability,
    registry: &HashMap<String, MediaUnderstandingProviderMetadata>,
    configured_models: Option<&HashMap<String, Vec<String>>>,
    include_configured_models: bool,
) -> Option<String> {
    let norm_id = normalize_media_provider_id(provider_id);
    if include_configured_models && capability == MediaUnderstandingCapability::Image {
        if let Some(cfg) = configured_models {
            for (key, models) in cfg {
                if normalize_media_provider_id(key) == norm_id {
                    if let Some(first) = models.iter().find(|m| !m.trim().is_empty()) {
                        return Some(first.trim().to_string());
                    }
                }
            }
        }
    }

    if let Some(meta) = registry.get(&norm_id) {
        if let Some(model) = meta.default_models.get(capability.as_str()) {
            let trimmed = model.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub fn resolve_auto_media_key_providers(
    capability: MediaUnderstandingCapability,
    registry: &HashMap<String, MediaUnderstandingProviderMetadata>,
    configured_image_providers: Option<&[String]>,
) -> Vec<String> {
    let cap_key = capability.as_str();
    let mut prioritized: Vec<(&MediaUnderstandingProviderMetadata, i32)> = registry
        .values()
        .filter(|p| p.capabilities.contains(&capability))
        .filter_map(|p| {
            p.auto_priority.get(cap_key).copied().map(|prio| (p, prio))
        })
        .collect();

    prioritized.sort_by(|(a, prio_a), (b, prio_b)| {
        prio_a.cmp(prio_b).then_with(|| a.id.cmp(&b.id))
    });

    let mut result: Vec<String> = prioritized
        .into_iter()
        .map(|(p, _)| normalize_media_provider_id(&p.id))
        .collect();

    if capability == MediaUnderstandingCapability::Image {
        if let Some(configured) = configured_image_providers {
            for prov in configured {
                let norm = normalize_media_provider_id(prov);
                if !result.contains(&norm) {
                    result.push(norm);
                }
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    result.retain(|id| seen.insert(id.clone()));
    result
}
