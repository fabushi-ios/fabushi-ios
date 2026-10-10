use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandBoxStoreSyncError { pub message: String }

impl SandBoxStoreSyncError {
    pub const NAME: &'static str = "SandBoxStoreSyncError";
    pub fn new(message: impl Into<String>) -> Self { Self { message: message.into() } }
}
impl fmt::Display for SandBoxStoreSyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for SandBoxStoreSyncError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_box_store_sync_error_contract() {
        let error = SandBoxStoreSyncError::new("sync failed");
        assert_eq!(SandBoxStoreSyncError::NAME, "SandBoxStoreSyncError");
        assert_eq!(error.to_string(), "sync failed");
    }
}
