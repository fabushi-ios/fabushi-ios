use mahayana_host_runtime::extensions::transcript::client_side_tool_v2_inventory::*;

#[test]
fn supported_projection_inventory_is_explicit_and_members_exist_in_both_frozen_unions() {
    assert_eq!(supported_projection("shellToolCall"),Some("RUN_TERMINAL_COMMAND_V2"));
    assert_eq!(supported_projection("mcpToolCall"),Some("CALL_MCP_TOOL"));
    assert!(supported_projection("sendMessageToolCall").is_none());

    for (source,target) in CLIENT_SIDE_TOOL_V2_SUPPORTED {
        assert!(SHIPPED_AGENT_TOOL_CALL_UNION.contains(source),"{source} missing from agent union");
        let enum_name=target.split_whitespace().next().unwrap();
        assert!(SHIPPED_CLIENT_SIDE_TOOL_V2_UNION.contains(&enum_name),"{enum_name} missing from ClientSideToolV2 union");
    }
}

#[test]
fn ordinary_transcript_exclusions_and_unrecovered_policy_fail_closed() {
    assert!(is_ordinary_transcript_only("sendMessageToolCall"));
    assert!(is_ordinary_transcript_only("approvalInteractions"));
    assert!(is_ordinary_transcript_only("awaitToolCall"));
    assert!(is_unprojected_agent_tool("deleteToolCall"));
    assert!(is_unprojected_client_side_variant("READ_FILE"));
    assert!(!is_unprojected_client_side_variant("READ_FILE_V2"));
    assert_eq!(
        CLIENT_SIDE_TOOL_V2_UNRECOVERED_POLICY,
        "not projected until each agent result can be losslessly matched to a generated ClientSideToolV2 result"
    );
    assert_eq!(
        CLIENT_SIDE_TOOL_V2_READ_BINARY_OUTPUTS_POLICY,
        "READ_FILE_V2 carries text; binary/blob identifiers remain in ordinary transcript"
    );
}

#[test]
fn unprojected_entries_are_real_shipped_variants_not_synthetic_names() {
    for name in UNPROJECTED_AGENT_TOOL_CALL_ONEOFS {
        assert!(SHIPPED_AGENT_TOOL_CALL_UNION.contains(name),"{name}");
        assert!(supported_projection(name).is_none(),"{name} unexpectedly projected");
    }
    for name in UNPROJECTED_CLIENT_SIDE_TOOL_V2_VARIANTS {
        assert!(SHIPPED_CLIENT_SIDE_TOOL_V2_UNION.contains(name),"{name}");
    }
}
