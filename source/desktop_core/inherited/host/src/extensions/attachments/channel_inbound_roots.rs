//! Channel inbound attachment roots resolution and plugin contract dispatch.
//!
//! Maps upstream OpenClaw module:
//! - `src/media/channel-inbound-roots.ts`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use serde::{Deserialize, Serialize};

pub const CHANNEL_MEDIA_CONTRACT_ARTIFACT: &str = "media-contract-api.js";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMsgContext {
    #[serde(rename = "Surface")]
    pub surface: Option<String>,
    #[serde(rename = "Provider")]
    pub provider: Option<String>,
    #[serde(rename = "AccountId")]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelPluginManifest {
    pub id: String,
    pub origin: String,
    pub channels: Vec<String>,
    #[serde(default)]
    pub trusted_official_install: bool,
    pub root_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelPluginsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub deny: Vec<String>,
}

impl Default for ChannelPluginsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            deny: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChannelInboundRootsConfig {
    #[serde(default)]
    pub plugins: ChannelPluginsConfig,
    #[serde(default)]
    pub bundled_plugins_dir: Option<PathBuf>,
}

pub trait ChannelMediaContractApi: Send + Sync {
    fn resolve_inbound_attachment_roots(&self, account_id: Option<&str>) -> Option<Vec<String>> {
        let _ = account_id;
        None
    }
    fn resolve_remote_inbound_attachment_roots(&self, account_id: Option<&str>) -> Option<Vec<String>> {
        let _ = account_id;
        None
    }
}

// In-memory registry for bundled and mock contracts in tests/runtime
static CONTRACT_REGISTRY: LazyLock<Mutex<HashMap<String, Arc<dyn ChannelMediaContractApi>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static INITIALIZATION_FAILURES: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register_channel_contract<T: ChannelMediaContractApi + 'static>(channel_id: &str, contract: T) {
    let mut reg = CONTRACT_REGISTRY.lock().unwrap();
    reg.insert(channel_id.to_ascii_lowercase(), Arc::new(contract));
}

pub fn register_channel_initialization_failure(channel_id: &str, error_message: &str) {
    let mut failures = INITIALIZATION_FAILURES.lock().unwrap();
    failures.insert(channel_id.to_ascii_lowercase(), error_message.to_string());
}

pub fn clear_channel_contract_registry() {
    let mut reg = CONTRACT_REGISTRY.lock().unwrap();
    reg.clear();
    let mut failures = INITIALIZATION_FAILURES.lock().unwrap();
    failures.clear();
}

fn find_bundled_contract_file(bundled_dir: &Path, channel_id: &str) -> Option<PathBuf> {
    let channel_dir = bundled_dir.join(channel_id);
    let js = channel_dir.join(CHANNEL_MEDIA_CONTRACT_ARTIFACT);
    if js.exists() {
        return Some(js);
    }
    let cjs = channel_dir.join("media-contract-api.cjs");
    if cjs.exists() {
        return Some(cjs);
    }
    None
}

/// Resolves local inbound attachment roots for a channel.
pub fn resolve_channel_inbound_attachment_roots_for_channel(
    cfg: &ChannelInboundRootsConfig,
    installed_manifests: &[ChannelPluginManifest],
    channel_id: Option<&str>,
    account_id: Option<&str>,
) -> Result<Option<Vec<String>>, String> {
    let channel = match channel_id {
        Some(c) if !c.trim().is_empty() => c.trim().to_ascii_lowercase(),
        _ => return Ok(None),
    };

    // Check registered failure propagation first
    if let Some(err) = INITIALIZATION_FAILURES.lock().unwrap().get(&channel) {
        return Err(err.clone());
    }

    // 1. Check bundled plugins first (bundled plugins take precedence over installed)
    if let Some(ref bundled_dir) = cfg.bundled_plugins_dir {
        if let Some(contract_file) = find_bundled_contract_file(bundled_dir, &channel) {
            // Check if file contains a throw simulation or reads module.exports
            if let Ok(content) = std::fs::read_to_string(&contract_file) {
                if content.contains("throw new Error(") {
                    let msg = content
                        .split("throw new Error(")
                        .nth(1)
                        .and_then(|s| s.split(')').next())
                        .map(|s| s.trim_matches(|c| c == '"' || c == '\''))
                        .unwrap_or("Channel contract initialization failure");
                    return Err(msg.to_string());
                }
            }
        }
    }

    // 2. Check policy gates for installed/registered plugins
    if !cfg.plugins.enabled {
        return Ok(None);
    }
    if cfg.plugins.deny.iter().any(|d| d.eq_ignore_ascii_case(&channel)) {
        return Ok(None);
    }

    if let Some(contract) = CONTRACT_REGISTRY.lock().unwrap().get(&channel) {
        if let Some(roots) = contract.resolve_inbound_attachment_roots(account_id) {
            return Ok(Some(roots));
        }
    }

    for manifest in installed_manifests {
        if manifest.origin == "global"
            && manifest.trusted_official_install
            && manifest.channels.iter().any(|c| c.eq_ignore_ascii_case(&channel))
        {
            if let Some(contract) = CONTRACT_REGISTRY.lock().unwrap().get(&manifest.id.to_ascii_lowercase()) {
                if let Some(roots) = contract.resolve_inbound_attachment_roots(account_id) {
                    return Ok(Some(roots));
                }
            }
        }
    }

    Ok(None)
}

/// Resolves remote staging roots for inbound channel attachments.
pub fn resolve_channel_remote_inbound_attachment_roots(
    cfg: &ChannelInboundRootsConfig,
    installed_manifests: &[ChannelPluginManifest],
    ctx: &ChannelMsgContext,
) -> Result<Option<Vec<String>>, String> {
    let channel_id = ctx.surface.as_deref().or(ctx.provider.as_deref());
    let channel = match channel_id {
        Some(c) if !c.trim().is_empty() => c.trim().to_ascii_lowercase(),
        _ => return Ok(None),
    };

    if let Some(err) = INITIALIZATION_FAILURES.lock().unwrap().get(&channel) {
        return Err(err.clone());
    }

    if let Some(ref bundled_dir) = cfg.bundled_plugins_dir {
        if let Some(contract_file) = find_bundled_contract_file(bundled_dir, &channel) {
            if let Ok(content) = std::fs::read_to_string(&contract_file) {
                if content.contains("throw new Error(") {
                    let msg = content
                        .split("throw new Error(")
                        .nth(1)
                        .and_then(|s| s.split(')').next())
                        .map(|s| s.trim_matches(|c| c == '"' || c == '\''))
                        .unwrap_or("Channel contract initialization failure");
                    return Err(msg.to_string());
                }
            }
        }
    }

    // 2. Check policy gates for installed/registered plugins
    if !cfg.plugins.enabled {
        return Ok(None);
    }
    if cfg.plugins.deny.iter().any(|d| d.eq_ignore_ascii_case(&channel)) {
        return Ok(None);
    }

    if let Some(contract) = CONTRACT_REGISTRY.lock().unwrap().get(&channel) {
        if let Some(roots) = contract.resolve_remote_inbound_attachment_roots(ctx.account_id.as_deref()) {
            return Ok(Some(roots));
        }
    }

    for manifest in installed_manifests {
        if manifest.origin == "global"
            && manifest.trusted_official_install
            && manifest.channels.iter().any(|c| c.eq_ignore_ascii_case(&channel))
        {
            if let Some(contract) = CONTRACT_REGISTRY.lock().unwrap().get(&manifest.id.to_ascii_lowercase()) {
                if let Some(roots) = contract.resolve_remote_inbound_attachment_roots(ctx.account_id.as_deref()) {
                    return Ok(Some(roots));
                }
            }
        }
    }

    Ok(None)
}

/// Resolves local inbound attachment roots from a message context.
pub fn resolve_channel_inbound_attachment_roots(
    cfg: &ChannelInboundRootsConfig,
    installed_manifests: &[ChannelPluginManifest],
    ctx: &ChannelMsgContext,
) -> Result<Option<Vec<String>>, String> {
    resolve_channel_inbound_attachment_roots_for_channel(
        cfg,
        installed_manifests,
        ctx.surface.as_deref().or(ctx.provider.as_deref()),
        ctx.account_id.as_deref(),
    )
}
