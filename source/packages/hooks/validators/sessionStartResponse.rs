use serde_json::Value;
use crate::package_hooks_validators_base::{ValidationResult, create_validation_result};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_session_start_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value); if !base.is_valid { return base; }
    let object = value.as_object().expect("base hook response accepted only objects");
    let mut errors = Vec::new();

    if let Some(env) = object.get("env") {
        match env.as_object() {
            None => errors.push("env must be an object if provided".to_owned()),
            Some(entries) => for (key, item) in entries {
                if !item.is_string() { errors.push(format!("env value for \"{key}\" must be a string")); }
            }
        }
    }
    if let Some(item) = object.get("additional_context") {
        if !item.is_string() { errors.push("additional_context must be a string if provided".to_owned()); }
    }
    if let Some(item) = object.get("continue") {
        if !item.is_boolean() { errors.push("continue must be a boolean if provided".to_owned()); }
    }
    if let Some(item) = object.get("user_message") {
        if !item.is_string() { errors.push("user_message must be a string if provided".to_owned()); }
    }
    create_validation_result(errors.is_empty(), errors)
}
#[cfg(test)]
mod tests {
    use super::*; use serde_json::json;
    #[test] fn validates_session_start_fields() {
        assert!(validate_session_start_response(&json!({"env":{"A":"1"},"continue":true})).is_valid);
        assert!(!validate_session_start_response(&json!({"env":{"A":1}})).is_valid);
        assert!(!validate_session_start_response(&json!({"continue":"yes"})).is_valid);
    }
}
