use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandLocalExecError { pub message: String }

impl SandLocalExecError {
    pub const NAME: &'static str = "SandLocalExecError";
    pub fn new(message: impl Into<String>) -> Self { Self { message: message.into() } }
}
impl fmt::Display for SandLocalExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for SandLocalExecError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_local_exec_error_identity_and_message() {
        let error = SandLocalExecError::new("runner unavailable");
        assert_eq!(SandLocalExecError::NAME, "SandLocalExecError");
        assert_eq!(error.to_string(), "runner unavailable");
    }
}
