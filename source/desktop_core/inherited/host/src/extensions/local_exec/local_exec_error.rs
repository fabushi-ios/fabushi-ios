#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SandLocalExecError {
    pub message: String,
}

impl SandLocalExecError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}
