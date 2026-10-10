pub const MAX_AGENT_STEPS: usize = 512;
pub const SEND_SLACK_MESSAGE_V2_TOOL_NAME: &str = "SendSlackMessageV2";
pub const SLACK_AGENT_TOOLS_MCP_SERVER_ID: &str = "Cursor Slack Tools";
pub const SLACK_SEND_MESSAGE_MCP_TOOL_NAME: &str = "send_slack_message";
pub const OFFER_REPOSITORY_SWITCH_TOOL_NAME: &str = "OfferRepositorySwitch";
pub const SLACK_OFFER_REPOSITORY_SWITCH_MCP_TOOL_NAME: &str = "offer_repository_switch";
pub const START_SLACK_STREAMING_TOOL_NAME: &str = "StartSlackStreaming";
pub const SLACK_START_STREAMING_MCP_TOOL_NAME: &str = "start_slack_streaming";
pub const SLACK_SET_STATUS_MCP_TOOL_NAME: &str = "set_slack_status";
pub const AUTOMATION_TOOLS_MCP_SERVER_ID: &str = "Cursor Automation Tools";
pub const NAMED_AGENT_HOME_STORE_PATH: &str = "/cursor/stores/home";
pub const NAMED_AGENT_SELF_MEMORY_FILE: &str = "SELF.md";
pub const NAMED_AGENT_STORE_SELF_PATH: &str = "/cursor/stores/home/SELF.md";
pub const NAMED_AGENT_STORE_ACTIVITY_DIR: &str = "/cursor/stores/home/activity";
pub const CURSOR_SUBSCRIPTIONS_MCP_SERVER_NAME: &str = "cursor-subscriptions";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_pinned_agent_contract() {
        assert_eq!(MAX_AGENT_STEPS, 512);
        assert_eq!(SEND_SLACK_MESSAGE_V2_TOOL_NAME, "SendSlackMessageV2");
        assert_eq!(SLACK_AGENT_TOOLS_MCP_SERVER_ID, "Cursor Slack Tools");
        assert_eq!(SLACK_SEND_MESSAGE_MCP_TOOL_NAME, "send_slack_message");
        assert_eq!(OFFER_REPOSITORY_SWITCH_TOOL_NAME, "OfferRepositorySwitch");
        assert_eq!(SLACK_OFFER_REPOSITORY_SWITCH_MCP_TOOL_NAME, "offer_repository_switch");
        assert_eq!(START_SLACK_STREAMING_TOOL_NAME, "StartSlackStreaming");
        assert_eq!(SLACK_START_STREAMING_MCP_TOOL_NAME, "start_slack_streaming");
        assert_eq!(SLACK_SET_STATUS_MCP_TOOL_NAME, "set_slack_status");
        assert_eq!(AUTOMATION_TOOLS_MCP_SERVER_ID, "Cursor Automation Tools");
        assert_eq!(NAMED_AGENT_HOME_STORE_PATH, "/cursor/stores/home");
        assert_eq!(NAMED_AGENT_SELF_MEMORY_FILE, "SELF.md");
        assert_eq!(NAMED_AGENT_STORE_SELF_PATH, "/cursor/stores/home/SELF.md");
        assert_eq!(NAMED_AGENT_STORE_ACTIVITY_DIR, "/cursor/stores/home/activity");
        assert_eq!(CURSOR_SUBSCRIPTIONS_MCP_SERVER_NAME, "cursor-subscriptions");
    }
}
