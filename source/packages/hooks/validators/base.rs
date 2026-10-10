use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<String>,
}

pub fn is_string(value: &Value) -> bool {
    value.is_string()
}

pub fn is_object(value: &Value) -> bool {
    value.is_object()
}

pub fn create_validation_result(is_valid: bool, errors: Vec<String>) -> ValidationResult {
    ValidationResult { is_valid, errors }
}

pub fn validate_optional_string(
    object: &Map<String, Value>,
    field_name: &str,
    errors: &mut Vec<String>,
) {
    if let Some(value) = object.get(field_name) {
        if !is_string(value) {
            errors.push(format!("{field_name} must be a string if provided"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base_object_and_optional_string_contract_matches_reference() {
        assert!(is_object(&json!({"a": 1})));
        assert!(!is_object(&json!([1, 2])));
        assert!(!is_object(&Value::Null));
        assert!(is_string(&json!("value")));

        let object = json!({"additional_context": 4}).as_object().unwrap().clone();
        let mut errors = Vec::new();
        validate_optional_string(&object, "additional_context", &mut errors);
        assert_eq!(errors, vec!["additional_context must be a string if provided"]);
    }
}
