//! `hello-ok` device-token admission for the NodeHost credential owner.
//!
//! This is deliberately independent of a WebSocket implementation. The
//! transport owner hands one decoded hello payload to this module; only a
//! valid device token is forwarded to the credential store.

use std::io;
use std::path::Path;

use serde_json::Value;

use crate::host_secret_store::{
    HostNodeDeviceIdentity, clear_host_node_token, write_host_node_token,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostHelloDeviceAuth {
    pub device_token: String,
    pub role: String,
    pub scopes: Vec<String>,
}

pub fn parse_hello_ok_device_auth(
    payload: &Value,
) -> Result<Option<NodeHostHelloDeviceAuth>, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "node hello-ok payload must be an object".to_string())?;
    if object.get("type").and_then(Value::as_str) != Some("hello-ok") {
        return Err("node hello auth requires a hello-ok payload".into());
    }
    let Some(auth) = object.get("auth") else {
        return Ok(None);
    };
    let auth = auth
        .as_object()
        .ok_or_else(|| "node hello-ok auth must be an object".to_string())?;
    let Some(device_token) = auth.get("deviceToken") else {
        return Ok(None);
    };
    let device_token = device_token
        .as_str()
        .map(str::trim)
        .filter(|token| !token.is_empty() && token.len() <= 256)
        .ok_or_else(|| "node hello-ok deviceToken is invalid".to_string())?;
    let role = auth
        .get("role")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|role| !role.is_empty() && role.len() <= 128)
        .ok_or_else(|| "node hello-ok role is invalid".to_string())?;
    let mut scopes = auth
        .get("scopes")
        .and_then(Value::as_array)
        .ok_or_else(|| "node hello-ok scopes are missing".to_string())?
        .iter()
        .map(|scope| {
            let scope = scope
                .as_str()
                .map(str::trim)
                .filter(|scope| !scope.is_empty() && scope.len() <= 256)
                .ok_or_else(|| "node hello-ok scope is invalid".to_string())?;
            if scope.chars().any(char::is_control) {
                return Err("node hello-ok scope contains control characters".into());
            }
            Ok(scope.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    scopes.sort();
    scopes.dedup();
    Ok(Some(NodeHostHelloDeviceAuth {
        device_token: device_token.to_string(),
        role: role.to_string(),
        scopes,
    }))
}

pub fn persist_hello_ok_device_auth(
    path: Option<&Path>,
    identity: &HostNodeDeviceIdentity,
    hello_payload: &Value,
    gateway_scope: Option<&str>,
    expected_token: Option<&str>,
) -> io::Result<Option<bool>> {
    let Some(auth) = parse_hello_ok_device_auth(hello_payload)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    else {
        return Ok(None);
    };
    let stored = write_host_node_token(
        path,
        identity.device_id(),
        &auth.role,
        gateway_scope,
        &auth.device_token,
        &auth.scopes,
        expected_token,
    )?;
    Ok(Some(stored))
}

pub fn clear_rejected_hello_device_auth(
    path: Option<&Path>,
    identity: &HostNodeDeviceIdentity,
    role: &str,
    gateway_scope: Option<&str>,
    expected_token: Option<&str>,
) -> io::Result<bool> {
    clear_host_node_token(
        path,
        identity.device_id(),
        role,
        gateway_scope,
        expected_token,
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{parse_hello_ok_device_auth, persist_hello_ok_device_auth};
    use crate::host_secret_store::load_or_create_host_node_identity;

    #[test]
    fn hello_device_token_is_strictly_parsed_and_rotation_fenced() {
        let root = std::env::temp_dir().join(format!(
            "fabushi-hello-auth-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let identity_path = root.with_extension("identity.json");
        let auth_path = root.with_extension("auth.json");
        let identity = load_or_create_host_node_identity(Some(&identity_path)).expect("identity");
        let hello = json!({
            "type": "hello-ok",
            "auth": {
                "deviceToken": "hello-token",
                "role": "node",
                "scopes": ["node.invoke", "node.invoke", "node.read"]
            }
        });
        let parsed = parse_hello_ok_device_auth(&hello)
            .expect("parse hello")
            .expect("device auth");
        assert_eq!(parsed.scopes, vec!["node.invoke", "node.read"]);
        assert_eq!(
            persist_hello_ok_device_auth(
                Some(&auth_path),
                &identity,
                &hello,
                Some("wss://gateway.example"),
                None,
            )
            .expect("persist hello"),
            Some(true)
        );
        assert_eq!(
            persist_hello_ok_device_auth(
                Some(&auth_path),
                &identity,
                &hello,
                Some("wss://gateway.example"),
                Some("stale-token"),
            )
            .expect("stale persistence"),
            Some(false)
        );
        let _ = std::fs::remove_file(identity_path);
        let _ = std::fs::remove_file(auth_path);
        let _ = std::fs::remove_file(root.with_extension("auth.json.lock"));
    }
}

