use serde_json::Value;

use crate::package_hooks_validators_base::{ValidationResult, create_validation_result};
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_stop_response(value: &Value) -> ValidationResult {
    let base = validate_base_hook_response(value);
    if !base.is_valid {
        return base;
    }

    let object = value.as_object().expect("base hook response accepted only objects");
    let mut errors = Vec::new();
    if let Some(followup) = object.get("followup_message") {
        if !followup.is_string() {
            errors.push("followup_message must be a string if provided".to_owned());
        }
    }
    create_validation_result(errors.is_empty(), errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validates_optional_followup_message() {
        assert!(validate_stop_response(&json!({})).is_valid);
        assert!(validate_stop_response(&json!({"followup_message":"continue"})).is_valid);
        let invalid = validate_stop_response(&json!({"followup_message":7}));
        assert!(!invalid.is_valid);
        assert_eq!(invalid.errors, vec!["followup_message must be a string if provided"]);
    }
}
