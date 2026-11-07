use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use super::{
    is_media_store_url, MediaUnderstandingCapability, NormalizedMediaAttachment,
};

pub const DEFAULT_MEDIA_RUNTIME_TIMEOUT_MS: u64 = 30_000;
pub const MIN_MEDIA_TIMEOUT_MS: u64 = 1_000;
pub const MAX_TIMER_TIMEOUT_MS: u64 = 2_147_483_647;
pub const DEFAULT_MEDIA_CONCURRENCY: usize = 2;
pub const CLI_OUTPUT_MAX_BUFFER: usize = 10 * 1024 * 1024;

pub fn coerce_openai_compatible_video_text(payload: &serde_json::Value) -> Option<String> {
    let message = payload.get("choices")?.get(0)?.get("message")?;
    if let Some(content) = message.get("content") {
        if let Some(s) = content.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        } else if let Some(arr) = content.as_array() {
            let parts: Vec<&str> = arr
                .iter()
                .filter_map(|p| p.get("text")?.as_str().map(str::trim))
                .filter(|s| !s.is_empty())
                .collect();
            if !parts.is_empty() {
                return Some(parts.join("\n"));
            }
        }
    }
    if let Some(reasoning) = message.get("reasoning_content").and_then(|r| r.as_str()) {
        let trimmed = reasoning.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

pub fn build_openai_compatible_video_request_body(
    model: &str,
    prompt: &str,
    mime: &str,
    buffer_base64: &str,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": prompt },
                    {
                        "type": "video_url",
                        "video_url": {
                            "url": format!("data:{mime};base64,{buffer_base64}")
                        }
                    }
                ]
            }
        ]
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaUnderstandingModelConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub entry_type: Option<String>, // "provider" or "cli"
    pub command: Option<String>,
    pub capabilities: Option<Vec<MediaUnderstandingCapability>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaUnderstandingProviderEntry {
    pub id: String,
    pub capabilities: Option<Vec<MediaUnderstandingCapability>>,
    pub shared_entry_capabilities: Option<Vec<MediaUnderstandingCapability>>,
    pub inherit_configured_image_capability: bool,
}

pub fn resolve_configured_media_entry_capabilities(
    entry: &MediaUnderstandingModelConfig,
) -> Option<Vec<MediaUnderstandingCapability>> {
    let caps = entry.capabilities.as_ref()?;
    if caps.is_empty() {
        None
    } else {
        Some(caps.clone())
    }
}

pub fn resolve_effective_media_entry_capabilities(
    entry: &MediaUnderstandingModelConfig,
    provider_registry: &HashMap<String, MediaUnderstandingProviderEntry>,
) -> Option<Vec<MediaUnderstandingCapability>> {
    if let Some(configured) = resolve_configured_media_entry_capabilities(entry) {
        return Some(configured);
    }
    let is_cli = entry.entry_type.as_deref() == Some("cli")
        || (entry.entry_type.is_none() && entry.command.is_some());
    if is_cli {
        return None;
    }
    let provider_id = entry.provider.as_deref()?.trim().to_ascii_lowercase();
    if provider_id.is_empty() {
        return None;
    }
    let provider = provider_registry.get(&provider_id)?;
    match &provider.shared_entry_capabilities {
        None => provider.capabilities.clone(),
        Some(shared) => {
            let effective = provider.capabilities.as_ref()?;
            let filtered: Vec<MediaUnderstandingCapability> = shared
                .iter()
                .filter(|c| effective.contains(c))
                .copied()
                .collect();
            Some(filtered)
        }
    }
}

pub fn matches_media_entry_capability(
    entry: &MediaUnderstandingModelConfig,
    capability: MediaUnderstandingCapability,
    provider_registry: &HashMap<String, MediaUnderstandingProviderEntry>,
) -> bool {
    resolve_effective_media_entry_capabilities(entry, provider_registry)
        .map(|caps| caps.contains(&capability))
        .unwrap_or(false)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaUnderstandingProvider {
    pub id: String,
    pub capabilities: Vec<MediaUnderstandingCapability>,
    pub default_models: HashMap<MediaUnderstandingCapability, String>,
    pub auto_priority: HashMap<MediaUnderstandingCapability, i32>,
    pub shared_entry_capabilities: Option<Vec<MediaUnderstandingCapability>>,
    pub inherit_configured_image_capability: bool,
}

pub fn normalize_media_provider_id(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

pub fn resolve_default_media_model_from_registry(
    provider_id: &str,
    capability: MediaUnderstandingCapability,
    registry: &HashMap<String, MediaUnderstandingProvider>,
) -> Option<String> {
    let normalized = normalize_media_provider_id(provider_id);
    let provider = registry.get(&normalized)?;
    provider.default_models.get(&capability).cloned()
}

pub fn resolve_auto_media_key_providers_from_registry(
    capability: MediaUnderstandingCapability,
    registry: &HashMap<String, MediaUnderstandingProvider>,
) -> Vec<String> {
    let mut entries: Vec<(&MediaUnderstandingProvider, i32)> = registry
        .values()
        .filter(|p| p.capabilities.contains(&capability))
        .filter_map(|p| p.auto_priority.get(&capability).map(|&prio| (p, prio)))
        .collect();

    entries.sort_by(|(p1, prio1), (p2, prio2)| {
        prio1.cmp(prio2).then_with(|| p1.id.cmp(&p2.id))
    });

    entries
        .into_iter()
        .map(|(p, _)| normalize_media_provider_id(&p.id))
        .collect()
}

pub fn build_media_understanding_registry(
    base_providers: &[MediaUnderstandingProvider],
    image_capable_providers: &[String],
    overrides: &HashMap<String, MediaUnderstandingProvider>,
) -> HashMap<String, MediaUnderstandingProvider> {
    let mut registry = HashMap::new();
    for p in base_providers {
        let key = normalize_media_provider_id(&p.id);
        registry.insert(key, p.clone());
    }
    for configured_id in image_capable_providers {
        let key = normalize_media_provider_id(configured_id);
        registry.entry(key.clone()).or_insert_with(|| MediaUnderstandingProvider {
            id: key,
            capabilities: vec![MediaUnderstandingCapability::Image],
            default_models: HashMap::new(),
            auto_priority: HashMap::new(),
            shared_entry_capabilities: None,
            inherit_configured_image_capability: false,
        });
    }
    for (k, v) in overrides {
        let key = normalize_media_provider_id(k);
        registry.insert(key, v.clone());
    }
    registry
}

pub fn resolve_provider_request_context(
    provider_base_url: Option<&str>,
    tools_base_url: Option<&str>,
    entry_base_url: Option<&str>,
    provider_headers: Option<&HashMap<String, String>>,
    tools_headers: Option<&HashMap<String, String>>,
    entry_headers: Option<&HashMap<String, String>>,
) -> (Option<String>, HashMap<String, String>) {
    let base_url = entry_base_url
        .or(tools_base_url)
        .or(provider_base_url)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut merged_headers = HashMap::new();
    if let Some(h) = provider_headers {
        for (k, v) in h {
            merged_headers.insert(k.clone(), v.clone());
        }
    }
    if let Some(h) = tools_headers {
        for (k, v) in h {
            merged_headers.insert(k.clone(), v.clone());
        }
    }
    if let Some(h) = entry_headers {
        for (k, v) in h {
            merged_headers.insert(k.clone(), v.clone());
        }
    }
    (base_url, merged_headers)
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum MediaCliModelUnavailableReason {
    MissingCommand,
    MissingAttachmentArg,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct MediaCliModelUnavailableError {
    pub reason: MediaCliModelUnavailableReason,
    pub message: String,
}

pub fn resolve_cli_model_entry(
    command: Option<&str>,
    args: Option<&[String]>,
) -> Result<(String, Vec<String>), MediaCliModelUnavailableError> {
    let cmd = command.map(str::trim).filter(|s| !s.is_empty());
    let cmd = match cmd {
        Some(c) => c.to_string(),
        None => {
            return Err(MediaCliModelUnavailableError {
                reason: MediaCliModelUnavailableReason::MissingCommand,
                message: "cli-missing-command; Set command to the media executable and args to pass the attachment, for example [\"{{AttachmentPath}}\"].".to_string(),
            });
        }
    };
    let args_vec = match args {
        Some(a) if !a.is_empty() => a.to_vec(),
        _ => {
            return Err(MediaCliModelUnavailableError {
                reason: MediaCliModelUnavailableReason::MissingAttachmentArg,
                message: "cli-missing-attachment-arg; Set args to pass the attachment, for example [\"{{AttachmentPath}}\"]. CLI stdin is not supplied.".to_string(),
            });
        }
    };
    Ok((cmd, args_vec))
}

pub fn resolve_timeout_ms(seconds: Option<f64>, fallback_seconds: f64) -> u64 {
    let val = match seconds {
        Some(s) if s.is_finite() && s > 0.0 => s,
        Some(_) => return MIN_MEDIA_TIMEOUT_MS,
        None if fallback_seconds.is_finite() && fallback_seconds > 0.0 => fallback_seconds,
        None => return MIN_MEDIA_TIMEOUT_MS,
    };
    let ms = (val * 1000.0).floor();
    if !ms.is_finite() || ms >= MAX_TIMER_TIMEOUT_MS as f64 {
        MAX_TIMER_TIMEOUT_MS
    } else if ms <= MIN_MEDIA_TIMEOUT_MS as f64 {
        MIN_MEDIA_TIMEOUT_MS
    } else {
        ms as u64
    }
}

pub fn resolve_media_runtime_timeout_ms(timeout_ms: Option<u64>) -> u64 {
    match timeout_ms {
        Some(ms) => {
            if ms > MAX_TIMER_TIMEOUT_MS {
                MAX_TIMER_TIMEOUT_MS
            } else if ms < MIN_MEDIA_TIMEOUT_MS {
                MIN_MEDIA_TIMEOUT_MS
            } else {
                ms
            }
        }
        None => DEFAULT_MEDIA_RUNTIME_TIMEOUT_MS,
    }
}

pub fn resolve_entry_run_prompt(
    base_prompt: Option<&str>,
    capability_default_prompt: &str,
    max_chars: Option<usize>,
    is_audio: bool,
) -> (String, bool) {
    let configured = base_prompt.map(str::trim).filter(|s| !s.is_empty());
    let has_configured = configured.is_some();
    let prompt_body = configured.unwrap_or(capability_default_prompt);
    let prompt = if let Some(chars) = max_chars {
        if !is_audio {
            format!("{} Respond in at most {} characters.", prompt_body, chars)
        } else {
            prompt_body.to_string()
        }
    } else {
        prompt_body.to_string()
    };
    (prompt, has_configured)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaScopeRuleMatch {
    pub channel: Option<String>,
    pub chat_type: Option<String>,
    pub key_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaScopeRule {
    pub match_rule: MediaScopeRuleMatch,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaUnderstandingScopeConfig {
    pub default_action: Option<String>,
    pub rules: Vec<MediaScopeRule>,
}

pub struct ScopeContext<'a> {
    pub surface: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub chat_type: Option<&'a str>,
    pub session_key: Option<&'a str>,
}

pub fn resolve_scope_decision(
    scope: Option<&MediaUnderstandingScopeConfig>,
    ctx: &ScopeContext,
) -> &'static str {
    let scope = match scope {
        Some(s) => s,
        None => return "allow",
    };
    let channel = ctx.surface.or(ctx.provider).map(|s| s.trim().to_lowercase());
    let chat_type = ctx.chat_type.map(|s| s.trim().to_lowercase());
    let session_key = ctx.session_key.map(|s| s.trim().to_lowercase()).unwrap_or_default();

    for rule in &scope.rules {
        if let Some(ref mc) = rule.match_rule.channel {
            if channel.as_deref() != Some(mc.trim().to_lowercase().as_str()) {
                continue;
            }
        }
        if let Some(ref mct) = rule.match_rule.chat_type {
            if chat_type.as_deref() != Some(mct.trim().to_lowercase().as_str()) {
                continue;
            }
        }
        if let Some(ref prefix) = rule.match_rule.key_prefix {
            if !session_key.starts_with(&prefix.trim().to_lowercase()) {
                continue;
            }
        }
        return if rule.action.eq_ignore_ascii_case("deny") {
            "deny"
        } else {
            "allow"
        };
    }
    match scope.default_action.as_deref() {
        Some(act) if act.eq_ignore_ascii_case("deny") => "deny",
        _ => "allow",
    }
}

pub fn preferred_media_model_rank(
    is_cli: bool,
    command: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
    preferred: &str,
) -> usize {
    if is_cli || command.is_some() {
        let cmd = command.unwrap_or("");
        if preferred == format!("cli:{}", cmd) {
            return 2;
        }
        return 0;
    }
    let p = provider.map(str::trim).unwrap_or("");
    let m = model.map(str::trim).unwrap_or("");
    if m.is_empty() {
        if preferred == format!("provider:{}", p) {
            return 2;
        }
        return 0;
    }
    if preferred == format!("{}/{}", p, m) {
        return 2;
    }
    if preferred == m {
        return 1;
    }
    0
}

pub fn resolve_model_entry_secret_owner_id(shared_index: usize) -> String {
    format!("media-model:shared:{}", shared_index)
}

pub fn normalize_decision_reason(reason: Option<&str>) -> Option<String> {
    let trimmed = reason?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = if let Some(rest) = trimmed.strip_prefix("Error:") {
        rest.trim()
    } else if let Some(rest) = trimmed.strip_prefix("ProviderHttpError:") {
        rest.trim()
    } else {
        trimmed
    };
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.to_string())
    }
}

pub fn summarize_decision_reason(reason: Option<&str>) -> Option<String> {
    let normalized = normalize_decision_reason(reason)?;
    let first = normalized.split(':').next()?.trim();
    let first = first.split(" [request_id=").next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

pub fn format_decision_summary(
    capability: &str,
    outcome: &str,
    success_count: usize,
    total_count: usize,
    chosen_provider: Option<&str>,
    chosen_model: Option<&str>,
    observed_backend: Option<&str>,
    requested_backend: Option<&str>,
    reason: Option<&str>,
) -> String {
    let count_label = if total_count > 0 {
        format!(" ({}/{})", success_count, total_count)
    } else {
        String::new()
    };
    let model_label = match (chosen_provider, chosen_model) {
        (Some(p), Some(m)) if !m.is_empty() && m != p => format!("{}/{}", p, m),
        (Some(p), _) => p.to_string(),
        _ => String::new(),
    };
    let backend_label = if let Some(b) = observed_backend {
        format!(" observed={}", b)
    } else if let Some(b) = requested_backend {
        format!(" requested={}", b)
    } else {
        String::new()
    };
    let via_label = if !model_label.is_empty() {
        format!(" via {}{}", model_label, backend_label)
    } else {
        String::new()
    };
    let reason_label = if let Some(short) = summarize_decision_reason(reason) {
        format!(" reason={}", short)
    } else {
        String::new()
    };
    format!("{}: {}{}{}{}", capability, outcome, count_label, via_label, reason_label)
}

pub fn is_native_media_deliverable(has_path: bool, url: Option<&str>) -> bool {
    if has_path {
        return true;
    }
    if let Some(u) = url {
        if is_media_store_url(u) {
            return true;
        }
    }
    false
}

pub fn normalize_media_attachments_with_sticker_rule(
    attachments: Vec<NormalizedMediaAttachment>,
    skip_sticker: bool,
) -> Vec<NormalizedMediaAttachment> {
    if skip_sticker {
        attachments.into_iter().filter(|a| a.index != 0).collect()
    } else {
        attachments
    }
}
