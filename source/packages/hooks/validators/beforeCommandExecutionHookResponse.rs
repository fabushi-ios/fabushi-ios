use serde_json::Value;
use crate::package_hooks_validators_base::{ValidationResult, create_validation_result};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_before_command_execution_hook_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value); if !base.is_valid { return base; }
    let object = value.as_object().expect("base hook response accepted only objects");
    let mut errors = Vec::new();
    if let Some(permission) = object.get("permission") {
        if !matches!(permission.as_str(), Some("allow" | "deny" | "ask")) {
            errors.push("Invalid permission value. Expected one of: allow, deny, ask, or undefined".to_owned());
        }
    }
    if let Some(message) = object.get("user_message") {
        if !message.is_string() { errors.push("Invalid user_message value. Expected a string if provided".to_owned()); }
    }
    if let Some(message) = object.get("agent_message") {
        if !message.is_string() { errors.push("Invalid agent_message value. Expected a string if provided".to_owned()); }
    }
    create_validation_result(errors.is_empty(), errors)
}
#[cfg(test)]
mod tests {
    use super::*; use serde_json::json;
    #[test] fn validates_command_permission_and_messages() {
        assert!(validate_before_command_execution_hook_response(&json!({"permission":"ask","agent_message":"review"})).is_valid);
        assert!(!validate_before_command_execution_hook_response(&json!({"permission":"later"})).is_valid);
        assert!(!validate_before_command_execution_hook_response(&json!({"agent_message":false})).is_valid);
    }
}
