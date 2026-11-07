//! Read-only attachment projection for provider-owned transcript replay.
//!
//! The attachment owner decides whether a durable attachment already carries
//! a provider-usable URL. Provider serializers still decide the request-part
//! shape. This boundary deliberately does not upload historical files or
//! reuse outgoing-media custody.

use serde_json::Value;
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReplayAttachmentProjectionError {
    #[error("attachment has no provider URL")]
    MissingUrl,
    #[error("attachment URL is invalid: {0}")]
    InvalidUrl(String),
    #[error("attachment URL scheme is unsupported: {0}")]
    UnsupportedScheme(String),
    #[error("attachment URL must not contain user credentials")]
    UserInfoNotAllowed,
}

/// Return a durable attachment's already-materialized provider URL.
///
/// Historical local paths, `file://` URLs, data URLs, and arbitrary schemes
/// are intentionally rejected. This function performs no I/O and therefore
/// cannot turn an unresolved attachment into an accepted replay input.
pub fn provider_replay_attachment_url(
    attachment: &Value,
) -> Result<String, ReplayAttachmentProjectionError> {
    let raw = attachment
        .get("url")
        .or_else(|| attachment.get("signedUrl"))
        .or_else(|| attachment.get("signed_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ReplayAttachmentProjectionError::MissingUrl)?;
    let parsed = Url::parse(raw)
        .map_err(|error| ReplayAttachmentProjectionError::InvalidUrl(error.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(ReplayAttachmentProjectionError::UnsupportedScheme(
            parsed.scheme().to_string(),
        ));
    }
    if parsed.host_str().is_none() {
        return Err(ReplayAttachmentProjectionError::InvalidUrl(
            "provider URL has no host".into(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ReplayAttachmentProjectionError::UserInfoNotAllowed);
    }
    Ok(raw.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_signed_https_url() {
        assert_eq!(
            provider_replay_attachment_url(&json!({
                "signed_url": "https://media.example.test/a.png?token=x"
            }))
            .expect("provider URL"),
            "https://media.example.test/a.png?token=x"
        );
    }

    #[test]
    fn rejects_local_and_data_urls() {
        for value in [
            json!({"file_path": "/tmp/a.png"}),
            json!({"url": "file:///tmp/a.png"}),
            json!({"url": "data:image/png;base64,abc"}),
        ] {
            assert!(provider_replay_attachment_url(&value).is_err());
        }
    }

    #[test]
    fn rejects_urls_with_userinfo() {
        assert!(matches!(
            provider_replay_attachment_url(&json!({"url": "https://user:pass@example.test/a"})),
            Err(ReplayAttachmentProjectionError::UserInfoNotAllowed)
        ));
    }
}
