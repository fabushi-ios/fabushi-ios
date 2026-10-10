pub const SECRET_REQUEST_MAX_LABEL_LENGTH: usize = 120;
pub const SECRET_REQUEST_MAX_DESCRIPTION_LENGTH: usize = 400;

fn clamp_line(raw: &str, max_length: usize) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_length)
        .collect()
}

fn clamp_block(raw: &str, max_length: usize) -> String {
    raw.trim().chars().take(max_length).collect()
}

pub fn clamp_secret_label(value: &str) -> String {
    clamp_line(value, SECRET_REQUEST_MAX_LABEL_LENGTH)
}

pub fn clamp_secret_description(value: &str) -> String {
    clamp_block(value, SECRET_REQUEST_MAX_DESCRIPTION_LENGTH)
}

pub fn summarize_secret_request(label: &str) -> String {
    format!("Requested a secret from the user securely: {label}")
}

pub fn build_secret_provided_ack(label: &str, target_kind: &str) -> String {
    format!(
        "[The user securely provided the requested secret: \"{label}\". It was written straight to its destination ({target_kind}); you never see the value and it is not in this conversation.]\nConfirm to the user that it is set, then continue. For a connector credential, the connection links within a few seconds, so you can check and report its status."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamps_secret_metadata_and_never_mentions_a_value() {
        assert_eq!(clamp_secret_label("  API   token  "), "API token");
        assert_eq!(summarize_secret_request("API token"), "Requested a secret from the user securely: API token");
        let ack = build_secret_provided_ack("API token", "connector");
        assert!(ack.contains("you never see the value"));
        assert!(ack.contains("(connector)"));
    }
}
