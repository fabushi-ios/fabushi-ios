use serde_json::Value;

use crate::package_hooks_validators_base::ValidationResult;
use crate::package_hooks_validators_stop_response::validate_stop_response;

pub fn validate_subagent_stop_response(value: &Value) -> ValidationResult {
    validate_stop_response(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_stop_response_alias_contract() {
        assert!(validate_subagent_stop_response(&json!({"followup_message":"ok"})).is_valid);
        assert!(!validate_subagent_stop_response(&json!({"followup_message":true})).is_valid);
    }
}
