pub fn is_slack_v1_5_thread_bound_session(
    is_slack_v1_5: Option<bool>,
    named_agent_session_kind: Option<&str>,
) -> bool {
    if is_slack_v1_5 != Some(true) {
        return false;
    }

    let session_kind = named_agent_session_kind.unwrap_or("").trim();
    session_kind.is_empty() || session_kind == "slack_thread"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_slack_v1_5_and_blank_or_thread_session_kind() {
        assert!(is_slack_v1_5_thread_bound_session(Some(true), None));
        assert!(is_slack_v1_5_thread_bound_session(Some(true), Some("  ")));
        assert!(is_slack_v1_5_thread_bound_session(
            Some(true),
            Some(" slack_thread ")
        ));
        assert!(!is_slack_v1_5_thread_bound_session(
            Some(true),
            Some("channel")
        ));
        assert!(!is_slack_v1_5_thread_bound_session(
            Some(false),
            Some("slack_thread")
        ));
        assert!(!is_slack_v1_5_thread_bound_session(None, None));
    }
}
