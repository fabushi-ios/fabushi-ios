use crate::r#box::generated_production::ProductionBoxResourceAccessor;
use crate::ports::mcp_state_executor::{
    McpStateExecResult, McpStateSuccess, decode_canonical_mcp_state_result,
    encode_canonical_mcp_state_args,
};
use super::mcp_service::BoxServerStatus;
use std::sync::Mutex;

pub const MCP_TOOL_EXEC_FIELD_NUMBER: u32 = 11;
pub const MCP_STATE_EXEC_FIELD_NUMBER: u32 = 36;

pub struct ProductionBoxMcpStateLoader {
    accessor: Mutex<ProductionBoxResourceAccessor>,
}

impl ProductionBoxMcpStateLoader {
    pub fn new(accessor: ProductionBoxResourceAccessor) -> Self {
        Self {
            accessor: Mutex::new(accessor),
        }
    }

    pub fn list_servers(
        &self,
        server_identifiers: &[String],
        kick_only: bool,
    ) -> Result<Vec<BoxServerStatus>, String> {
        let args = encode_mcp_state_args(server_identifiers, kick_only);
        let bytes = self
            .accessor
            .lock()
            .map_err(|_| "production Box MCP state accessor lock poisoned".to_string())?
            .execute_raw_resource(&(), MCP_STATE_EXEC_FIELD_NUMBER, args)
            .map_err(|error| error.to_string())?;
        decode_mcp_state_result(&bytes)
    }
}

pub fn encode_mcp_state_args(server_identifiers: &[String], kick_only: bool) -> Vec<u8> {
    encode_canonical_mcp_state_args(server_identifiers, kick_only)
}

pub fn decode_mcp_state_result(bytes: &[u8]) -> Result<Vec<BoxServerStatus>, String> {
    let McpStateExecResult::Success(McpStateSuccess { servers }) =
        decode_canonical_mcp_state_result(bytes)
            .map_err(|error| format!("Box MCP state decode failed: {error}"))?;
    Ok(servers
        .into_iter()
        .map(|server| BoxServerStatus {
            server_identifier: server.server_identifier,
            status: if server.status.is_empty() {
                "connected".into()
            } else {
                server.status
            },
            status_detail: server
                .error_message
                .filter(|message| !message.trim().is_empty()),
            tool_count: server.tools.len(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_mcp_state_wire_contract_is_field_36_and_preserves_kick_only() {
        assert_eq!(MCP_STATE_EXEC_FIELD_NUMBER, 36);
        let encoded = encode_mcp_state_args(&["calendar".into(), "github".into()], true);
        let decoded = crate::ports::mcp_state_executor::decode_canonical_mcp_state_args(
            encoded.as_slice(),
        )
        .expect("decode args");
        assert_eq!(decoded.server_identifiers, vec!["calendar", "github"]);
        assert!(decoded.kick_only);
    }

    #[test]
    fn decodes_box_server_status_without_reimplementing_tool_schema() {
        let bytes = crate::ports::mcp_state_executor::encode_canonical_mcp_state_result(
            &McpStateExecResult::Success(
            McpStateSuccess {
                servers: vec![crate::ports::mcp_state_executor::McpStateServer {
                    server_name: "Calendar".into(),
                    server_identifier: "calendar".into(),
                    status: "connected".into(),
                    error_message: Some("healthy detail".into()),
                    tools: vec![
                        crate::ports::mcp_state_executor::McpStateToolDefinition {
                            name: "search".into(),
                            provider_identifier: "calendar".into(),
                            tool_name: "search".into(),
                            description: Some("Search".into()),
                            input_schema: serde_json::json!({"type":"object"}),
                        },
                        crate::ports::mcp_state_executor::McpStateToolDefinition {
                            name: "create".into(),
                            provider_identifier: "calendar".into(),
                            tool_name: "create".into(),
                            description: Some("Create".into()),
                            input_schema: serde_json::json!({"type":"object"}),
                        },
                    ],
                }],
            },
        ),
        )
        .expect("encode canonical state");
        let servers = decode_mcp_state_result(&bytes).expect("decode state");
        assert_eq!(servers[0].server_identifier, "calendar");
        assert_eq!(servers[0].status, "connected");
        assert_eq!(servers[0].tool_count, 2);
        assert_eq!(servers[0].status_detail.as_deref(), Some("healthy detail"));
    }
}


pub fn decode_hex_payload(raw: &str) -> Result<Vec<u8>, String> {
    let raw = raw.trim();
    if raw.len() % 2 != 0 {
        return Err("Box MCP protobuf hex payload must have even length".into());
    }
    raw.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char)
                .to_digit(16)
                .ok_or_else(|| "Box MCP protobuf payload contains non-hex data".to_string())?;
            let low = (pair[1] as char)
                .to_digit(16)
                .ok_or_else(|| "Box MCP protobuf payload contains non-hex data".to_string())?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

pub fn encode_hex_payload(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn execute_box_mcp_raw(
    accessor: &mut ProductionBoxResourceAccessor,
    field_number: u32,
    payload_hex: &str,
) -> Result<String, String> {
    if !matches!(field_number, MCP_TOOL_EXEC_FIELD_NUMBER | MCP_STATE_EXEC_FIELD_NUMBER) {
        return Err(format!("unsupported Box MCP ExecService field {field_number}"));
    }
    let payload = decode_hex_payload(payload_hex)?;
    let response = accessor
        .execute_raw_resource(&(), field_number, payload)
        .map_err(|error| error.to_string())?;
    Ok(encode_hex_payload(&response))
}
