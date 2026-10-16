use serde_json::Value;

pub fn preprocess_lenient_enum_value(value: Value, options: &[&str]) -> Value {
    match value {
        Value::String(value) => {
            if options.iter().any(|option| *option == value.as_str()) {
                return Value::String(value);
            }

            let lower = value.to_lowercase();
            let matches: Vec<&str> = options
                .iter()
                .copied()
                .filter(|option| option.to_lowercase() == lower)
                .collect();

            if matches.len() == 1 {
                Value::String(matches[0].to_owned())
            } else {
                Value::String(value)
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_only_unique_case_insensitive_matches() {
        let options = ["auto", "Fast", "slow"];
        assert_eq!(
            preprocess_lenient_enum_value(json!("FAST"), &options),
            json!("Fast")
        );
        assert_eq!(
            preprocess_lenient_enum_value(json!("auto"), &options),
            json!("auto")
        );
        assert_eq!(
            preprocess_lenient_enum_value(json!("other"), &options),
            json!("other")
        );
        assert_eq!(
            preprocess_lenient_enum_value(json!(7), &options),
            json!(7)
        );

        let ambiguous = ["Fast", "FAST"];
        assert_eq!(
            preprocess_lenient_enum_value(json!("fast"), &ambiguous),
            json!("fast")
        );
    }
}
