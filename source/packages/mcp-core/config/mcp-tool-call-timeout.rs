pub const DEFAULT_MCP_TOOL_CALL_MAX_TOTAL_TIMEOUT_MS: u64 = 60 * 60_000;
pub const LEGACY_MCP_TOOL_CALL_TIMEOUT_MS: u64 = 60 * 60_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_timeouts_match_grok_reference() {
        assert_eq!(DEFAULT_MCP_TOOL_CALL_MAX_TOTAL_TIMEOUT_MS, 3_600_000);
        assert_eq!(LEGACY_MCP_TOOL_CALL_TIMEOUT_MS, 3_600_000);
    }
}
