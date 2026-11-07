//! Challenge-bound NodeHost device-auth proof construction.
//!
//! This module owns no socket, token persistence, pairing lookup, or retry.
//! The eventual transport owner supplies the challenge and receives the
//! credential-owner outputs through this narrow proof boundary.

use serde::Serialize;

use crate::host_secret_store::{HostNodeDeviceIdentity, HostNodeDeviceToken};

#[derive(Clone)]
pub struct NodeHostDeviceAuthContext<'a> {
    pub client_id: &'a str,
    pub client_mode: &'a str,
    pub role: &'a str,
    pub scopes: &'a [String],
    pub signed_at_ms: u64,
    pub token: Option<&'a str>,
    pub nonce: &'a str,
    pub platform: Option<&'a str>,
    pub device_family: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeHostDeviceAuthProof {
    pub id: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    pub signature: String,
    #[serde(rename = "signedAt")]
    pub signed_at_ms: u64,
    pub nonce: String,
}

fn normalize_metadata(value: Option<&str>) -> String {
    value
        .unwrap_or_default()
        .trim()
        .chars()
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn validate_component(name: &str, value: &str, allow_empty: bool) -> Result<(), String> {
    if (!allow_empty && value.is_empty())
        || value.len() > 512
        || value.contains('|')
        || value.chars().any(char::is_control)
    {
        return Err(format!("node device auth {name} is invalid"));
    }
    Ok(())
}

pub fn build_device_auth_payload_v3(
    identity: &HostNodeDeviceIdentity,
    context: &NodeHostDeviceAuthContext<'_>,
) -> Result<String, String> {
    let client_id = context.client_id.trim();
    let client_mode = context.client_mode.trim();
    let role = context.role.trim();
    let nonce = context.nonce.trim();
    validate_component("client id", client_id, false)?;
    validate_component("client mode", client_mode, false)?;
    validate_component("role", role, false)?;
    validate_component("nonce", nonce, false)?;
    let scopes = context
        .scopes
        .iter()
        .map(|scope| scope.trim())
        .collect::<Vec<_>>();
    if scopes
        .iter()
        .any(|scope| {
            scope.is_empty()
                || scope.contains(',')
                || scope.contains('|')
                || scope.chars().any(char::is_control)
        })
    {
        return Err("node device auth scopes are invalid".into());
    }
    let token = context.token.unwrap_or_default();
    validate_component("token", token, true)?;
    let platform = normalize_metadata(context.platform);
    let device_family = normalize_metadata(context.device_family);
    validate_component("platform", &platform, true)?;
    validate_component("device family", &device_family, true)?;
    Ok([
        "v3".to_string(),
        identity.device_id().to_string(),
        client_id.to_string(),
        client_mode.to_string(),
        role.to_string(),
        scopes.join(","),
        context.signed_at_ms.to_string(),
        token.to_string(),
        nonce.to_string(),
        platform,
        device_family,
    ]
    .join("|"))
}

pub fn build_device_auth_proof(
    identity: &HostNodeDeviceIdentity,
    context: &NodeHostDeviceAuthContext<'_>,
) -> Result<NodeHostDeviceAuthProof, String> {
    let payload = build_device_auth_payload_v3(identity, context)?;
    Ok(NodeHostDeviceAuthProof {
        id: identity.device_id().to_string(),
        public_key: identity.public_key_raw_base64_url(),
        signature: identity.sign_payload_base64_url(&payload),
        signed_at_ms: context.signed_at_ms,
        nonce: context.nonce.trim().to_string(),
    })
}

pub fn context_from_cached_token<'a>(
    identity: &HostNodeDeviceIdentity,
    token: &'a HostNodeDeviceToken,
    client_id: &'a str,
    client_mode: &'a str,
    nonce: &'a str,
    signed_at_ms: u64,
    platform: Option<&'a str>,
    device_family: Option<&'a str>,
) -> Result<NodeHostDeviceAuthContext<'a>, String> {
    if token.device_id() != identity.device_id() {
        return Err("node device token does not belong to the signing identity".into());
    }
    Ok(NodeHostDeviceAuthContext {
        client_id,
        client_mode,
        role: token.role(),
        scopes: token.scopes(),
        signed_at_ms,
        token: Some(token.secret()),
        nonce,
        platform,
        device_family,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        NodeHostDeviceAuthContext, build_device_auth_payload_v3, build_device_auth_proof,
    };
    use crate::host_secret_store::load_or_create_host_node_identity;

    #[test]
    fn v3_payload_normalizes_metadata_and_binds_nonce() {
        let path = std::env::temp_dir().join(format!(
            "fabushi-node-auth-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let identity = load_or_create_host_node_identity(Some(&path)).expect("identity");
        let scopes = vec!["node.read".to_string(), "node.invoke".to_string()];
        let context = NodeHostDeviceAuthContext {
            client_id: "node-host",
            client_mode: "node",
            role: "node",
            scopes: &scopes,
            signed_at_ms: 123,
            token: Some("shared-token"),
            nonce: "challenge-nonce",
            platform: Some("MaC OS"),
            device_family: Some("Native-Node"),
        };
        let payload = build_device_auth_payload_v3(&identity, &context).expect("payload");
        assert!(payload.ends_with("|mac os|native-node"));
        assert!(payload.contains("|shared-token|challenge-nonce|"));
        let proof = build_device_auth_proof(&identity, &context).expect("proof");
        assert_eq!(proof.id, identity.device_id());
        assert_eq!(proof.nonce, "challenge-nonce");
        let _ = std::fs::remove_file(path);
    }
}

