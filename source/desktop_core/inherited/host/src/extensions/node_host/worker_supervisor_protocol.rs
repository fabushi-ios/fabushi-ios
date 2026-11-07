//! Strict node-worker supervisor identity and receipt admission.
//!
//! This module owns correlation validation only. Process/container lifecycle,
//! durable launch receipts, cleanup, and worker side effects remain with the
//! existing worker/supervisor owners.

use serde_json::Value;

pub const WORKER_RESULT_JSON_MAX_BYTES: usize = 64 * 1024;
pub const WORKER_ERROR_TEXT_MAX_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeWorkerSupervisorIdentity {
    pub launch_id: String,
    pub plan_hash: String,
    pub environment_id: String,
    pub session_id: String,
    pub owner_epoch: u64,
    pub placement_generation: u64,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeWorkerSupervisorReceiptState {
    Pending,
    Running,
    Completed { result_json: String },
    Failed { error_text: String },
    Interrupted { error_text: String },
    Cancelled { error_text: String },
    OutcomeUnknown { error_text: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeWorkerSupervisorReceipt {
    pub identity: NodeWorkerSupervisorIdentity,
    pub state: NodeWorkerSupervisorReceiptState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeWorkerLaunchBinding {
    pub launch_id: String,
    pub gateway_namespace: String,
    pub expected_bundle_hash: String,
    pub placement_generation: u64,
    pub workspace: Option<NodeWorkerWorkspaceIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeWorkerWorkspaceIdentity {
    pub environment_id: String,
    pub session_id: String,
    pub owner_epoch: u64,
}

/// Validate only the cross-field bindings that protect a worker launch.
/// Descriptor feature/schema validation remains with the worker descriptor
/// owner; this function must not become a second launch-plan parser.
pub fn validate_worker_launch_bindings(value: &Value) -> Result<NodeWorkerLaunchBinding, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "INVALID_REQUEST: worker launch must be an object".to_string())?;
    if object.get("environmentSession") != Some(&Value::from(1)) {
        return Err("INVALID_REQUEST: node worker environment lifetime support required".into());
    }
    if object
        .get("idleRetention")
        .is_some_and(|value| value != &Value::Bool(true))
    {
        return Err("INVALID_REQUEST: idleRetention must be true".into());
    }
    if let Some(session_key) = object.get("sessionKey") {
        let valid = session_key
            .as_str()
            .is_some_and(|value| !value.is_empty() && value.len() <= 1_024 && value.trim() == value && !value.contains('\0'));
        if !valid {
            return Err("INVALID_REQUEST: invalid worker sessionKey".into());
        }
    }
    let launch_id = identifier(object, "launchId")?;
    let expected_bundle_hash = plan_hash(object, "expectedBundleHash")?;
    let gateway_namespace = object
        .get("gatewayNamespace")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value.chars().next().is_some_and(|ch| ch.is_ascii_alphanumeric())
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        })
        .ok_or_else(|| "INVALID_REQUEST: invalid gatewayNamespace".to_string())?
        .to_string();
    let placement_generation = non_negative_integer(object, "placementGeneration")?;
    let descriptor = object
        .get("descriptor")
        .and_then(Value::as_object)
        .ok_or_else(|| "INVALID_REQUEST: invalid worker launch descriptor".to_string())?;
    // Provider identity transport and platform trust are captured by the
    // dedicated node-worker owner. They must never arrive as caller-controlled
    // launch fields or become part of a durable descriptor/receipt.
    for field in PROTECTED_ENVIRONMENT_FIELDS {
        if object.contains_key(field) || contains_object_key(&Value::Object(descriptor.clone()), field) {
            return Err(format!(
                "INVALID_REQUEST: {field} must be supplied by the node-worker environment owner"
            ));
        }
    }
    for field in WORKSPACE_IDENTITY_FIELDS {
        if contains_object_key(&Value::Object(descriptor.clone()), field) {
            return Err(format!(
                "INVALID_REQUEST: {field} must be bound by the worker owner, not descriptor"
            ));
        }
    }
    let workspace = match (
        object.get("environmentId"),
        object.get("sessionId"),
        object.get("ownerEpoch"),
    ) {
        (None, None, None) => None,
        (Some(environment_id), Some(session_id), Some(owner_epoch)) => Some(
            NodeWorkerWorkspaceIdentity {
                environment_id: bounded_workspace_identifier(environment_id, "environmentId")?,
                session_id: bounded_workspace_identifier(session_id, "sessionId")?,
                owner_epoch: owner_epoch.as_u64().ok_or_else(|| {
                    "INVALID_REQUEST: ownerEpoch must be a non-negative integer".to_string()
                })?,
            },
        ),
        _ => {
            return Err(
                "INVALID_REQUEST: workspace identity requires environmentId, sessionId, and ownerEpoch"
                    .into(),
            )
        }
    };
    let assignment = descriptor
        .get("assignment")
        .and_then(Value::as_object)
        .ok_or_else(|| "INVALID_REQUEST: invalid worker launch assignment".to_string())?;
    if assignment.get("turnId").and_then(Value::as_str) != Some(launch_id.as_str()) {
        return Err("INVALID_REQUEST: launchId must match descriptor assignment turnId".into());
    }
    let handshake = descriptor
        .get("admission")
        .and_then(Value::as_object)
        .and_then(|admission| admission.get("handshake"))
        .and_then(Value::as_object)
        .ok_or_else(|| "INVALID_REQUEST: invalid worker launch handshake".to_string())?;
    if handshake.get("bundleHash").and_then(Value::as_str) != Some(expected_bundle_hash.as_str()) {
        return Err("INVALID_REQUEST: descriptor bundle hash does not match expectedBundleHash".into());
    }
    Ok(NodeWorkerLaunchBinding {
        launch_id,
        gateway_namespace,
        expected_bundle_hash,
        placement_generation,
        workspace,
    })
}

fn bounded_workspace_identifier(value: &Value, field: &str) -> Result<String, String> {
    let value = value
        .as_str()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value.trim() == *value
                && !value.contains('\0')
        })
        .ok_or_else(|| format!("INVALID_REQUEST: {field} must be a bounded identifier"))?;
    Ok(value.to_string())
}

const PROTECTED_ENVIRONMENT_FIELDS: [&str; 4] = [
    "managedIdentityTransport",
    "identityEndpoint",
    "identityHeader",
    "platformTrust",
];
const WORKSPACE_IDENTITY_FIELDS: [&str; 3] = ["environmentId", "sessionId", "ownerEpoch"];

fn contains_object_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(key) || object.values().any(|child| contains_object_key(child, key))
        }
        Value::Array(values) => values.iter().any(|child| contains_object_key(child, key)),
        _ => false,
    }
}

pub fn parse_worker_identity(value: &Value) -> Result<NodeWorkerSupervisorIdentity, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "INVALID_REQUEST: worker identity must be an object".to_string())?;
    require_exact_keys(
        object,
        &[
            "launchId",
            "planHash",
            "environmentId",
            "sessionId",
            "ownerEpoch",
            "placementGeneration",
            "runId",
        ],
    )?;
    parse_identity_fields(object)
}

fn parse_identity_fields(object: &serde_json::Map<String, Value>) -> Result<NodeWorkerSupervisorIdentity, String> {
    Ok(NodeWorkerSupervisorIdentity {
        launch_id: identifier(object, "launchId")?,
        plan_hash: plan_hash(object, "planHash")?,
        environment_id: identifier(object, "environmentId")?,
        session_id: identifier(object, "sessionId")?,
        owner_epoch: non_negative_integer(object, "ownerEpoch")?,
        placement_generation: non_negative_integer(object, "placementGeneration")?,
        run_id: identifier(object, "runId")?,
    })
}

pub fn parse_worker_receipt(value: &Value) -> Result<NodeWorkerSupervisorReceipt, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "INVALID_REQUEST: worker receipt must be an object".to_string())?;
    let identity = parse_identity_fields(object)?;
    let state = object
        .get("state")
        .and_then(Value::as_str)
        .ok_or_else(|| "INVALID_REQUEST: worker receipt state is required".to_string())?;
    let state = match state {
        "pending" => {
            require_exact_keys(object, &identity_keys_with("state"))?;
            NodeWorkerSupervisorReceiptState::Pending
        }
        "running" => {
            require_exact_keys(object, &identity_keys_with("state"))?;
            NodeWorkerSupervisorReceiptState::Running
        }
        "completed" => {
            require_exact_keys(object, &identity_keys_with_two("state", "resultJson"))?;
            NodeWorkerSupervisorReceiptState::Completed {
                result_json: bounded_result_json(object.get("resultJson"))?,
            }
        }
        "failed" | "interrupted" | "cancelled" | "outcome-unknown" => {
            require_exact_keys(object, &identity_keys_with_two("state", "errorText"))?;
            let error_text = bounded_error_text(object.get("errorText"))?;
            match state {
                "failed" => NodeWorkerSupervisorReceiptState::Failed { error_text },
                "interrupted" => NodeWorkerSupervisorReceiptState::Interrupted { error_text },
                "cancelled" => NodeWorkerSupervisorReceiptState::Cancelled { error_text },
                _ => NodeWorkerSupervisorReceiptState::OutcomeUnknown { error_text },
            }
        }
        _ => return Err("INVALID_REQUEST: invalid worker receipt state".into()),
    };
    Ok(NodeWorkerSupervisorReceipt { identity, state })
}

pub fn worker_turn_matches_identity(
    receipt: &NodeWorkerSupervisorIdentity,
    expected: &NodeWorkerSupervisorIdentity,
) -> bool {
    receipt == expected
}

fn identifier(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 256 && value.trim() == *value && !value.contains('\0'))
        .ok_or_else(|| format!("INVALID_REQUEST: {key} must be a bounded identifier"))?;
    Ok(value.to_string())
}

fn plan_hash(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, String> {
    let value = identifier(object, key)?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        return Err("INVALID_REQUEST: planHash must be 64 lowercase hexadecimal characters".into());
    }
    Ok(value)
}

fn non_negative_integer(object: &serde_json::Map<String, Value>, key: &str) -> Result<u64, String> {
    object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("INVALID_REQUEST: {key} must be a non-negative integer"))
}

fn bounded_result_json(value: Option<&Value>) -> Result<String, String> {
    let value = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.as_bytes().len() <= WORKER_RESULT_JSON_MAX_BYTES)
        .ok_or_else(|| "INVALID_REQUEST: resultJson must be bounded JSON".to_string())?;
    let parsed: Value = serde_json::from_str(value)
        .map_err(|_| "INVALID_REQUEST: resultJson must be valid JSON".to_string())?;
    if !parsed.is_object() {
        return Err("INVALID_REQUEST: resultJson must be a JSON object".into());
    }
    Ok(value.to_owned())
}

fn bounded_error_text(value: Option<&Value>) -> Result<String, String> {
    let value = value
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.as_bytes().len() <= WORKER_ERROR_TEXT_MAX_BYTES
                && !value.contains('\r')
                && !value.contains('\n')
        })
        .ok_or_else(|| "INVALID_REQUEST: errorText must be bounded single-line text".to_string())?;
    Ok(value.to_string())
}

fn identity_keys_with(extra: &str) -> Vec<&str> {
    vec![
        "launchId",
        "planHash",
        "environmentId",
        "sessionId",
        "ownerEpoch",
        "placementGeneration",
        "runId",
        extra,
    ]
}

fn identity_keys_with_two<'a>(first: &'a str, second: &'a str) -> Vec<&'a str> {
    let mut keys = identity_keys_with(first);
    keys.push(second);
    keys
}

fn require_exact_keys(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<(), String> {
    if object.len() != keys.len() || object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err("INVALID_REQUEST: unsupported worker protocol fields".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_worker_receipt, parse_worker_identity, validate_worker_launch_bindings,
        worker_turn_matches_identity,
    };
    use serde_json::json;

    fn identity() -> serde_json::Value {
        json!({
            "launchId":"launch-1",
            "planHash":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "environmentId":"env-1",
            "sessionId":"session-1",
            "ownerEpoch":2,
            "placementGeneration":3,
            "runId":"run-1"
        })
    }

    #[test]
    fn identity_is_exactly_bound_and_unknown_fields_fail() {
        let parsed = parse_worker_identity(&identity()).expect("identity");
        assert_eq!(parsed.owner_epoch, 2);
        let mut unknown = identity();
        unknown.as_object_mut().expect("object").insert("extra".into(), json!(true));
        assert!(parse_worker_identity(&unknown).is_err());
    }

    #[test]
    fn receipts_are_bounded_and_match_all_generation_identity_fields() {
        let mut receipt = identity();
        receipt.as_object_mut().expect("object").extend([
            ("state".into(), json!("completed")),
            ("resultJson".into(), json!(r#"{"ok":true}"#)),
        ]);
        let parsed = parse_worker_receipt(&receipt).expect("receipt");
        assert!(matches!(parsed.state, super::NodeWorkerSupervisorReceiptState::Completed { .. }));
        assert!(worker_turn_matches_identity(&parsed.identity, &parse_worker_identity(&identity()).expect("identity")));
    }

    #[test]
    fn launch_binding_requires_cross_field_identity_matches() {
        let value = json!({
            "environmentSession": 1,
            "launchId": "launch-1",
            "gatewayNamespace": "gw-main",
            "expectedBundleHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "placementGeneration": 3,
            "descriptor": {
                "assignment": {"turnId": "launch-1"},
                "admission": {"handshake": {"bundleHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}
            }
        });
        assert_eq!(validate_worker_launch_bindings(&value).expect("binding").launch_id, "launch-1");
    }

    #[test]
    fn launch_binding_rejects_caller_owned_identity_or_platform_trust() {
        let mut value = json!({
            "environmentSession": 1,
            "launchId": "launch-1",
            "gatewayNamespace": "gw-main",
            "expectedBundleHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "placementGeneration": 3,
            "descriptor": {
                "assignment": {"turnId": "launch-1"},
                "admission": {"handshake": {"bundleHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}
            }
        });
        value["descriptor"]["identityHeader"] = json!("caller-secret");
        let error = validate_worker_launch_bindings(&value).expect_err("caller identity must be rejected");
        assert!(error.contains("identityHeader"));

        value["descriptor"]
            .as_object_mut()
            .expect("descriptor")
            .remove("identityHeader");
        value["platformTrust"] = json!({"SSL_CERT_FILE": "/caller/ca.pem"});
        let error = validate_worker_launch_bindings(&value).expect_err("caller trust must be rejected");
        assert!(error.contains("platformTrust"));

        value.as_object_mut().expect("launch").remove("platformTrust");
        value["descriptor"]["admission"]["identityEndpoint"] = json!("http://identity");
        let error = validate_worker_launch_bindings(&value).expect_err("nested identity must be rejected");
        assert!(error.contains("identityEndpoint"));
    }

    #[test]
    fn launch_binding_preserves_complete_workspace_identity_and_rejects_partial_identity() {
        let mut value = json!({
            "environmentSession": 1,
            "launchId": "launch-1",
            "gatewayNamespace": "gw-main",
            "expectedBundleHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "placementGeneration": 3,
            "environmentId": "env-1",
            "sessionId": "session-1",
            "ownerEpoch": 4,
            "descriptor": {
                "assignment": {"turnId": "launch-1"},
                "admission": {"handshake": {"bundleHash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}
            }
        });
        let binding = validate_worker_launch_bindings(&value).expect("workspace binding");
        assert_eq!(binding.workspace.as_ref().map(|identity| identity.owner_epoch), Some(4));

        value.as_object_mut().expect("launch").remove("sessionId");
        let error = validate_worker_launch_bindings(&value).expect_err("partial workspace identity");
        assert!(error.contains("workspace identity"));
    }
}
