#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Sand send could not persist to the addressed agent's store (db locked or closed); rejecting so the client retry is not swallowed.")]
pub struct SandSendNotPersistedError;
