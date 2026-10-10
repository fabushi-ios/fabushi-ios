use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookAdditionalContextTooLargeError {
    pub hook_event_name: String,
    pub actual_length: usize,
    pub max_length: usize,
}

impl HookAdditionalContextTooLargeError {
    pub fn new(
        hook_event_name: impl Into<String>,
        actual_length: usize,
        max_length: usize,
    ) -> Self {
        Self {
            hook_event_name: hook_event_name.into(),
            actual_length,
            max_length,
        }
    }
}

impl fmt::Display for HookAdditionalContextTooLargeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Hook additional_context for {} is {} chars (max {}).",
            self.hook_event_name, self.actual_length, self.max_length
        )
    }
}

impl Error for HookAdditionalContextTooLargeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_preserves_recovered_fields_and_message() {
        let error = HookAdditionalContextTooLargeError::new("beforePromptSubmit", 300, 256);
        assert_eq!(error.hook_event_name, "beforePromptSubmit");
        assert_eq!(error.actual_length, 300);
        assert_eq!(error.max_length, 256);
        assert_eq!(
            error.to_string(),
            "Hook additional_context for beforePromptSubmit is 300 chars (max 256)."
        );
    }
}
