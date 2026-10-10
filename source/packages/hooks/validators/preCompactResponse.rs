use serde_json::Value;
use crate::package_hooks_validators_base::{ValidationResult, create_validation_result};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_pre_compact_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value);
    if !base.is_valid { return base; }
    let object = value.as_object().expect("base hook response accepted only objects");
    if object.is_empty() { return create_validation_result(true, Vec::new()); }
    let mut errors = Vec::new();
    if let Some(message) = object.get("user_message") {
        if !message.is_string() { errors.push("user_message must be a string".to_owned()); }
    }
    create_validation_result(errors.is_empty(), errors)
}
#[cfg(test)]
mod tests {
    use super::*; use serde_json::json;
    #[test] fn preserves_pre_compact_contract() {
        assert!(validate_pre_compact_response(&json!({})).is_valid);
        assert!(validate_pre_compact_response(&json!({"user_message":"ok"})).is_valid);
        assert!(!validate_pre_compact_response(&json!({"user_message":1})).is_valid);
    }
}
