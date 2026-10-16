use serde_json::Value;

use crate::package_hooks_validators_base::ValidationResult;
use crate::package_hooks_validators_post_tool_use_failure_response::validate_post_tool_use_failure_response;

pub fn validate_post_tool_use_response(value: &Value) -> ValidationResult {
    validate_post_tool_use_failure_response(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_post_tool_use_failure_validator_alias() {
        assert!(validate_post_tool_use_response(&json!({"additional_context":"ok"})).is_valid);
        assert!(!validate_post_tool_use_response(&json!({"additional_context":false})).is_valid);
    }
}
