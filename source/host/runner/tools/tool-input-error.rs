use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandToolInputError { pub message: String }

impl SandToolInputError {
    pub const NAME: &'static str = "SandToolInputError";
    pub fn new(message: impl Into<String>) -> Self { Self { message: message.into() } }
}
impl fmt::Display for SandToolInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for SandToolInputError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_input_error_keeps_grok_name() {
        let error = SandToolInputError::new("invalid input");
        assert_eq!(SandToolInputError::NAME, "SandToolInputError");
        assert_eq!(error.to_string(), "invalid input");
    }
}
