use serde_json::Value;

/// Mirrors Grok's preprocessLenientBoolean at the JSON request boundary.
pub fn preprocess_lenient_boolean(value: Value) -> Value {
    match value {
        Value::String(value) if value == "true" => Value::Bool(true),
        Value::String(value) if value == "false" => Value::Bool(false),
        other => other,
    }
}

/// Mirrors z.preprocess(..., z.boolean()) for JSON-shaped tool arguments.
pub fn lenient_boolean(value: Value) -> Result<bool, Value> {
    match preprocess_lenient_boolean(value) {
        Value::Bool(value) => Ok(value),
        other => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn coerces_only_exact_true_and_false_strings() {
        assert_eq!(preprocess_lenient_boolean(json!("true")), json!(true));
        assert_eq!(preprocess_lenient_boolean(json!("false")), json!(false));
        assert_eq!(preprocess_lenient_boolean(json!("TRUE")), json!("TRUE"));
        assert_eq!(preprocess_lenient_boolean(json!(1)), json!(1));
        assert_eq!(preprocess_lenient_boolean(json!(null)), json!(null));
    }

    #[test]
    fn boolean_validator_accepts_native_and_lenient_string_values() {
        assert_eq!(lenient_boolean(json!(true)), Ok(true));
        assert_eq!(lenient_boolean(json!("false")), Ok(false));
        assert_eq!(lenient_boolean(json!("yes")), Err(json!("yes")));
    }
}
