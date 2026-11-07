#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SandGatewayCommandError {
    pub message: String,
}

impl SandGatewayCommandError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GatewayError {
    #[error("gateway is unreachable: {0}")]
    Unreachable(String),
    #[error("{0}")]
    Command(#[from] SandGatewayCommandError),
    #[error("gateway protocol error: {0}")]
    Protocol(String),
}
