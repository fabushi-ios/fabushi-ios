use std::error::Error;
use std::fmt;
use std::future::Future;

pub const HOOK_SETTINGS_HINT: &str =
    "To view or modify configured hooks, go to Cursor Settings > Hooks.";
pub const HOOK_DENIAL_AGENT_NOTE: &str =
    "Agent note: Do not suggest workarounds to the blocked tool.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDeniedError {
    pub failure_type: &'static str,
    pub reason: String,
}

impl HookDeniedError {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            failure_type: "permission_denied",
            reason: reason.into(),
        }
    }
}

impl fmt::Display for HookDeniedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Hook denied: {}", self.reason)
    }
}

impl Error for HookDeniedError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailClosedError {
    pub failure_type: &'static str,
    pub reason: String,
    pub cause: String,
}

impl FailClosedError {
    pub fn new(reason: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            failure_type: "error",
            reason: reason.into(),
            cause: cause.into(),
        }
    }
}

impl fmt::Display for FailClosedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Hook failed (fail-closed): {}", self.reason)
    }
}

impl Error for FailClosedError {}

pub async fn with_fail_closed<T, E, F>(
    future: F,
    action_description: Option<&str>,
) -> Result<T, FailClosedError>
where
    E: fmt::Display,
    F: Future<Output = Result<T, E>>,
{
    match future.await {
        Ok(value) => Ok(value),
        Err(error) => {
            let error_message = error.to_string();
            let reason = create_hook_fail_closed_message(
                action_description,
                Some(error_message.as_str()),
            );
            Err(FailClosedError::new(reason, error_message))
        }
    }
}

pub fn append_agent_denial_note(message: &str) -> String {
    if message.contains(HOOK_DENIAL_AGENT_NOTE) {
        return message.to_owned();
    }
    format!("{message}\n\n{HOOK_DENIAL_AGENT_NOTE}")
}

pub fn create_hook_denial_message(
    action_description: &str,
    user_message: Option<&str>,
) -> String {
    let base_message = match user_message.filter(|message| !message.is_empty()) {
        Some(message) => format!(
            "{action_description} was blocked by a hook: {message}"
        ),
        None => format!("{action_description} was blocked by a hook."),
    };
    append_agent_denial_note(&format!(
        "{base_message}\n\n{HOOK_SETTINGS_HINT}"
    ))
}

pub fn create_hook_fail_closed_message(
    action_description: Option<&str>,
    error_message: Option<&str>,
) -> String {
    let action = action_description.unwrap_or("Action");
    let error_detail = match error_message.filter(|message| !message.is_empty()) {
        Some(message) => format!(": {message}"),
        None => ".".to_owned(),
    };
    format!(
        "{action} was blocked because a configured hook failed to execute{error_detail}\n\nThis is a safety measure (fail-closed) - when hooks cannot be evaluated, the action is blocked to prevent potentially unsafe operations.\n\n{HOOK_SETTINGS_HINT}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denial_message_preserves_user_reason_settings_hint_and_agent_note() {
        let message = create_hook_denial_message("Shell command", Some("policy denied"));
        assert!(message.contains("Shell command was blocked by a hook: policy denied"));
        assert!(message.contains(HOOK_SETTINGS_HINT));
        assert!(message.ends_with(HOOK_DENIAL_AGENT_NOTE));
        assert_eq!(message.matches(HOOK_DENIAL_AGENT_NOTE).count(), 1);
    }

    #[test]
    fn appending_agent_note_is_idempotent() {
        let once = append_agent_denial_note("blocked");
        let twice = append_agent_denial_note(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn fail_closed_message_uses_reference_defaults() {
        assert_eq!(
            create_hook_fail_closed_message(None, None),
            format!(
                "Action was blocked because a configured hook failed to execute.\n\nThis is a safety measure (fail-closed) - when hooks cannot be evaluated, the action is blocked to prevent potentially unsafe operations.\n\n{HOOK_SETTINGS_HINT}"
            )
        );
    }

    #[tokio::test]
    async fn with_fail_closed_wraps_the_original_failure() {
        let result = with_fail_closed(
            async { Err::<(), _>("hook process unavailable") },
            Some("Edit file"),
        )
        .await
        .unwrap_err();

        assert_eq!(result.failure_type, "error");
        assert_eq!(result.cause, "hook process unavailable");
        assert!(result.reason.starts_with(
            "Edit file was blocked because a configured hook failed to execute: hook process unavailable"
        ));
        assert_eq!(
            result.to_string(),
            format!("Hook failed (fail-closed): {}", result.reason)
        );
    }

    #[test]
    fn denied_error_matches_reference_failure_type_and_display() {
        let error = HookDeniedError::new("tool policy");
        assert_eq!(error.failure_type, "permission_denied");
        assert_eq!(error.reason, "tool policy");
        assert_eq!(error.to_string(), "Hook denied: tool policy");
    }
}
