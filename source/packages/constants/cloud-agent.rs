pub const ENV_SETUP_SKILL_ID: &str = "env-setup";
pub const ENV_SETUP_MANAGED_SKILL_DIRECTORY: &str = "/.cursor/skills-cursor/env-setup/";
pub const ENV_SETUP_MANAGED_SKILL_PATH: &str = "/.cursor/skills-cursor/env-setup/SKILL.md";
pub const CLOUD_AGENT_SINGLE_REPO_WORKSPACE_ROOT: &str = "/workspace";
pub const CLOUD_AGENT_ARTIFACTS_DIR: &str = "/opt/cursor/artifacts/";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_reference_cloud_paths() {
        assert!(ENV_SETUP_MANAGED_SKILL_DIRECTORY.contains(ENV_SETUP_SKILL_ID));
        assert_eq!(ENV_SETUP_MANAGED_SKILL_PATH, format!("{ENV_SETUP_MANAGED_SKILL_DIRECTORY}SKILL.md"));
        assert_eq!(CLOUD_AGENT_SINGLE_REPO_WORKSPACE_ROOT, "/workspace");
        assert_eq!(CLOUD_AGENT_ARTIFACTS_DIR, "/opt/cursor/artifacts/");
    }
}
