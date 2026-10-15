use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnMessage {
    pub speaker_kind: String,
    pub speaker_name: String,
    pub is_self: bool,
    pub text: String,
}

fn clamp_guest_name(value: &str) -> String {
    let value = value.replace(['\r', '\n'], " ").trim().chars().take(120).collect::<String>();
    if value.is_empty() { "Someone".into() } else { value }
}

pub fn normalize_turn_messages(raw: &Value) -> Vec<TurnMessage> {
    let Some(items) = raw.as_array() else { return Vec::new() };
    items.iter().take(24).filter_map(|value| {
        let object = value.as_object()?;
        let text = object.get("text")?.as_str()?.trim();
        if text.is_empty() { return None; }
        Some(TurnMessage {
            speaker_kind: if object.get("speakerKind").and_then(Value::as_str) == Some("agent") { "agent".into() } else { "human".into() },
            speaker_name: clamp_guest_name(object.get("speakerName").and_then(Value::as_str).unwrap_or("")),
            is_self: object.get("isSelf").and_then(Value::as_bool) == Some(true),
            text: text.to_string(),
        })
    }).collect()
}
