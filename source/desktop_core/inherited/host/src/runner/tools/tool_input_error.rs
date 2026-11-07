#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SandToolInputError {
    pub message: String,
}

impl SandToolInputError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }

    pub const fn name(&self) -> &'static str {
        "SandToolInputError"
    }
}
