//! NodeHost SecretInput materialization boundary.
//!
//! This module deliberately contains no persistence, environment lookup, or
//! concrete secret provider. A product-owned caller injects a resolver, which
//! must register materialized values with its redaction lifecycle before
//! returning the short-lived lease to the transport owner.

use std::fmt;
use std::sync::Arc;

use super::gateway_connection::gateway_origin_scope;

const MAX_REFERENCE_BYTES: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostSecretInputRef {
    reference: String,
}

impl NodeHostSecretInputRef {
    pub fn new(reference: impl Into<String>) -> Result<Self, String> {
        let reference = reference.into();
        let trimmed = reference.trim();
        if trimmed.is_empty()
            || trimmed.len() > MAX_REFERENCE_BYTES
            || trimmed.chars().any(char::is_control)
        {
            return Err("NodeHost SecretInput reference is invalid".into());
        }
        Ok(Self {
            reference: trimmed.to_string(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.reference
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostCloudflareAccessInput {
    pub client_id: NodeHostSecretInputRef,
    pub client_secret: NodeHostSecretInputRef,
}

impl NodeHostCloudflareAccessInput {
    pub fn new(
        client_id: NodeHostSecretInputRef,
        client_secret: NodeHostSecretInputRef,
    ) -> Self {
        Self {
            client_id,
            client_secret,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct NodeHostCloudflareAccessCredentials {
    origin_scope: String,
    client_id: String,
    client_secret: String,
}

impl NodeHostCloudflareAccessCredentials {
    /// Construct only from a materializer implementation. The returned value
    /// is ephemeral and must not be persisted or placed in session state.
    pub(crate) fn new(
        origin_scope: String,
        client_id: String,
        client_secret: String,
    ) -> Result<Self, String> {
        if origin_scope.trim().is_empty()
            || origin_scope.chars().any(char::is_control)
            || client_id.trim().is_empty()
            || client_secret.trim().is_empty()
            || client_id.chars().any(char::is_control)
            || client_secret.chars().any(char::is_control)
        {
            return Err("materialized Cloudflare Access credentials are invalid".into());
        }
        Ok(Self {
            origin_scope,
            client_id,
            client_secret,
        })
    }

    pub(crate) fn origin_scope(&self) -> &str {
        &self.origin_scope
    }

    pub(crate) fn client_id(&self) -> &str {
        &self.client_id
    }

    pub(crate) fn client_secret(&self) -> &str {
        &self.client_secret
    }
}

impl fmt::Debug for NodeHostCloudflareAccessCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeHostCloudflareAccessCredentials")
            .field("origin_scope", &self.origin_scope)
            .field("client_id", &"<redacted>")
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

pub trait NodeHostSecretMaterializer: Send + Sync {
    fn materialize_cloudflare_access(
        &self,
        origin: &str,
        input: &NodeHostCloudflareAccessInput,
    ) -> Result<NodeHostCloudflareAccessCredentials, String>;
}

pub type SharedNodeHostSecretMaterializer = Arc<dyn NodeHostSecretMaterializer>;

pub(crate) fn validate_materializer_origin(
    configured_gateway_url: &str,
    materializer_origin: &str,
) -> Result<(), String> {
    let expected = gateway_origin_scope(configured_gateway_url);
    let actual = gateway_origin_scope(materializer_origin);
    if expected != actual {
        return Err("NodeHost SecretInput origin scope does not match Gateway".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        NodeHostCloudflareAccessCredentials, NodeHostSecretInputRef,
        validate_materializer_origin,
    };

    #[test]
    fn secret_input_rejects_empty_or_control_reference() {
        assert!(NodeHostSecretInputRef::new(" ").is_err());
        assert!(NodeHostSecretInputRef::new("secret\nref").is_err());
    }

    #[test]
    fn materializer_origin_is_path_scoped() {
        assert!(validate_materializer_origin(
            "wss://gateway.example/ws",
            "wss://gateway.example/ws/",
        )
        .is_ok());
        assert!(validate_materializer_origin(
            "https://gateway.example/ws",
            "https://gateway.example/other",
        )
        .is_err());
    }

    #[test]
    fn returned_lease_with_wrong_origin_is_rejected() {
        let credentials = NodeHostCloudflareAccessCredentials::new(
            "wss://gateway.example/other".into(),
            "client-id".into(),
            "client-secret".into(),
        )
        .expect("credential shape");
        assert!(validate_materializer_origin(
            "wss://gateway.example/ws",
            credentials.origin_scope(),
        )
        .is_err());
    }

    #[test]
    fn credential_debug_never_contains_materialized_values() {
        let credentials = NodeHostCloudflareAccessCredentials::new(
            "wss://gateway.example/ws".into(),
            "client-id-secret-value".into(),
            "client-secret-value".into(),
        )
        .expect("credential shape");
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("client-id-secret-value"));
        assert!(!debug.contains("client-secret-value"));
        assert!(debug.contains("<redacted>"));
    }
}
