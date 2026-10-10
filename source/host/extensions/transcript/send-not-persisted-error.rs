use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandSendNotPersistedError;

impl SandSendNotPersistedError {
    pub const NAME: &'static str = "SandSendNotPersistedError";
    pub const MESSAGE: &'static str =
        "Sand send could not persist to the addressed agent's store (db locked or closed); rejecting so the client retry is not swallowed.";
}
impl fmt::Display for SandSendNotPersistedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(Self::MESSAGE) }
}
impl std::error::Error for SandSendNotPersistedError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_retry_safe_failure_message() {
        assert!(SandSendNotPersistedError.to_string().contains("client retry"));
    }
}
