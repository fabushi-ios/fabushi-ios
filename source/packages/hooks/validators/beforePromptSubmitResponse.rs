use serde_json::Value;
use crate::package_hooks_validators_base::{ValidationResult, create_validation_result};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_before_prompt_submit_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value); if !base.is_valid { return base; }
    let object = value.as_object().expect("base hook response accepted only objects");
    let mut errors = Vec::new();
    if let Some(continue_value) = object.get("continue") {
        if !continue_value.is_boolean() { errors.push("continue must be a boolean if provided".to_owned()); }
    }
    for field in ["user_message", "additional_context"] {
        if let Some(item) = object.get(field) {
            if !item.is_string() { errors.push(format!("{field} must be a string if provided")); }
        }
    }
    create_validation_result(errors.is_empty(), errors)
}
#[cfg(test)]
mod tests {
    use super::*; use serde_json::json;
    #[test] fn validates_continue_and_text_fields() {
        assert!(validate_before_prompt_submit_response(&json!({"continue":true,"additional_context":"x"})).is_valid);
        assert!(!validate_before_prompt_submit_response(&json!({"continue":"yes"})).is_valid);
        assert!(!validate_before_prompt_submit_response(&json!({"user_message":5})).is_valid);
    }
}
