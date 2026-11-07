pub const SECRET_REQUEST_MAX_LABEL_LENGTH: usize = 120;
pub const SECRET_REQUEST_MAX_DESCRIPTION_LENGTH: usize = 400;

pub fn clamp_secret_label(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SECRET_REQUEST_MAX_LABEL_LENGTH)
        .collect()
}

pub fn clamp_secret_description(value: &str) -> String {
    value.trim().chars().take(SECRET_REQUEST_MAX_DESCRIPTION_LENGTH).collect()
}

pub fn summarize_secret_request(label: &str) -> String {
    format!("Requested a secret from the user securely: {label}")
}

pub fn build_secret_provided_ack(label: &str, target_kind: &str) -> String {
    format!(
        "[The user securely provided the requested secret: \"{label}\". It was written straight to its destination ({target_kind}); you never see the value and it is not in this conversation.]\nConfirm to the user that it is set, then continue. For a connector credential, the connection links within a few seconds, so you can check and report its status."
    )
}
