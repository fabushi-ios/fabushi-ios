use serde_json::{Map, Value};

pub const MAX_SEARCH_RESULTS: usize = 200;

pub fn required_text(params: &Map<String, Value>, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} is required"))
}

pub fn bounded_limit(params: &Map<String, Value>) -> usize {
    params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(MAX_SEARCH_RESULTS as u64)
        .clamp(1, MAX_SEARCH_RESULTS as u64) as usize
}

pub fn scope_key(account_key: &str, conversation_id: &str) -> Result<String, String> {
    let account_key = account_key.trim();
    let conversation_id = conversation_id.trim();
    if account_key.is_empty() {
        return Err("accountKey is required".into());
    }
    if conversation_id.is_empty() {
        return Err("conversationId is required".into());
    }
    if account_key.contains('\u{1f}') || conversation_id.contains('\u{1f}') {
        return Err("content-search scope contains reserved separator".into());
    }
    Ok(format!("{account_key}\u{1f}{conversation_id}"))
}

pub fn conversation_id_from_scope(scope_id: &str) -> &str {
    scope_id
        .split_once('\u{1f}')
        .map(|(_, value)| value)
        .unwrap_or(scope_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_and_conversation_form_one_fenced_scope() {
        assert_eq!(
            scope_key("account-a", "conversation-1").unwrap(),
            "account-a\u{1f}conversation-1"
        );
        assert_ne!(
            scope_key("account-a", "conversation-1").unwrap(),
            scope_key("account-b", "conversation-1").unwrap()
        );
        assert_eq!(
            conversation_id_from_scope("account-a\u{1f}conversation-1"),
            "conversation-1"
        );
    }
}
