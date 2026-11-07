//! Outbound media load options normalization and host read capability gating.
//!
//! Ported with 100% behavioral equivalence from OpenClaw:
//! - `src/media/load-options.ts`

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutboundMediaLocalRoots {
    Any(String), // "any"
    Explicit(Vec<PathBuf>),
}

impl OutboundMediaLocalRoots {
    pub fn is_any(&self) -> bool {
        match self {
            Self::Any(s) => s.eq_ignore_ascii_case("any"),
            Self::Explicit(_) => false,
        }
    }

    pub fn as_explicit_slice(&self) -> Option<&[PathBuf]> {
        match self {
            Self::Explicit(v) => Some(v.as_slice()),
            Self::Any(_) => None,
        }
    }
}

/// Host-provided file access used when a runtime can read outbound media from local disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMediaAccess {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_roots: Option<Vec<PathBuf>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<PathBuf>,
    #[serde(default)]
    pub has_read_file: bool,
}

/// Legacy and current knobs accepted by outbound media loaders before normalization.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMediaLoadParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_access: Option<OutboundMediaAccess>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_local_roots: Option<OutboundMediaLocalRoots>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_explicit_proxy_dns: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimize_images: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<PathBuf>,
    #[serde(default)]
    pub has_read_file: bool,
}

/// Normalized outbound media loader options consumed by fetch/local media helpers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboundMediaLoadOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_roots: Option<OutboundMediaLocalRoots>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust_explicit_proxy_dns: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimize_images: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_dir: Option<PathBuf>,
    pub host_read_capability: bool,
}

/// Normalizes empty root lists while preserving the explicit all-roots opt-in sentinel.
pub fn resolve_outbound_media_local_roots(
    media_local_roots: Option<&OutboundMediaLocalRoots>,
) -> Option<OutboundMediaLocalRoots> {
    match media_local_roots {
        Some(OutboundMediaLocalRoots::Any(s)) => {
            if s.eq_ignore_ascii_case("any") {
                Some(OutboundMediaLocalRoots::Any("any".to_string()))
            } else {
                None
            }
        }
        Some(OutboundMediaLocalRoots::Explicit(roots)) => {
            if roots.is_empty() {
                None
            } else {
                Some(OutboundMediaLocalRoots::Explicit(roots.clone()))
            }
        }
        None => None,
    }
}

/// Collapses legacy read/root parameters into the current host media access shape.
pub fn resolve_outbound_media_access(
    media_access: Option<&OutboundMediaAccess>,
    media_local_roots: Option<&OutboundMediaLocalRoots>,
    has_legacy_read_file: bool,
) -> Option<OutboundMediaAccess> {
    let access_local_roots = media_access
        .and_then(|a| a.local_roots.as_ref())
        .map(|r| OutboundMediaLocalRoots::Explicit(r.clone()));
    let effective_roots_opt = access_local_roots.as_ref().or(media_local_roots);
    let resolved_roots = resolve_outbound_media_local_roots(effective_roots_opt);

    let local_roots = match resolved_roots {
        Some(OutboundMediaLocalRoots::Explicit(v)) => Some(v),
        Some(OutboundMediaLocalRoots::Any(_)) => None,
        None => None,
    };

    let has_read_file = media_access.map(|a| a.has_read_file).unwrap_or(false) || has_legacy_read_file;
    let workspace_dir = media_access.and_then(|a| a.workspace_dir.clone());

    if local_roots.is_none() && !has_read_file && workspace_dir.is_none() {
        return None;
    }

    Some(OutboundMediaAccess {
        local_roots,
        workspace_dir,
        has_read_file,
    })
}

/// Builds the canonical media load options shared by outbound attachment paths.
pub fn build_outbound_media_load_options(
    params: &OutboundMediaLoadParams,
) -> Result<OutboundMediaLoadOptions, String> {
    let explicit_local_roots = resolve_outbound_media_local_roots(params.media_local_roots.as_ref());
    let media_access = resolve_outbound_media_access(
        params.media_access.as_ref(),
        if explicit_local_roots.as_ref().map(|r| r.is_any()).unwrap_or(false) {
            None
        } else {
            explicit_local_roots.as_ref()
        },
        if params.media_access.as_ref().map(|a| a.has_read_file).unwrap_or(false) {
            false
        } else {
            params.has_read_file
        },
    );

    let workspace_dir = media_access
        .as_ref()
        .and_then(|a| a.workspace_dir.clone())
        .or_else(|| params.workspace_dir.clone());

    let has_read_file = media_access
        .as_ref()
        .map(|a| a.has_read_file)
        .unwrap_or(params.has_read_file);

    let local_roots = media_access
        .as_ref()
        .and_then(|a| a.local_roots.as_ref().map(|r| OutboundMediaLocalRoots::Explicit(r.clone())))
        .or(explicit_local_roots);

    if has_read_file && local_roots.is_none() {
        return Err(
            "Host media read requires explicit localRoots. Pass mediaAccess.localRoots or opt in with localRoots: \"any\"."
                .to_string(),
        );
    }

    Ok(OutboundMediaLoadOptions {
        max_bytes: params.max_bytes,
        local_roots,
        proxy_url: params.proxy_url.clone(),
        trust_explicit_proxy_dns: params.trust_explicit_proxy_dns,
        optimize_images: params.optimize_images,
        workspace_dir,
        host_read_capability: has_read_file,
    })
}
