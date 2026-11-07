#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SandBoxStoreSyncError { pub message: String }
impl SandBoxStoreSyncError { pub fn new(message: impl Into<String>) -> Self { Self { message: message.into() } } }
