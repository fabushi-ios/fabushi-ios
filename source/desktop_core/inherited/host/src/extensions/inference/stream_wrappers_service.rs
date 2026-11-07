//! OpenClaw LLM Provider Stream Wrappers, Cache Semantics, Tool Compat, and Tier Observation
//!
//! Mirrors:
//! - `src/llm/providers/stream-wrappers/anthropic-family-cache-semantics.ts`
//! - `src/llm/providers/stream-wrappers/anthropic-family-tool-payload-compat.ts`
//! - `src/llm/providers/stream-wrappers/openai-service-tier-observation.ts`
//! - `src/llm/providers/stream-wrappers/google-thinking-payload.ts`
//! - `src/llm/providers/stream-wrappers/stream-payload-utils.ts`
//! - `src/llm/providers/openai-fast-mode.ts`
//! - `src/llm/providers/minimax-fast-mode.ts`

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnthropicCacheFamily {
    AnthropicDirect,
    AnthropicBedrock,
    CustomAnthropicApi,
}

/// Matches Bedrock Application Inference Profile ARNs across all AWS partitions
pub fn is_anthropic_bedrock_model(model_id: &str) -> bool {
    let lower = model_id.to_ascii_lowercase();

    if lower.contains("anthropic.claude") || lower.contains("anthropic/claude") {
        return true;
    }

    let bedrock_re = Regex::new(r"^arn:aws(-cn|-us-gov)?:bedrock:.*:application-inference-profile/").unwrap();
    if bedrock_re.is_match(&lower) && lower.contains("claude") {
        return true;
    }

    false
}

pub fn resolve_anthropic_cache_family(
    model_id: &str,
    api: &str,
    base_url: Option<&str>,
) -> AnthropicCacheFamily {
    if is_anthropic_bedrock_model(model_id) || api.contains("bedrock") {
        AnthropicCacheFamily::AnthropicBedrock
    } else if let Some(url) = base_url {
        if url.contains("api.anthropic.com") {
            AnthropicCacheFamily::AnthropicDirect
        } else {
            AnthropicCacheFamily::CustomAnthropicApi
        }
    } else {
        AnthropicCacheFamily::AnthropicDirect
    }
}

/// Normalizes empty schema or incomplete schema into valid JSON Schema object for Anthropic
pub fn normalize_anthropic_tool_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) if map.is_empty() => {
            json!({
                "type": "object",
                "properties": {}
            })
        }
        Value::Object(map) => {
            let mut clone = map.clone();
            if !clone.contains_key("type") {
                clone.insert("type".to_string(), Value::String("object".to_string()));
            }
            if !clone.contains_key("properties") {
                clone.insert("properties".to_string(), json!({}));
            }
            Value::Object(clone)
        }
        _ => json!({
            "type": "object",
            "properties": {}
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiServiceTierObserver {
    pub account_route: String,
    pub requested_tier: Option<String>,
    pub response_tier: Option<String>,
    pub rejections: usize,
    pub cooloff_until_ms: u64,
}

impl OpenAiServiceTierObserver {
    pub fn new(account_route: impl Into<String>, requested_tier: Option<String>) -> Self {
        Self {
            account_route: account_route.into(),
            requested_tier,
            response_tier: None,
            rejections: 0,
            cooloff_until_ms: 0,
        }
    }

    pub fn record_observation(&mut self, response_tier: Option<String>) {
        self.response_tier = response_tier;
    }

    pub fn record_tier_rejection(&mut self, now_ms: u64, cooloff_duration_ms: u64) {
        self.rejections += 1;
        self.cooloff_until_ms = now_ms.saturating_add(cooloff_duration_ms);
    }

    pub fn effective_tier(&self, now_ms: u64) -> Option<String> {
        if now_ms < self.cooloff_until_ms {
            // Under cooloff after rejection, fallback to default (None / "default")
            return Some("default".to_string());
        }
        self.requested_tier.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoogleThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
}

pub fn is_google_thinking_required_model(model_id: &str) -> bool {
    model_id.to_ascii_lowercase().contains("gemini-2.5-pro")
}

pub fn resolve_google_thinking_level(
    model_id: &str,
    budget: Option<u32>,
) -> Option<GoogleThinkingLevel> {
    let lower = model_id.to_ascii_lowercase();
    if !lower.contains("gemini") {
        return None;
    }

    if let Some(b) = budget {
        if b == 0 && is_google_thinking_required_model(model_id) {
            // Gemini 2.5 Pro rejects budget 0
            return Some(GoogleThinkingLevel::Low);
        }
        if b <= 1024 {
            Some(GoogleThinkingLevel::Minimal)
        } else if b <= 4096 {
            Some(GoogleThinkingLevel::Low)
        } else if b <= 16384 {
            Some(GoogleThinkingLevel::Medium)
        } else {
            Some(GoogleThinkingLevel::High)
        }
    } else if is_google_thinking_required_model(model_id) {
        Some(GoogleThinkingLevel::Medium)
    } else {
        None
    }
}
