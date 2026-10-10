use serde_json::Value;

use crate::package_hooks_validators_base::{
    ValidationResult, create_validation_result, validate_optional_string,
};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_post_tool_use_failure_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value);
    if !base.is_valid {
        return base;
    }

    let object = value.as_object().expect("base hook response accepted only objects");
    let mut errors = Vec::new();
    validate_optional_string(object, "additional_context", &mut errors);
    create_validation_result(errors.is_empty(), errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_optional_additional_context_string() {
        assert!(validate_post_tool_use_failure_response(&json!({})).is_valid);
        assert!(validate_post_tool_use_failure_response(&json!({"additional_context":"ok"})).is_valid);
        let invalid = validate_post_tool_use_failure_response(&json!({"additional_context":4}));
        assert!(!invalid.is_valid);
        assert_eq!(invalid.errors, vec!["additional_context must be a string if provided"]);
    }
}
