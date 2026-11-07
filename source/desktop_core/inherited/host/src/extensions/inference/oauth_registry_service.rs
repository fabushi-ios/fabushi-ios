//! OpenClaw LLM OAuth Provider Registry & Token Refresh
//!
//! Mirrors:
//! - `src/llm/oauth.ts`
//! - `src/llm/utils/oauth/index.ts`
//! - `src/llm/utils/oauth/types.ts`
//! - `src/llm/utils/oauth/anthropic.ts`
//! - `src/llm/utils/oauth/openai-chatgpt.ts`
//! - `src/llm/utils/oauth/abort.ts`

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

pub const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
pub const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const ANTHROPIC_DEFAULT_CALLBACK_HOST: &str = "127.0.0.1";
pub const ANTHROPIC_CALLBACK_PORT: u16 = 53692;

pub fn validate_anthropic_callback_host(host: &str) -> Result<(), &'static str> {
    match host.trim() {
        "localhost" | "127.0.0.1" | "::1" => Ok(()),
        _ => Err("Anthropic OAuth callback host must be localhost, 127.0.0.1, or ::1"),
    }
}

pub fn generate_pkce_challenge_from_verifier(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    URL_SAFE_NO_PAD.encode(hash)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OAuthProviderId {
    Anthropic,
    OpenAICodex,
}

impl OAuthProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            OAuthProviderId::Anthropic => "anthropic",
            OAuthProviderId::OpenAICodex => "openai",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
    pub token_type: String,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthError {
    NotConfigured,
    TokenExpired { expires_at_ms: u64, now_ms: u64 },
    RefreshFailed(String),
}

#[derive(Debug, Default)]
pub struct OAuthProviderRegistry {
    credentials: HashMap<OAuthProviderId, OAuthCredentials>,
}

impl OAuthProviderRegistry {
    pub fn new() -> Self {
        Self {
            credentials: HashMap::new(),
        }
    }

    pub fn store_credentials(&mut self, provider: OAuthProviderId, creds: OAuthCredentials) {
        self.credentials.insert(provider, creds);
    }

    pub fn get_credentials(&self, provider: &OAuthProviderId) -> Option<&OAuthCredentials> {
        self.credentials.get(provider)
    }

    pub fn is_token_expired(&self, provider: &OAuthProviderId, now_ms: u64) -> bool {
        match self.credentials.get(provider) {
            Some(creds) => now_ms >= creds.expires_at_ms,
            None => true,
        }
    }

    pub fn get_api_key(&self, provider: &OAuthProviderId, now_ms: u64) -> Result<String, OAuthError> {
        let creds = self.credentials.get(provider).ok_or(OAuthError::NotConfigured)?;
        if now_ms >= creds.expires_at_ms {
            return Err(OAuthError::TokenExpired {
                expires_at_ms: creds.expires_at_ms,
                now_ms,
            });
        }
        Ok(creds.access_token.clone())
    }
}
