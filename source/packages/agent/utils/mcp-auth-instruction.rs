pub const MCP_AUTH_INSTRUCTION: &str = "If a relevant server is marked as needing authentication, or if an MCP tool call fails with an authentication/authorization error, call `mcp_auth` for that server, then inspect that server again and retry the original request if appropriate. Do not call `mcp_auth` just because it is listed, and do not repeatedly call it if authentication did not fix the failure. Do not call `mcp_auth` in parallel; authenticate only one server at a time.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_instruction_preserves_retry_and_serial_auth_policy() {
        assert!(MCP_AUTH_INSTRUCTION.contains("authentication/authorization error"));
        assert!(MCP_AUTH_INSTRUCTION.contains("retry the original request if appropriate"));
        assert!(MCP_AUTH_INSTRUCTION.contains("Do not call `mcp_auth` in parallel"));
        assert!(MCP_AUTH_INSTRUCTION.ends_with("authenticate only one server at a time."));
    }
}
