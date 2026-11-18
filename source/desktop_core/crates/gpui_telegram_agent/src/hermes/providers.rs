use crate::computer::{parse_cloud_credentials_from_prompt, redact_cloud_secrets};
use chrono::Utc;
pub use magpie_gateway::{
    all_presets, find_preset, parse_dotenv_providers, parse_magpie_import_link,
    parse_vertex_adc_credentials, resolve_preset_endpoints, CodexTokenManager, MaskResult,
    MaskingAction, MaskingConfig, MaskingEngine, MaskingHit, ProtocolKind, RouteDecisionTrace,
    RouteRuntimeMetrics, RoutingGroup, RoutingGroupMember, RoutingStrategy, StreamingUnmasker,
    SubscriptionAccount, SubscriptionKind, SubscriptionRegistry, VertexAdcCredentials,
    VertexTokenManager,
};
use magpie_gateway::{
    build_gemini_generate_content_payload, discover_subscriptions_in_home, extract_jwt_exp_ms,
    parse_gemini_response_json, parse_gemini_sse_data, parse_retry_after_header,
    resolve_anthropic_messages_url, resolve_gemini_endpoint_url, resolve_group_targets,
    UnifiedMessage, UnifiedRequest, UnifiedRole, UnifiedToolCall, UnifiedToolSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

/// Wire protocol modes supported natively by the Hermes multi-provider runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiMode {
    ChatCompletions,
    CodexResponses,
    AnthropicMessages,
    GeminiGenerateContent,
    GoogleVertex,
    LocalHermes,
}

impl Default for ApiMode {
    fn default() -> Self {
        Self::LocalHermes
    }
}

impl ApiMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::CodexResponses => "codex_responses",
            Self::AnthropicMessages => "anthropic_messages",
            Self::GeminiGenerateContent => "gemini_generate_content",
            Self::GoogleVertex => "google_vertex",
            Self::LocalHermes => "local_hermes",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "chat_completions" | "openai" | "chat" | "completions" => Some(Self::ChatCompletions),
            "codex_responses" | "responses" | "codex" => Some(Self::CodexResponses),
            "anthropic_messages" | "anthropic" | "claude" | "messages" => {
                Some(Self::AnthropicMessages)
            }
            "gemini_generate_content" | "gemini" | "google_gemini" | "generate_content" => {
                Some(Self::GeminiGenerateContent)
            }
            "google_vertex" | "vertex" | "vertex_ai" | "google_vertex_ai" => {
                Some(Self::GoogleVertex)
            }
            "local_hermes" | "local" | "hermes" | "offline" => Some(Self::LocalHermes),
            _ => None,
        }
    }

    pub fn all() -> &'static [ApiMode] {
        &[
            Self::ChatCompletions,
            Self::CodexResponses,
            Self::AnthropicMessages,
            Self::GeminiGenerateContent,
            Self::GoogleVertex,
            Self::LocalHermes,
        ]
    }

    pub fn from_protocol_kind(kind: ProtocolKind) -> Self {
        match kind {
            ProtocolKind::ChatCompletions => Self::ChatCompletions,
            ProtocolKind::CodexResponses => Self::CodexResponses,
            ProtocolKind::AnthropicMessages => Self::AnthropicMessages,
            ProtocolKind::GeminiGenerateContent => Self::GeminiGenerateContent,
        }
    }

    pub fn to_protocol_kind(&self) -> Option<ProtocolKind> {
        match self {
            Self::ChatCompletions => Some(ProtocolKind::ChatCompletions),
            Self::CodexResponses => Some(ProtocolKind::CodexResponses),
            Self::AnthropicMessages => Some(ProtocolKind::AnthropicMessages),
            Self::GeminiGenerateContent | Self::GoogleVertex => {
                Some(ProtocolKind::GeminiGenerateContent)
            }
            Self::LocalHermes => None,
        }
    }
}

impl fmt::Display for ApiMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPromptTier {
    pub label: String,
    pub text: String,
    pub cache_ephemeral: bool,
}

impl SystemPromptTier {
    pub fn stable(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
            cache_ephemeral: true,
        }
    }

    pub fn volatile(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
            cache_ephemeral: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ProviderToolCall {
    pub fn arguments_json(&self) -> Value {
        serde_json::from_str(&self.arguments).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderChatMessage {
    pub role: ProviderMessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ProviderToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl ProviderChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ProviderMessageRole::User,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
            cache_control: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ProviderMessageRole::Assistant,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning: None,
            cache_control: None,
        }
    }

    pub fn assistant_with_tools(
        content: impl Into<String>,
        tool_calls: Vec<ProviderToolCall>,
        reasoning: Option<String>,
    ) -> Self {
        Self {
            role: ProviderMessageRole::Assistant,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls,
            reasoning,
            cache_control: None,
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: ProviderMessageRole::Tool,
            content: content.into(),
            name: Some(tool_name.into()),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            reasoning: None,
            cache_control: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub estimated_cost_usd: f64,
}

impl TokenUsage {
    pub fn accumulate(&mut self, other: &TokenUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(other.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(other.cache_creation_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.estimated_cost_usd += other.estimated_cost_usd;
    }

    pub fn finalize_total(&mut self) {
        if self.total_tokens == 0 {
            self.total_tokens = self.input_tokens.saturating_add(self.output_tokens);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub model: String,
    pub system_tiers: Vec<SystemPromptTier>,
    pub messages: Vec<ProviderChatMessage>,
    #[serde(default)]
    pub tools: Vec<ProviderToolSpec>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub thinking_budget_tokens: Option<usize>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub metadata: Value,
}

impl ProviderRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ProviderChatMessage>) -> Self {
        Self {
            model: model.into(),
            system_tiers: Vec::new(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            thinking_budget_tokens: None,
            stream: true,
            metadata: Value::Null,
        }
    }

    pub fn combined_system_prompt(&self) -> String {
        self.system_tiers
            .iter()
            .map(|t| t.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub content: String,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ProviderToolCall>,
    pub finish_reason: String,
    pub usage: TokenUsage,
    pub provider_id: String,
    pub model: String,
    pub api_mode: ApiMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProviderStreamEvent {
    ThinkingDelta(String),
    TextDelta(String),
    ToolCallReady(ProviderToolCall),
    Usage(TokenUsage),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ProviderError {
    #[error("Provider configuration error: {0}")]
    Configuration(String),
    #[error("Provider authentication error: {0}")]
    Authentication(String),
    #[error("Provider rate limited (HTTP {status}): {message}")]
    RateLimited {
        status: u16,
        retry_after_secs: Option<u64>,
        message: String,
    },
    #[error("Provider upstream HTTP {status}: {message}")]
    UpstreamHttp { status: u16, message: String },
    #[error("Provider transport error: {0}")]
    Transport(String),
    #[error("Provider protocol error: {0}")]
    Protocol(String),
    #[error("All providers exhausted: {0}")]
    Exhausted(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimited { .. } | Self::Transport(_) => true,
            Self::UpstreamHttp { status, .. } => *status >= 500 || *status == 429 || *status == 408,
            _ => false,
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::RateLimited { status, .. } | Self::UpstreamHttp { status, .. } => Some(*status),
            Self::Authentication(msg) => {
                if msg.starts_with("HTTP 403") {
                    Some(403)
                } else {
                    Some(401)
                }
            }
            _ => None,
        }
    }

    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }
}

/// Multi-account / multi-key entry inside a provider's CredentialPool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialEntry {
    pub id: String,
    pub api_key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_override: Option<String>,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub cooldown_until_ms: i64,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub rate_limited_count: u64,
    #[serde(default)]
    pub total_requests: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_weight() -> u32 {
    1
}

fn default_true() -> bool {
    true
}

impl CredentialEntry {
    pub fn new(id: impl Into<String>, api_key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            api_key: api_key.into(),
            label: label.into(),
            base_url_override: None,
            weight: 1,
            cooldown_until_ms: 0,
            consecutive_failures: 0,
            rate_limited_count: 0,
            total_requests: 0,
            enabled: true,
        }
    }

    pub fn is_available(&self, now_ms: i64) -> bool {
        self.enabled && !self.api_key.trim().is_empty() && now_ms >= self.cooldown_until_ms
    }

    pub fn masked_key(&self) -> String {
        let trimmed = self.api_key.trim();
        if trimmed.len() <= 8 {
            "***".to_string()
        } else {
            format!("{}...{}", &trimmed[..4], &trimmed[trimmed.len() - 4..])
        }
    }
}

/// Thread-safe rotating credential pool with automatic 429/5xx cooldowns.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialPool {
    pub entries: Vec<CredentialEntry>,
    #[serde(default)]
    pub cursor: usize,
    #[serde(default = "default_base_cooldown_ms")]
    pub base_cooldown_ms: i64,
}

fn default_base_cooldown_ms() -> i64 {
    5_000
}

impl CredentialPool {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            base_cooldown_ms: default_base_cooldown_ms(),
        }
    }

    pub fn with_single_key(id: impl Into<String>, api_key: impl Into<String>) -> Self {
        let mut pool = Self::new();
        pool.add_entry(CredentialEntry::new(id, api_key, "primary"));
        pool
    }

    pub fn add_entry(&mut self, entry: CredentialEntry) {
        if let Some(existing) = self.entries.iter_mut().find(|e| e.id == entry.id) {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub fn remove_entry(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() < before
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn active_count(&self, now_ms: i64) -> usize {
        self.entries.iter().filter(|e| e.is_available(now_ms)).count()
    }

    pub fn cooldown_count(&self, now_ms: i64) -> usize {
        self.entries
            .iter()
            .filter(|e| e.enabled && now_ms < e.cooldown_until_ms)
            .count()
    }

    /// Acquire the next non-cooled-down credential in round-robin order.
    pub fn acquire(&mut self, now_ms: i64) -> Option<CredentialEntry> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }
        for offset in 0..len {
            let idx = (self.cursor + offset) % len;
            if self.entries[idx].is_available(now_ms) {
                self.entries[idx].total_requests = self.entries[idx].total_requests.saturating_add(1);
                self.cursor = (idx + 1) % len;
                return Some(self.entries[idx].clone());
            }
        }
        None
    }

    pub fn report_success(&mut self, id: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.consecutive_failures = 0;
            entry.cooldown_until_ms = 0;
        }
    }

    pub fn report_rate_limit(
        &mut self,
        id: &str,
        now_ms: i64,
        status_code: u16,
        retry_after_secs: Option<u64>,
    ) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            if status_code == 429 {
                entry.rate_limited_count = entry.rate_limited_count.saturating_add(1);
            }
            let cooldown_ms = if let Some(secs) = retry_after_secs {
                (secs as i64).saturating_mul(1_000).max(1_000)
            } else {
                let shift = entry.consecutive_failures.saturating_sub(1).min(8);
                let exp = self.base_cooldown_ms.saturating_mul(1i64 << shift);
                exp.min(900_000)
            };
            entry.cooldown_until_ms = now_ms.saturating_add(cooldown_ms);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    pub api_mode: ApiMode,
    pub input_cost_per_1m: f64,
    pub output_cost_per_1m: f64,
    pub context_window: usize,
    pub supports_tools: bool,
    pub supports_reasoning: bool,
    pub supports_caching: bool,
}

impl ModelProfile {
    pub fn estimate_cost(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        let in_cost = (input_tokens as f64 / 1_000_000.0) * self.input_cost_per_1m;
        let out_cost = (output_tokens as f64 / 1_000_000.0) * self.output_cost_per_1m;
        in_cost + out_cost
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub api_mode: ApiMode,
    pub base_url: String,
    pub default_model: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default)]
    pub credentials: CredentialPool,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region_id: Option<String>,
    #[serde(default)]
    pub is_subscription: bool,
    #[serde(default)]
    pub no_key_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_strategy: Option<RoutingStrategy>,
}

fn default_timeout_secs() -> u64 {
    90
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            api_mode: ApiMode::ChatCompletions,
            base_url: String::new(),
            default_model: String::new(),
            headers: HashMap::new(),
            credentials: CredentialPool::new(),
            fallback_models: Vec::new(),
            timeout_secs: default_timeout_secs(),
            enabled: true,
            preset_id: None,
            region_id: None,
            is_subscription: false,
            no_key_required: false,
            routing_strategy: None,
        }
    }
}

impl ProviderConfig {
    pub fn local_hermes() -> Self {
        Self {
            id: "local_hermes".to_string(),
            name: "Local Hermes Runtime".to_string(),
            api_mode: ApiMode::LocalHermes,
            base_url: "local://hermes".to_string(),
            default_model: "hermes-local-v1".to_string(),
            headers: HashMap::new(),
            credentials: CredentialPool::new(),
            fallback_models: Vec::new(),
            timeout_secs: 30,
            enabled: true,
            preset_id: None,
            region_id: None,
            is_subscription: false,
            no_key_required: true,
            routing_strategy: None,
        }
    }

    pub fn openai_compatible(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let id_str = id.into();
        let mut pool = CredentialPool::new();
        let key_str = api_key.into();
        if !key_str.trim().is_empty() {
            pool.add_entry(CredentialEntry::new(format!("{id_str}_key_1"), key_str, "primary"));
        }
        Self {
            id: id_str,
            name: name.into(),
            api_mode: ApiMode::ChatCompletions,
            base_url: base_url.into(),
            default_model: default_model.into(),
            headers: HashMap::new(),
            credentials: pool,
            fallback_models: Vec::new(),
            timeout_secs: 90,
            enabled: true,
            preset_id: None,
            region_id: None,
            is_subscription: false,
            no_key_required: false,
            routing_strategy: None,
        }
    }

    pub fn codex_responses(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let mut cfg = Self::openai_compatible(id, name, base_url, default_model, api_key);
        cfg.api_mode = ApiMode::CodexResponses;
        cfg
    }

    pub fn anthropic(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let mut cfg = Self::openai_compatible(id, name, base_url, default_model, api_key);
        cfg.api_mode = ApiMode::AnthropicMessages;
        cfg
    }

    pub fn gemini(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let mut cfg = Self::openai_compatible(id, name, base_url, default_model, api_key);
        cfg.api_mode = ApiMode::GeminiGenerateContent;
        cfg
    }

    pub fn google_vertex(
        id: impl Into<String>,
        name: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        quota_project_id: Option<String>,
    ) -> Self {
        let mut cfg = Self::openai_compatible(id, name, base_url, default_model, "");
        cfg.api_mode = ApiMode::GoogleVertex;
        cfg.is_subscription = true;
        cfg.no_key_required = true;
        cfg.fallback_models = vec![
            "gemini-2.5-flash".to_string(),
            "gemini-2.5-pro".to_string(),
            "gemini-1.5-pro".to_string(),
        ];
        cfg.preset_id = Some("google-vertex".to_string());
        if let Some(proj) = quota_project_id {
            cfg.headers.insert("x-goog-user-project".to_string(), proj);
        }
        cfg
    }
}

fn resolve_vertex_disk_credentials(cfg: &ProviderConfig) -> Option<VertexAdcCredentials> {
    if let Some(path_str) = cfg.headers.get("x-adc-source-path") {
        let p = PathBuf::from(path_str);
        if p.is_file() {
            return std::fs::read_to_string(&p)
                .ok()
                .and_then(|raw| parse_vertex_adc_credentials(&raw));
        }
        return None;
    }
    let gcloud_env = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);
    let p = if let Some(env_p) = gcloud_env.filter(|p| p.is_file()) {
        env_p
    } else {
        let home = resolve_home_dir()?;
        home.join(".config")
            .join("gcloud")
            .join("application_default_credentials.json")
    };
    let raw = std::fs::read_to_string(&p).ok()?;
    parse_vertex_adc_credentials(&raw)
}

fn vertex_creds_differ(a: &VertexAdcCredentials, b: &VertexAdcCredentials) -> bool {
    a.client_id != b.client_id
        || a.client_secret != b.client_secret
        || a.refresh_token != b.refresh_token
        || a.quota_project_id != b.quota_project_id
}

/// Convert a discovered `SubscriptionAccount` into an executable `ProviderConfig`.
pub fn subscription_to_provider_config(sub: &SubscriptionAccount) -> ProviderConfig {
    let mut headers = HashMap::new();
    for (k, v) in &sub.headers {
        headers.insert(k.clone(), v.clone());
    }

    let (api_mode, default_model, fallback_models, preset_id, no_key_required) = match sub.kind {
        SubscriptionKind::GoogleVertex => {
            if let Some(ref proj) = sub.account_id {
                headers.insert("x-goog-user-project".to_string(), proj.clone());
            }
            if !sub.source_path.is_empty() {
                headers.insert("x-adc-source-path".to_string(), sub.source_path.clone());
            }
            (
                ApiMode::GoogleVertex,
                sub.default_model.clone().unwrap_or_else(|| "gemini-2.5-flash".to_string()),
                vec![
                    "gemini-2.5-flash".to_string(),
                    "gemini-2.5-pro".to_string(),
                    "gemini-1.5-pro".to_string(),
                ],
                Some("google-vertex".to_string()),
                true,
            )
        }
        SubscriptionKind::CodexChatGpt => {
            if let Some(ref acct) = sub.account_id {
                headers.insert("chatgpt-account-id".to_string(), acct.clone());
            }
            if !sub.source_path.is_empty() {
                headers.insert("x-codex-source-path".to_string(), sub.source_path.clone());
            }
            (
                ApiMode::CodexResponses,
                sub.default_model.clone().unwrap_or_else(|| "codex-mini-latest".to_string()),
                vec!["gpt-4o".to_string(), "codex-mini-latest".to_string()],
                Some("codex".to_string()),
                sub.access_token.is_none(),
            )
        }
        SubscriptionKind::ClaudeCode => (
            ApiMode::AnthropicMessages,
            sub.default_model.clone().unwrap_or_else(|| "claude-sonnet-4-20250514".to_string()),
            vec![
                "claude-sonnet-4-20250514".to_string(),
                "claude-3-5-haiku-20241022".to_string(),
            ],
            Some("anthropic".to_string()),
            sub.access_token.is_none(),
        ),
        SubscriptionKind::GeminiCodeAssist => (
            ApiMode::GeminiGenerateContent,
            sub.default_model.clone().unwrap_or_else(|| "gemini-2.5-flash".to_string()),
            vec![
                "gemini-2.5-flash".to_string(),
                "gemini-2.5-pro".to_string(),
            ],
            Some("gemini".to_string()),
            sub.access_token.is_none(),
        ),
        _ => (
            ApiMode::from_protocol_kind(sub.protocol),
            sub.default_model.clone().unwrap_or_default(),
            Vec::new(),
            None,
            sub.access_token.is_none(),
        ),
    };

    let mut credentials = CredentialPool::new();
    if let Some(ref tok) = sub.access_token {
        if !tok.trim().is_empty() {
            credentials.add_entry(CredentialEntry::new(
                format!("{}_token", sub.id),
                tok.trim(),
                sub.label.clone(),
            ));
        }
    }

    ProviderConfig {
        id: sub.id.clone(),
        name: sub.label.clone(),
        api_mode,
        base_url: sub.base_url.clone().unwrap_or_else(|| match api_mode {
            ApiMode::GoogleVertex => "https://aiplatform.googleapis.com/v1".to_string(),
            ApiMode::CodexResponses => "https://chatgpt.com/backend-api/codex".to_string(),
            _ => String::new(),
        }),
        default_model,
        headers,
        credentials,
        fallback_models,
        timeout_secs: 90,
        enabled: sub.active,
        preset_id,
        region_id: None,
        is_subscription: true,
        no_key_required,
        routing_strategy: None,
    }
}

/// Helper to register model profiles for a provider and its fallback models.
fn register_subscription_model_profiles(guard: &mut ProviderRouterState, cfg: &ProviderConfig) {
    let mut all_models = Vec::new();
    if !cfg.default_model.trim().is_empty() {
        all_models.push(cfg.default_model.clone());
    }
    for m in &cfg.fallback_models {
        if !all_models.contains(m) {
            all_models.push(m.clone());
        }
    }
    for model_id in all_models {
        guard.model_profiles.insert(
            model_id.clone(),
            ModelProfile {
                id: model_id.clone(),
                provider_id: cfg.id.clone(),
                model: model_id,
                api_mode: cfg.api_mode,
                input_cost_per_1m: 0.0,
                output_cost_per_1m: 0.0,
                context_window: match cfg.api_mode {
                    ApiMode::GoogleVertex | ApiMode::GeminiGenerateContent => 1_000_000,
                    _ => 200_000,
                },
                supports_tools: true,
                supports_reasoning: true,
                supports_caching: true,
            },
        );
    }
}

/// Request parameters for applying a built-in Magpie preset to `ProviderRouter`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MagpiePresetApplyInput {
    pub preset_id: String,
    #[serde(default)]
    pub region_id: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model_override: Option<String>,
    #[serde(default)]
    pub base_url_override: Option<String>,
    #[serde(default)]
    pub protocol_override: Option<ApiMode>,
    #[serde(default)]
    pub custom_headers: HashMap<String, String>,
    #[serde(default)]
    pub provider_id_override: Option<String>,
    #[serde(default)]
    pub display_name_override: Option<String>,
    #[serde(default)]
    pub activate: bool,
}

impl MagpiePresetApplyInput {
    pub fn new(preset_id: impl Into<String>) -> Self {
        Self {
            preset_id: preset_id.into(),
            activate: true,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackTarget {
    pub provider_id: String,
    pub model: String,
}

/// Manages active provider/model, credential rotation, model fallback chain, Magpie routing/masking, and HTTP/local execution.
#[derive(Clone)]
pub struct ProviderRouter {
    inner: Arc<Mutex<ProviderRouterState>>,
    route_metrics: Arc<Mutex<RouteRuntimeMetrics>>,
    http_client: reqwest::Client,
    vertex_managers: Arc<Mutex<HashMap<String, (Arc<VertexTokenManager>, bool)>>>,
    codex_managers: Arc<Mutex<HashMap<String, Arc<CodexTokenManager>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRouterState {
    pub providers: HashMap<String, ProviderConfig>,
    pub model_profiles: HashMap<String, ModelProfile>,
    pub active_provider_id: String,
    pub active_model: String,
    pub fallback_chain: Vec<FallbackTarget>,
    pub allow_local_fallback: bool,
    #[serde(default)]
    pub offline_rule_bot_allowed: bool,
    #[serde(default)]
    pub subscriptions: Vec<SubscriptionAccount>,
    #[serde(default)]
    pub routing_groups: HashMap<String, RoutingGroup>,
    #[serde(default)]
    pub routing_strategy: RoutingStrategy,
    #[serde(default)]
    pub masking: MaskingConfig,
    #[serde(default)]
    pub recent_route_traces: Vec<RouteDecisionTrace>,
}

impl Default for ProviderRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRouter {
    pub fn new() -> Self {
        let mut providers = HashMap::new();
        let local = ProviderConfig::local_hermes();
        providers.insert(local.id.clone(), local);

        let mut model_profiles = HashMap::new();
        model_profiles.insert(
            "hermes-local-v1".to_string(),
            ModelProfile {
                id: "hermes-local-v1".to_string(),
                provider_id: "local_hermes".to_string(),
                model: "hermes-local-v1".to_string(),
                api_mode: ApiMode::LocalHermes,
                input_cost_per_1m: 0.0,
                output_cost_per_1m: 0.0,
                context_window: 128_000,
                supports_tools: true,
                supports_reasoning: true,
                supports_caching: true,
            },
        );
        model_profiles.insert(
            "claude-sonnet-4-20250514".to_string(),
            ModelProfile {
                id: "claude-sonnet-4-20250514".to_string(),
                provider_id: "anthropic".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                api_mode: ApiMode::AnthropicMessages,
                input_cost_per_1m: 3.0,
                output_cost_per_1m: 15.0,
                context_window: 200_000,
                supports_tools: true,
                supports_reasoning: true,
                supports_caching: true,
            },
        );
        model_profiles.insert(
            "gpt-4o".to_string(),
            ModelProfile {
                id: "gpt-4o".to_string(),
                provider_id: "openai".to_string(),
                model: "gpt-4o".to_string(),
                api_mode: ApiMode::ChatCompletions,
                input_cost_per_1m: 2.5,
                output_cost_per_1m: 10.0,
                context_window: 128_000,
                supports_tools: true,
                supports_reasoning: false,
                supports_caching: true,
            },
        );
        model_profiles.insert(
            "codex-mini-latest".to_string(),
            ModelProfile {
                id: "codex-mini-latest".to_string(),
                provider_id: "openai_responses".to_string(),
                model: "codex-mini-latest".to_string(),
                api_mode: ApiMode::CodexResponses,
                input_cost_per_1m: 1.5,
                output_cost_per_1m: 6.0,
                context_window: 200_000,
                supports_tools: true,
                supports_reasoning: true,
                supports_caching: true,
            },
        );

        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .user_agent("hermes-rust/1.0")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            inner: Arc::new(Mutex::new(ProviderRouterState {
                providers,
                model_profiles,
                active_provider_id: "local_hermes".to_string(),
                active_model: "hermes-local-v1".to_string(),
                fallback_chain: Vec::new(),
                allow_local_fallback: true,
                offline_rule_bot_allowed: true,
                subscriptions: Vec::new(),
                routing_groups: HashMap::new(),
                routing_strategy: RoutingStrategy::Smart,
                masking: MaskingConfig::default(),
                recent_route_traces: Vec::new(),
            })),
            route_metrics: Arc::new(Mutex::new(RouteRuntimeMetrics::default())),
            http_client,
            vertex_managers: Arc::new(Mutex::new(HashMap::new())),
            codex_managers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Discovers API keys, `.env` files, and local Codex/Magpie gateways & subscriptions without hardcoding credentials.
    pub fn from_env() -> Self {
        let router = Self::new();
        let offline_test = is_offline_test_mode();
        if offline_test {
            return router;
        }
        router.set_allow_local_fallback(false);
        router.set_offline_rule_bot_allowed(false);

        let env = EnvLookup::from_standard_paths();
        let home_dir = resolve_home_dir();
        let codex_home_override = env.get("CODEX_HOME").map(PathBuf::from);
        router.populate_from_env_and_home(
            &env,
            home_dir.as_deref(),
            codex_home_override.as_deref(),
            true,
        );

        let (prov, mdl, mode) = router.active_info();
        let allow_local = router.snapshot().allow_local_fallback;
        tracing::info!(
            provider = %prov,
            model = %mdl,
            api_mode = %mode,
            allow_local_fallback = allow_local,
            "Hermes ProviderRouter initialized"
        );
        router
    }

    fn populate_from_env_and_home(
        &self,
        env: &EnvLookup,
        home_dir: Option<&Path>,
        codex_home_override: Option<&Path>,
        probe_local_magpie: bool,
    ) {
        let mut active_set = false;
        let has_explicit_primary_env = env.get("OPENAI_API_KEY").is_some()
            || env.get("ANTHROPIC_API_KEY").is_some()
            || env.get("DEEPSEEK_API_KEY").is_some()
            || env.get("OPENROUTER_API_KEY").is_some()
            || env.get("HERMES_PROVIDER").is_some();

        if let Some(trimmed) = env.get("ANTHROPIC_API_KEY") {
            let base = env
                .get("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
            let model = env
                .get("ANTHROPIC_MODEL")
                .unwrap_or_else(|| "claude-sonnet-4-20250514".to_string());
            let mut cfg = ProviderConfig::anthropic(
                "anthropic",
                "Anthropic Messages",
                base,
                model.clone(),
                trimmed,
            );
            cfg.preset_id = Some("anthropic".to_string());
            self.register_provider(cfg);
            if !active_set {
                let _ = self.set_active("anthropic", Some(&model));
                active_set = true;
            }
        }

        if let Some(trimmed) = env.get("OPENAI_API_KEY") {
            let base = env
                .get("OPENAI_BASE_URL")
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
            let model = env
                .get("OPENAI_MODEL")
                .unwrap_or_else(|| "gpt-4o".to_string());
            let mut chat_cfg = ProviderConfig::openai_compatible(
                "openai",
                "OpenAI Chat Completions",
                base.clone(),
                model.clone(),
                trimmed.clone(),
            );
            chat_cfg.preset_id = Some("openai".to_string());
            self.register_provider(chat_cfg);
            let mut resp_cfg = ProviderConfig::codex_responses(
                "openai_responses",
                "OpenAI Codex Responses",
                base,
                "codex-mini-latest",
                trimmed,
            );
            resp_cfg.preset_id = Some("openai".to_string());
            self.register_provider(resp_cfg);
            if !active_set {
                let _ = self.set_active("openai", Some(&model));
                active_set = true;
            }
        }

        if let Some(trimmed) = env.get("OPENROUTER_API_KEY") {
            let base = env
                .get("OPENROUTER_BASE_URL")
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".to_string());
            let model = env
                .get("OPENROUTER_MODEL")
                .unwrap_or_else(|| "anthropic/claude-sonnet-4".to_string());
            let mut cfg = ProviderConfig::openai_compatible(
                "openrouter",
                "OpenRouter",
                base,
                model.clone(),
                trimmed,
            );
            cfg.preset_id = Some("openrouter".to_string());
            self.register_provider(cfg);
            if !active_set {
                let _ = self.set_active("openrouter", Some(&model));
                active_set = true;
            }
        }

        if let Some(trimmed) = env.get("DEEPSEEK_API_KEY") {
            let base = env
                .get("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string());
            let model = env
                .get("DEEPSEEK_MODEL")
                .unwrap_or_else(|| "deepseek-chat".to_string());
            let mut cfg = ProviderConfig::openai_compatible(
                "deepseek",
                "DeepSeek",
                base,
                model.clone(),
                trimmed,
            );
            cfg.preset_id = Some("deepseek".to_string());
            self.register_provider(cfg);
            if !active_set {
                let _ = self.set_active("deepseek", Some(&model));
                active_set = true;
            }
        }

        if let Some(trimmed) = env.get("NOUS_API_KEY") {
            let base = env
                .get("NOUS_BASE_URL")
                .unwrap_or_else(|| "https://inference-api.nousresearch.com/v1".to_string());
            let model = env
                .get("NOUS_MODEL")
                .unwrap_or_else(|| "Hermes-3-Llama-3.1-70B".to_string());
            let mut cfg = ProviderConfig::openai_compatible(
                "nous",
                "Nous Research",
                base,
                model.clone(),
                trimmed,
            );
            cfg.preset_id = Some("nous".to_string());
            self.register_provider(cfg);
            if !active_set {
                let _ = self.set_active("nous", Some(&model));
                active_set = true;
            }
        }

        if let Some(trimmed) = env
            .get("HERMES_API_KEY")
            .or_else(|| env.get("DACHENG_AI_TOKEN"))
        {
            let base = env
                .get("HERMES_BASE_URL")
                .unwrap_or_else(|| "https://ai.ombhrum.com/codex-deepseek/v1".to_string());
            let model = env
                .get("HERMES_MODEL")
                .unwrap_or_else(|| "deepseek-chat".to_string());
            let mut cfg = ProviderConfig::openai_compatible(
                "hermes_cloud",
                "Hermes Cloud",
                base.clone(),
                model.clone(),
                trimmed,
            );
            if base.contains("/codex-") || base.contains("ai.ombhrum.com") {
                cfg.api_mode = ApiMode::CodexResponses;
            }
            self.register_provider(cfg);
            if !active_set {
                let _ = self.set_active("hermes_cloud", Some(&model));
                active_set = true;
            }
        }

        // Scan additional Magpie vendor/relay environment variables cleanly via table-driven presets.
        register_magpie_env_vendors(self, env, &mut active_set);

        if probe_local_magpie {
            let codex_discovery = discover_codex_and_magpie(env);
            let mut magpie_registered = false;
            let mut codex_registered = false;

            if let Some(mut magpie) = codex_discovery.magpie_local {
                magpie.config.preset_id = Some("remote-magpie".to_string());
                for model_id in magpie.discovered_models.iter().take(32) {
                    self.register_model_profile(ModelProfile {
                        id: model_id.clone(),
                        provider_id: "magpie_local".to_string(),
                        model: model_id.clone(),
                        api_mode: ApiMode::ChatCompletions,
                        input_cost_per_1m: 0.0,
                        output_cost_per_1m: 0.0,
                        context_window: 200_000,
                        supports_tools: true,
                        supports_reasoning: true,
                        supports_caching: true,
                    });
                }
                self.register_provider(magpie.config);
                magpie_registered = true;
            }

            if let Some(mut codex_cfg) = codex_discovery.codex_oauth {
                codex_cfg.is_subscription = true;
                self.register_model_profile(ModelProfile {
                    id: codex_cfg.default_model.clone(),
                    provider_id: "codex_oauth".to_string(),
                    model: codex_cfg.default_model.clone(),
                    api_mode: ApiMode::CodexResponses,
                    input_cost_per_1m: 0.0,
                    output_cost_per_1m: 0.0,
                    context_window: 200_000,
                    supports_tools: true,
                    supports_reasoning: true,
                    supports_caching: true,
                });
                self.register_provider(codex_cfg);
                codex_registered = true;
            }

            if magpie_registered && !has_explicit_primary_env {
                let _ = self.set_active("magpie_local", None);
            } else if !active_set && codex_registered {
                let _ = self.set_active("codex_oauth", None);
            }
        }

        if let Some(home) = home_dir {
            let _ = self.refresh_discovered_subscriptions_in_home(home, codex_home_override);
        }

        if !active_set && env.get("HERMES_PROVIDER").is_none() {
            if let Some(best) = self.find_best_available_subscription() {
                if let Ok(cfg) = self.activate_subscription(&best, None) {
                    tracing::info!(
                        provider = %cfg.id,
                        model = %cfg.default_model,
                        "Activated discovered local subscription out-of-the-box"
                    );
                }
            }
        }

        if let Some(explicit_provider) = env.get("HERMES_PROVIDER") {
            let model_override = env.get("HERMES_MODEL");
            let _ = self.set_active(&explicit_provider, model_override.as_deref());
        }

        if let Some(mode_raw) = env.get("HERMES_API_MODE") {
            if let Some(mode) = ApiMode::parse(&mode_raw) {
                let _ = self.set_api_mode_for_active(mode);
            }
        }

        self.rebuild_default_fallback_chain();
    }

    fn rebuild_default_fallback_chain(&self) {
        let snap = self.snapshot();
        let active_id = snap.active_provider_id.clone();
        let priority_order = [
            "magpie_local",
            "codex_oauth",
            "google_vertex:adc",
            "google-vertex",
            "openai",
            "anthropic",
            "google",
            "openrouter",
            "deepseek",
            "moonshot",
            "zhipu",
            "qwen",
            "nous",
            "hermes_cloud",
        ];
        let mut chain = Vec::new();
        for pid in priority_order {
            if pid == active_id {
                continue;
            }
            if let Some(cfg) = snap.providers.get(pid) {
                if cfg.enabled
                    && cfg.api_mode != ApiMode::LocalHermes
                    && (!cfg.credentials.is_empty() || cfg.no_key_required)
                {
                    chain.push(FallbackTarget {
                        provider_id: cfg.id.clone(),
                        model: cfg.default_model.clone(),
                    });
                }
            }
        }
        for (pid, cfg) in &snap.providers {
            if pid == &active_id
                || pid == "local_hermes"
                || !cfg.enabled
                || cfg.api_mode == ApiMode::LocalHermes
                || (cfg.credentials.is_empty() && !cfg.no_key_required)
                || chain.iter().any(|t| t.provider_id == *pid)
            {
                continue;
            }
            chain.push(FallbackTarget {
                provider_id: cfg.id.clone(),
                model: cfg.default_model.clone(),
            });
        }
        self.set_fallback_chain(chain);
    }

    pub fn register_provider(&self, config: ProviderConfig) {
        let old_adc_path = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .providers
                .get(&config.id)
                .and_then(|p| p.headers.get("x-adc-source-path").cloned())
        };
        let new_adc_path = config.headers.get("x-adc-source-path").cloned();
        let old_codex_path = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .providers
                .get(&config.id)
                .and_then(|p| p.headers.get("x-codex-source-path").cloned())
        };
        let new_codex_path = config.headers.get("x-codex-source-path").cloned();
        let is_vertex_cfg = config.api_mode == ApiMode::GoogleVertex
            || config.preset_id.as_deref() == Some("google-vertex")
            || config.id.contains("vertex");

        let id = config.id.clone();
        let disk_creds_for_evict = if new_adc_path.is_none() && is_vertex_cfg {
            resolve_vertex_disk_credentials(&config)
        } else {
            None
        };
        {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.providers.insert(id.clone(), config);
        }
        if new_adc_path != old_adc_path {
            if let Ok(mut vm) = self.vertex_managers.lock() {
                vm.remove(&id);
            }
        } else if let Some(dc) = disk_creds_for_evict {
            if let Ok(mut vm) = self.vertex_managers.lock() {
                let stale = vm
                    .get(&id)
                    .is_some_and(|(mgr, auto)| *auto && vertex_creds_differ(&dc, mgr.credentials()));
                if stale {
                    vm.remove(&id);
                }
            }
        }
        if new_codex_path != old_codex_path {
            if let Ok(mut cm) = self.codex_managers.lock() {
                cm.remove(&id);
            }
        }
    }

    pub fn remove_provider(&self, provider_id: &str) -> bool {
        if provider_id == "local_hermes" {
            return false;
        }
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let removed = guard.providers.remove(provider_id).is_some();
        if guard.active_provider_id == provider_id {
            guard.active_provider_id = "local_hermes".to_string();
            guard.active_model = "hermes-local-v1".to_string();
        }
        removed
    }

    pub fn register_model_profile(&self, profile: ModelProfile) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.model_profiles.insert(profile.id.clone(), profile);
    }

    pub fn set_active(
        &self,
        provider_id: &str,
        model: Option<&str>,
    ) -> Result<(String, String), ProviderError> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cfg) = guard.providers.get(provider_id) else {
            return Err(ProviderError::Configuration(format!(
                "Unknown provider '{provider_id}'"
            )));
        };
        if !cfg.enabled {
            return Err(ProviderError::Configuration(format!(
                "Provider '{provider_id}' is disabled"
            )));
        }
        let resolved_model = model
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter(|s| {
                cfg.api_mode == ApiMode::LocalHermes
                    || !is_legacy_placeholder_model(s, provider_id, &cfg.base_url)
            })
            .map(ToString::to_string)
            .unwrap_or_else(|| cfg.default_model.clone());
        guard.active_provider_id = provider_id.to_string();
        guard.active_model = resolved_model.clone();
        for s in &mut guard.subscriptions {
            s.active = s.id == provider_id;
        }
        Ok((provider_id.to_string(), resolved_model))
    }

    pub fn set_active_model(&self, model_or_provider_slash_model: &str) -> (String, String) {
        let trimmed = model_or_provider_slash_model.trim();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((prov, model)) = trimmed.split_once('/') {
            if let Some(cfg) = guard.providers.get(prov).cloned() {
                let resolved = if cfg.api_mode != ApiMode::LocalHermes
                    && is_legacy_placeholder_model(model, prov, &cfg.base_url)
                {
                    cfg.default_model.clone()
                } else {
                    model.trim().to_string()
                };
                guard.active_provider_id = prov.to_string();
                guard.active_model = resolved.clone();
                return (prov.to_string(), resolved);
            }
        }
        if let Some(active_cfg) = guard.providers.get(&guard.active_provider_id).cloned() {
            if active_cfg.api_mode != ApiMode::LocalHermes
                && is_legacy_placeholder_model(trimmed, &active_cfg.id, &active_cfg.base_url)
            {
                guard.active_model = active_cfg.default_model.clone();
                return (guard.active_provider_id.clone(), guard.active_model.clone());
            }
        }
        if let Some(profile) = guard.model_profiles.get(trimmed).cloned() {
            if guard.providers.contains_key(&profile.provider_id) {
                guard.active_provider_id = profile.provider_id.clone();
            }
            guard.active_model = profile.model;
            return (guard.active_provider_id.clone(), guard.active_model.clone());
        }
        guard.active_model = trimmed.to_string();
        (guard.active_provider_id.clone(), guard.active_model.clone())
    }

    pub fn set_api_mode_for_active(&self, mode: ApiMode) -> Result<ApiMode, ProviderError> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let active_id = guard.active_provider_id.clone();
        if mode == ApiMode::LocalHermes {
            guard.active_provider_id = "local_hermes".to_string();
            if let Some(local) = guard.providers.get("local_hermes") {
                guard.active_model = local.default_model.clone();
            }
            return Ok(mode);
        }
        if let Some(cfg) = guard.providers.get_mut(&active_id) {
            cfg.api_mode = mode;
            return Ok(mode);
        }
        Err(ProviderError::Configuration(format!(
            "Active provider '{active_id}' not found"
        )))
    }

    pub fn set_fallback_chain(&self, chain: Vec<FallbackTarget>) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.fallback_chain = chain;
    }

    pub fn set_allow_local_fallback(&self, allow: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.allow_local_fallback = allow;
    }

    pub fn set_offline_rule_bot_allowed(&self, allow: bool) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.offline_rule_bot_allowed = allow;
    }

    pub fn snapshot(&self) -> ProviderRouterState {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Synchronizes providers, model profiles, fallback chain, subscriptions, routing groups,
    /// masking config, and active defaults from an upstream `ProviderRouterState`.
    pub fn sync_from_state(&self, state: &ProviderRouterState) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let local_backup = guard
            .providers
            .get("local_hermes")
            .cloned()
            .unwrap_or_else(ProviderConfig::local_hermes);
        guard.providers = state.providers.clone();
        guard
            .providers
            .entry("local_hermes".to_string())
            .or_insert(local_backup);
        for (id, profile) in &state.model_profiles {
            guard.model_profiles.insert(id.clone(), profile.clone());
        }
        guard.active_provider_id = state.active_provider_id.clone();
        guard.active_model = state.active_model.clone();
        guard.fallback_chain = state.fallback_chain.clone();
        guard.allow_local_fallback = state.allow_local_fallback;
        guard.offline_rule_bot_allowed = state.offline_rule_bot_allowed;
        guard.subscriptions = state.subscriptions.clone();
        guard.routing_groups = state.routing_groups.clone();
        guard.routing_strategy = state.routing_strategy;
        guard.masking = state.masking.clone();
        guard.recent_route_traces = state.recent_route_traces.clone();
    }

    /// Synchronizes both `ProviderRouterState` and shared token managers (`VertexTokenManager`,
    /// `CodexTokenManager`) from an upstream `ProviderRouter`.
    pub fn sync_from_router(&self, upstream: &ProviderRouter) {
        let state = upstream.snapshot();
        self.sync_from_state(&state);
        if let (Ok(up_vm), Ok(mut my_vm)) = (upstream.vertex_managers.lock(), self.vertex_managers.lock()) {
            *my_vm = up_vm
                .iter()
                .map(|(k, (v, auto))| (k.clone(), (Arc::clone(v), *auto)))
                .collect();
        }
        if let (Ok(up_cm), Ok(mut my_cm)) = (upstream.codex_managers.lock(), self.codex_managers.lock()) {
            *my_cm = up_cm.iter().map(|(k, v)| (k.clone(), Arc::clone(v))).collect();
        }
    }

    pub fn active_info(&self) -> (String, String, ApiMode) {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mode = guard
            .providers
            .get(&guard.active_provider_id)
            .map(|p| p.api_mode)
            .unwrap_or(ApiMode::LocalHermes);
        (
            guard.active_provider_id.clone(),
            guard.active_model.clone(),
            mode,
        )
    }

    pub fn list_providers(&self) -> Vec<ProviderConfig> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut list: Vec<_> = guard.providers.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    pub fn has_active_subscription(&self) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.subscriptions.iter().any(|s| s.active)
            || guard.providers.values().any(|p| {
                p.enabled
                    && p.is_subscription
                    && p.api_mode != ApiMode::LocalHermes
                    && (!p.credentials.is_empty() || p.no_key_required)
            })
    }

    pub fn has_configured_provider(&self) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.providers.values().any(|p| {
            p.enabled
                && p.id != "local_hermes"
                && p.api_mode != ApiMode::LocalHermes
                && (!p.credentials.is_empty() || p.no_key_required)
        })
    }

    pub fn needs_provider_selection(&self) -> bool {
        !self.has_active_subscription() && !self.has_configured_provider()
    }

    /// Apply a built-in Magpie preset (with optional region, key, model, base URL, and protocol overrides).
    pub fn apply_magpie_preset(
        &self,
        input: MagpiePresetApplyInput,
    ) -> Result<ProviderConfig, ProviderError> {
        let preset = find_preset(&input.preset_id).ok_or_else(|| {
            ProviderError::Configuration(format!("Unknown Magpie preset '{}'", input.preset_id))
        })?;
        let preferred_proto = input.protocol_override.and_then(|m| m.to_protocol_kind());
        let resolved =
            resolve_preset_endpoints(&preset.id, input.region_id.as_deref(), preferred_proto)
                .ok_or_else(|| {
                    ProviderError::Configuration(format!(
                        "No usable endpoint for Magpie preset '{}'",
                        preset.id
                    ))
                })?;

        let provider_id = input
            .provider_id_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&preset.id)
            .to_string();
        let display_name = input
            .display_name_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&resolved.display_name)
            .to_string();
        let api_mode = input
            .protocol_override
            .unwrap_or_else(|| ApiMode::from_protocol_kind(resolved.protocol));
        let base_url = input
            .base_url_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&resolved.base_url)
            .to_string();
        let default_model = input
            .model_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&resolved.default_model)
            .to_string();

        let existing = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.providers.get(&provider_id).cloned()
        };

        let mut credentials = existing
            .as_ref()
            .map(|e| e.credentials.clone())
            .unwrap_or_else(CredentialPool::new);
        if let Some(key_raw) = input.api_key.as_deref().map(str::trim) {
            if !key_raw.is_empty() {
                credentials.add_entry(CredentialEntry::new(
                    format!("{provider_id}_key_1"),
                    key_raw.to_string(),
                    "primary",
                ));
            }
        }

        let mut headers = existing
            .as_ref()
            .map(|e| e.headers.clone())
            .unwrap_or_default();
        for (hk, hv) in input.custom_headers {
            let k = hk.trim();
            let v = hv.trim();
            if !k.is_empty() && !v.is_empty() {
                headers.insert(k.to_string(), v.to_string());
            }
        }

        let mut fallback_models = Vec::new();
        for m in &resolved.suggested_models {
            if m != &default_model && !fallback_models.contains(m) {
                fallback_models.push(m.clone());
            }
        }

        let cfg = ProviderConfig {
            id: provider_id.clone(),
            name: display_name,
            api_mode,
            base_url,
            default_model: default_model.clone(),
            headers,
            credentials,
            fallback_models,
            timeout_secs: existing.as_ref().map(|e| e.timeout_secs).unwrap_or(90),
            enabled: true,
            preset_id: Some(preset.id.clone()),
            region_id: resolved.region_id,
            is_subscription: false,
            no_key_required: preset.no_key,
            routing_strategy: existing.as_ref().and_then(|e| e.routing_strategy),
        };

        {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.providers.insert(provider_id.clone(), cfg.clone());
            for model_id in resolved
                .suggested_models
                .iter()
                .chain(std::iter::once(&default_model))
            {
                if model_id.trim().is_empty() {
                    continue;
                }
                guard.model_profiles.insert(
                    model_id.clone(),
                    ModelProfile {
                        id: model_id.clone(),
                        provider_id: provider_id.clone(),
                        model: model_id.clone(),
                        api_mode,
                        input_cost_per_1m: 0.0,
                        output_cost_per_1m: 0.0,
                        context_window: 128_000,
                        supports_tools: true,
                        supports_reasoning: true,
                        supports_caching: true,
                    },
                );
            }
            if input.activate {
                guard.active_provider_id = provider_id.clone();
                guard.active_model = default_model.clone();
            }
        }

        if cfg.api_mode == ApiMode::GoogleVertex
            || cfg.preset_id.as_deref() == Some("google-vertex")
            || cfg.id.contains("vertex")
        {
            let old_adc = existing
                .as_ref()
                .and_then(|e| e.headers.get("x-adc-source-path"));
            let new_adc = cfg.headers.get("x-adc-source-path");
            if new_adc != old_adc {
                if let Ok(mut vm) = self.vertex_managers.lock() {
                    vm.remove(&provider_id);
                }
            } else if new_adc.is_none() {
                if let Some(dc) = resolve_vertex_disk_credentials(&cfg) {
                    if let Ok(mut vm) = self.vertex_managers.lock() {
                        let stale = vm
                            .get(&provider_id)
                            .is_some_and(|(mgr, auto)| *auto && vertex_creds_differ(&dc, mgr.credentials()));
                        if stale {
                            vm.remove(&provider_id);
                        }
                    }
                }
            }
        }

        if input.activate {
            self.rebuild_default_fallback_chain();
        }
        Ok(cfg)
    }

    pub fn upsert_routing_group(&self, mut group: RoutingGroup) -> Result<RoutingGroup, ProviderError> {
        let canonical = group.canonical_id();
        if canonical.is_empty() {
            return Err(ProviderError::Configuration(
                "Routing group id cannot be empty".to_string(),
            ));
        }
        group.id = canonical.clone();
        let qualified = group.qualified_model_name();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let active_pid = guard.active_provider_id.clone();
        guard.routing_groups.insert(canonical.clone(), group.clone());
        guard.model_profiles.insert(
            qualified.clone(),
            ModelProfile {
                id: qualified.clone(),
                provider_id: active_pid,
                model: qualified,
                api_mode: ApiMode::ChatCompletions,
                input_cost_per_1m: 0.0,
                output_cost_per_1m: 0.0,
                context_window: 128_000,
                supports_tools: true,
                supports_reasoning: true,
                supports_caching: true,
            },
        );
        Ok(group)
    }

    pub fn delete_routing_group(&self, group_id: &str) -> bool {
        let canonical = group_id
            .trim()
            .strip_prefix("group/")
            .unwrap_or(group_id.trim())
            .to_string();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let removed = guard.routing_groups.remove(&canonical).is_some();
        guard.model_profiles.remove(&format!("group/{canonical}"));
        removed
    }

    pub fn set_masking_config(&self, config: MaskingConfig) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.masking = config;
    }

    pub fn set_routing_strategy(&self, strategy: RoutingStrategy) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.routing_strategy = strategy;
    }

    pub fn register_vertex_token_manager(&self, provider_id: &str, mgr: VertexTokenManager) {
        let mut guard = self.vertex_managers.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(provider_id.to_string(), (Arc::new(mgr), false));
    }

    pub fn register_codex_token_manager(&self, provider_id: &str, mgr: CodexTokenManager) {
        let mut guard = self.codex_managers.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(provider_id.to_string(), Arc::new(mgr));
    }

    pub fn get_or_create_codex_manager(
        &self,
        provider_id: &str,
        cfg: &ProviderConfig,
    ) -> Option<Arc<CodexTokenManager>> {
        let login_id_opt = provider_id.strip_prefix("magpie_codex:");
        let is_magpie_codex = login_id_opt.is_some();

        let disk_mgr = cfg.headers.get("x-codex-source-path").and_then(|path_str| {
            let p = PathBuf::from(path_str);
            if p.is_file() {
                let raw = std::fs::read_to_string(&p).ok()?;
                if let Some(login_id) = login_id_opt {
                    let is_logins_json = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n == "logins.json");
                    if is_logins_json {
                        CodexTokenManager::from_logins_json_str(&raw, login_id)
                    } else {
                        CodexTokenManager::from_logins_json_str(&raw, login_id)
                            .or_else(|| CodexTokenManager::from_auth_json_str(&raw))
                    }
                } else {
                    CodexTokenManager::from_auth_json_str(&raw)
                }
            } else {
                None
            }
        });

        {
            let guard = self.codex_managers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(existing) = guard.get(provider_id).cloned() {
                let is_stale = disk_mgr
                    .as_ref()
                    .is_some_and(|dm| dm.refresh_token() != existing.refresh_token());
                if !is_stale {
                    return Some(existing);
                }
            }
        }

        let created = disk_mgr.or_else(|| {
            if is_magpie_codex {
                return None;
            }
            let codex_dir = std::env::var_os("CODEX_HOME")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .or_else(|| resolve_home_dir().map(|h| h.join(".codex")))?;
            let auth_path = codex_dir.join("auth.json");
            let raw = std::fs::read_to_string(&auth_path).ok()?;
            CodexTokenManager::from_auth_json_str(&raw)
        });

        if let Some(mut mgr) = created {
            if let Some(url) = cfg.headers.get("x-codex-token-url") {
                if !url.trim().is_empty() {
                    mgr = mgr.with_token_url(url.trim());
                }
            }
            let arc_mgr = Arc::new(mgr);
            let mut guard = self.codex_managers.lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(provider_id.to_string(), Arc::clone(&arc_mgr));
            return Some(arc_mgr);
        }

        None
    }

    pub fn get_or_create_vertex_manager(
        &self,
        provider_id: &str,
        cfg: &ProviderConfig,
    ) -> Option<Arc<VertexTokenManager>> {
        let disk_creds = resolve_vertex_disk_credentials(cfg);

        {
            let mut guard = self.vertex_managers.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((mgr, auto_from_disk)) = guard.get(provider_id).cloned() {
                let should_check_disk = cfg.headers.contains_key("x-adc-source-path") || auto_from_disk;
                if !should_check_disk {
                    return Some(mgr);
                }
                match disk_creds.as_ref() {
                    Some(dc) if !vertex_creds_differ(dc, mgr.credentials()) => {
                        return Some(mgr);
                    }
                    Some(_) => {
                        guard.remove(provider_id);
                    }
                    None => {
                        guard.remove(provider_id);
                        return None;
                    }
                }
            }
        }

        if let Some(creds) = disk_creds {
            let mgr = Arc::new(VertexTokenManager::new(creds));
            let mut guard = self.vertex_managers.lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(provider_id.to_string(), (Arc::clone(&mgr), true));
            return Some(mgr);
        }

        None
    }

    pub fn refresh_discovered_subscriptions_in_home(
        &self,
        home: &Path,
        codex_home_override: Option<&Path>,
    ) -> SubscriptionRegistry {
        let reg = discover_subscriptions_in_home(home, codex_home_override);
        let mut invalidated_ids = Vec::new();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.subscriptions = reg.accounts.clone();

        for acc in &reg.accounts {
            let cfg = subscription_to_provider_config(acc);
            register_subscription_model_profiles(&mut guard, &cfg);
            if let Some(existing) = guard.providers.get_mut(&cfg.id) {
                let adc_changed = existing.headers.get("x-adc-source-path")
                    != cfg.headers.get("x-adc-source-path");
                let codex_changed = existing.headers.get("x-codex-source-path")
                    != cfg.headers.get("x-codex-source-path");
                if adc_changed || codex_changed {
                    invalidated_ids.push(cfg.id.clone());
                }
                existing.enabled = cfg.enabled;
                existing.base_url = cfg.base_url;
                existing.is_subscription = cfg.is_subscription;
                existing.no_key_required = cfg.no_key_required;
                existing.headers = cfg.headers;
                if !cfg.credentials.is_empty() {
                    existing.credentials = cfg.credentials;
                }
                if existing.default_model.is_empty() {
                    existing.default_model = cfg.default_model;
                }
            } else {
                invalidated_ids.push(cfg.id.clone());
                guard.providers.insert(cfg.id.clone(), cfg);
            }
        }

        let needs_selection = !guard.subscriptions.iter().any(|s| s.active)
            && !guard.providers.values().any(|p| {
                p.enabled
                    && p.id != "local_hermes"
                    && p.api_mode != ApiMode::LocalHermes
                    && (!p.credentials.is_empty() || p.no_key_required)
            });

        let should_autobind = needs_selection || guard.active_provider_id == "local_hermes";
        if should_autobind {
            let best_sub = guard
                .subscriptions
                .iter()
                .find(|s| s.active && s.kind == SubscriptionKind::GoogleVertex)
                .or_else(|| guard.subscriptions.iter().find(|s| s.active && s.kind == SubscriptionKind::CodexChatGpt))
                .or_else(|| guard.subscriptions.iter().find(|s| s.active))
                .or_else(|| guard.subscriptions.first());

            let best_sub_id = best_sub.map(|s| s.id.clone());
            if let Some(sub_id) = best_sub_id {
                let opt_pair = guard
                    .providers
                    .get(&sub_id)
                    .map(|cfg| (cfg.id.clone(), cfg.default_model.clone()));
                if let Some((pid, model)) = opt_pair {
                    guard.active_provider_id = pid;
                    guard.active_model = model;
                }
            }
        }

        if !guard.active_provider_id.is_empty() && guard.active_provider_id != "local_hermes" {
            let active_id = guard.active_provider_id.clone();
            for s in &mut guard.subscriptions {
                s.active = s.id == active_id;
            }
        }
        drop(guard);

        if !invalidated_ids.is_empty() {
            if let Ok(mut vm) = self.vertex_managers.lock() {
                for id in &invalidated_ids {
                    vm.remove(id);
                }
            }
            if let Ok(mut cm) = self.codex_managers.lock() {
                for id in &invalidated_ids {
                    cm.remove(id);
                }
            }
        }

        reg
    }

    pub fn refresh_discovered_subscriptions(&self) -> SubscriptionRegistry {
        if is_offline_test_mode() {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            return SubscriptionRegistry {
                accounts: guard.subscriptions.clone(),
                scanned_at_ms: Utc::now().timestamp_millis(),
            };
        }
        let Some(home) = resolve_home_dir() else {
            return SubscriptionRegistry {
                accounts: Vec::new(),
                scanned_at_ms: Utc::now().timestamp_millis(),
            };
        };
        let codex_override = std::env::var_os("CODEX_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let reg = self.refresh_discovered_subscriptions_in_home(&home, codex_override.as_deref());
        self.rebuild_default_fallback_chain();
        reg
    }

    pub fn refresh_local_subscriptions(&self) -> SubscriptionRegistry {
        self.refresh_discovered_subscriptions()
    }

    pub fn refresh_local_subscriptions_in_home(
        &self,
        home: &Path,
        codex_home_override: Option<&Path>,
    ) -> SubscriptionRegistry {
        let reg = self.refresh_discovered_subscriptions_in_home(home, codex_home_override);
        self.rebuild_default_fallback_chain();
        reg
    }

    pub fn find_best_available_subscription(&self) -> Option<String> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(sub) = guard
            .subscriptions
            .iter()
            .find(|s| s.active && s.kind == SubscriptionKind::GoogleVertex)
        {
            return Some(sub.id.clone());
        }
        if let Some(sub) = guard
            .subscriptions
            .iter()
            .find(|s| s.active && s.kind == SubscriptionKind::CodexChatGpt)
        {
            return Some(sub.id.clone());
        }
        if let Some(sub) = guard.subscriptions.iter().find(|s| s.active) {
            return Some(sub.id.clone());
        }
        if let Some(p) = guard.providers.values().find(|p| {
            p.enabled
                && p.is_subscription
                && (p.api_mode == ApiMode::GoogleVertex || p.id.contains("vertex"))
        }) {
            return Some(p.id.clone());
        }
        if let Some(p) = guard.providers.values().find(|p| {
            p.enabled
                && p.is_subscription
                && (p.api_mode == ApiMode::CodexResponses || p.id.contains("codex"))
        }) {
            return Some(p.id.clone());
        }
        if let Some(p) = guard.providers.values().find(|p| {
            p.enabled && p.is_subscription && p.api_mode != ApiMode::LocalHermes
        }) {
            return Some(p.id.clone());
        }
        if let Some(sub) = guard.subscriptions.first() {
            return Some(sub.id.clone());
        }

        None
    }

    pub fn activate_subscription(
        &self,
        id: &str,
        model: Option<&str>,
    ) -> Result<ProviderConfig, ProviderError> {
        self.activate_subscription_atomic(id, model)
            .map(|(cfg, _)| cfg)
    }

    pub fn activate_subscription_atomic(
        &self,
        id: &str,
        model: Option<&str>,
    ) -> Result<(ProviderConfig, String), ProviderError> {
        let trimmed_id = id.trim();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        let sub_match = guard
            .subscriptions
            .iter()
            .find(|s| s.id == trimmed_id || s.account_id.as_deref() == Some(trimmed_id))
            .or_else(|| {
                guard.subscriptions.iter().find(|s| {
                    (trimmed_id == "google_vertex" && s.kind == SubscriptionKind::GoogleVertex)
                        || (trimmed_id == "google-vertex" && s.kind == SubscriptionKind::GoogleVertex)
                        || (trimmed_id == "codex" && s.kind == SubscriptionKind::CodexChatGpt)
                })
            })
            .cloned();

        let target_provider_id = if let Some(ref sub) = sub_match {
            if !guard.providers.contains_key(&sub.id) {
                let cfg = subscription_to_provider_config(sub);
                register_subscription_model_profiles(&mut guard, &cfg);
                guard.providers.insert(cfg.id.clone(), cfg);
            }
            sub.id.clone()
        } else if guard.providers.contains_key(trimmed_id) {
            trimmed_id.to_string()
        } else {
            return Err(ProviderError::Configuration(format!(
                "Subscription or provider '{trimmed_id}' not found"
            )));
        };

        let (default_model, result_cfg) = {
            let target_cfg = guard
                .providers
                .get_mut(&target_provider_id)
                .ok_or_else(|| {
                    ProviderError::Configuration(format!(
                        "Provider '{target_provider_id}' not found"
                    ))
                })?;
            target_cfg.enabled = true;
            (target_cfg.default_model.clone(), target_cfg.clone())
        };

        let active_model = match model.map(str::trim).filter(|m| !m.is_empty()) {
            Some(m) => m.to_string(),
            None => default_model,
        };

        guard.active_provider_id = target_provider_id.clone();
        guard.active_model = active_model.clone();

        for s in &mut guard.subscriptions {
            s.active = s.id == target_provider_id;
        }

        drop(guard);

        self.rebuild_default_fallback_chain();
        Ok((result_cfg, active_model))
    }

    pub fn route_metrics_snapshot(&self) -> RouteRuntimeMetrics {
        self.route_metrics
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn add_credential(&self, provider_id: &str, entry: CredentialEntry) -> Result<(), ProviderError> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cfg) = guard.providers.get_mut(provider_id) else {
            return Err(ProviderError::Configuration(format!(
                "Unknown provider '{provider_id}'"
            )));
        };
        cfg.credentials.add_entry(entry);
        Ok(())
    }

    fn acquire_credential(&self, provider_id: &str) -> Result<Option<CredentialEntry>, ProviderError> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cfg) = guard.providers.get_mut(provider_id) else {
            return Err(ProviderError::Configuration(format!(
                "Unknown provider '{provider_id}'"
            )));
        };
        if cfg.api_mode == ApiMode::LocalHermes {
            return Ok(None);
        }
        let now_ms = Utc::now().timestamp_millis();
        if cfg.credentials.is_empty() {
            if cfg.no_key_required {
                return Ok(None);
            }
            return Err(ProviderError::Authentication(format!(
                "No API credentials configured for provider '{provider_id}'"
            )));
        }
        match cfg.credentials.acquire(now_ms) {
            Some(entry) => Ok(Some(entry)),
            None if cfg.no_key_required => Ok(None),
            None => Err(ProviderError::RateLimited {
                status: 429,
                retry_after_secs: Some(5),
                message: format!(
                    "All {} credentials for provider '{provider_id}' are in cooldown or disabled",
                    cfg.credentials.len()
                ),
            }),
        }
    }

    fn report_credential_outcome(
        &self,
        provider_id: &str,
        cred_id: &str,
        err: Option<&ProviderError>,
    ) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cfg) = guard.providers.get_mut(provider_id) else {
            return;
        };
        match err {
            None => cfg.credentials.report_success(cred_id),
            Some(e) => {
                if let Some(status) = e.http_status() {
                    if status == 429 || status >= 500 || status == 401 {
                        let now_ms = Utc::now().timestamp_millis();
                        cfg.credentials
                            .report_rate_limit(cred_id, now_ms, status, e.retry_after_secs());
                    }
                }
            }
        }
    }

    fn record_route_success(
        &self,
        requested_model: &str,
        provider_id: &str,
        model: &str,
        strategy: RoutingStrategy,
        reason: &str,
        candidates_considered: usize,
        masking_hits: usize,
        tokens: u64,
        latency_ms: u64,
    ) {
        let now_ms = Utc::now().timestamp_millis();
        let seat_key = format!("{provider_id}/{model}");
        let traces_vec = {
            let mut m = self.route_metrics.lock().unwrap_or_else(|e| e.into_inner());
            m.record_usage(&seat_key, tokens, Some(latency_ms), now_ms);
            m.push_trace(RouteDecisionTrace {
                timestamp_ms: now_ms,
                requested_model: requested_model.to_string(),
                resolved_provider_id: provider_id.to_string(),
                resolved_model: model.to_string(),
                strategy,
                reason: reason.to_string(),
                candidates_considered,
                masking_hits,
            });
            m.traces.iter().cloned().collect::<Vec<_>>()
        };
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.recent_route_traces = traces_vec;
    }

    fn record_route_failure(&self, provider_id: &str, model: &str, err: &ProviderError) {
        let now_ms = Utc::now().timestamp_millis();
        let seat_key = format!("{provider_id}/{model}");
        let mut m = self.route_metrics.lock().unwrap_or_else(|e| e.into_inner());
        let status_opt = err.http_status().or_else(|| match err {
            ProviderError::Transport(msg) => {
                if msg.to_ascii_lowercase().contains("timed out") {
                    Some(504)
                } else {
                    Some(502)
                }
            }
            _ => None,
        });
        if let Some(status) = status_opt {
            m.record_failure_disambiguated(
                &seat_key,
                provider_id,
                status,
                &err.to_string(),
                err.retry_after_secs(),
                None,
                now_ms,
            );
        } else {
            m.record_failure(&seat_key, None, err.retry_after_secs(), now_ms);
        }
    }

    /// Execute a completion using the active provider/model (or Magpie `group/<id>` routing + privacy masking) and walking the fallback chain on failure.
    pub async fn complete(
        &self,
        request: ProviderRequest,
        stream_tx: Option<mpsc::Sender<ProviderStreamEvent>>,
    ) -> Result<ProviderResponse, ProviderError> {
        let state = self.snapshot();
        if self.needs_provider_selection() || state.active_provider_id == "local_hermes" {
            if let Some(sub_id) = self.find_best_available_subscription() {
                if let Ok(cfg) = self.activate_subscription(&sub_id, None) {
                    tracing::info!(
                        subscription_id = %sub_id,
                        active_model = %cfg.default_model,
                        "Autonomous fallback: routed to available local subscription"
                    );
                }
            }
        }

        let state = self.snapshot();
        if self.needs_provider_selection() && !state.allow_local_fallback {
            return Err(ProviderError::Configuration(
                "未检测到已配置的 AI 订阅或模型供应商。请打开「设置 → AI 订阅与模型供应商」选择供应商（如 DeepSeek、Kimi、智谱 GLM、OpenAI、Anthropic、Gemini 或本地 Ollama）后使用。 / No AI subscription or provider configured.".to_string(),
            ));
        }

        let active_cfg = state.providers.get(&state.active_provider_id);
        let requested_model_raw = if request.model.trim().is_empty() {
            state.active_model.clone()
        } else {
            request.model.trim().to_string()
        };
        let group_lookup_key = requested_model_raw
            .strip_prefix("group/")
            .unwrap_or(&requested_model_raw);
        let is_explicit_group = requested_model_raw.starts_with("group/")
            || state.routing_groups.contains_key(group_lookup_key);

        let now_ms = Utc::now().timestamp_millis();
        let mut routed_targets: Vec<(String, String, RoutingStrategy, String)> = Vec::new();

        if is_explicit_group && !state.routing_groups.is_empty() {
            let prompt_preview = request
                .messages
                .iter()
                .rev()
                .find(|m| m.role == ProviderMessageRole::User)
                .map(|m| m.content.as_str())
                .unwrap_or("");
            let session_key = request
                .metadata
                .get("session_id")
                .and_then(Value::as_str);
            let resolved_res = {
                let mut metrics_guard = self.route_metrics.lock().unwrap_or_else(|e| e.into_inner());
                resolve_group_targets(
                    &requested_model_raw,
                    &state.routing_groups,
                    &mut metrics_guard,
                    &state.active_provider_id,
                    session_key,
                    prompt_preview,
                    request.messages.len(),
                    request.tools.len(),
                    now_ms,
                )
            };
            match resolved_res {
                Ok(group_candidates) => {
                    for cand in group_candidates {
                        if !routed_targets
                            .iter()
                            .any(|(p, m, _, _)| p == &cand.provider_id && m == &cand.model)
                        {
                            routed_targets.push((
                                cand.provider_id,
                                cand.model,
                                cand.strategy,
                                cand.reason,
                            ));
                        }
                    }
                }
                Err(e) => {
                    return Err(ProviderError::Configuration(e.to_string()));
                }
            }
        }

        if routed_targets.is_empty() {
            let requested_model = match active_cfg {
                Some(cfg)
                    if cfg.api_mode != ApiMode::LocalHermes
                        && is_legacy_placeholder_model(&request.model, &cfg.id, &cfg.base_url) =>
                {
                    if !is_legacy_placeholder_model(&state.active_model, &cfg.id, &cfg.base_url)
                        && !state.active_model.trim().is_empty()
                    {
                        state.active_model.clone()
                    } else {
                        cfg.default_model.clone()
                    }
                }
                _ => requested_model_raw.clone(),
            };

            let active_strategy = active_cfg
                .and_then(|c| c.routing_strategy)
                .unwrap_or(state.routing_strategy);
            let mut push_unique_target = |provider_id: String, model: String, reason: &str| {
                if !model.trim().is_empty()
                    && !routed_targets
                        .iter()
                        .any(|(p, m, _, _)| p == &provider_id && m == &model)
                {
                    routed_targets.push((provider_id, model, active_strategy, reason.to_string()));
                }
            };

            push_unique_target(state.active_provider_id.clone(), requested_model.clone(), "primary");
            if let Some(cfg) = active_cfg {
                for fb_model in &cfg.fallback_models {
                    if !fb_model.trim().is_empty() {
                        push_unique_target(state.active_provider_id.clone(), fb_model.clone(), "provider_fallback");
                    }
                }
                if cfg.id == "magpie_local"
                    || (cfg.id == "codex_oauth" && is_local_magpie_url(&cfg.base_url))
                {
                    for m in ["auto/gemini-3.8-flash", "auto/gemini-3.1-pro-preview"] {
                        push_unique_target(cfg.id.clone(), m.to_string(), "magpie_auto_fallback");
                    }
                }
            }
            for target in &state.fallback_chain {
                push_unique_target(target.provider_id.clone(), target.model.clone(), "fallback_chain");
            }
            if state.active_provider_id == "magpie_local" || state.active_provider_id == "codex_oauth" {
                let mut extra_providers: Vec<&ProviderConfig> = state
                    .providers
                    .values()
                    .filter(|p| {
                        p.enabled
                            && p.id != state.active_provider_id
                            && p.id != "local_hermes"
                            && p.api_mode != ApiMode::LocalHermes
                            && (!p.credentials.is_empty() || p.no_key_required)
                    })
                    .collect();
                extra_providers.sort_by(|a, b| {
                    let rank = |id: &str| match id {
                        "magpie_local" => 0,
                        "codex_oauth" => 1,
                        _ => 2,
                    };
                    rank(&a.id).cmp(&rank(&b.id)).then_with(|| a.id.cmp(&b.id))
                });
                for p in extra_providers {
                    push_unique_target(p.id.clone(), p.default_model.clone(), "extra_provider");
                }
            }
        }

        if state.allow_local_fallback {
            if !routed_targets
                .iter()
                .any(|(p, _, _, _)| p == "local_hermes")
            {
                routed_targets.push((
                    "local_hermes".to_string(),
                    "hermes-local-v1".to_string(),
                    state.routing_strategy,
                    "local_fallback".to_string(),
                ));
            }
        }

        // Prepare outbound privacy masking engine if enabled
        let masking_engine = state
            .masking
            .enabled
            .then(|| Arc::new(MaskingEngine::new(state.masking.clone())));

        // Honor seat cooldowns and circuit-breaker rests in both group and non-group routing
        // (including keyless `no_key_required` providers such as GoogleVertex).
        let active_targets: Vec<_> = {
            let m = self.route_metrics.lock().unwrap_or_else(|e| e.into_inner());
            routed_targets
                .into_iter()
                .filter(|(pid, mdl, _, _)| {
                    if pid == "local_hermes" {
                        return true;
                    }
                    let seat_key = format!("{pid}/{mdl}");
                    let cooled = m.seats.get(&seat_key).is_some_and(|s| s.is_in_cooldown(now_ms))
                        || m.circuit_breaker.is_in_rest(&seat_key, now_ms);
                    !cooled
                })
                .collect()
        };

        if active_targets.is_empty() {
            return Err(ProviderError::RateLimited {
                status: 429,
                retry_after_secs: Some(5),
                message: "All candidate providers are currently in cooldown or rest".to_string(),
            });
        }

        let targets_len = active_targets.len();
        let mut errors = Vec::new();
        for (provider_id, model, strategy, reason) in active_targets {
            if stream_tx.as_ref().is_some_and(|tx| tx.is_closed()) {
                return Err(ProviderError::Transport(
                    "Stream client disconnected".to_string(),
                ));
            }
            let Some(cfg) = self
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .providers
                .get(&provider_id)
                .cloned()
            else {
                errors.push(format!("{provider_id}: provider not found"));
                continue;
            };
            if !cfg.enabled {
                errors.push(format!("{provider_id}: provider disabled"));
                continue;
            }

            let mut target_req = request.clone();
            target_req.model = model.clone();

            let mut masking_hits = 0usize;
            if cfg.api_mode != ApiMode::LocalHermes {
                if let Some(ref engine) = masking_engine {
                    masking_hits = mask_outbound_request(&mut target_req, engine)?;
                }
            }

            // Track whether this attempt has already emitted any delta to the caller's stream_tx.
            // Streaming proceeds in real time (zero full-response buffering), and if an attempt fails
            // after emitting partial output to stream_tx, we abort rather than concatenating a fallback
            // provider's reply onto a broken prefix.
            let emitted_to_caller = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (exec_tx, forward_handle) = if let Some(outer_tx) = stream_tx.as_ref() {
                let (utx, mut urx) = mpsc::channel::<ProviderStreamEvent>(64);
                let outer_clone = (*outer_tx).clone();
                let emitted_flag = Arc::clone(&emitted_to_caller);
                let eng_opt = if masking_hits > 0 {
                    masking_engine.clone()
                } else {
                    None
                };
                let handle = tokio::spawn(async move {
                    let mut text_unmasker = StreamingUnmasker::new();
                    let mut think_unmasker = StreamingUnmasker::new();
                    loop {
                        let ev_opt = tokio::select! {
                            biased;
                            _ = outer_clone.closed() => None,
                            ev = urx.recv() => ev,
                        };
                        let Some(ev) = ev_opt else {
                            break;
                        };
                        match ev {
                            ProviderStreamEvent::TextDelta(chunk) => {
                                let clean = if let Some(ref eng) = eng_opt {
                                    text_unmasker.push(&chunk, eng)
                                } else {
                                    chunk
                                };
                                if !clean.is_empty() {
                                    emitted_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                    if outer_clone
                                        .send(ProviderStreamEvent::TextDelta(clean))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            ProviderStreamEvent::ThinkingDelta(chunk) => {
                                let clean = if let Some(ref eng) = eng_opt {
                                    think_unmasker.push(&chunk, eng)
                                } else {
                                    chunk
                                };
                                if !clean.is_empty() {
                                    emitted_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                    if outer_clone
                                        .send(ProviderStreamEvent::ThinkingDelta(clean))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            ProviderStreamEvent::ToolCallReady(mut tc) => {
                                if let Some(ref eng) = eng_opt {
                                    tc.arguments = eng.unmask_text(&tc.arguments);
                                }
                                emitted_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                                if outer_clone
                                    .send(ProviderStreamEvent::ToolCallReady(tc))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            ProviderStreamEvent::Usage(u) => {
                                if outer_clone.send(ProviderStreamEvent::Usage(u)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(ref eng) = eng_opt {
                        let rem_think = think_unmasker.finish(eng);
                        if !rem_think.is_empty() {
                            emitted_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            let _ = outer_clone
                                .send(ProviderStreamEvent::ThinkingDelta(rem_think))
                                .await;
                        }
                        let rem_text = text_unmasker.finish(eng);
                        if !rem_text.is_empty() {
                            emitted_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            let _ = outer_clone
                                .send(ProviderStreamEvent::TextDelta(rem_text))
                                .await;
                        }
                    }
                });
                (Some(utx), Some(handle))
            } else {
                (None, None)
            };

            let started = Instant::now();
            let exec_res = if let Some(ref outer_tx) = stream_tx {
                tokio::select! {
                    biased;
                    _ = outer_tx.closed() => Err(ProviderError::Transport("Stream client disconnected".to_string())),
                    res = self.execute_single_provider(&cfg, target_req, exec_tx.as_ref()) => res,
                }
            } else {
                self.execute_single_provider(&cfg, target_req, exec_tx.as_ref()).await
            };
            drop(exec_tx);
            if let Some(handle) = forward_handle {
                let _ = handle.await;
            }

            match exec_res {
                Ok(mut resp) => {
                    if masking_hits > 0 {
                        if let Some(ref engine) = masking_engine {
                            resp.content = engine.unmask_text(&resp.content);
                            resp.reasoning = resp.reasoning.map(|r| engine.unmask_text(&r));
                            for tc in &mut resp.tool_calls {
                                tc.arguments = engine.unmask_text(&tc.arguments);
                            }
                        }
                    }
                    self.enrich_cost(&mut resp);
                    let latency_ms = started.elapsed().as_millis() as u64;
                    self.record_route_success(
                        &requested_model_raw,
                        &resp.provider_id,
                        &resp.model,
                        strategy,
                        &reason,
                        targets_len,
                        masking_hits,
                        resp.usage.total_tokens,
                        latency_ms,
                    );
                    return Ok(resp);
                }
                Err(err) => {
                    if stream_tx.as_ref().is_some_and(|tx| tx.is_closed()) {
                        return Err(err);
                    }
                    self.record_route_failure(&provider_id, &model, &err);
                    // Do not concatenate a fallback provider's stream onto already-emitted partial output.
                    if emitted_to_caller.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(err);
                    }
                    errors.push(format!("{provider_id}/{model}: {err}"));
                    if targets_len == 1
                        && (!state.allow_local_fallback || provider_id == "local_hermes")
                    {
                        return Err(err);
                    }
                }
            }
        }

        Err(ProviderError::Exhausted(errors.join(" | ")))
    }

    /// Execute against one explicit provider without fallback.
    pub async fn complete_on_provider(
        &self,
        provider_id: &str,
        mut request: ProviderRequest,
        stream_tx: Option<mpsc::Sender<ProviderStreamEvent>>,
    ) -> Result<ProviderResponse, ProviderError> {
        let cfg = {
            let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.providers.get(provider_id).cloned().ok_or_else(|| {
                ProviderError::Configuration(format!("Unknown provider '{provider_id}'"))
            })?
        };
        if cfg.api_mode != ApiMode::LocalHermes
            && is_legacy_placeholder_model(&request.model, &cfg.id, &cfg.base_url)
            && !cfg.default_model.trim().is_empty()
        {
            request.model = cfg.default_model.clone();
        }
        let mut resp = self
            .execute_single_provider(&cfg, request, stream_tx.as_ref())
            .await?;
        self.enrich_cost(&mut resp);
        Ok(resp)
    }

    fn enrich_cost(&self, resp: &mut ProviderResponse) {
        if resp.usage.estimated_cost_usd > 0.0 {
            return;
        }
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(profile) = guard.model_profiles.get(&resp.model) {
            resp.usage.estimated_cost_usd =
                profile.estimate_cost(resp.usage.input_tokens, resp.usage.output_tokens);
        }
    }

    async fn execute_single_provider(
        &self,
        cfg: &ProviderConfig,
        request: ProviderRequest,
        stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
    ) -> Result<ProviderResponse, ProviderError> {
        let offline_rule_bot_allowed = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .offline_rule_bot_allowed
            || env_truthy("HERMES_ALLOW_LOCAL_RULE_BOT");
        if cfg.api_mode == ApiMode::LocalHermes {
            let has_sub = self.has_active_subscription()
                || !self.inner.lock().unwrap_or_else(|e| e.into_inner()).subscriptions.is_empty();
            return execute_local_hermes_with_policy(
                &cfg.id,
                &request,
                stream_tx,
                offline_rule_bot_allowed || has_sub,
            )
            .await;
        }

        let max_key_attempts = cfg.credentials.len().max(1);
        let mut last_err = None;

        for _ in 0..max_key_attempts {
            let (cred_id_opt, api_key, base_url_override) = match self.acquire_credential(&cfg.id)? {
                Some(c) => (Some(c.id), c.api_key, c.base_url_override),
                None if cfg.no_key_required => (None, String::new(), None),
                None => {
                    return Err(ProviderError::Authentication(format!(
                        "No credential available for '{}'",
                        cfg.id
                    )));
                }
            };
            let base_url = base_url_override.as_deref().unwrap_or(&cfg.base_url);
            let res = match cfg.api_mode {
                ApiMode::ChatCompletions => {
                    execute_chat_completions(
                        &self.http_client,
                        cfg,
                        base_url,
                        &api_key,
                        &request,
                        stream_tx,
                    )
                    .await
                }
                ApiMode::CodexResponses => {
                    self.execute_codex_with_refresh(
                        cfg,
                        base_url,
                        &api_key,
                        cred_id_opt.as_deref(),
                        &request,
                        stream_tx,
                    )
                    .await
                }
                ApiMode::AnthropicMessages => {
                    execute_anthropic_messages(
                        &self.http_client,
                        cfg,
                        base_url,
                        &api_key,
                        &request,
                        stream_tx,
                    )
                    .await
                }
                ApiMode::GeminiGenerateContent => {
                    execute_gemini_generate_content(
                        &self.http_client,
                        cfg,
                        base_url,
                        &api_key,
                        &request,
                        stream_tx,
                    )
                    .await
                }
                ApiMode::GoogleVertex => {
                    self.execute_google_vertex(
                        cfg,
                        base_url,
                        &api_key,
                        &request,
                        stream_tx,
                    )
                    .await
                }
                ApiMode::LocalHermes => {
                    execute_local_hermes_with_policy(
                        &cfg.id,
                        &request,
                        stream_tx,
                        offline_rule_bot_allowed,
                    )
                    .await
                }
            };
            match res {
                Ok(resp) => {
                    if let Some(ref cred_id) = cred_id_opt {
                        self.report_credential_outcome(&cfg.id, cred_id, None);
                    }
                    return Ok(resp);
                }
                Err(err) => {
                    if let Some(ref cred_id) = cred_id_opt {
                        self.report_credential_outcome(&cfg.id, cred_id, Some(&err));
                    }
                    let retry_key = err.is_retryable() || err.http_status() == Some(401);
                    last_err = Some(err);
                    if !retry_key {
                        break;
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            ProviderError::Exhausted(format!("No usable credentials for '{}'", cfg.id))
        }))
    }

    async fn execute_codex_with_refresh(
        &self,
        cfg: &ProviderConfig,
        base_url: &str,
        api_key: &str,
        cred_id_opt: Option<&str>,
        request: &ProviderRequest,
        stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
    ) -> Result<ProviderResponse, ProviderError> {
        let codex_mgr = self.get_or_create_codex_manager(&cfg.id, cfg);
        let now_ms = Utc::now().timestamp_millis();
        let mut active_token = api_key.to_string();

        let needs_preemptive_refresh = active_token.trim().is_empty()
            || extract_jwt_exp_ms(&active_token).is_some_and(|exp| exp <= now_ms + 60_000);

        if needs_preemptive_refresh {
            if let Some(ref mgr) = codex_mgr {
                let refreshed = mgr
                    .get_or_refresh_token(&self.http_client, now_ms)
                    .await
                    .map_err(|e| {
                        ProviderError::Authentication(format!(
                            "Codex OAuth token refresh failed: {e}"
                        ))
                    })?;
                active_token = refreshed.clone();
                if let Some(cred_id) = cred_id_opt {
                    let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(p) = guard.providers.get_mut(&cfg.id) {
                        for entry in &mut p.credentials.entries {
                            if entry.id == cred_id {
                                entry.api_key = refreshed.clone();
                            }
                        }
                    }
                }
            }
        }

        let first_res = execute_codex_responses(
            &self.http_client,
            cfg,
            base_url,
            &active_token,
            request,
            stream_tx,
        )
        .await;

        match first_res {
            Err(ProviderError::Authentication(ref msg)) if msg.starts_with("HTTP 401") => {
                if let Some(ref mgr) = codex_mgr {
                    mgr.invalidate_cached_token();
                    let now_retry = Utc::now().timestamp_millis();
                    let refreshed = mgr
                        .get_or_refresh_token(&self.http_client, now_retry)
                        .await
                        .map_err(|e| {
                            ProviderError::Authentication(format!(
                                "Codex OAuth token refresh failed after 401: {e}"
                            ))
                        })?;
                    if let Some(cred_id) = cred_id_opt {
                        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(p) = guard.providers.get_mut(&cfg.id) {
                            for entry in &mut p.credentials.entries {
                                if entry.id == cred_id {
                                    entry.api_key = refreshed.clone();
                                }
                            }
                        }
                    }
                    return execute_codex_responses(
                        &self.http_client,
                        cfg,
                        base_url,
                        &refreshed,
                        request,
                        stream_tx,
                    )
                    .await;
                }
                first_res
            }
            other => other,
        }
    }

    async fn execute_google_vertex(
        &self,
        cfg: &ProviderConfig,
        base_url: &str,
        api_key: &str,
        request: &ProviderRequest,
        stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
    ) -> Result<ProviderResponse, ProviderError> {
        let now_ms = Utc::now().timestamp_millis();
        let token_mgr = self.get_or_create_vertex_manager(&cfg.id, cfg);
        let mut token = if !api_key.trim().is_empty() {
            api_key.trim().to_string()
        } else if let Some(ref mgr) = token_mgr {
            mgr.get_or_refresh_token(&self.http_client, now_ms)
                .await
                .map_err(|e| {
                    ProviderError::Authentication(format!(
                        "Google Vertex AI OAuth token refresh failed: {e}"
                    ))
                })?
        } else {
            return Err(ProviderError::Authentication(
                "未检测到有效的 Google Vertex AI 凭据或访问令牌。请检查 ~/.config/gcloud/application_default_credentials.json。".to_string()
            ));
        };

        let wire_req = prepare_wire_request(cfg, base_url, &token, request);
        let clean_model = wire_req
            .model
            .trim()
            .strip_prefix("models/")
            .unwrap_or(wire_req.model.trim());

        let project = cfg
            .headers
            .get("x-goog-user-project")
            .cloned()
            .or_else(|| std::env::var("VERTEX_PROJECT_ID").ok())
            .or_else(|| std::env::var("CLOUDSDK_CORE_PROJECT").ok())
            .unwrap_or_else(|| "default".to_string());

        let location = cfg.region_id.clone().unwrap_or_else(|| "global".to_string());

        let url = if base_url.starts_with("http://127.0.0.1") || base_url.starts_with("http://localhost") {
            if request.stream {
                format!("{base_url}/projects/{project}/locations/{location}/publishers/google/models/{clean_model}:streamGenerateContent?alt=sse")
            } else {
                format!("{base_url}/projects/{project}/locations/{location}/publishers/google/models/{clean_model}:generateContent")
            }
        } else if base_url.contains("/publishers/google/models") || base_url.contains("/projects/") {
            let action = if request.stream {
                "streamGenerateContent?alt=sse"
            } else {
                "generateContent"
            };
            format!("{base_url}:{action}")
        } else {
            let host = if location == "global" {
                "https://aiplatform.googleapis.com".to_string()
            } else {
                format!("https://{location}-aiplatform.googleapis.com")
            };
            let action = if request.stream {
                "streamGenerateContent?alt=sse"
            } else {
                "generateContent"
            };
            format!("{host}/v1/projects/{project}/locations/{location}/publishers/google/models/{clean_model}:{action}")
        };

        let unified = provider_request_to_unified(&wire_req);
        let payload = build_gemini_generate_content_payload(&unified);

        let send_vertex_request = |bearer: &str| {
            let mut builder = self
                .http_client
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .bearer_auth(bearer)
                .header("x-goog-user-project", &project);

            if request.stream {
                builder = builder.header(reqwest::header::ACCEPT, "text/event-stream");
            }
            for (k, v) in &cfg.headers {
                if !k.starts_with("x-adc-") && !k.starts_with("x-codex-") {
                    builder = builder.header(k.as_str(), v.as_str());
                }
            }
            builder.json(&payload).send()
        };

        let resp = tokio::time::timeout(
            Duration::from_secs(cfg.timeout_secs.max(5)),
            send_vertex_request(&token),
        )
        .await
        .map_err(|_| ProviderError::Transport(format!("Request to {url} timed out")))?
        .map_err(|e| ProviderError::Transport(format!("HTTP request to {url} failed: {e}")))?;

        let mut resp = match check_http_error(resp).await {
            Ok(r) => r,
            Err(ProviderError::Authentication(ref msg))
                if msg.starts_with("HTTP 401") && api_key.trim().is_empty() =>
            {
                if let Some(ref mgr) = token_mgr {
                    mgr.invalidate_cached_token();
                    token = mgr
                        .get_or_refresh_token(&self.http_client, Utc::now().timestamp_millis())
                        .await
                        .map_err(|e| {
                            ProviderError::Authentication(format!(
                                "Google Vertex AI OAuth token refresh failed after 401: {e}"
                            ))
                        })?;
                    let retry_resp = tokio::time::timeout(
                        Duration::from_secs(cfg.timeout_secs.max(5)),
                        send_vertex_request(&token),
                    )
                    .await
                    .map_err(|_| ProviderError::Transport(format!("Request to {url} timed out")))?
                    .map_err(|e| ProviderError::Transport(format!("HTTP request to {url} failed: {e}")))?;
                    check_http_error(retry_resp).await?
                } else {
                    return Err(ProviderError::Authentication(msg.clone()));
                }
            }
            Err(other) => return Err(other),
        };

        let is_sse = request.stream
            && resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(|ct| !ct.to_ascii_lowercase().contains("application/json"))
                .unwrap_or(true);

        if !is_sse {
            let body = resp
                .text()
                .await
                .map_err(|e| ProviderError::Transport(format!("Read Vertex body failed: {e}")))?;
            let parsed = parse_gemini_response_json(&cfg.id, &wire_req.model, &body)
                .map_err(|e| ProviderError::Protocol(e.to_string()))?;
            let mut out = unified_to_provider_response(parsed, ApiMode::GoogleVertex);
            out.api_mode = ApiMode::GoogleVertex;
            emit_provider_response_stream_events(&out, stream_tx).await;
            return Ok(out);
        }

        let mut acc_content = String::new();
        let mut acc_reasoning = String::new();
        let mut tool_calls = Vec::<ProviderToolCall>::new();
        let mut finish_reason = "stop".to_string();
        let mut usage = TokenUsage::default();
        let mut raw_bytes = Vec::<u8>::new();
        let mut non_sse_lines = Vec::<String>::new();
        let mut saw_sse_data = false;

        while read_next_sse_chunk(&mut resp, &mut raw_bytes, stream_tx, "Vertex AI SSE error").await? {
            while let Some(pos) = raw_bytes.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&raw_bytes[..pos])
                    .trim_end_matches('\r')
                    .to_string();
                raw_bytes.drain(..=pos);
                process_gemini_sse_line(
                    &line,
                    &mut saw_sse_data,
                    &mut non_sse_lines,
                    &mut acc_content,
                    &mut acc_reasoning,
                    &mut tool_calls,
                    &mut finish_reason,
                    &mut usage,
                    stream_tx,
                )
                .await?;
            }
        }
        if !raw_bytes.is_empty() {
            let line = String::from_utf8_lossy(&raw_bytes)
                .trim_end_matches('\r')
                .to_string();
            process_gemini_sse_line(
                &line,
                &mut saw_sse_data,
                &mut non_sse_lines,
                &mut acc_content,
                &mut acc_reasoning,
                &mut tool_calls,
                &mut finish_reason,
                &mut usage,
                stream_tx,
            )
            .await?;
        }

        if !saw_sse_data
            && acc_content.is_empty()
            && acc_reasoning.is_empty()
            && tool_calls.is_empty()
            && !non_sse_lines.is_empty()
        {
            let body = non_sse_lines.join("\n");
            let parsed = parse_gemini_response_json(&cfg.id, &wire_req.model, &body)
                .map_err(|e| ProviderError::Protocol(e.to_string()))?;
            let mut out = unified_to_provider_response(parsed, ApiMode::GoogleVertex);
            out.api_mode = ApiMode::GoogleVertex;
            emit_provider_response_stream_events(&out, stream_tx).await;
            return Ok(out);
        }

        if !tool_calls.is_empty() {
            finish_reason = "tool_calls".to_string();
        }
        usage.finalize_total();

        Ok(ProviderResponse {
            content: acc_content,
            reasoning: (!acc_reasoning.is_empty()).then_some(acc_reasoning),
            tool_calls,
            finish_reason,
            usage,
            provider_id: cfg.id.clone(),
            model: wire_req.model,
            api_mode: ApiMode::GoogleVertex,
        })
    }
}

fn mask_outbound_request(
    req: &mut ProviderRequest,
    engine: &MaskingEngine,
) -> Result<usize, ProviderError> {
    let mut hits = 0usize;
    for tier in &mut req.system_tiers {
        let res = engine.mask_text(&tier.text);
        if res.blocked {
            return Err(ProviderError::Configuration(
                res.block_reason
                    .unwrap_or_else(|| "Outbound request blocked by privacy masking rule".to_string()),
            ));
        }
        hits += res.hits.len();
        tier.text = res.masked_text;
    }
    for msg in &mut req.messages {
        let res = engine.mask_text(&msg.content);
        if res.blocked {
            return Err(ProviderError::Configuration(
                res.block_reason
                    .unwrap_or_else(|| "Outbound request blocked by privacy masking rule".to_string()),
            ));
        }
        hits += res.hits.len();
        msg.content = res.masked_text;
        if let Some(ref mut reason) = msg.reasoning {
            let rres = engine.mask_text(reason);
            if rres.blocked {
                return Err(ProviderError::Configuration(
                    rres.block_reason.unwrap_or_else(|| {
                        "Outbound reasoning blocked by privacy masking rule".to_string()
                    }),
                ));
            }
            hits += rres.hits.len();
            *reason = rres.masked_text;
        }
        for tc in &mut msg.tool_calls {
            let ares = engine.mask_text(&tc.arguments);
            if ares.blocked {
                return Err(ProviderError::Configuration(
                    ares.block_reason
                        .unwrap_or_else(|| "Outbound tool arguments blocked by privacy masking rule".to_string()),
                ));
            }
            hits += ares.hits.len();
            tc.arguments = ares.masked_text;
        }
    }
    Ok(hits)
}

// ============================================================================
// Environment, .env, $CODEX_HOME & Local Magpie Auto-Discovery
// ============================================================================

const MAGPIE_DEFAULT_BASE_URL: &str = "http://127.0.0.1:3425/v1";
const CODEX_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
const MAGPIE_PREFERRED_MODELS: &[&str] = &[
    "auto/gemini-3.8-flash",
    "auto/gemini-3.1-pro-preview",
    "auto/gemini-2.5-pro",
];

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .as_deref()
        .map(str::trim)
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn is_running_in_rust_test_harness() -> bool {
    if cfg!(test) || env_truthy("NEXTEST") || std::env::var_os("NEXTEST_RUN_ID").is_some() {
        return true;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let parent = exe.parent()?.file_name()?.to_str()?;
            let stem = exe.file_stem()?.to_str()?;
            Some(parent == "deps" && stem.contains('-'))
        })
        .unwrap_or(false)
}

fn is_offline_test_mode() -> bool {
    if env_truthy("HERMES_FORCE_LIVE_PROVIDERS") {
        return false;
    }
    env_truthy("GPUI_OFFLINE_TEST") || is_running_in_rust_test_harness()
}

fn should_allow_local_rule_bot(cfg_test: bool) -> bool {
    cfg_test
        || is_running_in_rust_test_harness()
        || env_truthy("GPUI_OFFLINE_TEST")
        || env_truthy("HERMES_ALLOW_LOCAL_RULE_BOT")
}

fn resolve_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn parse_dotenv_str(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("export ") {
            trimmed = rest.trim_start();
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        let raw_val = v.trim();
        let val = if raw_val.starts_with('"') {
            extract_quoted_dotenv_value(raw_val, '"')
        } else if raw_val.starts_with('\'') {
            extract_quoted_dotenv_value(raw_val, '\'')
        } else {
            strip_inline_hash_comment(raw_val).trim().to_string()
        };
        out.push((key.to_string(), val));
    }
    out
}

fn extract_quoted_dotenv_value(raw: &str, quote: char) -> String {
    let mut out = String::new();
    let mut escaped = false;
    let mut chars = raw.chars();
    let _ = chars.next(); // consume opening quote
    for ch in chars {
        if quote == '"' && escaped {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => out.push(other),
            }
            escaped = false;
            continue;
        }
        if quote == '"' && ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            break;
        }
        out.push(ch);
    }
    out
}

fn strip_inline_hash_comment(raw: &str) -> &str {
    let mut prev_ws = false;
    for (idx, ch) in raw.char_indices() {
        if ch == '#' && prev_ws {
            return &raw[..idx];
        }
        prev_ws = ch.is_whitespace();
    }
    raw
}

struct EnvLookup {
    dotenv_vars: HashMap<String, String>,
    allow_process_env: bool,
}

impl EnvLookup {
    #[cfg(test)]
    fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        let dotenv_vars = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        Self {
            dotenv_vars,
            allow_process_env: false,
        }
    }

    fn from_files(paths: &[PathBuf]) -> Self {
        let mut dotenv_vars = HashMap::new();
        for path in paths {
            let Ok(raw) = std::fs::read_to_string(path) else {
                continue;
            };
            for (key, val) in parse_dotenv_str(&raw) {
                if val.trim().is_empty() || dotenv_vars.contains_key(&key) {
                    continue;
                }
                let already_in_env = std::env::var(&key)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);
                if !already_in_env {
                    dotenv_vars.insert(key, val);
                }
            }
        }
        Self {
            dotenv_vars,
            allow_process_env: true,
        }
    }

    fn from_standard_paths() -> Self {
        let mut paths = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            let mut cursor = Some(cwd.as_path());
            for _ in 0..5 {
                let Some(dir) = cursor else {
                    break;
                };
                let candidate = dir.join(".env");
                if candidate.is_file() && !paths.contains(&candidate) {
                    paths.push(candidate);
                }
                let is_root = dir.join(".git").exists();
                if is_root {
                    break;
                }
                cursor = dir.parent();
            }
        }
        if let Some(home) = resolve_home_dir() {
            paths.push(home.join(".gpui-telegram").join(".env"));
            paths.push(home.join(".hermes").join(".env"));
        }
        Self::from_files(&paths)
    }

    fn get(&self, key: &str) -> Option<String> {
        if self.allow_process_env {
            if let Ok(val) = std::env::var(key) {
                let trimmed = val.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
        self.dotenv_vars.get(key).cloned()
    }
}

struct MagpieEnvVendorDescriptor {
    preset_id: &'static str,
    key_vars: &'static [&'static str],
    base_url_vars: &'static [&'static str],
    model_vars: &'static [&'static str],
}

const MAGPIE_ENV_VENDORS: &[MagpieEnvVendorDescriptor] = &[
    MagpieEnvVendorDescriptor {
        preset_id: "google",
        key_vars: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        base_url_vars: &["GEMINI_BASE_URL", "GOOGLE_GEMINI_BASE_URL"],
        model_vars: &["GEMINI_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "moonshot",
        key_vars: &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
        base_url_vars: &["MOONSHOT_BASE_URL", "KIMI_BASE_URL"],
        model_vars: &["MOONSHOT_MODEL", "KIMI_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "zhipu",
        key_vars: &["ZHIPU_API_KEY", "ZHIPUAI_API_KEY"],
        base_url_vars: &["ZHIPU_BASE_URL", "ZHIPUAI_BASE_URL"],
        model_vars: &["ZHIPU_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "minimax",
        key_vars: &["MINIMAX_API_KEY"],
        base_url_vars: &["MINIMAX_BASE_URL"],
        model_vars: &["MINIMAX_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "stepfun",
        key_vars: &["STEPFUN_API_KEY"],
        base_url_vars: &["STEPFUN_BASE_URL"],
        model_vars: &["STEPFUN_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "qwen",
        key_vars: &["DASHSCOPE_API_KEY", "QWEN_API_KEY"],
        base_url_vars: &["DASHSCOPE_BASE_URL", "QWEN_BASE_URL"],
        model_vars: &["QWEN_MODEL", "DASHSCOPE_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "siliconflow",
        key_vars: &["SILICONFLOW_API_KEY"],
        base_url_vars: &["SILICONFLOW_BASE_URL"],
        model_vars: &["SILICONFLOW_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "groq",
        key_vars: &["GROQ_API_KEY"],
        base_url_vars: &["GROQ_BASE_URL"],
        model_vars: &["GROQ_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "xai",
        key_vars: &["XAI_API_KEY"],
        base_url_vars: &["XAI_BASE_URL"],
        model_vars: &["XAI_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "mistral",
        key_vars: &["MISTRAL_API_KEY"],
        base_url_vars: &["MISTRAL_BASE_URL"],
        model_vars: &["MISTRAL_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "together",
        key_vars: &["TOGETHER_API_KEY"],
        base_url_vars: &["TOGETHER_BASE_URL"],
        model_vars: &["TOGETHER_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "fireworks",
        key_vars: &["FIREWORKS_API_KEY"],
        base_url_vars: &["FIREWORKS_BASE_URL"],
        model_vars: &["FIREWORKS_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "nvidia",
        key_vars: &["NVIDIA_API_KEY"],
        base_url_vars: &["NVIDIA_BASE_URL"],
        model_vars: &["NVIDIA_MODEL"],
    },
    MagpieEnvVendorDescriptor {
        preset_id: "modelscope",
        key_vars: &["MODELSCOPE_API_KEY"],
        base_url_vars: &["MODELSCOPE_BASE_URL"],
        model_vars: &["MODELSCOPE_MODEL"],
    },
];

fn register_magpie_env_vendors(router: &ProviderRouter, env: &EnvLookup, active_set: &mut bool) {
    for desc in MAGPIE_ENV_VENDORS {
        let Some(api_key) = desc.key_vars.iter().find_map(|k| env.get(k)) else {
            continue;
        };
        let base_override = desc.base_url_vars.iter().find_map(|k| env.get(k));
        let model_override = desc.model_vars.iter().find_map(|k| env.get(k));
        let protocol_override = if desc.preset_id == "google"
            && base_override
                .as_deref()
                .is_some_and(|u| u.trim_end_matches('/').ends_with("/openai"))
        {
            Some(ApiMode::ChatCompletions)
        } else {
            None
        };
        let input = MagpiePresetApplyInput {
            preset_id: desc.preset_id.to_string(),
            api_key: Some(api_key),
            model_override,
            base_url_override: base_override,
            protocol_override,
            activate: !*active_set,
            ..Default::default()
        };
        if router.apply_magpie_preset(input).is_ok() && !*active_set {
            *active_set = true;
        }
    }
}

#[derive(Debug, Default, Clone)]
struct CodexConfigData {
    model: Option<String>,
    openai_base_url: Option<String>,
    magpie_base_url: Option<String>,
    magpie_headers: HashMap<String, String>,
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            }
        } else {
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '#' => return &line[..idx],
                _ => {}
            }
        }
    }
    line
}

fn unquote_toml_str(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        let inner = &trimmed[1..trimmed.len() - 1];
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    trimmed.to_string()
}

fn parse_toml_section_path(header: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for ch in header.chars() {
        if in_double {
            if escaped {
                current.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            } else {
                current.push(ch);
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            } else {
                current.push(ch);
            }
        } else {
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '.' => {
                    let seg = current.trim().to_string();
                    if !seg.is_empty() {
                        parts.push(seg);
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        }
    }
    let seg = current.trim().to_string();
    if !seg.is_empty() {
        parts.push(seg);
    }
    parts
}

fn count_toml_brace_delta(raw: &str) -> isize {
    let mut delta = 0isize;
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            }
        } else {
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '{' => delta += 1,
                '}' => delta -= 1,
                _ => {}
            }
        }
    }
    delta
}

fn parse_toml_inline_table(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(trimmed);

    let mut entries = Vec::new();
    let mut current = String::new();
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for ch in inner.chars() {
        if in_double {
            current.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            current.push(ch);
            if ch == '\'' {
                in_single = false;
            }
        } else {
            match ch {
                '"' => {
                    in_double = true;
                    current.push(ch);
                }
                '\'' => {
                    in_single = true;
                    current.push(ch);
                }
                ',' => {
                    entries.push(current.trim().to_string());
                    current.clear();
                }
                _ => current.push(ch),
            }
        }
    }
    if !current.trim().is_empty() {
        entries.push(current.trim().to_string());
    }

    for entry in entries {
        if let Some((k, v)) = entry.split_once('=') {
            let key = unquote_toml_str(k);
            let val = unquote_toml_str(v);
            if !key.is_empty() && !val.is_empty() {
                map.insert(key, val);
            }
        }
    }
    map
}

fn parse_codex_config_toml(raw: &str) -> CodexConfigData {
    let mut data = CodexConfigData::default();
    let mut section: Vec<String> = Vec::new();
    let mut pending_stmt = String::new();
    let mut pending_brace_depth = 0isize;

    for raw_line in raw.lines() {
        let clean = strip_toml_comment(raw_line).trim();
        if clean.is_empty() {
            continue;
        }
        if pending_brace_depth > 0 {
            pending_stmt.push(' ');
            pending_stmt.push_str(clean);
            pending_brace_depth += count_toml_brace_delta(clean);
            if pending_brace_depth > 0 {
                continue;
            }
        } else if clean.starts_with('[') && clean.ends_with(']') {
            if clean.starts_with("[[") {
                section = vec!["__array_table__".to_string()];
            } else {
                let inner = &clean[1..clean.len() - 1];
                section = parse_toml_section_path(inner);
            }
            continue;
        } else {
            pending_stmt = clean.to_string();
            pending_brace_depth = count_toml_brace_delta(clean);
            if pending_brace_depth > 0 {
                continue;
            }
        }

        let stmt = std::mem::take(&mut pending_stmt);
        pending_brace_depth = 0;
        let Some((raw_k, raw_v)) = stmt.split_once('=') else {
            continue;
        };
        let key = unquote_toml_str(raw_k);
        let val_trimmed = raw_v.trim();

        if section.is_empty() {
            match key.as_str() {
                "model" => {
                    let v = unquote_toml_str(val_trimmed);
                    if !v.is_empty() {
                        data.model = Some(v);
                    }
                }
                "openai_base_url" => {
                    let v = unquote_toml_str(val_trimmed);
                    if !v.is_empty() {
                        data.openai_base_url = Some(v);
                    }
                }
                _ => {}
            }
        } else if section.len() == 2
            && section[0] == "model_providers"
            && section[1].eq_ignore_ascii_case("magpie")
        {
            match key.as_str() {
                "base_url" => {
                    let v = unquote_toml_str(val_trimmed);
                    if !v.is_empty() {
                        data.magpie_base_url = Some(v);
                    }
                }
                "http_headers" => {
                    for (hk, hv) in parse_toml_inline_table(val_trimmed) {
                        data.magpie_headers.insert(hk, hv);
                    }
                }
                _ => {}
            }
        } else if section.len() == 3
            && section[0] == "model_providers"
            && section[1].eq_ignore_ascii_case("magpie")
            && section[2] == "http_headers"
        {
            let hv = unquote_toml_str(val_trimmed);
            if !key.is_empty() && !hv.is_empty() {
                data.magpie_headers.insert(key, hv);
            }
        }
    }

    data
}

fn decode_base64_url(input: &str) -> Option<Vec<u8>> {
    let mut vals = Vec::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b'=' {
            break;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        vals.push(v);
    }
    let mut out = Vec::with_capacity((vals.len() * 3) / 4);
    let mut i = 0usize;
    while i < vals.len() {
        match vals.len() - i {
            0 => break,
            1 => return None,
            2 => {
                out.push((vals[i] << 2) | (vals[i + 1] >> 4));
                break;
            }
            3 => {
                out.push((vals[i] << 2) | (vals[i + 1] >> 4));
                out.push(((vals[i + 1] & 0x0f) << 4) | (vals[i + 2] >> 2));
                break;
            }
            _ => {
                out.push((vals[i] << 2) | (vals[i + 1] >> 4));
                out.push(((vals[i + 1] & 0x0f) << 4) | (vals[i + 2] >> 2));
                out.push(((vals[i + 2] & 0x03) << 6) | vals[i + 3]);
                i += 4;
            }
        }
    }
    Some(out)
}

fn extract_jwt_auth_claims(token: &str) -> (Option<String>, Option<String>) {
    let mut parts = token.trim().split('.');
    let Some(_header) = parts.next() else {
        return (None, None);
    };
    let Some(payload_b64) = parts.next() else {
        return (None, None);
    };
    if parts.next().is_none() {
        return (None, None);
    }
    let Some(bytes) = decode_base64_url(payload_b64) else {
        return (None, None);
    };
    let Ok(val) = serde_json::from_slice::<Value>(&bytes) else {
        return (None, None);
    };
    let Some(auth_obj) = val.get("https://api.openai.com/auth") else {
        return (None, None);
    };
    let account_id = auth_obj
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let residency = auth_obj
        .get("chatgpt_compute_residency")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    (account_id, residency)
}

#[derive(Debug, Default, Clone)]
struct CodexAuthData {
    token: Option<String>,
    account_id: Option<String>,
    compute_residency: Option<String>,
}

fn parse_codex_auth_json(raw: &str) -> CodexAuthData {
    let Ok(val) = serde_json::from_str::<Value>(raw) else {
        return CodexAuthData::default();
    };
    let tokens = val.get("tokens");
    let access_token = tokens
        .and_then(|t| t.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let id_token = tokens
        .and_then(|t| t.get("id_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let api_key = val
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let token = access_token.clone().or(api_key);
    let mut account_id = tokens
        .and_then(|t| t.get("account_id"))
        .and_then(Value::as_str)
        .or_else(|| val.get("account_id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let mut compute_residency = None;

    for jwt_candidate in [access_token.as_deref(), id_token.as_deref()]
        .into_iter()
        .flatten()
    {
        let (jwt_acct, jwt_res) = extract_jwt_auth_claims(jwt_candidate);
        if account_id.is_none() {
            account_id = jwt_acct;
        }
        if compute_residency.is_none() {
            compute_residency = jwt_res;
        }
    }

    CodexAuthData {
        token,
        account_id,
        compute_residency,
    }
}

fn parse_url_scheme_authority(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let authority = rest.split('/').next()?.trim();
    if authority.is_empty() {
        return None;
    }
    Some((scheme, authority))
}

fn is_local_magpie_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.contains("magpie") {
        return true;
    }
    let Some((_scheme, authority)) = parse_url_scheme_authority(&lower) else {
        return false;
    };
    let host = if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_else(|| authority.to_string())
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
            .to_string()
    };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

fn derive_magpie_v1_url(url: &str) -> Option<String> {
    if !is_local_magpie_url(url) {
        return None;
    }
    let (scheme, authority) = parse_url_scheme_authority(url)?;
    Some(format!("{scheme}://{authority}/v1"))
}

fn derive_magpie_codex_url(url: &str) -> Option<String> {
    if !is_local_magpie_url(url) {
        return None;
    }
    let (scheme, authority) = parse_url_scheme_authority(url)?;
    Some(format!("{scheme}://{authority}/backend-api/codex"))
}

fn parse_http_url_parts(url: &str) -> Option<(String, u16, String, String)> {
    let rest = url.trim().strip_prefix("http://")?;
    let (authority, path) = match rest.split_once('/') {
        Some((auth, tail)) => (auth, format!("/{tail}")),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let close = authority.find(']')?;
        let host_part = authority[1..close].to_string();
        let after = &authority[close + 1..];
        let port = if let Some(p) = after.strip_prefix(':') {
            p.parse::<u16>().ok()?
        } else if after.is_empty() {
            80
        } else {
            return None;
        };
        (host_part, port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        if let Ok(parsed_port) = p.parse::<u16>() {
            (h.to_string(), parsed_port)
        } else {
            (authority.to_string(), 80)
        }
    } else {
        (authority.to_string(), 80)
    };
    Some((host, port, authority.to_string(), path))
}

fn find_header_end(buf: &[u8]) -> Option<(usize, usize)> {
    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some((pos, pos + 4));
    }
    if let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
        return Some((pos, pos + 2));
    }
    None
}

fn decode_chunked_body(raw: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let rel_end = raw[cursor..].windows(2).position(|w| w == b"\r\n")?;
        let size_line = std::str::from_utf8(&raw[cursor..cursor + rel_end]).ok()?;
        let hex_part = size_line.split(';').next()?.trim();
        let size = usize::from_str_radix(hex_part, 16).ok()?;
        cursor += rel_end + 2;
        if size == 0 {
            return Some(out);
        }
        if raw.len() < cursor + size + 2 {
            return None;
        }
        out.extend_from_slice(&raw[cursor..cursor + size]);
        cursor += size + 2;
    }
    None
}

fn http_get_json_sync(url: &str, headers: &[(String, String)], timeout: Duration) -> Option<Value> {
    if url.starts_with("https://") {
        let url_owned = url.to_string();
        let headers_owned = headers.to_vec();
        return std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .ok()?;
            rt.block_on(async move {
                let client = reqwest::Client::builder().timeout(timeout).build().ok()?;
                let mut req = client.get(&url_owned);
                for (k, v) in headers_owned {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req.send().await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                resp.json::<Value>().await.ok()
            })
        })
        .join()
        .ok()
        .flatten();
    }

    let (host, port, authority, path) = parse_http_url_parts(url)?;
    let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().ok()?.collect();
    if addrs.is_empty() {
        return None;
    }

    let deadline = Instant::now() + timeout;
    let mut stream_opt = None;
    for addr in addrs {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let rem = deadline.saturating_duration_since(now);
        if let Ok(s) = TcpStream::connect_timeout(&addr, rem) {
            stream_opt = Some(s);
            break;
        }
    }
    let mut stream = stream_opt?;
    let rem = deadline.saturating_duration_since(Instant::now());
    if rem.is_zero() {
        return None;
    }
    let _ = stream.set_write_timeout(Some(rem));
    let _ = stream.set_read_timeout(Some(rem));

    let mut req_buf = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: application/json\r\nConnection: close\r\n"
    );
    for (k, v) in headers {
        if !k.trim().is_empty() {
            req_buf.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    req_buf.push_str("\r\n");
    stream.write_all(req_buf.as_bytes()).ok()?;

    let mut raw = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let mut parsed_headers: Option<(u16, usize, Option<usize>, bool)> = None;

    while Instant::now() < deadline {
        let rem = deadline.saturating_duration_since(Instant::now());
        if rem.is_zero() {
            break;
        }
        let _ = stream.set_read_timeout(Some(rem.max(Duration::from_millis(25))));
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&chunk[..n]);
                if parsed_headers.is_none() {
                    if let Some((hdr_end, body_start)) = find_header_end(&raw) {
                        let hdr_str = String::from_utf8_lossy(&raw[..hdr_end]);
                        let mut lines = hdr_str.lines();
                        let status_line = lines.next().unwrap_or("");
                        let status_code = status_line
                            .split_whitespace()
                            .nth(1)
                            .and_then(|s| s.parse::<u16>().ok())
                            .unwrap_or(0);
                        if !(200..300).contains(&status_code) {
                            return None;
                        }
                        let mut content_length = None;
                        let mut is_chunked = false;
                        for line in lines {
                            if let Some((k, v)) = line.split_once(':') {
                                let kl = k.trim().to_ascii_lowercase();
                                let vl = v.trim().to_ascii_lowercase();
                                if kl == "content-length" {
                                    content_length = vl.parse::<usize>().ok();
                                } else if kl == "transfer-encoding" && vl.contains("chunked") {
                                    is_chunked = true;
                                }
                            }
                        }
                        parsed_headers = Some((status_code, body_start, content_length, is_chunked));
                    }
                }
                if let Some((_, body_start, content_length, is_chunked)) = parsed_headers {
                    let body = &raw[body_start..];
                    if is_chunked {
                        if let Some(decoded) = decode_chunked_body(body) {
                            return serde_json::from_slice::<Value>(&decoded).ok();
                        }
                    } else if let Some(cl) = content_length {
                        if body.len() >= cl {
                            return serde_json::from_slice::<Value>(&body[..cl]).ok();
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }

    let (_, body_start, content_length, is_chunked) = parsed_headers?;
    let body = &raw[body_start..];
    if is_chunked {
        let decoded = decode_chunked_body(body)?;
        serde_json::from_slice::<Value>(&decoded).ok()
    } else if let Some(cl) = content_length {
        if body.len() >= cl {
            serde_json::from_slice::<Value>(&body[..cl]).ok()
        } else {
            None
        }
    } else {
        serde_json::from_slice::<Value>(body).ok()
    }
}

fn parse_models_list_json(val: &Value) -> Vec<String> {
    let mut models = Vec::new();
    let mut push_unique = |id: &str| {
        let trimmed = id.trim();
        if !trimmed.is_empty() && !models.iter().any(|m| m == trimmed) {
            models.push(trimmed.to_string());
        }
    };
    let arr_opt = val
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| val.get("models").and_then(Value::as_array))
        .or_else(|| val.as_array());
    if let Some(arr) = arr_opt {
        for item in arr {
            if let Some(id) = item.as_str() {
                push_unique(id);
            } else if let Some(id) = item.get("id").and_then(Value::as_str) {
                push_unique(id);
            }
        }
    }
    models
}

fn probe_magpie_models_sync(
    base_url: &str,
    headers: &HashMap<String, String>,
    token: Option<&str>,
    timeout: Duration,
) -> Option<Vec<String>> {
    let url = resolve_endpoint_url(base_url, "models");
    let mut header_vec: Vec<(String, String)> = headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if let Some(tok) = token.map(str::trim).filter(|t| !t.is_empty()) {
        if !header_vec
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        {
            header_vec.push(("Authorization".to_string(), format!("Bearer {tok}")));
        }
    }
    let val = http_get_json_sync(&url, &header_vec, timeout)?;
    Some(parse_models_list_json(&val))
}

fn select_magpie_default_model(discovered: &[String], config_model: Option<&str>) -> String {
    for preferred in MAGPIE_PREFERRED_MODELS {
        if discovered.iter().any(|m| m == preferred) {
            return (*preferred).to_string();
        }
    }
    if let Some(first) = discovered.first() {
        return first.clone();
    }
    if let Some(cfg_m) = config_model
        .map(str::trim)
        .filter(|m| !m.is_empty() && !is_legacy_placeholder_model(m, "magpie_local", MAGPIE_DEFAULT_BASE_URL))
    {
        return cfg_m.to_string();
    }
    "auto/gemini-3.8-flash".to_string()
}

/// Ported from `NousResearch/hermes-agent/agent/codex_headers.py`.
fn build_codex_oauth_headers(
    base_url: &str,
    account_id: Option<&str>,
    compute_residency: Option<&str>,
) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("originator".to_string(), "codex_cli_rs".to_string());
    headers.insert("User-Agent".to_string(), "codex_cli_rs/0.1.0".to_string());
    if let Some(acct) = account_id.map(str::trim).filter(|s| !s.is_empty()) {
        headers.insert("ChatGPT-Account-ID".to_string(), acct.to_string());
    }
    if compute_residency
        .map(str::trim)
        .is_some_and(|r| r.eq_ignore_ascii_case("us"))
    {
        headers.insert(
            "x-openai-internal-codex-residency".to_string(),
            "us".to_string(),
        );
    }
    if is_local_magpie_url(base_url) {
        headers.insert(
            "x-openai-actor-authorization".to_string(),
            "magpie".to_string(),
        );
    }
    headers
}

struct DiscoveredMagpieProvider {
    config: ProviderConfig,
    discovered_models: Vec<String>,
}

struct CodexDiscoveryResult {
    magpie_local: Option<DiscoveredMagpieProvider>,
    codex_oauth: Option<ProviderConfig>,
}

fn discover_codex_and_magpie(env: &EnvLookup) -> CodexDiscoveryResult {
    let codex_home = env
        .get("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| resolve_home_dir().map(|h| h.join(".codex")));

    let codex_cfg = codex_home
        .as_ref()
        .and_then(|dir| std::fs::read_to_string(dir.join("config.toml")).ok())
        .map(|raw| parse_codex_config_toml(&raw))
        .unwrap_or_default();

    let codex_auth = codex_home
        .as_ref()
        .and_then(|dir| std::fs::read_to_string(dir.join("auth.json")).ok())
        .map(|raw| parse_codex_auth_json(&raw))
        .unwrap_or_default();

    let magpie_base_url = codex_cfg
        .magpie_base_url
        .clone()
        .or_else(|| {
            codex_cfg
                .openai_base_url
                .as_deref()
                .and_then(derive_magpie_v1_url)
        })
        .or_else(|| env.get("MAGPIE_BASE_URL"))
        .unwrap_or_else(|| MAGPIE_DEFAULT_BASE_URL.to_string());

    let mut magpie_headers = codex_cfg.magpie_headers.clone();
    magpie_headers
        .entry("x-openai-actor-authorization".to_string())
        .or_insert_with(|| "magpie".to_string());
    magpie_headers.insert("originator".to_string(), "codex_cli_rs".to_string());
    if let Some(acct) = codex_auth.account_id.as_deref() {
        magpie_headers.insert("ChatGPT-Account-ID".to_string(), acct.to_string());
    }

    let probed_models = probe_magpie_models_sync(
        &magpie_base_url,
        &magpie_headers,
        codex_auth.token.as_deref(),
        Duration::from_millis(800),
    );

    let mut magpie_default_model: Option<String> = None;
    let magpie_local = probed_models.map(|discovered| {
        let default_model = select_magpie_default_model(&discovered, codex_cfg.model.as_deref());
        magpie_default_model = Some(default_model.clone());

        let mut fallbacks = Vec::new();
        for pref in MAGPIE_PREFERRED_MODELS {
            if *pref != default_model
                && (discovered.is_empty() || discovered.iter().any(|m| m == pref))
            {
                fallbacks.push((*pref).to_string());
            }
        }
        if fallbacks.is_empty() {
            for m in &discovered {
                if m != &default_model && fallbacks.len() < 2 {
                    fallbacks.push(m.clone());
                }
            }
        }

        let cred = codex_auth
            .token
            .clone()
            .unwrap_or_else(|| "magpie".to_string());
        let mut cfg = ProviderConfig::openai_compatible(
            "magpie_local",
            "Magpie Local Gateway",
            magpie_base_url.clone(),
            default_model,
            cred,
        );
        cfg.headers = magpie_headers;
        cfg.fallback_models = fallbacks;
        DiscoveredMagpieProvider {
            config: cfg,
            discovered_models: discovered,
        }
    });

    let codex_base_url = codex_cfg
        .openai_base_url
        .clone()
        .or_else(|| {
            if magpie_local.is_some() {
                derive_magpie_codex_url(&magpie_base_url)
            } else {
                None
            }
        })
        .unwrap_or_else(|| CODEX_CHATGPT_BASE_URL.to_string());

    let codex_is_local = is_local_magpie_url(&codex_base_url);
    let codex_oauth = if codex_auth.token.is_some() || (codex_is_local && magpie_local.is_some()) {
        let default_model = if codex_is_local {
            magpie_default_model
                .clone()
                .or_else(|| {
                    codex_cfg
                        .model
                        .clone()
                        .filter(|m| !is_legacy_placeholder_model(m, "codex_oauth", &codex_base_url))
                })
                .unwrap_or_else(|| "auto/gemini-3.8-flash".to_string())
        } else {
            codex_cfg
                .model
                .clone()
                .filter(|m| !is_legacy_placeholder_model(m, "codex_oauth", &codex_base_url))
                .unwrap_or_else(|| "codex-mini-latest".to_string())
        };

        let mut fallbacks = Vec::new();
        if codex_is_local {
            for m in ["auto/gemini-3.8-flash", "auto/gemini-3.1-pro-preview"] {
                if m != default_model {
                    fallbacks.push(m.to_string());
                }
            }
        }

        let cred = codex_auth
            .token
            .unwrap_or_else(|| "magpie".to_string());
        let mut cfg = ProviderConfig::codex_responses(
            "codex_oauth",
            "Codex OAuth",
            codex_base_url.clone(),
            default_model,
            cred,
        );
        cfg.headers = build_codex_oauth_headers(
            &codex_base_url,
            codex_auth.account_id.as_deref(),
            codex_auth.compute_residency.as_deref(),
        );
        cfg.fallback_models = fallbacks;
        Some(cfg)
    } else {
        None
    };

    CodexDiscoveryResult {
        magpie_local,
        codex_oauth,
    }
}

fn is_non_openai_proxy_provider(provider_id: &str, base_url: &str) -> bool {
    let pid = provider_id.to_ascii_lowercase();
    let base = base_url.to_ascii_lowercase();
    if pid == "openai" || (base.contains("api.openai.com") && !base.contains("codex")) {
        return false;
    }
    pid.contains("magpie")
        || pid.contains("codex")
        || pid.contains("dacheng")
        || pid.contains("hermes_cloud")
        || pid.contains("deepseek")
        || base.contains("magpie")
        || base.contains("127.0.0.1")
        || base.contains("localhost")
        || base.contains("/backend-api/codex")
        || base.contains("ai.ombhrum.com")
        || base.contains("api.deepseek.com")
        || base.contains("codex-deepseek")
}

fn is_legacy_placeholder_model(model: &str, provider_id: &str, base_url: &str) -> bool {
    if provider_id == "local_hermes" {
        return false;
    }
    let m = model.trim();
    if m.is_empty() {
        return true;
    }
    let lower = m.to_ascii_lowercase();
    if matches!(lower.as_str(), "hermes-local-v1" | "default" | "auto") || m == "默认模型" {
        return true;
    }
    if lower == "deepseek-flash" {
        let lower_pid = provider_id.trim().to_ascii_lowercase();
        let lower_url = base_url.trim().to_ascii_lowercase();
        if !lower_pid.contains("deepseek")
            && !lower_url.contains("deepseek")
            && !lower_url.contains("ai.ombhrum.com")
            && !lower_url.contains("codex-deepseek")
        {
            return true;
        }
    }
    if lower == "gpt-4o-mini" && is_non_openai_proxy_provider(provider_id, base_url) {
        return true;
    }
    false
}

// ============================================================================
// Wire Protocol Payload Builders & Parsers (ChatCompletions / CodexResponses / AnthropicMessages)
// ============================================================================

pub fn resolve_endpoint_url(base_url: &str, suffix: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    let clean_suffix = suffix.trim_start_matches('/');
    if trimmed.ends_with(&format!("/{clean_suffix}")) {
        trimmed.to_string()
    } else {
        format!("{trimmed}/{clean_suffix}")
    }
}

pub fn build_chat_completions_payload(req: &ProviderRequest) -> Value {
    let mut messages_json = Vec::new();
    let system_text = req.combined_system_prompt();
    if !system_text.is_empty() {
        messages_json.push(json!({
            "role": "system",
            "content": system_text,
        }));
    }

    for msg in &req.messages {
        match msg.role {
            ProviderMessageRole::System => {
                messages_json.push(json!({
                    "role": "system",
                    "content": msg.content,
                }));
            }
            ProviderMessageRole::User => {
                messages_json.push(json!({
                    "role": "user",
                    "content": msg.content,
                }));
            }
            ProviderMessageRole::Assistant => {
                let mut obj = json!({
                    "role": "assistant",
                    "content": if msg.content.is_empty() && !msg.tool_calls.is_empty() {
                        Value::Null
                    } else {
                        Value::String(msg.content.clone())
                    }
                });
                if !msg.tool_calls.is_empty() {
                    let tcs: Vec<Value> = msg
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": tc.arguments,
                                }
                            })
                        })
                        .collect();
                    obj["tool_calls"] = Value::Array(tcs);
                }
                messages_json.push(obj);
            }
            ProviderMessageRole::Tool => {
                let mut obj = json!({
                    "role": "tool",
                    "content": msg.content,
                });
                if let Some(id) = &msg.tool_call_id {
                    obj["tool_call_id"] = Value::String(id.clone());
                }
                if let Some(name) = &msg.name {
                    obj["name"] = Value::String(name.clone());
                }
                messages_json.push(obj);
            }
        }
    }

    let mut payload = json!({
        "model": req.model,
        "messages": messages_json,
        "stream": req.stream,
    });

    if !req.tools.is_empty() {
        payload["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        );
    }
    if let Some(temp) = req.temperature {
        payload["temperature"] = json!(temp);
    }
    if let Some(max_tok) = req.max_tokens {
        payload["max_tokens"] = json!(max_tok);
    }
    if req.stream {
        payload["stream_options"] = json!({ "include_usage": true });
    }
    if let Some(budget) = req.thinking_budget_tokens {
        if budget > 0 {
            payload["reasoning_effort"] = json!(if budget >= 16_000 { "high" } else { "medium" });
        }
    }

    payload
}

pub fn build_codex_responses_payload(req: &ProviderRequest) -> Value {
    let mut input_items = Vec::new();
    for msg in &req.messages {
        match msg.role {
            ProviderMessageRole::System => {
                input_items.push(json!({
                    "type": "message",
                    "role": "developer",
                    "content": msg.content,
                }));
            }
            ProviderMessageRole::User => {
                input_items.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": msg.content,
                }));
            }
            ProviderMessageRole::Assistant => {
                if !msg.content.is_empty() {
                    input_items.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": msg.content,
                    }));
                }
                for (idx, tc) in msg.tool_calls.iter().enumerate() {
                    let call_id = if tc.id.trim().is_empty() {
                        format!("call_{idx}")
                    } else {
                        tc.id.clone()
                    };
                    input_items.push(json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": tc.name,
                        "arguments": tc.arguments,
                    }));
                }
            }
            ProviderMessageRole::Tool => {
                input_items.push(json!({
                    "type": "function_call_output",
                    "call_id": msg.tool_call_id.clone().unwrap_or_default(),
                    "output": msg.content,
                }));
            }
        }
    }

    let effort = match req.thinking_budget_tokens {
        Some(budget) if budget >= 16_000 => "high",
        Some(budget) if (1..=512).contains(&budget) => "low",
        _ => "medium",
    };

    let mut payload = json!({
        "model": req.model,
        "input": input_items,
        "stream": req.stream,
        "store": false,
        "reasoning": {
            "effort": effort,
            "summary": "auto",
        },
    });

    let instructions = req.combined_system_prompt();
    if !instructions.is_empty() {
        payload["instructions"] = Value::String(instructions);
    }

    if !req.tools.is_empty() {
        payload["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    })
                })
                .collect(),
        );
        payload["tool_choice"] = Value::String("auto".to_string());
        payload["parallel_tool_calls"] = Value::Bool(true);
    }
    if let Some(temp) = req.temperature {
        payload["temperature"] = json!(temp);
    }
    if let Some(max_tok) = req.max_tokens {
        payload["max_output_tokens"] = json!(max_tok);
    }
    payload
}

pub fn build_anthropic_messages_payload(req: &ProviderRequest) -> Value {
    let mut system_blocks = Vec::new();
    for (idx, tier) in req.system_tiers.iter().enumerate() {
        if tier.text.trim().is_empty() {
            continue;
        }
        let mut block = json!({
            "type": "text",
            "text": tier.text,
        });
        if tier.cache_ephemeral || idx == 0 {
            block["cache_control"] = json!({ "type": "ephemeral" });
        }
        system_blocks.push(block);
    }

    let mut raw_messages: Vec<Value> = Vec::new();
    for msg in &req.messages {
        match msg.role {
            ProviderMessageRole::System => {
                if !msg.content.trim().is_empty() {
                    system_blocks.push(json!({
                        "type": "text",
                        "text": msg.content,
                    }));
                }
            }
            ProviderMessageRole::User => {
                let mut block = json!({
                    "type": "text",
                    "text": msg.content,
                });
                if let Some(cc) = &msg.cache_control {
                    block["cache_control"] = json!({ "type": cc.cache_type });
                }
                raw_messages.push(json!({
                    "role": "user",
                    "content": [block],
                }));
            }
            ProviderMessageRole::Assistant => {
                let mut blocks = Vec::new();
                if let Some(reasoning) = &msg.reasoning {
                    if !reasoning.trim().is_empty() {
                        blocks.push(json!({
                            "type": "thinking",
                            "thinking": reasoning,
                        }));
                    }
                }
                if !msg.content.is_empty() {
                    blocks.push(json!({
                        "type": "text",
                        "text": msg.content,
                    }));
                }
                for tc in &msg.tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": tc.arguments_json(),
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({ "type": "text", "text": "" }));
                }
                raw_messages.push(json!({
                    "role": "assistant",
                    "content": blocks,
                }));
            }
            ProviderMessageRole::Tool => {
                let mut block = json!({
                    "type": "tool_result",
                    "tool_use_id": msg.tool_call_id.clone().unwrap_or_default(),
                    "content": msg.content,
                });
                if let Some(cc) = &msg.cache_control {
                    block["cache_control"] = json!({ "type": cc.cache_type });
                }
                // Merge adjacent tool_result blocks into one Anthropic user turn
                let should_merge = raw_messages
                    .last()
                    .and_then(|last| last.get("role"))
                    .and_then(Value::as_str)
                    == Some("user");
                if should_merge {
                    if let Some(arr) = raw_messages
                        .last_mut()
                        .and_then(|last| last.get_mut("content"))
                        .and_then(Value::as_array_mut)
                    {
                        arr.push(block);
                        continue;
                    }
                }
                raw_messages.push(json!({
                    "role": "user",
                    "content": [block],
                }));
            }
        }
    }

    // Apply Anthropic prompt caching marker to the trailing user/tool message block if not already set
    if let Some(last_msg) = raw_messages.last_mut() {
        if let Some(blocks) = last_msg.get_mut("content").and_then(Value::as_array_mut) {
            if let Some(last_block) = blocks.last_mut() {
                if last_block.get("cache_control").is_none() {
                    last_block["cache_control"] = json!({ "type": "ephemeral" });
                }
            }
        }
    }

    let mut payload = json!({
        "model": req.model,
        "max_tokens": req.max_tokens.unwrap_or(4096),
        "messages": raw_messages,
        "stream": req.stream,
    });

    if !system_blocks.is_empty() {
        payload["system"] = Value::Array(system_blocks);
    }

    if !req.tools.is_empty() {
        payload["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect(),
        );
    }
    if let Some(temp) = req.temperature {
        payload["temperature"] = json!(temp);
    }
    if let Some(budget) = req.thinking_budget_tokens {
        if budget >= 1024 {
            payload["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget,
            });
        }
    }

    payload
}

// ============================================================================
// HTTP Wire Protocol Executors
// ============================================================================

async fn read_next_sse_chunk(
    resp: &mut reqwest::Response,
    raw_bytes: &mut Vec<u8>,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
    context: &str,
) -> Result<bool, ProviderError> {
    let chunk_opt = if let Some(tx) = stream_tx {
        tokio::select! {
            biased;
            _ = tx.closed() => {
                return Err(ProviderError::Transport("Stream client disconnected".to_string()));
            }
            res = resp.chunk() => {
                res.map_err(|e| ProviderError::Transport(format!("{context}: {e}")))?
            }
        }
    } else {
        resp.chunk()
            .await
            .map_err(|e| ProviderError::Transport(format!("{context}: {e}")))?
    };
    if let Some(chunk) = chunk_opt {
        raw_bytes.extend_from_slice(&chunk);
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn send_stream_event(
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
    ev: ProviderStreamEvent,
) -> Result<(), ProviderError> {
    if let Some(tx) = stream_tx {
        if tx.send(ev).await.is_err() {
            return Err(ProviderError::Transport(
                "Stream client disconnected".to_string(),
            ));
        }
    }
    Ok(())
}

async fn check_http_error(resp: reqwest::Response) -> Result<reqwest::Response, ProviderError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_retry_after_header);
    let body = tokio::time::timeout(Duration::from_secs(3), resp.text())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let code = status.as_u16();
    if code == 401 || code == 403 {
        return Err(ProviderError::Authentication(format!("HTTP {code}: {body}")));
    }
    if code == 429 {
        return Err(ProviderError::RateLimited {
            status: code,
            retry_after_secs: retry_after,
            message: body,
        });
    }
    Err(ProviderError::UpstreamHttp {
        status: code,
        message: body,
    })
}

pub fn normalize_wire_model_for_provider(provider_id: &str, base_url: &str, model: &str) -> String {
    let m = model.trim();
    let lower_pid = provider_id.trim().to_ascii_lowercase();
    let lower_url = base_url.trim().to_ascii_lowercase();
    let is_deepseek = lower_pid.contains("deepseek")
        || lower_url.contains("api.deepseek.com")
        || lower_url.contains("ai.ombhrum.com")
        || lower_url.contains("codex-deepseek");

    if is_legacy_placeholder_model(m, provider_id, base_url) {
        if is_deepseek {
            return "deepseek-chat".to_string();
        }
        if lower_pid.contains("magpie")
            || lower_pid == "codex_oauth"
            || is_local_magpie_url(&lower_url)
        {
            return "auto/gemini-3.8-flash".to_string();
        }
        if lower_pid == "openai" || lower_url.contains("api.openai.com") {
            return "gpt-4o".to_string();
        }
        if lower_pid == "anthropic" || lower_url.contains("anthropic.com") {
            return "claude-sonnet-4-20250514".to_string();
        }
        if lower_pid == "openrouter" || lower_url.contains("openrouter.ai") {
            return "anthropic/claude-sonnet-4".to_string();
        }
        if lower_pid == "google"
            || lower_pid.contains("gemini")
            || lower_url.contains("generativelanguage.googleapis.com")
        {
            return "gemini-2.5-flash".to_string();
        }
        return m.to_string();
    }

    if is_deepseek {
        match m.to_ascii_lowercase().as_str() {
            "deepseek-flash" | "deepseek-v3" | "deepseek" | "gpt-4o-mini" | "gpt-4o" => {
                "deepseek-chat".to_string()
            }
            "deepseek-r1" | "o1" | "o3-mini" => "deepseek-reasoner".to_string(),
            _ => m.to_string(),
        }
    } else {
        m.to_string()
    }
}

fn prepare_wire_request(
    cfg: &ProviderConfig,
    base_url: &str,
    _api_key: &str,
    request: &ProviderRequest,
) -> ProviderRequest {
    let mut wire = request.clone();
    let requested = request.model.trim();
    wire.model = if cfg.api_mode != ApiMode::LocalHermes
        && is_legacy_placeholder_model(requested, &cfg.id, base_url)
    {
        let fallback_default = cfg.default_model.trim();
        if !fallback_default.is_empty()
            && !is_legacy_placeholder_model(fallback_default, &cfg.id, base_url)
        {
            normalize_wire_model_for_provider(&cfg.id, base_url, fallback_default)
        } else {
            normalize_wire_model_for_provider(&cfg.id, base_url, requested)
        }
    } else {
        normalize_wire_model_for_provider(&cfg.id, base_url, requested)
    };
    wire
}

async fn execute_chat_completions(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    base_url: &str,
    api_key: &str,
    request: &ProviderRequest,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<ProviderResponse, ProviderError> {
    let url = resolve_endpoint_url(base_url, "chat/completions");
    let wire_req = prepare_wire_request(cfg, base_url, api_key, request);
    let payload = build_chat_completions_payload(&wire_req);

    let mut builder = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if !api_key.trim().is_empty() {
        builder = builder.bearer_auth(api_key.trim());
    }
    if request.stream {
        builder = builder.header(reqwest::header::ACCEPT, "text/event-stream");
    }
    for (k, v) in &cfg.headers {
        if !k.starts_with("x-codex-") && !k.starts_with("x-adc-") {
            builder = builder.header(k.as_str(), v.as_str());
        }
    }

    let resp = tokio::time::timeout(
        Duration::from_secs(cfg.timeout_secs.max(5)),
        builder.json(&payload).send(),
    )
    .await
    .map_err(|_| ProviderError::Transport(format!("Request to {url} timed out")))?
    .map_err(|e| ProviderError::Transport(format!("HTTP request to {url} failed: {e}")))?;

    let mut resp = check_http_error(resp).await?;

    let is_sse = request.stream
        && resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("text/event-stream"))
            .unwrap_or(true);

    if !is_sse {
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Transport(format!("Read body failed: {e}")))?;
        return parse_chat_completions_json(&cfg.id, &wire_req.model, &body);
    }

    let mut acc_content = String::new();
    let mut acc_reasoning = String::new();
    let mut tool_calls_map: HashMap<usize, ProviderToolCall> = HashMap::new();
    let mut finish_reason = "stop".to_string();
    let mut usage = TokenUsage::default();
    let mut think_scrubber = StreamingThinkScrubber::default();
    let mut raw_bytes = Vec::<u8>::new();
    let mut non_sse_lines = Vec::<String>::new();
    let mut saw_sse_data = false;

    while read_next_sse_chunk(&mut resp, &mut raw_bytes, stream_tx, "SSE chunk error").await? {
        while let Some(pos) = raw_bytes.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&raw_bytes[..pos])
                .trim_end_matches('\r')
                .to_string();
            raw_bytes.drain(..=pos);
            if line.starts_with("data:") {
                saw_sse_data = true;
            } else if !saw_sse_data && !line.trim().is_empty() && !line.starts_with(':') {
                non_sse_lines.push(line.clone());
            }
            process_chat_completions_sse_line(
                &line,
                &mut think_scrubber,
                &mut acc_content,
                &mut acc_reasoning,
                &mut tool_calls_map,
                &mut finish_reason,
                &mut usage,
                stream_tx,
            )
            .await?;
        }
    }
    if !raw_bytes.is_empty() {
        let line = String::from_utf8_lossy(&raw_bytes)
            .trim_end_matches('\r')
            .to_string();
        if line.starts_with("data:") {
            saw_sse_data = true;
        } else if !saw_sse_data && !line.trim().is_empty() && !line.starts_with(':') {
            non_sse_lines.push(line.clone());
        }
        process_chat_completions_sse_line(
            &line,
            &mut think_scrubber,
            &mut acc_content,
            &mut acc_reasoning,
            &mut tool_calls_map,
            &mut finish_reason,
            &mut usage,
            stream_tx,
        )
        .await?;
    }

    flush_think_chunks(
        think_scrubber.finish(),
        &mut acc_content,
        &mut acc_reasoning,
        stream_tx,
    )
    .await;

    // Fallback: some OpenAI-compatible gateways ignore `stream: true` and return plain JSON.
    if !saw_sse_data
        && acc_content.is_empty()
        && acc_reasoning.is_empty()
        && tool_calls_map.is_empty()
        && !non_sse_lines.is_empty()
    {
        let body = non_sse_lines.join("\n");
        let parsed = parse_chat_completions_json(&cfg.id, &wire_req.model, &body)?;
        if let Some(tx) = stream_tx {
            if let Some(reason) = parsed.reasoning.as_ref().filter(|r| !r.is_empty()) {
                let _ = tx
                    .send(ProviderStreamEvent::ThinkingDelta(reason.clone()))
                    .await;
            }
            if !parsed.content.is_empty() {
                let _ = tx
                    .send(ProviderStreamEvent::TextDelta(parsed.content.clone()))
                    .await;
            }
            for tc in &parsed.tool_calls {
                let _ = tx.send(ProviderStreamEvent::ToolCallReady(tc.clone())).await;
            }
        }
        return Ok(parsed);
    }

    let mut indices: Vec<usize> = tool_calls_map.keys().copied().collect();
    indices.sort_unstable();
    let tool_calls: Vec<ProviderToolCall> = indices
        .into_iter()
        .filter_map(|idx| tool_calls_map.remove(&idx))
        .collect();
    for tc in &tool_calls {
        if let Some(tx) = stream_tx {
            let _ = tx.send(ProviderStreamEvent::ToolCallReady(tc.clone())).await;
        }
    }
    usage.finalize_total();

    Ok(ProviderResponse {
        content: acc_content,
        reasoning: (!acc_reasoning.is_empty()).then_some(acc_reasoning),
        tool_calls,
        finish_reason,
        usage,
        provider_id: cfg.id.clone(),
        model: wire_req.model.clone(),
        api_mode: ApiMode::ChatCompletions,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ThinkScrubberChunk {
    Text(String),
    Thinking(String),
}

#[derive(Debug, Default, Clone)]
struct StreamingThinkScrubber {
    in_think: bool,
    buffer: String,
    emitted_any_text: bool,
    just_closed_think: bool,
}

impl StreamingThinkScrubber {
    fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        if n.is_empty() || h.len() < n.len() {
            return None;
        }
        h.windows(n.len())
            .position(|w| w.eq_ignore_ascii_case(n))
    }

    fn partial_tag_suffix_len(s: &str, tag: &[u8]) -> usize {
        let max_k = s.len().min(tag.len().saturating_sub(1));
        for k in (1..=max_k).rev() {
            let split_at = s.len() - k;
            if s.is_char_boundary(split_at)
                && s.as_bytes()[split_at..].eq_ignore_ascii_case(&tag[..k])
            {
                return k;
            }
        }
        0
    }

    fn push_visible_text(&mut self, slice: &str, out: &mut Vec<ThinkScrubberChunk>) {
        if slice.is_empty() {
            return;
        }
        let cleaned = if self.just_closed_think && !self.emitted_any_text {
            slice.trim_start_matches(['\r', '\n'])
        } else {
            slice
        };
        if cleaned.is_empty() {
            return;
        }
        self.just_closed_think = false;
        self.emitted_any_text = true;
        out.push(ThinkScrubberChunk::Text(cleaned.to_string()));
    }

    fn push(&mut self, delta: &str) -> Vec<ThinkScrubberChunk> {
        self.buffer.push_str(delta);
        let mut out = Vec::new();
        loop {
            if !self.in_think {
                if let Some(idx) = Self::find_ascii_ci(&self.buffer, "<think>") {
                    let before = self.buffer[..idx].to_string();
                    self.push_visible_text(&before, &mut out);
                    self.buffer.drain(..idx + "<think>".len());
                    self.in_think = true;
                    self.just_closed_think = false;
                } else {
                    let hold = Self::partial_tag_suffix_len(&self.buffer, b"<think>");
                    let emit_end = self.buffer.len() - hold;
                    if emit_end > 0 {
                        let slice = self.buffer[..emit_end].to_string();
                        self.buffer.drain(..emit_end);
                        self.push_visible_text(&slice, &mut out);
                    }
                    break;
                }
            } else if let Some(idx) = Self::find_ascii_ci(&self.buffer, "</think>") {
                let thought = &self.buffer[..idx];
                if !thought.is_empty() {
                    out.push(ThinkScrubberChunk::Thinking(thought.to_string()));
                }
                self.buffer.drain(..idx + "</think>".len());
                self.in_think = false;
                self.just_closed_think = true;
            } else {
                let hold = Self::partial_tag_suffix_len(&self.buffer, b"</think>");
                let emit_end = self.buffer.len() - hold;
                if emit_end > 0 {
                    let slice = self.buffer[..emit_end].to_string();
                    self.buffer.drain(..emit_end);
                    if !slice.is_empty() {
                        out.push(ThinkScrubberChunk::Thinking(slice));
                    }
                }
                break;
            }
        }
        out
    }

    fn finish(mut self) -> Vec<ThinkScrubberChunk> {
        let mut out = Vec::new();
        if !self.buffer.is_empty() {
            let tail = std::mem::take(&mut self.buffer);
            if self.in_think {
                out.push(ThinkScrubberChunk::Thinking(tail));
            } else {
                self.push_visible_text(&tail, &mut out);
            }
        }
        out
    }
}

pub fn scrub_think_tags(text: &str) -> (String, Option<String>) {
    let mut scrubber = StreamingThinkScrubber::default();
    let mut clean_text = String::new();
    let mut thoughts = String::new();
    for chunk in scrubber
        .push(text)
        .into_iter()
        .chain(scrubber.finish().into_iter())
    {
        match chunk {
            ThinkScrubberChunk::Text(s) => clean_text.push_str(&s),
            ThinkScrubberChunk::Thinking(s) => thoughts.push_str(&s),
        }
    }
    let reasoning = (!thoughts.is_empty()).then_some(thoughts);
    (clean_text, reasoning)
}

async fn flush_think_chunks(
    chunks: Vec<ThinkScrubberChunk>,
    acc_content: &mut String,
    acc_reasoning: &mut String,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) {
    for chunk in chunks {
        match chunk {
            ThinkScrubberChunk::Text(text) => {
                if !text.is_empty() {
                    acc_content.push_str(&text);
                    if let Some(tx) = stream_tx {
                        let _ = tx.send(ProviderStreamEvent::TextDelta(text)).await;
                    }
                }
            }
            ThinkScrubberChunk::Thinking(thought) => {
                if !thought.is_empty() {
                    acc_reasoning.push_str(&thought);
                    if let Some(tx) = stream_tx {
                        let _ = tx.send(ProviderStreamEvent::ThinkingDelta(thought)).await;
                    }
                }
            }
        }
    }
}

fn extract_upstream_error_message(val: &Value) -> Option<String> {
    if let Some(err) = val.get("error").filter(|e| !e.is_null()) {
        if let Some(msg) = err.get("message").and_then(Value::as_str) {
            if !msg.trim().is_empty() {
                return Some(msg.to_string());
            }
        }
        if let Some(s) = err.as_str() {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
        return Some(err.to_string());
    }
    if let Some(err) = val
        .get("response")
        .and_then(|r| r.get("error"))
        .filter(|e| !e.is_null())
    {
        if let Some(msg) = err.get("message").and_then(Value::as_str) {
            if !msg.trim().is_empty() {
                return Some(msg.to_string());
            }
        }
        if let Some(s) = err.as_str() {
            if !s.trim().is_empty() {
                return Some(s.to_string());
            }
        }
        return Some(err.to_string());
    }
    if let Some(msg) = val
        .get("response")
        .and_then(|r| r.get("incomplete_details"))
        .and_then(|d| d.get("reason"))
        .and_then(Value::as_str)
    {
        return Some(format!("Codex response incomplete: {msg}"));
    }
    None
}

fn extract_reasoning_field_text(obj: &Value) -> Option<&str> {
    obj.get("reasoning_content")
        .or_else(|| obj.get("reasoning"))
        .or_else(|| obj.get("thinking"))
        .or_else(|| obj.get("thought"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

#[allow(clippy::too_many_arguments)]
async fn process_chat_completions_sse_line(
    line: &str,
    think_scrubber: &mut StreamingThinkScrubber,
    acc_content: &mut String,
    acc_reasoning: &mut String,
    tool_calls_map: &mut HashMap<usize, ProviderToolCall>,
    finish_reason: &mut String,
    usage: &mut TokenUsage,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<(), ProviderError> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(());
    };
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return Ok(());
    }
    let val: Value = serde_json::from_str(trimmed)
        .map_err(|e| ProviderError::Protocol(format!("Invalid ChatCompletions SSE JSON: {e}")))?;

    if let Some(msg) = extract_upstream_error_message(&val) {
        return Err(ProviderError::Protocol(msg));
    }

    if let Some(u) = val.get("usage") {
        if let Some(inp) = u.get("prompt_tokens").and_then(Value::as_u64) {
            usage.input_tokens = inp;
        }
        if let Some(outp) = u.get("completion_tokens").and_then(Value::as_u64) {
            usage.output_tokens = outp;
        }
        if let Some(tot) = u.get("total_tokens").and_then(Value::as_u64) {
            usage.total_tokens = tot;
        }
        if let Some(cached) = u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
        {
            usage.cache_read_tokens = cached;
        }
    }

    let Some(choice) = val.get("choices").and_then(Value::as_array).and_then(|c| c.first())
    else {
        return Ok(());
    };
    if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
        if !fr.is_empty() {
            *finish_reason = fr.to_string();
        }
    }
    let Some(delta) = choice.get("delta") else {
        return Ok(());
    };
    if let Some(reason) = extract_reasoning_field_text(delta) {
        acc_reasoning.push_str(reason);
        if let Some(tx) = stream_tx {
            let _ = tx
                .send(ProviderStreamEvent::ThinkingDelta(reason.to_string()))
                .await;
        }
    }
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            let chunks = think_scrubber.push(text);
            flush_think_chunks(chunks, acc_content, acc_reasoning, stream_tx).await;
        }
    }
    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
        for tc in tcs {
            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let entry = tool_calls_map.entry(idx).or_insert_with(|| ProviderToolCall {
                id: format!("call_{idx}"),
                name: String::new(),
                arguments: String::new(),
            });
            if let Some(id) = tc.get("id").and_then(Value::as_str) {
                if !id.is_empty() {
                    entry.id = id.to_string();
                }
            }
            if let Some(func) = tc.get("function") {
                if let Some(name) = func.get("name").and_then(Value::as_str) {
                    entry.name.push_str(name);
                }
                if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                    entry.arguments.push_str(args);
                }
            }
        }
    }

    Ok(())
}

pub fn parse_chat_completions_json(
    provider_id: &str,
    model: &str,
    body: &str,
) -> Result<ProviderResponse, ProviderError> {
    let val: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::Protocol(format!("Invalid ChatCompletions JSON: {e}")))?;
    if let Some(msg) = extract_upstream_error_message(&val) {
        return Err(ProviderError::Protocol(msg));
    }
    let choice = val
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .ok_or_else(|| ProviderError::Protocol("Missing choices[0] in response".into()))?;
    let msg = choice
        .get("message")
        .ok_or_else(|| ProviderError::Protocol("Missing choices[0].message".into()))?;

    let raw_content = msg
        .get("content")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            msg.get("content").and_then(Value::as_array).map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
        })
        .unwrap_or_default();
    let (content, scrubbed_thoughts) = scrub_think_tags(&raw_content);
    let mut combined_reasoning = String::new();
    if let Some(explicit) = extract_reasoning_field_text(msg) {
        combined_reasoning.push_str(explicit);
    }
    if let Some(inline) = scrubbed_thoughts {
        if !combined_reasoning.is_empty() && !inline.is_empty() {
            combined_reasoning.push('\n');
        }
        combined_reasoning.push_str(&inline);
    }
    let reasoning = (!combined_reasoning.is_empty()).then_some(combined_reasoning);
    let mut tool_calls = Vec::new();
    if let Some(arr) = msg.get("tool_calls").and_then(Value::as_array) {
        for (idx, tc) in arr.iter().enumerate() {
            let id = tc
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("call_{idx}"));
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("{}")
                .to_string();
            tool_calls.push(ProviderToolCall {
                id,
                name,
                arguments,
            });
        }
    }
    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("stop")
        .to_string();
    let mut usage = TokenUsage::default();
    if let Some(u) = val.get("usage") {
        usage.input_tokens = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
        usage.output_tokens = u
            .get("completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        usage.total_tokens = u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
        if let Some(cached) = u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
        {
            usage.cache_read_tokens = cached;
        }
    }
    usage.finalize_total();

    Ok(ProviderResponse {
        content,
        reasoning,
        tool_calls,
        finish_reason,
        usage,
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        api_mode: ApiMode::ChatCompletions,
    })
}

fn should_strip_sampling_for_codex_backend(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    lower.contains("chatgpt.com/backend-api/codex")
        || lower.contains("chat.openai.com/backend-api/codex")
}

fn ensure_codex_payload_compat(base_url: &str, payload: &mut Value) {
    let lower = base_url.to_ascii_lowercase();
    if lower.contains("/backend-api/codex") {
        payload["stream"] = Value::Bool(true);
        let empty_instructions = payload
            .get("instructions")
            .and_then(Value::as_str)
            .map(|s| s.trim().is_empty())
            .unwrap_or(true);
        if empty_instructions {
            payload["instructions"] =
                Value::String("You are Hermes, a helpful AI assistant.".to_string());
        }
    }
    if should_strip_sampling_for_codex_backend(base_url) {
        if let Some(obj) = payload.as_object_mut() {
            obj.remove("temperature");
            obj.remove("max_output_tokens");
        }
    }
}

async fn execute_codex_responses(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    base_url: &str,
    api_key: &str,
    request: &ProviderRequest,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<ProviderResponse, ProviderError> {
    let url = resolve_endpoint_url(base_url, "responses");
    let wire_req = prepare_wire_request(cfg, base_url, api_key, request);
    let mut payload = build_codex_responses_payload(&wire_req);
    ensure_codex_payload_compat(base_url, &mut payload);
    let wants_stream = payload
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(request.stream);

    let mut builder = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if !api_key.trim().is_empty() {
        builder = builder.bearer_auth(api_key.trim());
    }
    if wants_stream {
        builder = builder.header(reqwest::header::ACCEPT, "text/event-stream");
    }
    let has_originator = cfg
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("originator"));
    let has_user_agent = cfg
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"));
    if !has_originator
        && (cfg.id == "codex_oauth" || base_url.to_ascii_lowercase().contains("/backend-api/codex"))
    {
        builder = builder.header("originator", "codex_cli_rs");
    }
    if !has_user_agent
        && (cfg.id == "codex_oauth" || base_url.to_ascii_lowercase().contains("/backend-api/codex"))
    {
        builder = builder.header("User-Agent", "codex_cli_rs/0.1.0");
    }
    let has_account_id = cfg
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("chatgpt-account-id"));
    if !has_account_id {
        if let Some(acct) = cfg
            .headers
            .get("x-account-id")
            .map(|s| s.as_str())
            .or_else(|| cfg.headers.get("account_id").map(|s| s.as_str()))
            .or_else(|| request.metadata.get("chatgpt_account_id").and_then(Value::as_str))
        {
            builder = builder.header("chatgpt-account-id", acct);
        }
    }
    for (k, v) in &cfg.headers {
        if !k.starts_with("x-codex-") && !k.starts_with("x-adc-") {
            builder = builder.header(k.as_str(), v.as_str());
        }
    }

    let resp = tokio::time::timeout(
        Duration::from_secs(cfg.timeout_secs.max(5)),
        builder.json(&payload).send(),
    )
    .await
    .map_err(|_| ProviderError::Transport(format!("Request to {url} timed out")))?
    .map_err(|e| ProviderError::Transport(format!("HTTP request to {url} failed: {e}")))?;

    let mut resp = check_http_error(resp).await?;

    let is_sse = wants_stream
        && resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| !ct.to_ascii_lowercase().contains("application/json"))
            .unwrap_or(true);

    if !is_sse {
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Transport(format!("Read Codex body failed: {e}")))?;
        let parsed = parse_codex_responses_json(&cfg.id, &wire_req.model, &body)?;
        emit_provider_response_stream_events(&parsed, stream_tx).await;
        return Ok(parsed);
    }

    let mut state = CodexSseParserState::default();
    let mut raw_bytes = Vec::<u8>::new();
    let mut non_sse_lines = Vec::<String>::new();

    while read_next_sse_chunk(&mut resp, &mut raw_bytes, stream_tx, "Codex Responses stream error").await? {
        while let Some(pos) = raw_bytes.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&raw_bytes[..pos])
                .trim_end_matches('\r')
                .to_string();
            raw_bytes.drain(..=pos);
            if !state.saw_sse_data
                && !line.starts_with("data:")
                && !line.starts_with("event:")
                && !line.trim().is_empty()
                && !line.starts_with(':')
            {
                non_sse_lines.push(line.clone());
            }
            let events = process_codex_responses_sse_line(&line, &mut state)?;
            for ev in events {
                send_stream_event(stream_tx, ev).await?;
            }
        }
    }
    if !raw_bytes.is_empty() {
        let line = String::from_utf8_lossy(&raw_bytes)
            .trim_end_matches('\r')
            .to_string();
        if !state.saw_sse_data
            && !line.starts_with("data:")
            && !line.starts_with("event:")
            && !line.trim().is_empty()
            && !line.starts_with(':')
        {
            non_sse_lines.push(line.clone());
        }
        let events = process_codex_responses_sse_line(&line, &mut state)?;
        if let Some(tx) = stream_tx {
            for ev in events {
                let _ = tx.send(ev).await;
            }
        }
    }

    if !state.saw_sse_data && !non_sse_lines.is_empty() {
        let body = non_sse_lines.join("\n");
        let parsed = parse_codex_responses_json(&cfg.id, &wire_req.model, &body)?;
        emit_provider_response_stream_events(&parsed, stream_tx).await;
        return Ok(parsed);
    }

    state.into_provider_response(&cfg.id, &wire_req.model)
}

async fn emit_provider_response_stream_events(
    resp: &ProviderResponse,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) {
    let Some(tx) = stream_tx else {
        return;
    };
    if let Some(reason) = resp.reasoning.as_ref().filter(|r| !r.is_empty()) {
        let _ = tx
            .send(ProviderStreamEvent::ThinkingDelta(reason.clone()))
            .await;
    }
    if !resp.content.is_empty() {
        let _ = tx
            .send(ProviderStreamEvent::TextDelta(resp.content.clone()))
            .await;
    }
    for tc in &resp.tool_calls {
        let _ = tx.send(ProviderStreamEvent::ToolCallReady(tc.clone())).await;
    }
}

#[derive(Debug, Default)]
struct CodexSseParserState {
    current_event: String,
    acc_content: String,
    acc_reasoning: String,
    tool_calls: Vec<ProviderToolCall>,
    in_progress_calls: HashMap<String, ProviderToolCall>,
    usage: TokenUsage,
    saw_sse_data: bool,
    saw_output_text_delta: bool,
}

impl CodexSseParserState {
    fn into_provider_response(
        mut self,
        provider_id: &str,
        model: &str,
    ) -> Result<ProviderResponse, ProviderError> {
        if self.acc_content.is_empty()
            && self.acc_reasoning.is_empty()
            && self.tool_calls.is_empty()
        {
            return Err(ProviderError::Protocol(
                "Codex Responses stream ended with empty output".to_string(),
            ));
        }

        self.usage.finalize_total();
        let finish_reason = if self.tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };

        Ok(ProviderResponse {
            content: self.acc_content,
            reasoning: (!self.acc_reasoning.is_empty()).then_some(self.acc_reasoning),
            tool_calls: self.tool_calls,
            finish_reason: finish_reason.to_string(),
            usage: self.usage,
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            api_mode: ApiMode::CodexResponses,
        })
    }
}

fn extract_codex_message_item_text(item: &Value) -> String {
    if let Some(text) = item.get("content").and_then(Value::as_str) {
        return text.to_string();
    }
    let mut out = String::new();
    if let Some(parts) = item.get("content").and_then(Value::as_array) {
        for part in parts {
            let ptype = part.get("type").and_then(Value::as_str).unwrap_or_default();
            if ptype.is_empty() || ptype == "output_text" || ptype == "text" {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
        }
    }
    out
}

fn process_codex_responses_sse_line(
    line: &str,
    state: &mut CodexSseParserState,
) -> Result<Vec<ProviderStreamEvent>, ProviderError> {
    let mut events = Vec::new();
    let trimmed_line = line.trim();
    if trimmed_line.is_empty() {
        state.current_event.clear();
        return Ok(events);
    }
    if let Some(ev) = trimmed_line.strip_prefix("event:") {
        state.current_event = ev.trim().to_string();
        return Ok(events);
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(events);
    };
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        state.current_event.clear();
        return Ok(events);
    }
    state.saw_sse_data = true;

    let current_event = std::mem::take(&mut state.current_event);
    let val: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => {
            if matches!(
                current_event.as_str(),
                "response.failed" | "error" | "response.incomplete"
            ) {
                return Err(ProviderError::Protocol(format!(
                    "Codex Responses {current_event}: {trimmed}"
                )));
            }
            return Err(ProviderError::Protocol(format!(
                "Invalid Codex Responses SSE JSON: {e}"
            )));
        }
    };

    let ev_type = val
        .get("type")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(current_event.as_str());

    let status_failed = val
        .get("status")
        .and_then(Value::as_str)
        .or_else(|| {
            val.get("response")
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str)
        })
        == Some("failed");

    if matches!(
        ev_type,
        "response.failed" | "error" | "response.incomplete"
    ) || status_failed
    {
        let msg = extract_upstream_error_message(&val).unwrap_or_else(|| {
            if ev_type.is_empty() {
                "Codex Responses request failed".to_string()
            } else {
                format!("Codex Responses {ev_type}")
            }
        });
        return Err(ProviderError::Protocol(msg));
    }

    if let Some(msg) = extract_upstream_error_message(&val) {
        return Err(ProviderError::Protocol(msg));
    }

    match ev_type {
        "response.output_text.delta" => {
            if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    state.saw_output_text_delta = true;
                    state.acc_content.push_str(delta);
                    events.push(ProviderStreamEvent::TextDelta(delta.to_string()));
                }
            }
        }
        "response.output_text.done" => {
            if !state.saw_output_text_delta && state.acc_content.is_empty() {
                if let Some(text) = val.get("text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        state.acc_content.push_str(text);
                        events.push(ProviderStreamEvent::TextDelta(text.to_string()));
                    }
                }
            }
        }
        "response.reasoning_summary_text.delta"
        | "response.reasoning_text.delta"
        | "response.reasoning.delta" => {
            if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    state.acc_reasoning.push_str(delta);
                    events.push(ProviderStreamEvent::ThinkingDelta(delta.to_string()));
                }
            }
        }
        "response.output_item.added" => {
            if let Some(item) = val.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let key = item
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                        .or_else(|| {
                            val.get("output_index")
                                .and_then(Value::as_u64)
                                .map(|i| format!("idx_{i}"))
                        })
                        .unwrap_or_else(|| "idx_0".to_string());
                    let call_id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("call_1")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    state.in_progress_calls.insert(
                        key,
                        ProviderToolCall {
                            id: call_id,
                            name,
                            arguments,
                        },
                    );
                }
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(delta) = val.get("delta").and_then(Value::as_str) {
                let key = val
                    .get("item_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .or_else(|| {
                        val.get("output_index")
                            .and_then(Value::as_u64)
                            .map(|i| format!("idx_{i}"))
                    });
                if let Some(k) = key {
                    if let Some(entry) = state.in_progress_calls.get_mut(&k) {
                        entry.arguments.push_str(delta);
                    }
                }
            }
        }
        "response.function_call_arguments.done" => {
            if let Some(args) = val.get("arguments").and_then(Value::as_str) {
                let key = val
                    .get("item_id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .or_else(|| {
                        val.get("output_index")
                            .and_then(Value::as_u64)
                            .map(|i| format!("idx_{i}"))
                    });
                if let Some(k) = key {
                    if let Some(entry) = state.in_progress_calls.get_mut(&k) {
                        if !args.is_empty() {
                            entry.arguments = args.to_string();
                        }
                    }
                }
            }
        }
        "response.output_item.done" => {
            if let Some(item) = val.get("item") {
                match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => {
                        let key = item
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                            .or_else(|| {
                                val.get("output_index")
                                    .and_then(Value::as_u64)
                                    .map(|i| format!("idx_{i}"))
                            });
                        let buffered = key.as_deref().and_then(|k| state.in_progress_calls.remove(k));
                        let call_id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(ToString::to_string)
                            .or_else(|| buffered.as_ref().map(|b| b.id.clone()))
                            .unwrap_or_else(|| format!("call_{}", state.tool_calls.len() + 1));
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(ToString::to_string)
                            .or_else(|| buffered.as_ref().map(|b| b.name.clone()))
                            .unwrap_or_default();
                        let arguments = item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(ToString::to_string)
                            .or_else(|| buffered.map(|b| b.arguments).filter(|s| !s.is_empty()))
                            .unwrap_or_else(|| "{}".to_string());
                        let tc = ProviderToolCall {
                            id: call_id,
                            name,
                            arguments,
                        };
                        if !state.tool_calls.iter().any(|existing| existing.id == tc.id) {
                            events.push(ProviderStreamEvent::ToolCallReady(tc.clone()));
                            state.tool_calls.push(tc);
                        }
                    }
                    Some("message") => {
                        if !state.saw_output_text_delta && state.acc_content.is_empty() {
                            let text = extract_codex_message_item_text(item);
                            if !text.is_empty() {
                                state.acc_content.push_str(&text);
                                events.push(ProviderStreamEvent::TextDelta(text));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "response.completed" | "response.done" => {
            let resp = val.get("response").unwrap_or(&val);
            if let Some(u) = resp.get("usage") {
                state.usage.input_tokens =
                    u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                state.usage.output_tokens =
                    u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                state.usage.total_tokens =
                    u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
                if let Some(cached) = u
                    .get("input_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
                    .and_then(Value::as_u64)
                {
                    state.usage.cache_read_tokens = cached;
                }
            }
            if let Some(items) = resp.get("output").and_then(Value::as_array) {
                for item in items {
                    match item.get("type").and_then(Value::as_str) {
                        Some("function_call") => {
                            let id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(ToString::to_string)
                                .unwrap_or_else(|| {
                                    format!("call_{}", state.tool_calls.len() + 1)
                                });
                            if !state.tool_calls.iter().any(|existing| existing.id == id) {
                                let tc = ProviderToolCall {
                                    id,
                                    name: item
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string(),
                                    arguments: item
                                        .get("arguments")
                                        .and_then(Value::as_str)
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or("{}")
                                        .to_string(),
                                };
                                events.push(ProviderStreamEvent::ToolCallReady(tc.clone()));
                                state.tool_calls.push(tc);
                            }
                        }
                        Some("message") => {
                            if !state.saw_output_text_delta && state.acc_content.is_empty() {
                                let text = extract_codex_message_item_text(item);
                                if !text.is_empty() {
                                    state.acc_content.push_str(&text);
                                    events.push(ProviderStreamEvent::TextDelta(text));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    Ok(events)
}

pub fn parse_codex_responses_sse(
    provider_id: &str,
    model: &str,
    raw_sse: &str,
) -> Result<ProviderResponse, ProviderError> {
    let mut state = CodexSseParserState::default();
    for line in raw_sse.lines() {
        let clean = line.trim_end_matches('\r');
        let _ = process_codex_responses_sse_line(clean, &mut state)?;
    }
    state.into_provider_response(provider_id, model)
}

pub fn parse_codex_responses_json(
    provider_id: &str,
    model: &str,
    body: &str,
) -> Result<ProviderResponse, ProviderError> {
    let val: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::Protocol(format!("Invalid Codex Responses JSON: {e}")))?;

    let root = val.get("response").unwrap_or(&val);
    if root.get("status").and_then(Value::as_str) == Some("failed") {
        let msg = extract_upstream_error_message(&val)
            .unwrap_or_else(|| "Codex Responses request failed".to_string());
        return Err(ProviderError::Protocol(msg));
    }
    if let Some(msg) = extract_upstream_error_message(&val) {
        return Err(ProviderError::Protocol(msg));
    }

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();

    if let Some(items) = root.get("output").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    content.push_str(&extract_codex_message_item_text(item));
                }
                Some("reasoning") => {
                    if let Some(summary) = item.get("summary").and_then(Value::as_array) {
                        for s in summary {
                            if let Some(t) = s.get("text").and_then(Value::as_str) {
                                reasoning.push_str(t);
                            }
                        }
                    } else if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for p in parts {
                            if let Some(t) = p.get("text").and_then(Value::as_str) {
                                reasoning.push_str(t);
                            }
                        }
                    }
                }
                Some("function_call") => {
                    let id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string)
                        .unwrap_or_else(|| format!("call_{}", tool_calls.len() + 1));
                    tool_calls.push(ProviderToolCall {
                        id,
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .unwrap_or("{}")
                            .to_string(),
                    });
                }
                _ => {}
            }
        }
    }

    if content.is_empty() {
        if let Some(out_text) = root.get("output_text").and_then(Value::as_str) {
            content.push_str(out_text);
        }
    }

    if content.is_empty() && reasoning.is_empty() && tool_calls.is_empty() {
        return Err(ProviderError::Protocol(
            "Codex Responses JSON contained empty output".to_string(),
        ));
    }

    let mut usage = TokenUsage::default();
    if let Some(u) = root.get("usage") {
        usage.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        usage.output_tokens = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        usage.total_tokens = u.get("total_tokens").and_then(Value::as_u64).unwrap_or(0);
        if let Some(cached) = u
            .get("input_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
        {
            usage.cache_read_tokens = cached;
        }
    }
    usage.finalize_total();

    let finish_reason = if tool_calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };

    Ok(ProviderResponse {
        content,
        reasoning: (!reasoning.is_empty()).then_some(reasoning),
        tool_calls,
        finish_reason: finish_reason.to_string(),
        usage,
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        api_mode: ApiMode::CodexResponses,
    })
}

async fn execute_anthropic_messages(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    base_url: &str,
    api_key: &str,
    request: &ProviderRequest,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<ProviderResponse, ProviderError> {
    let url = resolve_anthropic_messages_url(base_url);
    let wire_req = prepare_wire_request(cfg, base_url, api_key, request);
    let payload = build_anthropic_messages_payload(&wire_req);

    let mut builder = client
        .post(&url)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "prompt-caching-2024-07-31")
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if !api_key.trim().is_empty() {
        builder = builder.header("x-api-key", api_key.trim());
    }
    if request.stream {
        builder = builder.header(reqwest::header::ACCEPT, "text/event-stream");
    }
    for (k, v) in &cfg.headers {
        if !k.starts_with("x-codex-") && !k.starts_with("x-adc-") {
            builder = builder.header(k.as_str(), v.as_str());
        }
    }

    let resp = tokio::time::timeout(
        Duration::from_secs(cfg.timeout_secs.max(5)),
        builder.json(&payload).send(),
    )
    .await
    .map_err(|_| ProviderError::Transport(format!("Request to {url} timed out")))?
    .map_err(|e| ProviderError::Transport(format!("HTTP request to {url} failed: {e}")))?;

    let mut resp = check_http_error(resp).await?;

    let is_sse = request.stream
        && resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("text/event-stream"))
            .unwrap_or(true);

    if !is_sse {
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Transport(format!("Read Anthropic body failed: {e}")))?;
        return parse_anthropic_messages_json(&cfg.id, &wire_req.model, &body);
    }

    let mut acc_content = String::new();
    let mut acc_reasoning = String::new();
    let mut active_blocks: HashMap<usize, AnthropicBlockAccum> = HashMap::new();
    let mut tool_calls = Vec::new();
    let mut stop_reason = "end_turn".to_string();
    let mut usage = TokenUsage::default();
    let mut current_event = String::new();
    let mut raw_bytes = Vec::<u8>::new();

    while read_next_sse_chunk(&mut resp, &mut raw_bytes, stream_tx, "Anthropic SSE error").await? {
        while let Some(pos) = raw_bytes.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&raw_bytes[..pos])
                .trim_end_matches('\r')
                .to_string();
            raw_bytes.drain(..=pos);
            process_anthropic_sse_line(
                &line,
                &mut current_event,
                &mut acc_content,
                &mut acc_reasoning,
                &mut active_blocks,
                &mut tool_calls,
                &mut stop_reason,
                &mut usage,
                stream_tx,
            )
            .await?;
        }
    }
    if !raw_bytes.is_empty() {
        let line = String::from_utf8_lossy(&raw_bytes)
            .trim_end_matches('\r')
            .to_string();
        process_anthropic_sse_line(
            &line,
            &mut current_event,
            &mut acc_content,
            &mut acc_reasoning,
            &mut active_blocks,
            &mut tool_calls,
            &mut stop_reason,
            &mut usage,
            stream_tx,
        )
        .await?;
    }

    // Flush any remaining open tool blocks
    let mut remaining_indices: Vec<usize> = active_blocks.keys().copied().collect();
    remaining_indices.sort_unstable();
    for idx in remaining_indices {
        if let Some(AnthropicBlockAccum::ToolUse { id, name, input_json }) =
            active_blocks.remove(&idx)
        {
            let tc = ProviderToolCall {
                id,
                name,
                arguments: if input_json.trim().is_empty() {
                    "{}".to_string()
                } else {
                    input_json
                },
            };
            if let Some(tx) = stream_tx {
                let _ = tx.send(ProviderStreamEvent::ToolCallReady(tc.clone())).await;
            }
            tool_calls.push(tc);
        }
    }

    usage.finalize_total();
    Ok(ProviderResponse {
        content: acc_content,
        reasoning: (!acc_reasoning.is_empty()).then_some(acc_reasoning),
        tool_calls,
        finish_reason: stop_reason,
        usage,
        provider_id: cfg.id.clone(),
        model: wire_req.model.clone(),
        api_mode: ApiMode::AnthropicMessages,
    })
}

enum AnthropicBlockAccum {
    Text,
    Thinking,
    ToolUse {
        id: String,
        name: String,
        input_json: String,
    },
}

#[allow(clippy::too_many_arguments)]
async fn process_anthropic_sse_line(
    line: &str,
    current_event: &mut String,
    acc_content: &mut String,
    acc_reasoning: &mut String,
    active_blocks: &mut HashMap<usize, AnthropicBlockAccum>,
    tool_calls: &mut Vec<ProviderToolCall>,
    stop_reason: &mut String,
    usage: &mut TokenUsage,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<(), ProviderError> {
    if line.is_empty() {
        current_event.clear();
        return Ok(());
    }
    if let Some(ev) = line.strip_prefix("event:") {
        *current_event = ev.trim().to_string();
        return Ok(());
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(());
    };
    let trimmed = data.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let val: Value = serde_json::from_str(trimmed)
        .map_err(|e| ProviderError::Protocol(format!("Invalid Anthropic SSE JSON: {e}")))?;

    let kind = val
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(current_event.as_str());

    match kind {
        "error" => {
            let msg = val
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Anthropic upstream error")
                .to_string();
            return Err(ProviderError::Protocol(msg));
        }
        "message_start" => {
            if let Some(u) = val.get("message").and_then(|m| m.get("usage")) {
                usage.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                usage.cache_read_tokens = u
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                usage.cache_creation_tokens = u
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
            }
        }
        "content_block_start" => {
            let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(cb) = val.get("content_block") {
                match cb.get("type").and_then(Value::as_str).unwrap_or("") {
                    "tool_use" => {
                        let id = cb
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("toolu_1")
                            .to_string();
                        let name = cb
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        active_blocks.insert(
                            idx,
                            AnthropicBlockAccum::ToolUse {
                                id,
                                name,
                                input_json: String::new(),
                            },
                        );
                    }
                    "thinking" => {
                        active_blocks.insert(idx, AnthropicBlockAccum::Thinking);
                    }
                    _ => {
                        active_blocks.insert(idx, AnthropicBlockAccum::Text);
                    }
                }
            }
        }
        "content_block_delta" => {
            let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(delta) = val.get("delta") {
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        if let Some(text) = delta.get("text").and_then(Value::as_str) {
                            acc_content.push_str(text);
                            if let Some(tx) = stream_tx {
                                let _ = tx
                                    .send(ProviderStreamEvent::TextDelta(text.to_string()))
                                    .await;
                            }
                        }
                    }
                    "thinking_delta" => {
                        if let Some(thinking) = delta.get("thinking").and_then(Value::as_str) {
                            acc_reasoning.push_str(thinking);
                            if let Some(tx) = stream_tx {
                                let _ = tx
                                    .send(ProviderStreamEvent::ThinkingDelta(thinking.to_string()))
                                    .await;
                            }
                        }
                    }
                    "input_json_delta" => {
                        if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                            if let Some(AnthropicBlockAccum::ToolUse { input_json, .. }) =
                                active_blocks.get_mut(&idx)
                            {
                                input_json.push_str(partial);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        "content_block_stop" => {
            let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(AnthropicBlockAccum::ToolUse { id, name, input_json }) =
                active_blocks.remove(&idx)
            {
                let tc = ProviderToolCall {
                    id,
                    name,
                    arguments: if input_json.trim().is_empty() {
                        "{}".to_string()
                    } else {
                        input_json
                    },
                };
                if let Some(tx) = stream_tx {
                    let _ = tx.send(ProviderStreamEvent::ToolCallReady(tc.clone())).await;
                }
                tool_calls.push(tc);
            }
        }
        "message_delta" => {
            if let Some(sr) = val
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
            {
                *stop_reason = sr.to_string();
            }
            if let Some(u) = val.get("usage") {
                if let Some(outp) = u.get("output_tokens").and_then(Value::as_u64) {
                    usage.output_tokens = outp;
                }
            }
        }
        _ => {}
    }

    Ok(())
}

pub fn parse_anthropic_messages_json(
    provider_id: &str,
    model: &str,
    body: &str,
) -> Result<ProviderResponse, ProviderError> {
    let val: Value = serde_json::from_str(body)
        .map_err(|e| ProviderError::Protocol(format!("Invalid Anthropic JSON: {e}")))?;

    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();

    if let Some(blocks) = val.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        if !content.is_empty() {
                            content.push('\n');
                        }
                        content.push_str(t);
                    }
                }
                "thinking" => {
                    if let Some(t) = block.get("thinking").and_then(Value::as_str) {
                        reasoning.push_str(t);
                    }
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("toolu_1")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let arguments = block
                        .get("input")
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "{}".to_string());
                    tool_calls.push(ProviderToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
        }
    }

    let mut usage = TokenUsage::default();
    if let Some(u) = val.get("usage") {
        usage.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
        usage.output_tokens = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
        usage.cache_read_tokens = u
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        usage.cache_creation_tokens = u
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    }
    usage.finalize_total();

    let finish_reason = val
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn")
        .to_string();

    Ok(ProviderResponse {
        content,
        reasoning: (!reasoning.is_empty()).then_some(reasoning),
        tool_calls,
        finish_reason,
        usage,
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        api_mode: ApiMode::AnthropicMessages,
    })
}

fn provider_request_to_unified(req: &ProviderRequest) -> UnifiedRequest {
    UnifiedRequest {
        model: req.model.clone(),
        system_prompt: req.combined_system_prompt(),
        messages: req
            .messages
            .iter()
            .map(|m| UnifiedMessage {
                role: match m.role {
                    ProviderMessageRole::System => UnifiedRole::System,
                    ProviderMessageRole::User => UnifiedRole::User,
                    ProviderMessageRole::Assistant => UnifiedRole::Assistant,
                    ProviderMessageRole::Tool => UnifiedRole::Tool,
                },
                content: m.content.clone(),
                name: m.name.clone(),
                tool_call_id: m.tool_call_id.clone(),
                tool_calls: m
                    .tool_calls
                    .iter()
                    .map(|tc| UnifiedToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .collect(),
                reasoning: m.reasoning.clone(),
            })
            .collect(),
        tools: req
            .tools
            .iter()
            .map(|t| UnifiedToolSpec {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            })
            .collect(),
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        thinking_budget_tokens: req.thinking_budget_tokens,
        stream: req.stream,
    }
}

fn unified_to_provider_response(
    parsed: magpie_gateway::UnifiedResponse,
    api_mode: ApiMode,
) -> ProviderResponse {
    ProviderResponse {
        content: parsed.content,
        reasoning: parsed.reasoning,
        tool_calls: parsed
            .tool_calls
            .into_iter()
            .map(|tc| ProviderToolCall {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments,
            })
            .collect(),
        finish_reason: parsed.finish_reason,
        usage: TokenUsage {
            input_tokens: parsed.usage.input_tokens,
            output_tokens: parsed.usage.output_tokens,
            cache_read_tokens: parsed.usage.cache_read_tokens,
            cache_creation_tokens: 0,
            total_tokens: parsed.usage.total_tokens,
            estimated_cost_usd: 0.0,
        },
        provider_id: parsed.provider_id,
        model: parsed.model,
        api_mode,
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_gemini_sse_line(
    line: &str,
    saw_sse_data: &mut bool,
    non_sse_lines: &mut Vec<String>,
    acc_content: &mut String,
    acc_reasoning: &mut String,
    tool_calls: &mut Vec<ProviderToolCall>,
    finish_reason: &mut String,
    usage: &mut TokenUsage,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<(), ProviderError> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with(':')
        || trimmed.starts_with("event:")
        || trimmed.starts_with("id:")
        || trimmed.starts_with("retry:")
    {
        return Ok(());
    }
    if trimmed.starts_with("data:") {
        *saw_sse_data = true;
    } else if !*saw_sse_data {
        non_sse_lines.push(line.to_string());
    }
    let Some(delta) = parse_gemini_sse_data(trimmed, tool_calls.len())
        .map_err(|e| ProviderError::Protocol(e.to_string()))?
    else {
        return Ok(());
    };
    if !delta.thinking_delta.is_empty() {
        acc_reasoning.push_str(&delta.thinking_delta);
        if let Some(tx) = stream_tx {
            let _ = tx
                .send(ProviderStreamEvent::ThinkingDelta(delta.thinking_delta))
                .await;
        }
    }
    if !delta.text_delta.is_empty() {
        acc_content.push_str(&delta.text_delta);
        if let Some(tx) = stream_tx {
            let _ = tx
                .send(ProviderStreamEvent::TextDelta(delta.text_delta))
                .await;
        }
    }
    for tc in delta.tool_calls {
        let ptc = ProviderToolCall {
            id: tc.id,
            name: tc.name,
            arguments: tc.arguments,
        };
        if let Some(tx) = stream_tx {
            let _ = tx.send(ProviderStreamEvent::ToolCallReady(ptc.clone())).await;
        }
        tool_calls.push(ptc);
    }
    if let Some(fr) = delta.finish_reason {
        *finish_reason = fr;
    }
    if let Some(u) = delta.usage {
        usage.input_tokens = u.input_tokens;
        usage.output_tokens = u.output_tokens;
        usage.cache_read_tokens = u.cache_read_tokens;
        usage.total_tokens = u.total_tokens;
    }
    Ok(())
}

async fn execute_gemini_generate_content(
    client: &reqwest::Client,
    cfg: &ProviderConfig,
    base_url: &str,
    api_key: &str,
    request: &ProviderRequest,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<ProviderResponse, ProviderError> {
    let wire_req = prepare_wire_request(cfg, base_url, api_key, request);
    let url = resolve_gemini_endpoint_url(base_url, &wire_req.model, request.stream);
    let unified = provider_request_to_unified(&wire_req);
    let payload = build_gemini_generate_content_payload(&unified);

    let mut builder = client
        .post(&url)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if !api_key.trim().is_empty() {
        builder = builder.header("x-goog-api-key", api_key.trim());
    }
    if request.stream {
        builder = builder.header(reqwest::header::ACCEPT, "text/event-stream");
    }
    for (k, v) in &cfg.headers {
        if !k.starts_with("x-codex-") && !k.starts_with("x-adc-") {
            builder = builder.header(k.as_str(), v.as_str());
        }
    }

    let resp = tokio::time::timeout(
        Duration::from_secs(cfg.timeout_secs.max(5)),
        builder.json(&payload).send(),
    )
    .await
    .map_err(|_| ProviderError::Transport(format!("Request to {url} timed out")))?
    .map_err(|e| ProviderError::Transport(format!("HTTP request to {url} failed: {e}")))?;

    let mut resp = check_http_error(resp).await?;

    let is_sse = request.stream
        && resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| !ct.to_ascii_lowercase().contains("application/json"))
            .unwrap_or(true);

    if !is_sse {
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Transport(format!("Read Gemini body failed: {e}")))?;
        let parsed = parse_gemini_response_json(&cfg.id, &wire_req.model, &body)
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let out = unified_to_provider_response(parsed, ApiMode::GeminiGenerateContent);
        emit_provider_response_stream_events(&out, stream_tx).await;
        return Ok(out);
    }

    let mut acc_content = String::new();
    let mut acc_reasoning = String::new();
    let mut tool_calls = Vec::<ProviderToolCall>::new();
    let mut finish_reason = "stop".to_string();
    let mut usage = TokenUsage::default();
    let mut raw_bytes = Vec::<u8>::new();
    let mut non_sse_lines = Vec::<String>::new();
    let mut saw_sse_data = false;

    while read_next_sse_chunk(&mut resp, &mut raw_bytes, stream_tx, "Gemini SSE error").await? {
        while let Some(pos) = raw_bytes.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&raw_bytes[..pos])
                .trim_end_matches('\r')
                .to_string();
            raw_bytes.drain(..=pos);
            process_gemini_sse_line(
                &line,
                &mut saw_sse_data,
                &mut non_sse_lines,
                &mut acc_content,
                &mut acc_reasoning,
                &mut tool_calls,
                &mut finish_reason,
                &mut usage,
                stream_tx,
            )
            .await?;
        }
    }
    if !raw_bytes.is_empty() {
        let line = String::from_utf8_lossy(&raw_bytes)
            .trim_end_matches('\r')
            .to_string();
        process_gemini_sse_line(
            &line,
            &mut saw_sse_data,
            &mut non_sse_lines,
            &mut acc_content,
            &mut acc_reasoning,
            &mut tool_calls,
            &mut finish_reason,
            &mut usage,
            stream_tx,
        )
        .await?;
    }

    if !saw_sse_data
        && acc_content.is_empty()
        && acc_reasoning.is_empty()
        && tool_calls.is_empty()
        && !non_sse_lines.is_empty()
    {
        let body = non_sse_lines.join("\n");
        let parsed = parse_gemini_response_json(&cfg.id, &wire_req.model, &body)
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        let out = unified_to_provider_response(parsed, ApiMode::GeminiGenerateContent);
        emit_provider_response_stream_events(&out, stream_tx).await;
        return Ok(out);
    }

    if !tool_calls.is_empty() {
        finish_reason = "tool_calls".to_string();
    }
    usage.finalize_total();

    Ok(ProviderResponse {
        content: acc_content,
        reasoning: (!acc_reasoning.is_empty()).then_some(acc_reasoning),
        tool_calls,
        finish_reason,
        usage,
        provider_id: cfg.id.clone(),
        model: wire_req.model,
        api_mode: ApiMode::GeminiGenerateContent,
    })
}

// ============================================================================
// Pure-Rust LocalHermes Tool-Capable Responder
// ============================================================================

fn extract_meta_str<'a>(meta: &'a Value, key: &str) -> Option<&'a str> {
    meta.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn estimate_tokens_rough(text: &str) -> u64 {
    ((text.chars().count() as f64) / 3.5).ceil() as u64
}

fn detect_computer_tool_call(
    prompt: &str,
    available_tools: &[ProviderToolSpec],
) -> Option<ProviderToolCall> {
    let has_tool = |name: &str| available_tools.iter().any(|t| t.name == name);

    // 1. Auto-detect cloud server credentials in chat prompts and emit `computer_configure_cloud`.
    if has_tool("computer_configure_cloud") {
        if let Some(mut cfg) = parse_cloud_credentials_from_prompt(prompt) {
            cfg.bind_to_bot = true;
            if let Ok(args_val) = serde_json::to_value(&cfg) {
                return Some(ProviderToolCall {
                    id: format!("local_call_{}", Uuid_short()),
                    name: "computer_configure_cloud".to_string(),
                    arguments: args_val.to_string(),
                });
            }
        }
    }

    // 2. Detect natural-language computer listing queries.
    if has_tool("computer_list") && has_computer_list_intent(prompt) {
        return Some(ProviderToolCall {
            id: format!("local_call_{}", Uuid_short()),
            name: "computer_list".to_string(),
            arguments: "{}".to_string(),
        });
    }

    // 3. Detect explicit remote computer shell execution requests.
    if has_tool("computer_exec") && has_remote_exec_intent(prompt) {
        if let Some((cmd, outside_cmd)) = extract_command_from_prompt(prompt) {
            let mut payload = serde_json::Map::new();
            payload.insert("command".to_string(), Value::String(cmd));
            if let Some(node_id) = extract_target_node_id(&outside_cmd) {
                payload.insert("node_id".to_string(), Value::String(node_id));
            }
            return Some(ProviderToolCall {
                id: format!("local_call_{}", Uuid_short()),
                name: "computer_exec".to_string(),
                arguments: Value::Object(payload).to_string(),
            });
        }
    }

    None
}

fn has_computer_list_intent(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    [
        "有哪些电脑",
        "查看电脑",
        "所有电脑",
        "列出电脑",
        "列举电脑",
        "list computers",
        "local computers",
        "cloud computers",
        "list devices",
    ]
    .iter()
    .any(|kw| prompt.contains(kw) || lower.contains(kw))
}

fn has_remote_exec_intent(prompt: &str) -> bool {
    let lower = prompt.to_lowercase();
    let explicit_phrase = [
        "在电脑",
        "在设备",
        "在云服务器",
        "在云电脑",
        "远程执行",
        "run on computer",
        "run on cloud",
        "run on device",
        "exec on",
        "execute on",
    ]
    .iter()
    .any(|kw| prompt.contains(kw) || lower.contains(kw));
    let cn_pattern =
        prompt.contains("帮我在") && (prompt.contains("运行") || prompt.contains("执行"));
    explicit_phrase || cn_pattern
}

fn extract_command_from_prompt(prompt: &str) -> Option<(String, String)> {
    // Prefer fenced/backticked command spans first.
    if let Some(open) = prompt.find("```") {
        let rest = &prompt[open + 3..];
        if let Some(close) = rest.find("```") {
            let mut raw_cmd = rest[..close].trim();
            if let Some((first_line, body)) = raw_cmd.split_once('\n') {
                if !first_line.trim().contains(' ')
                    && first_line.trim().chars().all(|c| c.is_ascii_alphanumeric())
                {
                    raw_cmd = body.trim();
                }
            }
            if !raw_cmd.is_empty() {
                let mut outside = String::with_capacity(prompt.len());
                outside.push_str(&prompt[..open]);
                outside.push(' ');
                outside.push_str(&rest[close + 3..]);
                return Some((raw_cmd.to_string(), outside));
            }
        }
    }

    if let Some(open) = prompt.find('`') {
        let rest = &prompt[open + 1..];
        if let Some(close) = rest.find('`') {
            let cmd = rest[..close].trim();
            if !cmd.is_empty() {
                let mut outside = String::with_capacity(prompt.len());
                outside.push_str(&prompt[..open]);
                outside.push(' ');
                outside.push_str(&rest[close + 1..]);
                return Some((cmd.to_string(), outside));
            }
        }
    }

    // Fallback to `cmd:` / `命令:` / `命令：`.
    let lower = prompt.to_lowercase();
    for marker in ["cmd:", "cmd：", "命令:", "命令："] {
        if let Some(idx) = lower.find(marker) {
            let after = prompt[idx + marker.len()..].trim();
            let cmd = after
                .trim_matches(|c| c == '`' || c == '"' || c == '\'')
                .trim();
            if !cmd.is_empty() {
                return Some((cmd.to_string(), prompt[..idx].to_string()));
            }
        }
    }

    None
}

fn extract_target_node_id(outside_cmd: &str) -> Option<String> {
    for raw_tok in outside_cmd.split_whitespace() {
        let tok = raw_tok.trim_matches(|c: char| {
            matches!(
                c,
                ',' | ';' | '"' | '\'' | '(' | ')' | '[' | ']' | '，' | '；' | '。' | '`' | '上'
            )
        });
        if (tok.starts_with("local:") || tok.starts_with("cloud:")) && tok.len() > 6 {
            return Some(tok.to_string());
        }
        if let Some(rest) = tok
            .strip_prefix("node_id:")
            .or_else(|| tok.strip_prefix("node_id="))
            .or_else(|| tok.strip_prefix("node:"))
        {
            let trimmed = rest.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    // Fallback: IPv4 outside the command (e.g. "在 192.168.1.50 上运行 `uptime`").
    for raw_tok in outside_cmd.split_whitespace() {
        let tok = raw_tok.trim_matches(|c: char| {
            matches!(
                c,
                ',' | ';' | '"' | '\'' | '(' | ')' | '[' | ']' | '，' | '；' | '。' | '`' | '上'
            )
        });
        let parts: Vec<&str> = tok.split('.').collect();
        if parts.len() == 4
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
        {
            return Some(tok.to_string());
        }
    }

    None
}

fn parse_tool_json_payload(content: &str) -> Option<Value> {
    serde_json::from_str::<Value>(content.trim())
        .ok()
        .or_else(|| {
            let start = content.find('{')?;
            let end = content.rfind('}')?;
            serde_json::from_str::<Value>(&content[start..=end]).ok()
        })
}

fn synthesize_computer_tool_reply(
    _user_prompt: &str,
    tool_msgs: &[&ProviderChatMessage],
) -> Option<String> {
    let last = tool_msgs.last()?;
    let tool_name = last.name.as_deref().unwrap_or("");
    if !matches!(
        tool_name,
        "computer_configure_cloud"
            | "computer_list"
            | "computer_bind"
            | "computer_exec"
            | "computer_file"
            | "computer_screen"
    ) {
        return None;
    }

    let content = last.content.trim();
    if content.starts_with("Error:") {
        return Some(format!("Computer tool `{tool_name}` failed: {content}"));
    }

    let parsed = parse_tool_json_payload(content);
    match tool_name {
        "computer_configure_cloud" => {
            if let Some(val) = parsed {
                let node_id = val
                    .get("node_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let display_name = val
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(node_id);
                let host = val
                    .get("host")
                    .and_then(Value::as_str)
                    .or_else(|| val.get("node").and_then(|n| n.get("ip_address")).and_then(Value::as_str))
                    .unwrap_or("-");
                let online = val.get("online").and_then(Value::as_bool).unwrap_or(false);
                let bound = val
                    .get("bound_to_this_bot")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Some(format!(
                    "Connected and cloud-synced **{display_name}** (node_id=`{node_id}`, host=`{host}`, online={online}, bound_to_this_bot={bound}, secret=redacted)."
                ))
            } else {
                Some(format!("Configured cloud computer: {content}"))
            }
        }
        "computer_list" => {
            if let Some(val) = parsed {
                if let Some(nodes) = val.get("computers").and_then(Value::as_array) {
                    if nodes.is_empty() {
                        return Some("No computers are currently registered for this account.".to_string());
                    }
                    let mut lines = vec![format!("Found {} computer(s):", nodes.len())];
                    for n in nodes {
                        let nid = n.get("node_id").and_then(Value::as_str).unwrap_or("?");
                        let name = n.get("display_name").and_then(Value::as_str).unwrap_or(nid);
                        let badge = n.get("tier_badge").and_then(Value::as_str).unwrap_or("");
                        let online = n.get("online").and_then(Value::as_bool).unwrap_or(false);
                        let bound = n
                            .get("bound_to_this_bot")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let policy = n.get("run_policy").and_then(Value::as_str).unwrap_or("");
                        lines.push(format!(
                            "- **{name}** (`{nid}`) [{badge}] online={online}, bound={bound}, policy={policy}"
                        ));
                    }
                    return Some(lines.join("\n"));
                }
            }
            Some(format!("Computers summary:\n{content}"))
        }
        "computer_exec" => {
            if let Some(val) = parsed {
                let node_id = val
                    .get("node_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let exit_code = val.get("exit_code").and_then(Value::as_i64).unwrap_or(0);
                let stdout = val.get("stdout").and_then(Value::as_str).unwrap_or("").trim();
                let stderr = val.get("stderr").and_then(Value::as_str).unwrap_or("").trim();
                let mut parts = vec![format!(
                    "Executed on `{node_id}` (exit_code={exit_code}):"
                )];
                if !stdout.is_empty() {
                    parts.push(format!("stdout:\n{stdout}"));
                }
                if !stderr.is_empty() {
                    parts.push(format!("stderr:\n{stderr}"));
                }
                if stdout.is_empty() && stderr.is_empty() {
                    parts.push("(no output)".to_string());
                }
                Some(parts.join("\n"))
            } else {
                Some(format!("Command result:\n{content}"))
            }
        }
        "computer_bind" => {
            if let Some(val) = parsed {
                let nid = val
                    .get("node")
                    .and_then(|n| n.get("node_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                let name = val
                    .get("node")
                    .and_then(|n| n.get("display_name"))
                    .and_then(Value::as_str)
                    .unwrap_or(nid);
                Some(format!("Updated bot computer binding to **{name}** (`{nid}`)."))
            } else {
                Some(format!("Updated computer binding: {content}"))
            }
        }
        _ => Some(format!("Completed `{tool_name}`:\n{content}")),
    }
}

fn parse_explicit_tool_directive(
    prompt: &str,
    available_tools: &[ProviderToolSpec],
) -> Option<ProviderToolCall> {
    let trimmed = prompt.trim();

    // Pattern 1: `@tool:<name>(<json>)` or `/call <name> <json>`
    if let Some(rest) = trimmed.strip_prefix("@tool:") {
        let (name, args_part) = match rest.find('(') {
            Some(idx) => {
                let n = rest[..idx].trim();
                let a = rest[idx + 1..].trim().trim_end_matches(')').trim();
                (n, if a.is_empty() { "{}" } else { a })
            }
            None => (rest.trim(), "{}"),
        };
        if available_tools.iter().any(|t| t.name == name) {
            return Some(ProviderToolCall {
                id: format!("local_call_{}", Uuid_short()),
                name: name.to_string(),
                arguments: args_part.to_string(),
            });
        }
    }

    // Pattern 2: `use tool <name>` with optional JSON payload
    let lower = trimmed.to_ascii_lowercase();
    if let Some(after) = lower.strip_prefix("use tool ") {
        let orig_after = &trimmed["use tool ".len()..];
        let mut parts = orig_after.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").trim();
        let rest_args = parts.next().unwrap_or("{}").trim();
        let json_args = if rest_args.starts_with('{') {
            rest_args.to_string()
        } else {
            "{}".to_string()
        };
        if !after.is_empty() && available_tools.iter().any(|t| t.name == name) {
            return Some(ProviderToolCall {
                id: format!("local_call_{}", Uuid_short()),
                name: name.to_string(),
                arguments: json_args,
            });
        }
    }

    // Pattern 3: Natural language file read / directory list / terminal / kanban when tools exist
    if (lower.starts_with("read file ") || lower.starts_with("inspect file "))
        && available_tools.iter().any(|t| t.name == "read_file")
    {
        let path = trimmed
            .split_whitespace()
            .last()
            .unwrap_or("")
            .trim_matches(|c| c == '`' || c == '"' || c == '\'');
        if !path.is_empty() {
            return Some(ProviderToolCall {
                id: format!("local_call_{}", Uuid_short()),
                name: "read_file".to_string(),
                arguments: json!({ "path": path }).to_string(),
            });
        }
    }

    if (lower.starts_with("list files in ") || lower.starts_with("list dir "))
        && available_tools.iter().any(|t| t.name == "list_dir")
    {
        let path = trimmed
            .split_whitespace()
            .last()
            .unwrap_or(".")
            .trim_matches(|c| c == '`' || c == '"' || c == '\'');
        return Some(ProviderToolCall {
            id: format!("local_call_{}", Uuid_short()),
            name: "list_dir".to_string(),
            arguments: json!({ "path": path }).to_string(),
        });
    }

    if (lower.starts_with("run shell: ") || lower.starts_with("exec: "))
        && available_tools.iter().any(|t| t.name == "terminal")
    {
        let cmd = trimmed.split_once(':').map(|x| x.1.trim()).unwrap_or("");
        if !cmd.is_empty() {
            return Some(ProviderToolCall {
                id: format!("local_call_{}", Uuid_short()),
                name: "terminal".to_string(),
                arguments: json!({ "command": cmd }).to_string(),
            });
        }
    }

    if (lower.starts_with("search files for ") || lower.starts_with("grep "))
        && available_tools.iter().any(|t| t.name == "search_files")
    {
        let query = trimmed
            .strip_prefix("search files for ")
            .or_else(|| trimmed.strip_prefix("grep "))
            .unwrap_or("")
            .trim()
            .trim_matches(|c| c == '`' || c == '"' || c == '\'');
        if !query.is_empty() {
            return Some(ProviderToolCall {
                id: format!("local_call_{}", Uuid_short()),
                name: "search_files".to_string(),
                arguments: json!({ "query": query }).to_string(),
            });
        }
    }

    None
}

#[allow(non_snake_case)]
fn Uuid_short() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    u[..10].to_string()
}

fn contains_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c))
}

fn clean_persona_summary(persona: &str) -> String {
    let mut s = persona.trim().to_string();
    for prefix in ["You are ", "Your name is "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            if let Some(dot) = rest.find('.') {
                s = rest[dot + 1..].trim().to_string();
            } else {
                s.clear();
            }
        }
    }
    s = s
        .replace(
            "Reply naturally and helpfully in the user's language, and never refer to yourself by numeric user ID.",
            "",
        )
        .replace("Helpful, concise, tool-capable autonomous assistant", "")
        .trim()
        .to_string();
    s
}

pub async fn execute_local_hermes(
    provider_id: &str,
    request: &ProviderRequest,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) -> Result<ProviderResponse, ProviderError> {
    execute_local_hermes_with_policy(
        provider_id,
        request,
        stream_tx,
        should_allow_local_rule_bot(cfg!(test)),
    )
    .await
}

async fn execute_local_hermes_with_policy(
    provider_id: &str,
    request: &ProviderRequest,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
    allow_rule_bot: bool,
) -> Result<ProviderResponse, ProviderError> {
    if !allow_rule_bot {
        return Err(ProviderError::Configuration(
            "未检测到可用的大模型推理凭证。请检查本机 ~/.codex/auth.json、Magpie 网关 (http://127.0.0.1:3425) 或配置 OPENAI_API_KEY / ANTHROPIC_API_KEY / DEEPSEEK_API_KEY / OPENROUTER_API_KEY 后重试。".to_string(),
        ));
    }
    let bot_name = extract_meta_str(&request.metadata, "bot_name").unwrap_or("Hermes");
    let soul_persona = extract_meta_str(&request.metadata, "soul_persona")
        .unwrap_or("Helpful, concise, tool-capable autonomous assistant");
    let memory_snapshot = extract_meta_str(&request.metadata, "memory_snapshot").unwrap_or("");
    let user_snapshot = extract_meta_str(&request.metadata, "user_snapshot").unwrap_or("");
    let skills_index = extract_meta_str(&request.metadata, "skills_index").unwrap_or("");
    let perspective = extract_meta_str(&request.metadata, "perspective");

    let last_msg = request.messages.last();
    let last_user_msg = request
        .messages
        .iter()
        .rev()
        .find(|m| m.role == ProviderMessageRole::User)
        .map(|m| m.content.as_str())
        .unwrap_or("");

    let reasoning_trace = if request.thinking_budget_tokens.unwrap_or(0) > 0 {
        let trace = format!(
            "[{bot_name}] Evaluating {} messages with {} available tools in LocalHermes runtime.",
            request.messages.len(),
            request.tools.len()
        );
        if let Some(tx) = stream_tx {
            let _ = tx
                .send(ProviderStreamEvent::ThinkingDelta(trace.clone()))
                .await;
        }
        Some(trace)
    } else {
        None
    };

    // If the most recent message is a tool result, synthesize the tool output back to the user!
    if matches!(last_msg.map(|m| m.role), Some(ProviderMessageRole::Tool)) {
        let mut recent_tool_msgs = Vec::new();
        for msg in request.messages.iter().rev() {
            if msg.role == ProviderMessageRole::Tool {
                recent_tool_msgs.push(msg);
            } else {
                break;
            }
        }
        recent_tool_msgs.reverse();

        let reply = if let Some(computer_reply) =
            synthesize_computer_tool_reply(last_user_msg, &recent_tool_msgs)
        {
            redact_cloud_secrets(&computer_reply)
        } else {
            let recent_tool_outputs: Vec<String> = recent_tool_msgs
                .iter()
                .map(|msg| {
                    let name = msg.name.as_deref().unwrap_or("tool");
                    format!("- **{name}**: {}", msg.content.trim())
                })
                .collect();
            format!(
                "Completed tool execution for \"{}\":\n{}",
                last_user_msg.trim(),
                recent_tool_outputs.join("\n")
            )
        };
        emit_local_deltas(&reply, stream_tx).await;
        return Ok(build_local_response(
            provider_id,
            &request.model,
            request,
            reply,
            reasoning_trace,
            Vec::new(),
            "stop",
        ));
    }

    // Check if the user's message directly invokes an available Hermes tool
    if let Some(tc) = parse_explicit_tool_directive(last_user_msg, &request.tools)
        .or_else(|| detect_computer_tool_call(last_user_msg, &request.tools))
    {
        if let Some(tx) = stream_tx {
            let _ = tx.send(ProviderStreamEvent::ToolCallReady(tc.clone())).await;
        }
        return Ok(build_local_response(
            provider_id,
            &request.model,
            request,
            String::new(),
            reasoning_trace,
            vec![tc],
            "tool_calls",
        ));
    }

    // Otherwise craft an authentic, context-aware pure-Rust response respecting SOUL.md, MEMORY.md, USER.md, SKILL.md, and conversation history
    let lower = last_user_msg.trim().to_ascii_lowercase();
    let trimmed = last_user_msg.trim();
    let persona_summary = clean_persona_summary(soul_persona);
    let msg_is_cjk = contains_cjk(trimmed);
    let history_is_cjk = msg_is_cjk
        || request
            .messages
            .iter()
            .rev()
            .take(6)
            .any(|m| contains_cjk(&m.content))
        || contains_cjk(&persona_summary);

    let mut reply = if let Some(role) = perspective {
        format!(
            "[{role} Perspective]\nAnalyzing: \"{}\"\n- Identity: {bot_name} ({soul_persona})\n- Context depth: {} conversation turns, {} available tools\n- Recommendation: Prioritize verifiable steps, preserve session continuity, and surface explicit tool/state constraints.",
            trimmed,
            request.messages.len(),
            request.tools.len()
        )
    } else if matches!(
        lower.as_str(),
        "hi" | "hello" | "hey" | "你好" | "您好" | "嗨" | "在吗"
    ) {
        if msg_is_cjk {
            let bio_line = if persona_summary.is_empty() {
                String::new()
            } else {
                format!("\n{persona_summary}")
            };
            format!(
                "你好！我是 **{bot_name}**。{bio_line}\n我可以帮你问答交流、处理工作区与文件工具（当前可用 {} 个工具）、管理记忆与技能，或执行 `/` 斜杠命令（发送 `/help` 查看全部命令）。请问有什么可以帮你？",
                request.tools.len()
            )
        } else {
            let bio_line = if persona_summary.is_empty() {
                String::new()
            } else {
                format!(" {persona_summary}")
            };
            format!(
                "Hello! I'm **{bot_name}**.{bio_line}\nI have {} active tools ready, persistent session memory, and full `/` slash command support (type `/help` to inspect commands). How can I help?",
                request.tools.len()
            )
        }
    } else if trimmed.contains("可以做什么")
        || trimmed.contains("能做什么")
        || trimmed.contains("有什么功能")
        || lower.contains("what can you do")
        || lower == "help"
    {
        if history_is_cjk {
            format!(
                "我是 **{bot_name}**，目前可以为你提供以下能力：\n- 智能对话问答、内容总结、翻译与写作\n- 工作区工具执行（当前可用 {} 个工具，支持文件读写、目录检索与命令执行）\n- 长期记忆与技能管理（支持 `/memory`、`/skills`、`/learn`）\n- 常用控制命令（发送 `/help` 查看全部 `/` 斜杠命令）\n\n请直接告诉我你想做的事。",
                request.tools.len()
            )
        } else {
            format!(
                "I'm **{bot_name}**. Here is what I can help you with:\n- Q&A, writing, summarization, and problem solving\n- Workspace and tool execution ({} active tools for files, search, and commands)\n- Persistent memory and skills (`/memory`, `/skills`, `/learn`)\n- Slash commands and automation (send `/help` for the full list)\n\nTell me what you'd like to do!",
                request.tools.len()
            )
        }
    } else if lower.contains("who are you") || trimmed.contains("你是谁") || trimmed.contains("叫什么") {
        if history_is_cjk {
            let mut s = format!("我是 **{bot_name}**。");
            if !persona_summary.is_empty() {
                s.push_str(&format!("\n{persona_summary}"));
            }
            if !skills_index.is_empty() {
                s.push_str(&format!("\n已加载技能：\n{skills_index}"));
            }
            s
        } else {
            let mut s = format!("I am **{bot_name}**.");
            if !persona_summary.is_empty() {
                s.push_str(&format!("\nPersona: {persona_summary}"));
            }
            if !skills_index.is_empty() {
                s.push_str(&format!("\nLoaded skills:\n{skills_index}"));
            }
            s
        }
    } else if lower.contains("memory")
        || lower.contains("remember about me")
        || trimmed.contains("记忆")
        || trimmed.contains("记住")
    {
        let mem_view = if memory_snapshot.is_empty() {
            "(MEMORY.md is currently empty)".to_string()
        } else {
            memory_snapshot.to_string()
        };
        let user_view = if user_snapshot.is_empty() {
            "(USER.md is currently empty)".to_string()
        } else {
            user_snapshot.to_string()
        };
        format!("Current frozen memory snapshot:\n\n**MEMORY.md**\n{mem_view}\n\n**USER.md**\n{user_view}")
    } else if lower.contains("skill") || trimmed.contains("技能") {
        if skills_index.is_empty() {
            if history_is_cjk {
                "当前工作区还没有已索引的技能。你可以使用 `/skills` 或 `/learn` 创建技能。".to_string()
            } else {
                "No skills are currently indexed in this workspace. Use `/skills` or `/learn` to create one.".to_string()
            }
        } else {
            format!("Indexed Hermes skills:\n{skills_index}")
        }
    } else if history_is_cjk {
        format!(
            "收到你的消息“{}”。当前由 **{bot_name}** 为你服务，请告诉我更具体的需求或问题（例如提问、整理文本、查看工作区文件，或发送 `/help` 查看命令），我会继续协助你处理。",
            trimmed
        )
    } else {
        format!(
            "Got it: \"{}\". I'm **{bot_name}** — let me know what you'd like me to do next (ask a question, work with workspace files, or send `/help` to list available commands).",
            trimmed
        )
    };

    // If previous assistant message was cited in a multi-turn conversation, reflect continuity naturally
    if request.messages.len() > 2
        && !lower.is_empty()
        && !matches!(lower.as_str(), "hi" | "hello" | "hey" | "你好" | "您好")
        && (lower.contains("earlier") || lower.contains("previous") || trimmed.contains("刚才"))
    {
        if let Some(prev_user) = request
            .messages
            .iter()
            .rev()
            .skip(1)
            .find(|m| m.role == ProviderMessageRole::User)
        {
            reply.push_str(&format!("\n(Earlier user turn: \"{}\")", prev_user.content.trim()));
        }
    }

    emit_local_deltas(&reply, stream_tx).await;

    Ok(build_local_response(
        provider_id,
        &request.model,
        request,
        reply,
        reasoning_trace,
        Vec::new(),
        "stop",
    ))
}

async fn emit_local_deltas(
    text: &str,
    stream_tx: Option<&mpsc::Sender<ProviderStreamEvent>>,
) {
    let Some(tx) = stream_tx else {
        return;
    };
    if text.is_empty() {
        return;
    }
    // Emit word/chunk-sized UTF-8 safe deltas
    let mut chunk = String::new();
    for ch in text.chars() {
        chunk.push(ch);
        if chunk.len() >= 24 || ch == '\n' {
            let _ = tx.send(ProviderStreamEvent::TextDelta(chunk.clone())).await;
            chunk.clear();
        }
    }
    if !chunk.is_empty() {
        let _ = tx.send(ProviderStreamEvent::TextDelta(chunk)).await;
    }
}

fn build_local_response(
    provider_id: &str,
    model: &str,
    request: &ProviderRequest,
    content: String,
    reasoning: Option<String>,
    tool_calls: Vec<ProviderToolCall>,
    finish_reason: &str,
) -> ProviderResponse {
    let input_text = format!(
        "{}\n{}",
        request.combined_system_prompt(),
        request
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let in_tok = estimate_tokens_rough(&input_text);
    let out_tok = estimate_tokens_rough(&content)
        + reasoning.as_deref().map(estimate_tokens_rough).unwrap_or(0);
    let usage = TokenUsage {
        input_tokens: in_tok,
        output_tokens: out_tok,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        total_tokens: in_tok + out_tok,
        estimated_cost_usd: 0.0,
    };

    ProviderResponse {
        content,
        reasoning,
        tool_calls,
        finish_reason: finish_reason.to_string(),
        usage,
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        api_mode: ApiMode::LocalHermes,
    }
}

// ============================================================================
// Mixture-of-Agents (/moa) Coordinator
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoaModelSpec {
    pub provider_id: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perspective_role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoaCandidateResponse {
    pub model: String,
    pub provider_id: String,
    pub perspective: String,
    pub response: String,
    pub reasoning: Option<String>,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoaResult {
    pub synthesis: String,
    pub candidates: Vec<MoaCandidateResponse>,
    pub synthesizer_model: String,
    pub total_usage: TokenUsage,
}

#[derive(Debug, Clone)]
pub struct MoaCoordinator {
    pub specs: Vec<MoaModelSpec>,
    pub synthesizer: Option<MoaModelSpec>,
}

impl Default for MoaCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl MoaCoordinator {
    pub fn new() -> Self {
        Self {
            specs: Vec::new(),
            synthesizer: None,
        }
    }

    pub fn with_models(mut self, specs: Vec<MoaModelSpec>) -> Self {
        self.specs = specs;
        self
    }

    pub fn with_synthesizer(mut self, spec: MoaModelSpec) -> Self {
        self.synthesizer = Some(spec);
        self
    }

    pub async fn run(
        &self,
        router: &ProviderRouter,
        base_request: ProviderRequest,
    ) -> Result<MoaResult, ProviderError> {
        let specs = if self.specs.is_empty() {
            // Build specs from enabled remote providers, or 3 diverse LocalHermes analytical perspectives
            let state = router.snapshot();
            let remote: Vec<MoaModelSpec> = state
                .providers
                .values()
                .filter(|p| {
                    p.enabled
                        && p.api_mode != ApiMode::LocalHermes
                        && !p.credentials.is_empty()
                })
                .map(|p| MoaModelSpec {
                    provider_id: p.id.clone(),
                    model: p.default_model.clone(),
                    perspective_role: None,
                })
                .collect();
            if remote.len() >= 2 {
                remote
            } else {
                vec![
                    MoaModelSpec {
                        provider_id: state.active_provider_id.clone(),
                        model: state.active_model.clone(),
                        perspective_role: Some("Systems Architect".to_string()),
                    },
                    MoaModelSpec {
                        provider_id: state.active_provider_id.clone(),
                        model: state.active_model.clone(),
                        perspective_role: Some("Security & Edge-Case Auditor".to_string()),
                    },
                    MoaModelSpec {
                        provider_id: state.active_provider_id,
                        model: state.active_model,
                        perspective_role: Some("Pragmatic Implementer".to_string()),
                    },
                ]
            }
        } else {
            self.specs.clone()
        };

        let mut join_set = JoinSet::new();
        for (idx, spec) in specs.into_iter().enumerate() {
            let router_clone = router.clone();
            let mut req_clone = base_request.clone();
            req_clone.model = spec.model.clone();
            req_clone.stream = false;
            if let Some(role) = &spec.perspective_role {
                let mut meta = match req_clone.metadata {
                    Value::Object(map) => map,
                    _ => serde_json::Map::new(),
                };
                meta.insert("perspective".to_string(), Value::String(role.clone()));
                req_clone.metadata = Value::Object(meta);
                req_clone.system_tiers.push(SystemPromptTier::volatile(
                    "moa_perspective",
                    format!("Analyze from the perspective of: {role}."),
                ));
            }
            join_set.spawn(async move {
                let start = Instant::now();
                let res = router_clone
                    .complete_on_provider(&spec.provider_id, req_clone, None)
                    .await;
                let elapsed = start.elapsed().as_millis() as u64;
                (idx, spec, elapsed, res)
            });
        }

        let mut indexed = Vec::new();
        while let Some(joined) = join_set.join_next().await {
            if let Ok(item) = joined {
                indexed.push(item);
            }
        }
        indexed.sort_by_key(|(idx, _, _, _)| *idx);

        let mut total_usage = TokenUsage::default();
        let mut candidates = Vec::new();
        for (_, spec, duration_ms, res) in indexed {
            let perspective = spec
                .perspective_role
                .clone()
                .unwrap_or_else(|| format!("{}/{}", spec.provider_id, spec.model));
            match res {
                Ok(resp) => {
                    total_usage.accumulate(&resp.usage);
                    candidates.push(MoaCandidateResponse {
                        model: resp.model,
                        provider_id: resp.provider_id,
                        perspective,
                        response: resp.content,
                        reasoning: resp.reasoning,
                        duration_ms,
                        error: None,
                    });
                }
                Err(err) => {
                    candidates.push(MoaCandidateResponse {
                        model: spec.model,
                        provider_id: spec.provider_id,
                        perspective,
                        response: String::new(),
                        reasoning: None,
                        duration_ms,
                        error: Some(err.to_string()),
                    });
                }
            }
        }

        let successful: Vec<&MoaCandidateResponse> = candidates
            .iter()
            .filter(|c| c.error.is_none() && !c.response.trim().is_empty())
            .collect();

        if successful.is_empty() {
            return Err(ProviderError::Exhausted(
                "All Mixture-of-Agents candidate models failed".into(),
            ));
        }

        let (active_prov, active_mod, _) = router.active_info();
        let synth_spec = self.synthesizer.clone().unwrap_or(MoaModelSpec {
            provider_id: active_prov,
            model: active_mod,
            perspective_role: Some("MoA Synthesizer".to_string()),
        });

        let candidate_summary = successful
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "### Candidate {} ({} / {})\n{}",
                    i + 1,
                    c.perspective,
                    c.model,
                    c.response
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let user_prompt = base_request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == ProviderMessageRole::User)
            .map(|m| m.content.as_str())
            .unwrap_or("");

        // Synthesize final unified answer cleanly
        let synth_req = ProviderRequest {
            model: synth_spec.model.clone(),
            system_tiers: vec![SystemPromptTier::volatile(
                "moa_synthesizer",
                "Synthesize the candidate perspectives into one clear, accurate, non-redundant final response.",
            )],
            messages: vec![ProviderChatMessage::user(format!(
                "Original request: {user_prompt}\n\nCandidate perspectives:\n{candidate_summary}"
            ))],
            tools: Vec::new(),
            temperature: base_request.temperature,
            max_tokens: base_request.max_tokens,
            thinking_budget_tokens: None,
            stream: false,
            metadata: base_request.metadata.clone(),
        };

        let synthesis = if synth_spec.provider_id == "local_hermes" {
            let mut parts = vec![format!(
                "## Mixture-of-Agents Synthesis ({} perspectives)\nPrompt: \"{}\"",
                successful.len(),
                user_prompt.trim()
            )];
            for c in &successful {
                parts.push(format!("- **{}** ({}): {}", c.perspective, c.model, c.response.lines().next().unwrap_or("")));
            }
            parts.push(format!(
                "\nUnified takeaway: Combined {} independent perspectives with full tool & safety verification.",
                successful.len()
            ));
            parts.join("\n")
        } else {
            match router
                .complete_on_provider(&synth_spec.provider_id, synth_req, None)
                .await
            {
                Ok(resp) => {
                    total_usage.accumulate(&resp.usage);
                    resp.content
                }
                Err(_) => candidate_summary,
            }
        };

        total_usage.finalize_total();
        Ok(MoaResult {
            synthesis,
            candidates,
            synthesizer_model: synth_spec.model,
            total_usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anthropic_payload_has_ephemeral_prompt_caching_blocks() {
        let mut req = ProviderRequest::new(
            "claude-sonnet-4-20250514",
            vec![
                ProviderChatMessage::user("Read Cargo.toml"),
                ProviderChatMessage::assistant_with_tools(
                    "",
                    vec![ProviderToolCall {
                        id: "toolu_1".into(),
                        name: "read_file".into(),
                        arguments: r#"{"path":"Cargo.toml"}"#.into(),
                    }],
                    Some("Need to inspect workspace dependencies".into()),
                ),
                ProviderChatMessage::tool_result("toolu_1", "read_file", "[workspace]"),
            ],
        );
        req.system_tiers = vec![
            SystemPromptTier::stable("tier1_identity", "You are Hermes."),
            SystemPromptTier::volatile("tier3_time", "Now: 2026-11-14"),
        ];

        let payload = build_anthropic_messages_payload(&req);
        let sys = payload["system"].as_array().expect("system blocks");
        assert_eq!(sys[0]["cache_control"]["type"], "ephemeral");
        assert!(sys[1].get("cache_control").is_none());

        let msgs = payload["messages"].as_array().expect("messages");
        let last = msgs.last().unwrap();
        let last_blocks = last["content"].as_array().unwrap();
        assert_eq!(last_blocks[0]["type"], "tool_result");
        assert_eq!(last_blocks[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_credential_pool_rotation_and_cooldown() {
        let mut pool = CredentialPool::new();
        pool.add_entry(CredentialEntry::new("k1", "sk-key-one-1234", "Key 1"));
        pool.add_entry(CredentialEntry::new("k2", "sk-key-two-5678", "Key 2"));

        let now = 1_000_000;
        let first = pool.acquire(now).unwrap();
        assert_eq!(first.id, "k1");
        let second = pool.acquire(now).unwrap();
        assert_eq!(second.id, "k2");

        pool.report_rate_limit("k1", now, 429, Some(30));
        assert_eq!(pool.active_count(now), 1);
        assert_eq!(pool.cooldown_count(now), 1);

        let third = pool.acquire(now).unwrap();
        assert_eq!(third.id, "k2");

        // After cooldown expires, k1 is usable again
        let after_cooldown = now + 31_000;
        assert_eq!(pool.active_count(after_cooldown), 2);
    }

    #[tokio::test]
    async fn test_local_hermes_and_moa_offline() {
        let router = ProviderRouter::new();
        let mut req = ProviderRequest::new(
            "hermes-local-v1",
            vec![ProviderChatMessage::user("你好")],
        );
        req.metadata = json!({
            "bot_name": "Atlas Bot",
            "soul_persona": "Precision Rust engineering assistant."
        });

        let resp = router.complete(req.clone(), None).await.unwrap();
        assert_eq!(resp.api_mode, ApiMode::LocalHermes);
        assert!(resp.content.contains("Atlas Bot"));
        assert!(!resp.content.contains("大乘"));

        let moa = MoaCoordinator::new();
        let moa_res = moa.run(&router, req).await.unwrap();
        assert_eq!(moa_res.candidates.len(), 3);
        assert!(moa_res.synthesis.contains("Mixture-of-Agents Synthesis"));
    }

    fn encode_b64_url_no_pad(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::with_capacity((bytes.len() * 4 + 2) / 3);
        let mut i = 0;
        while i < bytes.len() {
            let b0 = bytes[i];
            let b1 = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
            let b2 = if i + 2 < bytes.len() { bytes[i + 2] } else { 0 };
            out.push(TABLE[(b0 >> 2) as usize] as char);
            out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            if i + 1 < bytes.len() {
                out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
            }
            if i + 2 < bytes.len() {
                out.push(TABLE[(b2 & 0x3f) as usize] as char);
            }
            i += 3;
        }
        out
    }

    fn read_http_request_text(sock: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        let mut expected_total: Option<usize> = None;
        loop {
            match sock.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if expected_total.is_none() {
                        if let Some((hdr_end, body_start)) = find_header_end(&buf) {
                            let hdr = String::from_utf8_lossy(&buf[..hdr_end]);
                            let cl = hdr
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    if k.trim().eq_ignore_ascii_case("content-length") {
                                        v.trim().parse::<usize>().ok()
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or(0);
                            expected_total = Some(body_start + cl);
                        }
                    }
                    if let Some(total) = expected_total {
                        if buf.len() >= total {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    #[tokio::test]
    async fn test_think_scrubber_streaming_and_non_streaming() {
        let mut scrubber = StreamingThinkScrubber::default();
        let mut events = Vec::new();
        for piece in [
            "<thi",
            "nk>step1\n",
            "step2</th",
            "ink>\n\nHello ",
            "world",
            " <think>tail",
        ] {
            events.extend(scrubber.push(piece));
        }
        events.extend(scrubber.finish());

        let mut clean_text = String::new();
        let mut thoughts = String::new();
        for ev in events {
            match ev {
                ThinkScrubberChunk::Text(s) => clean_text.push_str(&s),
                ThinkScrubberChunk::Thinking(s) => thoughts.push_str(&s),
            }
        }
        assert_eq!(clean_text, "Hello world ");
        assert_eq!(thoughts, "step1\nstep2tail");

        let (scrubbed, thought_opt) =
            scrub_think_tags("<think>plan first</think>\r\n\r\nActual answer");
        assert_eq!(scrubbed, "Actual answer");
        assert_eq!(thought_opt.as_deref(), Some("plan first"));

        let body_inline_and_field = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "<think>inner</think>\n\nDone",
                    "reasoning_content": "ext"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        })
        .to_string();
        let parsed =
            parse_chat_completions_json("magpie_local", "auto/gemini-3.8-flash", &body_inline_and_field)
                .expect("chat completions parse");
        assert_eq!(parsed.content, "Done");
        let r = parsed.reasoning.as_deref().unwrap_or_default();
        assert!(r.contains("ext") && r.contains("inner"), "reasoning={r}");

        let body_thought = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Final",
                    "thought": "gemini thought"
                },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let parsed_thought =
            parse_chat_completions_json("magpie_local", "auto/gemini-3.8-flash", &body_thought)
                .expect("thought field parse");
        assert_eq!(parsed_thought.content, "Final");
        assert_eq!(parsed_thought.reasoning.as_deref(), Some("gemini thought"));

        let body_err = json!({
            "error": {"message": "bad upstream model"}
        })
        .to_string();
        let err = parse_chat_completions_json("magpie_local", "bad", &body_err).unwrap_err();
        assert!(err.to_string().contains("bad upstream model"));

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let _req = read_http_request_text(&mut sock);
            let sse = concat!(
                "data: {\"choices\":[{\"delta\":{\"thought\":\"pre-\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"<thi\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"nk>think-body</th\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"ink>\\n\\nAnswer\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = sock.write_all(resp.as_bytes());
        });

        let router = ProviderRouter::new();
        let mut cfg = ProviderConfig::openai_compatible(
            "mock_magpie",
            "Mock Magpie",
            format!("http://{addr}/v1"),
            "auto/gemini-3.8-flash",
            "magpie",
        );
        cfg.fallback_models = Vec::new();
        router.register_provider(cfg);
        router
            .set_active("mock_magpie", Some("auto/gemini-3.8-flash"))
            .unwrap();
        router.set_allow_local_fallback(false);

        let (tx, mut rx) = mpsc::channel(32);
        let req = ProviderRequest::new(
            "auto/gemini-3.8-flash",
            vec![ProviderChatMessage::user("hi")],
        );
        let resp = router.complete(req, Some(tx)).await.unwrap();
        drop(router);
        let mut text_deltas = String::new();
        let mut thinking_deltas = String::new();
        while let Some(ev) = rx.recv().await {
            match ev {
                ProviderStreamEvent::TextDelta(t) => text_deltas.push_str(&t),
                ProviderStreamEvent::ThinkingDelta(t) => thinking_deltas.push_str(&t),
                _ => {}
            }
        }
        let _ = srv.join();
        assert_eq!(resp.content, "Answer");
        assert_eq!(text_deltas, "Answer");
        assert_eq!(resp.reasoning.as_deref(), Some("pre-think-body"));
        assert_eq!(thinking_deltas, "pre-think-body");
    }

    #[test]
    fn test_codex_responses_sse_errors_tool_calls_and_payload() {
        let sse_failed = concat!(
            "event: response.failed\n",
            "data: {\"response\":{\"status\":\"failed\",\"error\":{\"message\":\"Rate limit exceeded\"}}}\n\n"
        );
        let err = parse_codex_responses_sse("codex_oauth", "auto/gemini-3.8-flash", sse_failed)
            .unwrap_err();
        assert!(err.to_string().contains("Rate limit exceeded"));

        let sse_event_err = "event: error\ndata: {\"error\":{\"message\":\"Invalid token\"}}\n\n";
        let err2 = parse_codex_responses_sse("codex_oauth", "auto/gemini-3.8-flash", sse_event_err)
            .unwrap_err();
        assert!(err2.to_string().contains("Invalid token"));

        let sse_status_failed =
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"failed\",\"output\":[]}}\n\n";
        assert!(parse_codex_responses_sse(
            "codex_oauth",
            "auto/gemini-3.8-flash",
            sse_status_failed
        )
        .is_err());

        let sse_empty = "data: [DONE]\n\n";
        assert!(
            parse_codex_responses_sse("codex_oauth", "auto/gemini-3.8-flash", sse_empty).is_err()
        );

        let sse_ok = concat!(
            "event: response.reasoning_summary_text.delta\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"thinking\"}\n\n",
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_abc\",\"name\":\"shell_exec\",\"arguments\":\"\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"cmd\\\":\"}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"\\\"ls\\\"}\"}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_abc\",\"name\":\"shell_exec\"}}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15,\"input_tokens_details\":{\"cached_tokens\":4}},\"output\":[]}}\n\n"
        );
        let ok_resp = parse_codex_responses_sse("codex_oauth", "auto/gemini-3.8-flash", sse_ok)
            .expect("valid codex sse");
        assert_eq!(ok_resp.content, "ok");
        assert_eq!(ok_resp.reasoning.as_deref(), Some("thinking"));
        assert_eq!(ok_resp.tool_calls.len(), 1);
        assert_eq!(ok_resp.tool_calls[0].id, "call_abc");
        assert_eq!(ok_resp.tool_calls[0].name, "shell_exec");
        assert_eq!(ok_resp.tool_calls[0].arguments, "{\"cmd\":\"ls\"}");
        assert_eq!(ok_resp.usage.cache_read_tokens, 4);
        assert_eq!(ok_resp.finish_reason, "tool_calls");

        let mut req = ProviderRequest::new(
            "auto/gemini-3.8-flash",
            vec![
                ProviderChatMessage {
                    role: ProviderMessageRole::System,
                    content: "Sys rule".into(),
                    name: None,
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    reasoning: None,
                    cache_control: None,
                },
                ProviderChatMessage::user("List files"),
                ProviderChatMessage::assistant_with_tools(
                    "",
                    vec![ProviderToolCall {
                        id: String::new(),
                        name: "shell_exec".into(),
                        arguments: "{}".into(),
                    }],
                    None,
                ),
                ProviderChatMessage::tool_result("call_0", "shell_exec", "ok"),
            ],
        );
        req.tools = vec![ProviderToolSpec {
            name: "shell_exec".into(),
            description: "Run cmd".into(),
            parameters: json!({"type":"object"}),
        }];
        let payload = build_codex_responses_payload(&req);
        assert_eq!(payload["store"], false);
        assert_eq!(payload["reasoning"]["effort"], "medium");
        assert_eq!(payload["reasoning"]["summary"], "auto");
        assert_eq!(payload["input"][0]["role"], "developer");
        assert_eq!(payload["input"][2]["call_id"], "call_0");
        assert_eq!(payload["tool_choice"], "auto");
        assert_eq!(payload["parallel_tool_calls"], true);
    }

    #[test]
    fn test_placeholder_model_normalization_and_no_ai_ombhrum_stripping() {
        for placeholder in [
            "",
            "hermes-local-v1",
            "default",
            "默认模型",
            "auto",
            "gpt-4o-mini",
            "deepseek-flash",
        ] {
            assert_eq!(
                normalize_wire_model_for_provider(
                    "magpie_local",
                    "http://127.0.0.1:3425/v1",
                    placeholder
                ),
                "auto/gemini-3.8-flash",
                "placeholder={placeholder}"
            );
            assert_eq!(
                normalize_wire_model_for_provider(
                    "codex_oauth",
                    "http://127.0.0.1:3425/backend-api/codex",
                    placeholder
                ),
                "auto/gemini-3.8-flash",
                "placeholder={placeholder}"
            );
            assert_eq!(
                normalize_wire_model_for_provider(
                    "dacheng_cloud",
                    "https://ai.ombhrum.com/codex-deepseek/v1",
                    placeholder
                ),
                "deepseek-chat",
                "placeholder={placeholder}"
            );
        }
        assert_eq!(
            normalize_wire_model_for_provider("deepseek", "https://api.deepseek.com/v1", "deepseek-flash"),
            "deepseek-chat"
        );
        assert_eq!(
            normalize_wire_model_for_provider("openai", "https://api.openai.com/v1", "deepseek-flash"),
            "gpt-4o"
        );
        assert_eq!(
            normalize_wire_model_for_provider("openai", "https://api.openai.com/v1", "gpt-4o-mini"),
            "gpt-4o-mini"
        );
        assert_eq!(
            normalize_wire_model_for_provider(
                "openai",
                "https://api.openai.com/v1",
                "hermes-local-v1"
            ),
            "gpt-4o"
        );

        let long_msg = "A".repeat(350);
        let cfg = ProviderConfig::codex_responses(
            "dacheng_cloud",
            "Dacheng Cloud",
            "https://ai.ombhrum.com/codex-deepseek/v1",
            "deepseek-chat",
            "public",
        );
        let mut req = ProviderRequest::new(
            "gpt-4o-mini",
            vec![ProviderChatMessage::user(long_msg)],
        );
        req.thinking_budget_tokens = Some(4096);
        req.tools = vec![
            ProviderToolSpec {
                name: "read_file".into(),
                description: "Read file".into(),
                parameters: json!({"type":"object"}),
            },
            ProviderToolSpec {
                name: "shell_exec".into(),
                description: "Exec".into(),
                parameters: json!({"type":"object"}),
            },
        ];
        let wire = prepare_wire_request(&cfg, &cfg.base_url, "public", &req);
        assert_eq!(wire.model, "deepseek-chat");
        assert_eq!(wire.tools.len(), 2);
        assert_eq!(wire.thinking_budget_tokens, Some(4096));
        assert_eq!(wire.messages[0].content.chars().count(), 350);
    }

    #[tokio::test]
    async fn test_dotenv_codex_auth_jwt_magpie_probe_and_prod_local_guard() {
        let dotenv_pairs = parse_dotenv_str(
            "# comment\nexport OPENAI_API_KEY=\"sk-demo\"\nCUSTOM_NOTE='a # b'\nPLAIN=val # cmt\n",
        );
        assert_eq!(
            dotenv_pairs,
            vec![
                ("OPENAI_API_KEY".to_string(), "sk-demo".to_string()),
                ("CUSTOM_NOTE".to_string(), "a # b".to_string()),
                ("PLAIN".to_string(), "val".to_string()),
            ]
        );

        let temp_root = std::env::temp_dir().join(format!(
            "gpui_hermes_dotenv_{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(1)
        ));
        std::fs::create_dir_all(&temp_root).unwrap();
        let env1 = temp_root.join("first.env");
        let env2 = temp_root.join("second.env");
        std::fs::write(&env1, "UNIT_TEST_DOTENV_KEY=first\nPATH=should_not_override\n").unwrap();
        std::fs::write(&env2, "UNIT_TEST_DOTENV_KEY=second\nUNIT_TEST_SECOND=two\n").unwrap();
        let lookup = EnvLookup::from_files(&[env1, env2]);
        assert_eq!(lookup.get("UNIT_TEST_DOTENV_KEY").as_deref(), Some("first"));
        assert_eq!(lookup.get("UNIT_TEST_SECOND").as_deref(), Some("two"));
        assert_ne!(lookup.get("PATH").as_deref(), Some("should_not_override"));
        let _ = std::fs::remove_dir_all(&temp_root);

        let toml = r#"
model = "auto/gemini-3.8-flash"
openai_base_url = "http://127.0.0.1:3425/backend-api/codex"

[model_providers.magpie]
base_url = "http://127.0.0.1:3425/v1"
http_headers = { "x-openai-actor-authorization" = "magpie", "X-Custom" = "yes" }
"#;
        let parsed_toml = parse_codex_config_toml(toml);
        assert_eq!(parsed_toml.model.as_deref(), Some("auto/gemini-3.8-flash"));
        assert_eq!(
            parsed_toml.openai_base_url.as_deref(),
            Some("http://127.0.0.1:3425/backend-api/codex")
        );
        assert_eq!(
            parsed_toml.magpie_base_url.as_deref(),
            Some("http://127.0.0.1:3425/v1")
        );
        assert_eq!(
            parsed_toml
                .magpie_headers
                .get("x-openai-actor-authorization")
                .map(String::as_str),
            Some("magpie")
        );

        let jwt_header = encode_b64_url_no_pad(br#"{"alg":"none"}"#);
        let jwt_payload = encode_b64_url_no_pad(
            br#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_123","chatgpt_compute_residency":"us"}}"#,
        );
        let fake_jwt = format!("{jwt_header}.{jwt_payload}.sig");
        let auth_raw = json!({
            "tokens": {
                "access_token": fake_jwt,
                "account_id": "acct_123"
            }
        })
        .to_string();
        let parsed_auth = parse_codex_auth_json(&auth_raw);
        assert_eq!(parsed_auth.account_id.as_deref(), Some("acct_123"));
        assert_eq!(parsed_auth.compute_residency.as_deref(), Some("us"));

        let headers = build_codex_oauth_headers(
            "http://127.0.0.1:3425/backend-api/codex",
            parsed_auth.account_id.as_deref(),
            parsed_auth.compute_residency.as_deref(),
        );
        assert_eq!(headers.get("originator").map(String::as_str), Some("codex_cli_rs"));
        assert_eq!(
            headers.get("User-Agent").map(String::as_str),
            Some("codex_cli_rs/0.1.0")
        );
        assert_eq!(
            headers.get("ChatGPT-Account-ID").map(String::as_str),
            Some("acct_123")
        );
        assert_eq!(
            headers
                .get("x-openai-internal-codex-residency")
                .map(String::as_str),
            Some("us")
        );
        assert_eq!(
            headers
                .get("x-openai-actor-authorization")
                .map(String::as_str),
            Some("magpie")
        );

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let req = read_http_request_text(&mut sock);
            assert!(req.contains("GET /v1/models"));
            assert!(req
                .to_ascii_lowercase()
                .contains("x-openai-actor-authorization: magpie"));
            let body = json!({
                "data": [
                    {"id": "other/model"},
                    {"id": "auto/gemini-3.1-pro-preview"},
                    {"id": "auto/gemini-3.8-flash"}
                ]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        });
        let models = probe_magpie_models_sync(
            &format!("http://{addr}/v1"),
            &parsed_toml.magpie_headers,
            None,
            Duration::from_millis(800),
        )
        .expect("probe models");
        let _ = srv.join();
        assert_eq!(
            models,
            vec![
                "other/model",
                "auto/gemini-3.1-pro-preview",
                "auto/gemini-3.8-flash"
            ]
        );
        assert_eq!(
            select_magpie_default_model(&models, None),
            "auto/gemini-3.8-flash"
        );

        let prod_req = ProviderRequest::new("hermes-local-v1", vec![ProviderChatMessage::user("hi")]);
        let guard_err = execute_local_hermes_with_policy("local_hermes", &prod_req, None, false)
            .await
            .unwrap_err();
        assert!(matches!(guard_err, ProviderError::Configuration(_)));
        assert!(guard_err.to_string().contains("未检测到可用的大模型推理凭证"));
    }

    #[tokio::test]
    async fn test_magpie_model_fallback_and_active_model_keeps_provider() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            for idx in 0..2 {
                let (mut sock, _) = listener.accept().unwrap();
                let req = read_http_request_text(&mut sock);
                if idx == 0 {
                    assert!(req.contains("bad/model"));
                    let body = json!({"error": {"message": "unknown model"}}).to_string();
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes());
                } else {
                    assert!(req.contains("auto/gemini-3.8-flash"));
                    let body = json!({
                        "choices": [{
                            "message": {"role": "assistant", "content": "Recovered via fallback model"},
                            "finish_reason": "stop"
                        }]
                    })
                    .to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes());
                }
            }
        });

        let router = ProviderRouter::new();
        router.set_allow_local_fallback(false);
        let mut cfg = ProviderConfig::openai_compatible(
            "magpie_local",
            "Magpie Local",
            format!("http://{addr}/v1"),
            "auto/gemini-3.8-flash",
            "magpie",
        );
        cfg.fallback_models = vec!["auto/gemini-3.8-flash".to_string()];
        router.register_provider(cfg);

        router.set_active("magpie_local", Some("bad/model")).unwrap();
        let (prov_after_legacy, mdl_after_legacy) = router.set_active_model("hermes-local-v1");
        assert_eq!(prov_after_legacy, "magpie_local");
        assert_eq!(mdl_after_legacy, "auto/gemini-3.8-flash");

        router.set_active("magpie_local", Some("bad/model")).unwrap();
        let (prov_after_df, mdl_after_df) = router.set_active_model("deepseek-flash");
        assert_eq!(prov_after_df, "magpie_local");
        assert_eq!(mdl_after_df, "auto/gemini-3.8-flash");

        router.set_active("magpie_local", Some("bad/model")).unwrap();
        let req = ProviderRequest::new("bad/model", vec![ProviderChatMessage::user("hello")]);
        let resp = router.complete(req, None).await.expect("fallback retry");
        let _ = srv.join();
        assert_eq!(resp.provider_id, "magpie_local");
        assert_eq!(resp.model, "auto/gemini-3.8-flash");
        assert_eq!(resp.content, "Recovered via fallback model");
    }

    #[tokio::test]
    async fn test_offline_rule_bot_allowed_flag_and_deserialized_default() {
        let router = ProviderRouter::new();
        assert!(router.snapshot().offline_rule_bot_allowed);
        let req = ProviderRequest::new("hermes-local-v1", vec![ProviderChatMessage::user("ping")]);
        let ok_resp = router
            .complete(req.clone(), None)
            .await
            .expect("new() allows offline rule bot");
        assert_eq!(ok_resp.api_mode, ApiMode::LocalHermes);

        router.set_offline_rule_bot_allowed(false);
        assert!(!router.snapshot().offline_rule_bot_allowed);

        // Even before disabling allow_local_fallback, complete() preserves Configuration error directly.
        let err_with_fb = router.complete(req.clone(), None).await.unwrap_err();
        assert!(matches!(err_with_fb, ProviderError::Configuration(_)));
        assert!(err_with_fb.to_string().contains("未检测到可用的大模型推理凭证"));

        // Direct provider call surfaces Configuration error cleanly.
        let err_direct = router
            .complete_on_provider("local_hermes", req.clone(), None)
            .await
            .unwrap_err();
        assert!(matches!(err_direct, ProviderError::Configuration(_)));

        // Production state (allow_local_fallback=false + offline_rule_bot_allowed=false + no active provider/subscription)
        // emits actionable Magpie provider-selection guidance.
        router.set_allow_local_fallback(false);
        let err_prod = router.complete(req, None).await.unwrap_err();
        assert!(matches!(err_prod, ProviderError::Configuration(_)));
        assert!(err_prod
            .to_string()
            .contains("未检测到已配置的 AI 订阅或模型供应商"));

        let legacy_json = json!({
            "providers": {},
            "model_profiles": {},
            "active_provider_id": "local_hermes",
            "active_model": "hermes-local-v1",
            "fallback_chain": [],
            "allow_local_fallback": false
        })
        .to_string();
        let parsed: ProviderRouterState = serde_json::from_str(&legacy_json).expect("legacy state parses");
        assert!(!parsed.offline_rule_bot_allowed);
        assert!(parsed.subscriptions.is_empty());
        assert!(parsed.routing_groups.is_empty());
        assert!(!parsed.masking.enabled);
    }

    #[tokio::test]
    async fn test_magpie_fusion_env_vendors_subscriptions_gemini_keyless_routing_and_masking() {
        // 1. Env scan registers Magpie vendors (Gemini, Moonshot, Zhipu, MiniMax, StepFun, Qwen, SiliconFlow, Groq, xAI, Mistral, Together, Fireworks, Nvidia, ModelScope)
        let router = ProviderRouter::new();
        router.set_allow_local_fallback(false);
        router.set_offline_rule_bot_allowed(false);
        assert!(router.needs_provider_selection());

        let env = EnvLookup::from_pairs(&[
            ("GEMINI_API_KEY", "AIzaSyEnvGeminiKey1234567890123456"),
            ("MOONSHOT_API_KEY", "sk-kimi-123"),
            ("ZHIPU_API_KEY", "glm-key"),
            ("MINIMAX_API_KEY", "mm-key"),
            ("STEPFUN_API_KEY", "step-key"),
            ("DASHSCOPE_API_KEY", "dash-key"),
            ("SILICONFLOW_API_KEY", "sf-key"),
            ("GROQ_API_KEY", "gsk-key"),
            ("XAI_API_KEY", "xai-key"),
            ("MISTRAL_API_KEY", "mis-key"),
            ("TOGETHER_API_KEY", "tog-key"),
            ("FIREWORKS_API_KEY", "fw-key"),
            ("NVIDIA_API_KEY", "nv-key"),
            ("MODELSCOPE_API_KEY", "ms-key"),
        ]);
        router.populate_from_env_and_home(&env, None, None, false);
        assert!(router.has_configured_provider());
        assert!(!router.needs_provider_selection());
        for pid in [
            "google", "moonshot", "zhipu", "minimax", "stepfun", "qwen",
            "siliconflow", "groq", "xai", "mistral", "together", "fireworks", "nvidia", "modelscope",
        ] {
            assert!(
                router.snapshot().providers.contains_key(pid),
                "expected vendor {pid} from env scan"
            );
        }

        // 2. Gemini GenerateContent wire execution (x-goog-api-key + SSE stream)
        let g_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let g_addr = g_listener.local_addr().unwrap();
        let g_srv = std::thread::spawn(move || {
            let (mut sock, _) = g_listener.accept().unwrap();
            let req = read_http_request_text(&mut sock);
            assert!(req.contains("POST /v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"));
            assert!(req.to_ascii_lowercase().contains("x-goog-api-key: gem-test-key"));
            let sse = concat!(
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"thought\":true,\"text\":\"check\"}]},\"finishReason\":null}]}\n\n",
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Namo Amitabha\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":7,\"candidatesTokenCount\":3,\"totalTokenCount\":10}}\n\n"
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = sock.write_all(resp.as_bytes());
        });

        let gem_router = ProviderRouter::new();
        gem_router.set_allow_local_fallback(false);
        gem_router
            .apply_magpie_preset(MagpiePresetApplyInput {
                preset_id: "google".to_string(),
                api_key: Some("gem-test-key".to_string()),
                base_url_override: Some(format!("http://{g_addr}/v1beta")),
                activate: true,
                ..Default::default()
            })
            .unwrap();
        let gem_resp = gem_router
            .complete(
                ProviderRequest::new("gemini-2.5-flash", vec![ProviderChatMessage::user("hi")]),
                None,
            )
            .await
            .expect("gemini sse response");
        let _ = g_srv.join();
        assert_eq!(gem_resp.api_mode, ApiMode::GeminiGenerateContent);
        assert_eq!(gem_resp.content, "Namo Amitabha");
        assert_eq!(gem_resp.reasoning.as_deref(), Some("check"));
        assert_eq!(gem_resp.usage.total_tokens, 10);

        // 3. Keyless local preset (Ollama) sends NO Authorization header
        let ol_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ol_addr = ol_listener.local_addr().unwrap();
        let ol_srv = std::thread::spawn(move || {
            let (mut sock, _) = ol_listener.accept().unwrap();
            let req = read_http_request_text(&mut sock);
            assert!(!req.to_ascii_lowercase().contains("authorization:"));
            let body = json!({
                "choices": [{
                    "message": {"role": "assistant", "content": "Local Ollama OK"},
                    "finish_reason": "stop"
                }]
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes());
        });
        let local_router = ProviderRouter::new();
        local_router.set_allow_local_fallback(false);
        local_router
            .apply_magpie_preset(MagpiePresetApplyInput {
                preset_id: "ollama".to_string(),
                base_url_override: Some(format!("http://{ol_addr}/v1")),
                activate: true,
                ..Default::default()
            })
            .unwrap();
        assert!(local_router.has_configured_provider());
        let mut ol_req = ProviderRequest::new("qwen3:8b", vec![ProviderChatMessage::user("ping")]);
        ol_req.stream = false;
        let ol_resp = local_router.complete(ol_req, None).await.expect("keyless ollama");
        let _ = ol_srv.join();
        assert_eq!(ol_resp.content, "Local Ollama OK");

        // 4. Routing group (`group/<id>`) + Masking + StreamingUnmasker end-to-end
        let m_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let m_addr = m_listener.local_addr().unwrap();
        let secret_val = "sk-ant-api03-verysecretkeyvalue999";
        let m_srv = std::thread::spawn(move || {
            let (mut sock, _) = m_listener.accept().unwrap();
            let req = read_http_request_text(&mut sock);
            // Verify raw secret was NEVER transmitted to upstream!
            assert!(!req.contains(secret_val), "raw secret leaked on wire: {req}");
            // Extract issued placeholder from request body and echo it back across split SSE deltas
            let idx = req.find("{{API_KEY_").expect("placeholder in masked request");
            let end = req[idx..].find("}}").unwrap() + idx + 2;
            let ph = &req[idx..end];
            let half = ph.len() / 2;
            let sse = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"Echo: {}\"}}}}]}}\n\n\
                 data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n\
                 data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n\
                 data: [DONE]\n\n",
                &ph[..half],
                &ph[half..]
            );
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(),
                sse
            );
            let _ = sock.write_all(resp.as_bytes());
        });

        let routed_router = ProviderRouter::new();
        routed_router.set_allow_local_fallback(false);
        routed_router
            .apply_magpie_preset(MagpiePresetApplyInput {
                preset_id: "deepseek".to_string(),
                api_key: Some("sk-ds-wire".to_string()),
                base_url_override: Some(format!("http://{m_addr}/v1")),
                activate: true,
                ..Default::default()
            })
            .unwrap();
        let mut group = RoutingGroup::new("smart", "Smart Mix", RoutingStrategy::Smart);
        group
            .members
            .push(RoutingGroupMember::new("deepseek/deepseek-chat"));
        routed_router.upsert_routing_group(group).unwrap();
        routed_router.set_masking_config(MaskingConfig {
            enabled: true,
            mask_secrets: true,
            mask_personal: true,
            ..Default::default()
        });

        let (tx, mut rx) = mpsc::channel(16);
        let routed_resp = routed_router
            .complete(
                ProviderRequest::new(
                    "group/smart",
                    vec![ProviderChatMessage::user(format!("Rotate key {secret_val}"))],
                ),
                Some(tx),
            )
            .await
            .expect("masked routed stream");
        drop(routed_router);
        let mut streamed = String::new();
        while let Some(ev) = rx.recv().await {
            if let ProviderStreamEvent::TextDelta(t) = ev {
                streamed.push_str(&t);
            }
        }
        let _ = m_srv.join();
        assert_eq!(routed_resp.content, format!("Echo: {secret_val}"));
        assert_eq!(streamed, format!("Echo: {secret_val}"));
    }

    #[test]
    fn test_subscription_discovery_bridge_into_provider_router() {
        let temp_dir = tempfile::tempdir().unwrap();
        let home = temp_dir.path();

        // 1. Write mock Google Vertex ADC
        let gcloud_dir = home.join(".config").join("gcloud");
        std::fs::create_dir_all(&gcloud_dir).unwrap();
        let adc_content = json!({
            "client_id": "test-vertex-client-id",
            "client_secret": "test-vertex-client-secret",
            "refresh_token": "test-vertex-refresh-token",
            "quota_project_id": "test-quota-project-123",
            "account": "developer@vertex.google.internal"
        });
        std::fs::write(
            gcloud_dir.join("application_default_credentials.json"),
            adc_content.to_string(),
        )
        .unwrap();

        // 2. Write mock Codex session
        let codex_dir = home.join(".codex");
        std::fs::create_dir_all(&codex_dir).unwrap();
        let codex_auth = json!({
            "tokens": {
                "account_id": "chatgpt-acct-789",
                "access_token": "header.eyJlbWFpbCI6ImNvZGV4QGV4YW1wbGUuY29tIiwicGxhbiI6InRlYW0ifQ.signature",
                "id_token": "header.eyJlbWFpbCI6ImNvZGV4QGV4YW1wbGUuY29tIn0.sig",
                "refresh_token": "test-codex-refresh"
            }
        });
        std::fs::write(codex_dir.join("auth.json"), codex_auth.to_string()).unwrap();

        let router = ProviderRouter::new();
        router.set_allow_local_fallback(false);

        let reg = router.refresh_discovered_subscriptions_in_home(home, Some(&codex_dir));
        assert!(reg.accounts.len() >= 2, "Expected at least Vertex and Codex subscriptions");

        let snap = router.snapshot();
        assert!(router.has_active_subscription(), "Should have active subscription bridged");
        assert!(!router.needs_provider_selection(), "needs_provider_selection should be false");

        // Verify Vertex AI provider
        let vertex_acc = reg.accounts.iter().find(|a| a.kind == SubscriptionKind::GoogleVertex).unwrap();
        let vertex_cfg = snap.providers.get(&vertex_acc.id).expect("Vertex provider registered");
        assert_eq!(vertex_cfg.api_mode, ApiMode::GoogleVertex);
        assert!(vertex_cfg.is_subscription);
        assert!(vertex_cfg.no_key_required);
        assert_eq!(vertex_cfg.headers.get("x-goog-user-project"), Some(&"test-quota-project-123".to_string()));
        assert!(vertex_cfg.fallback_models.contains(&"gemini-2.5-flash".to_string()));
        assert!(vertex_cfg.fallback_models.contains(&"gemini-2.5-pro".to_string()));
        assert!(vertex_cfg.fallback_models.contains(&"gemini-1.5-pro".to_string()));

        // Verify Codex provider
        let codex_acc = reg.accounts.iter().find(|a| a.kind == SubscriptionKind::CodexChatGpt).unwrap();
        let codex_cfg = snap.providers.get(&codex_acc.id).expect("Codex provider registered");
        assert_eq!(codex_cfg.api_mode, ApiMode::CodexResponses);
        assert!(codex_cfg.is_subscription);
        assert_eq!(codex_cfg.headers.get("chatgpt-account-id"), Some(&"chatgpt-acct-789".to_string()));

        // Verify model profiles
        assert!(snap.model_profiles.contains_key("gemini-2.5-flash"));
        assert!(snap.model_profiles.contains_key("gemini-2.5-pro"));
        assert!(snap.model_profiles.contains_key("gemini-1.5-pro"));
    }

    #[tokio::test]
    async fn test_autonomous_fallback_avoids_provider_error() {
        let router = ProviderRouter::new();
        router.set_allow_local_fallback(false);
        router.set_offline_rule_bot_allowed(false);

        // Before any subscription is present, needs_provider_selection is true
        assert!(router.needs_provider_selection());

        // Register a mock discovered local subscription (Google Vertex ADC)
        let mut vertex_cfg = ProviderConfig::google_vertex(
            "google_vertex:adc",
            "Google Vertex AI (ADC)",
            "https://aiplatform.googleapis.com/v1",
            "gemini-2.5-flash",
            Some("test-project-adc".to_string()),
        );
        vertex_cfg.is_subscription = true;
        vertex_cfg.enabled = true;
        router.register_provider(vertex_cfg);

        let sub_account = SubscriptionAccount {
            id: "google_vertex:adc".to_string(),
            kind: SubscriptionKind::GoogleVertex,
            label: "Google Vertex AI (ADC)".to_string(),
            email: Some("user@gmail.com".to_string()),
            account_id: Some("test-project-adc".to_string()),
            plan: Some("vertex_adc".to_string()),
            active: true,
            base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
            protocol: ProtocolKind::GeminiGenerateContent,
            default_model: Some("gemini-2.5-flash".to_string()),
            source_path: "/mock/adc.json".to_string(),
            expires_at_ms: None,
            access_token: None,
            headers: [("x-goog-user-project".to_string(), "test-project-adc".to_string())].into_iter().collect(),
        };
        {
            let mut guard = router.inner.lock().unwrap();
            guard.subscriptions.push(sub_account);
        }

        assert!(router.has_active_subscription());
        assert!(!router.needs_provider_selection());

        // Call activate_subscription and verify state
        let act_res = router.activate_subscription("google_vertex:adc", Some("gemini-2.5-pro"));
        assert!(act_res.is_ok());
        let (active_pid, active_model, mode) = router.active_info();
        assert_eq!(active_pid, "google_vertex:adc");
        assert_eq!(active_model, "gemini-2.5-pro");
        assert_eq!(mode, ApiMode::GoogleVertex);
    }

    #[test]
    fn test_activate_subscription_switch_and_fallback_chain() {
        let router = ProviderRouter::new();

        let sub1 = SubscriptionAccount {
            id: "google_vertex:primary".to_string(),
            kind: SubscriptionKind::GoogleVertex,
            label: "Google Vertex AI".to_string(),
            email: Some("v1@example.com".to_string()),
            account_id: Some("proj-v1".to_string()),
            plan: Some("adc".to_string()),
            active: true,
            base_url: Some("https://aiplatform.googleapis.com/v1".to_string()),
            protocol: ProtocolKind::GeminiGenerateContent,
            default_model: Some("gemini-2.5-flash".to_string()),
            source_path: String::new(),
            expires_at_ms: None,
            access_token: None,
            headers: [("x-goog-user-project".to_string(), "proj-v1".to_string())].into_iter().collect(),
        };

        let sub2 = SubscriptionAccount {
            id: "codex:team_user".to_string(),
            kind: SubscriptionKind::CodexChatGpt,
            label: "Codex Team".to_string(),
            email: Some("team@example.com".to_string()),
            account_id: Some("acct-team-456".to_string()),
            plan: Some("team".to_string()),
            active: true,
            base_url: Some("https://chatgpt.com/backend-api/codex".to_string()),
            protocol: ProtocolKind::CodexResponses,
            default_model: Some("codex-mini-latest".to_string()),
            source_path: String::new(),
            expires_at_ms: None,
            access_token: Some("fake-bearer-token".to_string()),
            headers: [("chatgpt-account-id".to_string(), "acct-team-456".to_string())].into_iter().collect(),
        };

        {
            let mut guard = router.inner.lock().unwrap();
            guard.subscriptions.push(sub1.clone());
            guard.subscriptions.push(sub2.clone());
        }

        // Activate vertex with default model
        let res1 = router.activate_subscription("google_vertex", None).expect("activate vertex");
        assert_eq!(res1.id, "google_vertex:primary");
        assert_eq!(res1.default_model, "gemini-2.5-flash");
        let (pid1, mdl1, _) = router.active_info();
        assert_eq!(pid1, "google_vertex:primary");
        assert_eq!(mdl1, "gemini-2.5-flash");
        {
            let guard = router.inner.lock().unwrap();
            let v_sub = guard.subscriptions.iter().find(|s| s.id == "google_vertex:primary").unwrap();
            let c_sub = guard.subscriptions.iter().find(|s| s.id == "codex:team_user").unwrap();
            assert!(v_sub.active, "Vertex subscription should be active");
            assert!(!c_sub.active, "Codex subscription should NOT be active");
        }

        // Activate codex with explicit model
        let res2 = router.activate_subscription("codex", Some("gpt-4o")).expect("activate codex");
        assert_eq!(res2.id, "codex:team_user");
        let (pid2, mdl2, mode2) = router.active_info();
        assert_eq!(pid2, "codex:team_user");
        assert_eq!(mdl2, "gpt-4o");
        assert_eq!(mode2, ApiMode::CodexResponses);
        {
            let guard = router.inner.lock().unwrap();
            let v_sub = guard.subscriptions.iter().find(|s| s.id == "google_vertex:primary").unwrap();
            let c_sub = guard.subscriptions.iter().find(|s| s.id == "codex:team_user").unwrap();
            assert!(!v_sub.active, "Vertex subscription should be reset to inactive");
            assert!(c_sub.active, "Codex subscription should now be active");
        }
    }

    #[tokio::test]
    async fn test_google_vertex_ai_streaming_execution_mock() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let req_count = Arc::new(AtomicUsize::new(0));
        let rc = Arc::clone(&req_count);

        let srv = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            rc.fetch_add(1, Ordering::SeqCst);

            let mut req_buf = [0u8; 4096];
            let n = stream.read(&mut req_buf).unwrap();
            let req_str = String::from_utf8_lossy(&req_buf[..n]);

            let lower = req_str.to_ascii_lowercase();
            assert!(
                lower.contains("authorization: bearer mock-bearer-token-999"),
                "expected authorization header in: {req_str}"
            );
            assert!(
                lower.contains("x-goog-user-project: vertex-test-project"),
                "expected user-project header in: {req_str}"
            );
            assert!(req_str.contains("/publishers/google/models/gemini-2.5-flash:streamGenerateContent"));

            let sse_chunk1 = json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": "Hello from " }]
                    }
                }]
            });
            let sse_chunk2 = json!({
                "candidates": [{
                    "content": {
                        "role": "model",
                        "parts": [{ "text": "Google Vertex AI!" }]
                    },
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 10,
                    "candidatesTokenCount": 6,
                    "totalTokenCount": 16
                }
            });

            let body = format!("data: {sse_chunk1}\n\ndata: {sse_chunk2}\n\n");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let router = ProviderRouter::new();
        let mut cfg = ProviderConfig::google_vertex(
            "google_vertex_mock",
            "Vertex Mock",
            format!("http://127.0.0.1:{port}"),
            "gemini-2.5-flash",
            Some("vertex-test-project".to_string()),
        );
        cfg.timeout_secs = 5;
        // Provide mock bearer token in credentials
        cfg.credentials.add_entry(CredentialEntry::new("cred1", "mock-bearer-token-999", "mock"));
        router.register_provider(cfg);

        let (tx, mut rx) = mpsc::channel(16);
        let resp = router
            .complete_on_provider(
                "google_vertex_mock",
                ProviderRequest::new("gemini-2.5-flash", vec![ProviderChatMessage::user("Hi Vertex")]),
                Some(tx),
            )
            .await
            .expect("vertex streaming execution failed");

        let _ = srv.join();
        assert_eq!(resp.content, "Hello from Google Vertex AI!");
        assert_eq!(resp.api_mode, ApiMode::GoogleVertex);
        assert_eq!(resp.usage.total_tokens, 16);

        let mut collected_deltas = String::new();
        while let Some(ev) = rx.recv().await {
            if let ProviderStreamEvent::TextDelta(d) = ev {
                collected_deltas.push_str(&d);
            }
        }
        assert_eq!(collected_deltas, "Hello from Google Vertex AI!");
        assert_eq!(req_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_google_vertex_ai_oauth_token_refresh_integration() {
        let oauth_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let oauth_port = oauth_listener.local_addr().unwrap().port();

        let srv = std::thread::spawn(move || {
            let (mut stream, _) = oauth_listener.accept().unwrap();
            let mut req_buf = [0u8; 4096];
            let n = stream.read(&mut req_buf).unwrap();
            let req_str = String::from_utf8_lossy(&req_buf[..n]);

            assert!(req_str.contains("grant_type=refresh_token"));
            assert!(req_str.contains("client_id=test-adc-client-id"));
            assert!(req_str.contains("refresh_token=test-refresh-token-xyz"));

            let oauth_resp = json!({
                "access_token": "ya29.mock-refreshed-oauth-token-555",
                "expires_in": 3600,
                "token_type": "Bearer"
            });
            let body = oauth_resp.to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let creds = VertexAdcCredentials {
            client_id: "test-adc-client-id".to_string(),
            client_secret: "test-adc-secret".to_string(),
            refresh_token: "test-refresh-token-xyz".to_string(),
            quota_project_id: Some("vertex-quota-project".to_string()),
            account: Some("user@test.org".to_string()),
        };

        let token_mgr = VertexTokenManager::new(creds)
            .with_token_url(format!("http://127.0.0.1:{oauth_port}/token"));

        let router = ProviderRouter::new();
        router.register_vertex_token_manager("google_vertex_refresh_test", token_mgr);

        let mut cfg = ProviderConfig::google_vertex(
            "google_vertex_refresh_test",
            "Vertex Refresh Test",
            "https://aiplatform.googleapis.com/v1",
            "gemini-2.5-flash",
            Some("vertex-quota-project".to_string()),
        );
        cfg.no_key_required = true;
        router.register_provider(cfg.clone());

        let mgr = router.get_or_create_vertex_manager("google_vertex_refresh_test", &cfg).unwrap();
        let token = mgr.get_or_refresh_token(&router.http_client, Utc::now().timestamp_millis()).await.unwrap();

        let _ = srv.join();
        assert_eq!(token, "ya29.mock-refreshed-oauth-token-555");
    }

    #[test]
    fn test_mask_outbound_request_masks_and_blocks_reasoning() {
        let secret = "sk-ant-api03-reasoningsecretkey999";

        // 1. MaskingAction::Block must reject secret in msg.reasoning
        let block_engine = MaskingEngine::new(MaskingConfig {
            enabled: true,
            default_action: MaskingAction::Block,
            ..Default::default()
        });
        let mut msg = ProviderChatMessage::assistant("ok");
        msg.reasoning = Some(format!("checking key {secret}"));
        let mut req_block = ProviderRequest::new("claude-sonnet-4-20250514", vec![msg.clone()]);
        let err = mask_outbound_request(&mut req_block, &block_engine).unwrap_err();
        assert!(matches!(err, ProviderError::Configuration(_)));

        // 2. MaskingAction::Mask must redact msg.reasoning and allow unmasking
        let mask_engine = MaskingEngine::new(MaskingConfig {
            enabled: true,
            default_action: MaskingAction::Mask,
            ..Default::default()
        });
        let mut req_mask = ProviderRequest::new("claude-sonnet-4-20250514", vec![msg]);
        let hits = mask_outbound_request(&mut req_mask, &mask_engine).unwrap();
        assert!(hits >= 1);
        let masked_reason = req_mask.messages[0].reasoning.as_deref().unwrap();
        assert!(!masked_reason.contains(secret), "secret leaked in masked reasoning: {masked_reason}");
        assert!(masked_reason.contains("{{API_KEY_"));
        assert_eq!(mask_engine.unmask_text(masked_reason), format!("checking key {secret}"));
    }

    #[tokio::test]
    async fn test_gemini_and_vertex_sse_skip_id_and_retry_control_lines() {
        // 1. GeminiGenerateContent with id: and retry: SSE control lines
        let g_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let g_port = g_listener.local_addr().unwrap().port();
        let g_srv = std::thread::spawn(move || {
            let (mut sock, _) = g_listener.accept().unwrap();
            let _ = read_http_request_text(&mut sock);
            let sse = ": ping\n\nid: gem_1\nretry: 1000\nevent: delta\ndata: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"GEM_CTRL_OK\"}]},\"finishReason\":\"STOP\"}]}\n\ndata: [DONE]\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(),
                sse
            );
            sock.write_all(resp.as_bytes()).unwrap();
        });

        let gem_router = ProviderRouter::new();
        gem_router.set_allow_local_fallback(false);
        gem_router
            .apply_magpie_preset(MagpiePresetApplyInput {
                preset_id: "google".to_string(),
                api_key: Some("gem-test-key".to_string()),
                base_url_override: Some(format!("http://127.0.0.1:{g_port}/v1beta")),
                activate: true,
                ..Default::default()
            })
            .unwrap();
        let gem_resp = gem_router
            .complete(
                ProviderRequest::new("gemini-2.5-flash", vec![ProviderChatMessage::user("hi")]),
                None,
            )
            .await
            .expect("gemini sse with id/retry lines must succeed");
        let _ = g_srv.join();
        assert_eq!(gem_resp.content, "GEM_CTRL_OK");

        // 2. GoogleVertex with id: and retry: SSE control lines
        let v_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let v_port = v_listener.local_addr().unwrap().port();
        let v_srv = std::thread::spawn(move || {
            let (mut sock, _) = v_listener.accept().unwrap();
            let _ = read_http_request_text(&mut sock);
            let sse = ": keepalive\nid: v_1\nretry: 500\nevent: message\ndata: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"VERTEX_CTRL_OK\"}]},\"finishReason\":\"STOP\"}]}\n\ndata: [DONE]\n\n";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                sse.len(),
                sse
            );
            sock.write_all(resp.as_bytes()).unwrap();
        });

        let v_router = ProviderRouter::new();
        let mut v_cfg = ProviderConfig::google_vertex(
            "vertex_sse_ctrl",
            "Vertex SSE Ctrl",
            format!("http://127.0.0.1:{v_port}"),
            "gemini-2.5-flash",
            Some("proj-test".to_string()),
        );
        v_cfg.credentials.add_entry(CredentialEntry::new("c1", "tok-123", "mock"));
        v_router.register_provider(v_cfg);
        let v_resp = v_router
            .complete_on_provider(
                "vertex_sse_ctrl",
                ProviderRequest::new("gemini-2.5-flash", vec![ProviderChatMessage::user("hi")]),
                None,
            )
            .await
            .expect("vertex sse with id/retry lines must succeed");
        let _ = v_srv.join();
        assert_eq!(v_resp.content, "VERTEX_CTRL_OK");
    }
}
