use serde_json::{Map, Value};

pub const MAX_TOOL_ANNOTATION_TITLE_LENGTH: usize = 256;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolAnnotations {
    pub title: Option<String>,
    pub read_only_hint: Option<bool>,
    pub destructive_hint: Option<bool>,
    pub idempotent_hint: Option<bool>,
    pub open_world_hint: Option<bool>,
}

pub fn tool_annotations_json_field(annotations: Option<&ToolAnnotations>) -> Option<String> {
    let annotations = annotations?;
    let mut bounded = Map::new();

    if let Some(title) = annotations.title.as_deref() {
        if !title.is_empty() {
            let bounded_title = title
                .chars()
                .take(MAX_TOOL_ANNOTATION_TITLE_LENGTH)
                .collect::<String>();
            bounded.insert("title".to_owned(), Value::String(bounded_title));
        }
    }

    for (key, value) in [
        ("readOnlyHint", annotations.read_only_hint),
        ("destructiveHint", annotations.destructive_hint),
        ("idempotentHint", annotations.idempotent_hint),
        ("openWorldHint", annotations.open_world_hint),
    ] {
        if let Some(value) = value {
            bounded.insert(key.to_owned(), Value::Bool(value));
        }
    }

    if bounded.is_empty() {
        None
    } else {
        Some(Value::Object(bounded).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn omits_absent_and_empty_annotations() {
        assert_eq!(tool_annotations_json_field(None), None);
        assert_eq!(
            tool_annotations_json_field(Some(&ToolAnnotations::default())),
            None
        );
    }

    #[test]
    fn bounds_title_and_serializes_only_known_boolean_hints() {
        let annotations = ToolAnnotations {
            title: Some("x".repeat(300)),
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            idempotent_hint: None,
            open_world_hint: Some(true),
        };
        let encoded = tool_annotations_json_field(Some(&annotations)).unwrap();
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            value.get("title").and_then(Value::as_str).unwrap().chars().count(),
            MAX_TOOL_ANNOTATION_TITLE_LENGTH
        );
        assert_eq!(value.get("readOnlyHint"), Some(&json!(true)));
        assert_eq!(value.get("destructiveHint"), Some(&json!(false)));
        assert_eq!(value.get("openWorldHint"), Some(&json!(true)));
        assert!(value.get("idempotentHint").is_none());
    }
}
