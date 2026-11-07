//! OpenClaw Secret Redaction, Token Masking, and PEM Sanitization
//!
//! Mirrors:
//! - `src/logging/redact.ts`
//! - `src/logging/redact-mask.ts`
//! - `src/logging/redact-bounded.ts`
//! - `src/logging/redact-json.ts`
//! - `src/logging/redact-linear-matchers.ts`

use regex::Regex;

/// Masks secret tokens keeping outer chars if long enough
pub fn mask_token(token: &str) -> String {
    let t = token.trim();
    let len = t.chars().count();
    if len <= 8 {
        "***".to_string()
    } else {
        let first_4: String = t.chars().take(4).collect();
        let last_4: String = t.chars().skip(len - 4).collect();
        format!("{}...{}", first_4, last_4)
    }
}

pub fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("secret")
        || lower.contains("key")
        || lower.contains("password")
        || lower.contains("auth")
        || lower.contains("credential")
        || lower.contains("session")
}

/// Redacts sensitive query parameters from URLs
pub fn redact_sensitive_query_params(url_str: &str) -> String {
    if !url_str.contains('?') {
        return url_str.to_string();
    }

    let parts: Vec<&str> = url_str.splitn(2, '?').collect();
    let base = parts[0];
    let query = parts[1];

    let params: Vec<String> = query
        .split('&')
        .map(|param| {
            if let Some(idx) = param.find('=') {
                let (k, _) = param.split_at(idx);
                if is_sensitive_key(k) {
                    format!("{}=[REDACTED]", k)
                } else {
                    param.to_string()
                }
            } else if is_sensitive_key(param) {
                format!("{}=[REDACTED]", param)
            } else {
                param.to_string()
            }
        })
        .collect();

    format!("{}?{}", base, params.join("&"))
}

/// Redacts PEM private key blocks
pub fn redact_pem_blocks(text: &str) -> String {
    let re = Regex::new(r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----").unwrap();
    re.replace_all(text, "[REDACTED_PRIVATE_KEY]").into_owned()
}

/// Redacts Bearer or Basic authorization tokens
pub fn redact_auth_header(header_value: &str) -> String {
    let trimmed = header_value.trim();
    if let Some(rest) = trimmed.strip_prefix("Bearer ") {
        format!("Bearer {}", mask_token(rest))
    } else if let Some(rest) = trimmed.strip_prefix("Basic ") {
        format!("Basic {}", mask_token(rest))
    } else if is_sensitive_key(trimmed) {
        "[REDACTED]".to_string()
    } else {
        mask_token(trimmed)
    }
}
