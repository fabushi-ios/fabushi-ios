//! PlatformExecBroker transport contract for the iOS static package runner.

use serde_json::{Value, json};
use std::path::Path;

pub const BOOTSTRAP_METHOD: &str = "dandelion.static.bootstrap";
pub const BOOTSTRAP_PROTOCOL: &str = "fabushi.ios.dandelion-static-bootstrap.v1";

pub struct Bootstrap<'a> {
    pub runner_id: &'a str,
    pub account_scope: &'a str,
    pub account_generation: &'a str,
    pub app_id: &'a str,
    pub version: &'a str,
    pub package_digest: &'a str,
    pub grant_token: &'a str,
    pub package_directory: &'a Path,
    pub private_directory: &'a Path,
    pub run_directory: &'a Path,
}

pub fn bootstrap_params(binding: Bootstrap<'_>) -> Value {
    json!({
        "protocol": BOOTSTRAP_PROTOCOL,
        "runnerId": binding.runner_id,
        "accountScope": binding.account_scope,
        "accountGeneration": binding.account_generation,
        "appId": binding.app_id,
        "version": binding.version,
        "packageDigest": binding.package_digest,
        "grantToken": binding.grant_token,
        "packageDirectory": binding.package_directory.to_string_lossy(),
        "privateDirectory": binding.private_directory.to_string_lossy(),
        "runDirectory": binding.run_directory.to_string_lossy(),
    })
}

pub fn validate_bootstrap_result(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Dandelion bootstrap result is not an object".to_string())?;
    if object.get("protocol").and_then(Value::as_str) != Some(BOOTSTRAP_PROTOCOL)
        || object.get("accepted").and_then(Value::as_bool) != Some(true)
    {
        return Err("Dandelion bootstrap was not explicitly accepted by the iOS adapter".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_requires_explicit_protocol_acceptance() {
        assert!(validate_bootstrap_result(&json!({
            "protocol": BOOTSTRAP_PROTOCOL,
            "accepted": true
        })).is_ok());
        assert!(validate_bootstrap_result(&json!({"accepted": true})).is_err());
    }
}
