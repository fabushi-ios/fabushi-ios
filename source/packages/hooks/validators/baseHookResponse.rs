use serde_json::Value;

use crate::package_hooks_validators_base::{ValidationResult, create_validation_result, is_object};

pub fn validate_base_hook_response(value: &Value) -> ValidationResult {
    if is_object(value) {
        create_validation_result(true, Vec::new())
    } else {
        create_validation_result(false, vec!["Expected an object".to_owned()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_only_json_objects() {
        assert!(validate_base_hook_response(&json!({})).is_valid);
        let result = validate_base_hook_response(&json!([]));
        assert!(!result.is_valid);
        assert_eq!(result.errors, vec!["Expected an object"]);
    }
}
