use serde_json::Value;

use crate::package_hooks_validators_base::ValidationResult;
use crate::package_hooks_validators_base_hook_response::validate_base_hook_response;

pub fn validate_after_edit_file_response(value: &Value) -> ValidationResult {
    validate_base_hook_response(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn delegates_to_base_hook_response_contract() {
        assert!(validate_after_edit_file_response(&json!({})).is_valid);
        assert!(!validate_after_edit_file_response(&json!("not-an-object")).is_valid);
    }
}
