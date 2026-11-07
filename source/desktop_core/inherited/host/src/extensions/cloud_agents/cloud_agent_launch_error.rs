#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SandCloudAgentLaunchError { pub message: String }
impl SandCloudAgentLaunchError { pub fn new(message: impl Into<String>) -> Self { Self { message: message.into() } } }
