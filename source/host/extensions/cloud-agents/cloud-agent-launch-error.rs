use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandCloudAgentLaunchError { pub message: String }

impl SandCloudAgentLaunchError {
    pub const NAME: &'static str = "SandCloudAgentLaunchError";
    pub fn new(message: impl Into<String>) -> Self { Self { message: message.into() } }
}
impl fmt::Display for SandCloudAgentLaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for SandCloudAgentLaunchError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_cloud_agent_launch_error_identity() {
        let error = SandCloudAgentLaunchError::new("launch failed");
        assert_eq!(SandCloudAgentLaunchError::NAME, "SandCloudAgentLaunchError");
        assert_eq!(error.to_string(), "launch failed");
    }
}
