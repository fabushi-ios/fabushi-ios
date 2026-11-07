use serde_json::Value;

/// Convert a camelCase identifier to snake_case.
pub fn camel_to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Coerce a JSON value into a boolean following standard OpenClaw normalization rules.
pub fn parse_boolean(val: &Value) -> Option<bool> {
    match val {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => {
            if n.as_i64() == Some(1) || n.as_f64() == Some(1.0) {
                Some(true)
            } else if n.as_i64() == Some(0) || n.as_f64() == Some(0.0) {
                Some(false)
            } else {
                None
            }
        }
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Read boolean or string params from exact or snake_case tool-input keys.
///
/// Prefers the exact key over the snake_case variant.
pub fn read_boolean_param(params: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    if let Some(val) = params.get(key) {
        if !val.is_null() {
            return parse_boolean(val);
        }
    }

    let snake = camel_to_snake_case(key);
    if let Some(val) = params.get(&snake) {
        if !val.is_null() {
            return parse_boolean(val);
        }
    }

    None
}
