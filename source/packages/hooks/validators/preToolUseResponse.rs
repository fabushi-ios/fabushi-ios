use serde_json::Value;

use crate::package_hooks_validators_base::{
    ValidationResult, create_validation_result, is_object, validate_optional_string,
};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_pre_tool_use_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value);
    if !base.is_valid {
        return base;
    }

    let object = value.as_object().expect("base hook response accepted only objects");
    let mut errors = Vec::new();

    if let Some(permission) = object.get("permission") {
        if !matches!(permission.as_str(), Some("allow" | "deny" | "ask")) {
            errors.push(
                "Invalid permission value. Expected one of: allow, deny, ask, or undefined"
                    .to_owned(),
            );
        }
    }
    if let Some(message) = object.get("user_message") {
        if !message.is_string() {
            errors.push("Invalid user_message value. Expected a string if provided".to_owned());
        }
    }
    if let Some(message) = object.get("agent_message") {
        if !message.is_string() {
            errors.push("Invalid agent_message value. Expected a string if provided".to_owned());
        }
    }
    if let Some(updated_input) = object.get("updated_input") {
        if !is_object(updated_input) {
            errors.push(
                "Invalid updated_input value. Expected a plain object if provided".to_owned(),
            );
        }
    }
    validate_optional_string(object, "additional_context", &mut errors);

    create_validation_result(errors.is_empty(), errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_pre_tool_use_response_contract() {
        assert!(validate_pre_tool_use_response(&json!({
            "permission": "ask",
            "user_message": "confirm",
            "agent_message": "reviewing",
            "updated_input": {"path": "a.txt"},
            "additional_context": "ctx"
        }))
        .is_valid);

        let invalid = validate_pre_tool_use_response(&json!({
            "permission": "later",
            "user_message": 1,
            "agent_message": false,
            "updated_input": [],
            "additional_context": 9
        }));
        assert!(!invalid.is_valid);
        assert_eq!(
            invalid.errors,
            vec![
                "Invalid permission value. Expected one of: allow, deny, ask, or undefined",
                "Invalid user_message value. Expected a string if provided",
                "Invalid agent_message value. Expected a string if provided",
                "Invalid updated_input value. Expected a plain object if provided",
                "additional_context must be a string if provided",
            ]
        );

        assert!(!validate_pre_tool_use_response(&json!([])).is_valid);
    }
}
