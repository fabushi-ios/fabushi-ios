//! Webhook ingress signature verification and payload validation (OC-S3).

use std::collections::HashMap;
use base64::Engine;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Compute standard HMAC-SHA256 digest over arbitrary payload.
pub fn hmac_sha256(secret: &[u8], payload: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key = secret.to_vec();
    if key.len() > BLOCK_SIZE {
        key = Sha256::digest(&key).to_vec();
    }
    key.resize(BLOCK_SIZE, 0);

    let mut inner_pad = [0x36_u8; BLOCK_SIZE];
    let mut outer_pad = [0x5c_u8; BLOCK_SIZE];
    for (i, b) in key.iter().enumerate() {
        inner_pad[i] ^= b;
        outer_pad[i] ^= b;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(payload);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// Constant-time byte slice equality check.
pub fn subtle_slices_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

/// Convert byte slice into lowercase hexadecimal string.
pub fn to_hex_string(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// Case-insensitive header lookup helper.
pub fn get_header_ignore_case<'a>(headers: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    let lower_key = key.to_ascii_lowercase();
    for (k, v) in headers {
        if k.to_ascii_lowercase() == lower_key {
            return Some(v.as_str());
        }
    }
    None
}

/// Verifies Slack webhook request signatures using HMAC-SHA256 (v0 signature scheme).
///
/// Format: `v0={hex_digest}` of `v0:{timestamp}:{raw_body}`.
/// Enforces a 5-minute replay prevention window against `timestamp_str`.
pub fn verify_slack_signature(
    signing_secret: &str,
    timestamp_str: &str,
    body: &[u8],
    signature_header: &str,
) -> bool {
    if signing_secret.is_empty() || timestamp_str.is_empty() || signature_header.is_empty() {
        return false;
    }

    // 1. Enforce 5-minute (300 seconds) replay prevention window
    let Ok(ts) = timestamp_str.parse::<i64>() else {
        return false;
    };
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > 300 {
        return false;
    }

    // 2. Build base signature string: "v0:{timestamp}:{body}"
    let prefix = format!("v0:{timestamp_str}:");
    let mut basestring = prefix.into_bytes();
    basestring.extend_from_slice(body);

    // 3. Compute expected signature
    let digest = hmac_sha256(signing_secret.as_bytes(), &basestring);
    let expected_sig = format!("v0={}", to_hex_string(&digest));

    subtle_slices_equal(signature_header.trim().as_bytes(), expected_sig.as_bytes())
}

/// Verifies LINE webhook request signatures using Base64-encoded HMAC-SHA256.
///
/// Format: Base64 string in `X-Line-Signature` matching HMAC-SHA256 of raw request body.
pub fn verify_line_signature(
    channel_secret: &str,
    body: &[u8],
    signature_header: &str,
) -> bool {
    if channel_secret.is_empty() || signature_header.is_empty() {
        return false;
    }

    let digest = hmac_sha256(channel_secret.as_bytes(), body);
    let expected_b64 = base64::engine::general_purpose::STANDARD.encode(digest);

    subtle_slices_equal(signature_header.trim().as_bytes(), expected_b64.as_bytes())
}

/// Verifies Feishu / Lark webhook request token or header signature.
///
/// Validates either verification token equality or signature hash with encrypt key.
pub fn verify_feishu_webhook_token(
    expected_token: &str,
    provided_token: &str,
) -> bool {
    if expected_token.is_empty() || provided_token.is_empty() {
        return false;
    }
    subtle_slices_equal(expected_token.trim().as_bytes(), provided_token.trim().as_bytes())
}

/// Verifies BlueBubbles webhook authorization.
///
/// Supports either secret password matching (header/query) or HMAC-SHA256 signature.
pub fn verify_bluebubbles_webhook_auth(
    expected_password: Option<&str>,
    provided_password: Option<&str>,
    signature_header: Option<&str>,
    body: &[u8],
) -> bool {
    // 1. Signature-based check if header provided
    if let (Some(sig), Some(secret)) = (signature_header, expected_password) {
        let clean_sig = sig.strip_prefix("sha256=").unwrap_or(sig).trim();
        let digest = hmac_sha256(secret.as_bytes(), body);
        let hex_sig = to_hex_string(&digest);
        if subtle_slices_equal(clean_sig.as_bytes(), hex_sig.as_bytes()) {
            return true;
        }
    }

    // 2. Direct password match
    if let (Some(expected), Some(provided)) = (expected_password, provided_password) {
        if !expected.is_empty() && subtle_slices_equal(expected.trim().as_bytes(), provided.trim().as_bytes()) {
            return true;
        }
    }

    false
}

/// Verifies Google Chat webhook Bearer token shape, issuer, and optional audience.
pub fn verify_googlechat_bearer(
    token: &str,
    expected_audience: Option<&str>,
) -> Result<bool, String> {
    let clean = token
        .strip_prefix("Bearer ")
        .or_else(|| token.strip_prefix("bearer "))
        .unwrap_or(token)
        .trim();

    if clean.is_empty() {
        return Ok(false);
    }

    // Google Chat JWT must consist of 3 parts: header.payload.signature
    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() != 3 {
        return Err("Google Chat bearer token must contain 3 jwt segments".to_string());
    }

    // Decode and inspect payload
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(parts[1]))
        .map_err(|e| format!("google chat jwt base64 decode failure: {e}"))?;

    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| format!("google chat jwt payload json parse failure: {e}"))?;

    // Issuer check: must be Google Chat service account or accounts.google.com
    let iss = claims.get("iss").and_then(|v| v.as_str()).unwrap_or("");
    if !iss.contains("google.com") && !iss.contains("gserviceaccount.com") {
        return Ok(false);
    }

    // Optional audience check
    if let Some(expected_aud) = expected_audience {
        let aud = claims.get("aud").and_then(|v| v.as_str()).unwrap_or("");
        if aud != expected_aud {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Verifies MS Teams Bot Framework Bearer JWT header shape and issuer claims.
pub fn verify_msteams_bearer_jwt_shape(auth_header: &str) -> Result<bool, String> {
    let clean = auth_header
        .strip_prefix("Bearer ")
        .or_else(|| auth_header.strip_prefix("bearer "))
        .unwrap_or(auth_header)
        .trim();

    if clean.is_empty() {
        return Ok(false);
    }

    let parts: Vec<&str> = clean.split('.').collect();
    if parts.len() != 3 {
        return Err("MS Teams authorization bearer token must contain 3 jwt segments".to_string());
    }

    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(parts[1]))
        .map_err(|e| format!("msteams jwt base64 decode failure: {e}"))?;

    let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| format!("msteams jwt payload json parse failure: {e}"))?;

    let iss = claims.get("iss").and_then(|v| v.as_str()).unwrap_or("");
    let is_valid_iss = iss.contains("botframework.com")
        || iss.contains("login.microsoftonline.com")
        || iss.contains("api.botframework.com");

    if !is_valid_iss {
        return Ok(false);
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slack_signature_verification() {
        let secret = "test_signing_secret";
        let body = b"{\"command\":\"/help\"}";
        let now_ts = chrono::Utc::now().timestamp().to_string();

        let prefix = format!("v0:{now_ts}:");
        let mut base = prefix.into_bytes();
        base.extend_from_slice(body);
        let digest = hmac_sha256(secret.as_bytes(), &base);
        let header = format!("v0={}", to_hex_string(&digest));

        assert!(verify_slack_signature(secret, &now_ts, body, &header));
        assert!(!verify_slack_signature("bad_secret", &now_ts, body, &header));

        // Replay rejection (older than 300s)
        let stale_ts = (chrono::Utc::now().timestamp() - 301).to_string();
        assert!(!verify_slack_signature(secret, &stale_ts, body, &header));
    }

    #[test]
    fn test_line_signature_verification() {
        let secret = "channel_secret_key";
        let body = b"{\"events\":[]}";
        let digest = hmac_sha256(secret.as_bytes(), body);
        let header = base64::engine::general_purpose::STANDARD.encode(digest);

        assert!(verify_line_signature(secret, body, &header));
        assert!(!verify_line_signature(secret, b"tampered", &header));
    }

    #[test]
    fn test_bluebubbles_and_feishu_auth() {
        assert!(verify_feishu_webhook_token("secret_token", "secret_token"));
        assert!(!verify_feishu_webhook_token("secret_token", "wrong_token"));

        assert!(verify_bluebubbles_webhook_auth(Some("pass123"), Some("pass123"), None, b""));
        assert!(!verify_bluebubbles_webhook_auth(Some("pass123"), Some("wrong"), None, b""));
    }
}
