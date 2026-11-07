use mahayana_node_agent_coordinator::mcp_lifecycle_relay::{
    HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL, MCP_INSTALL_PLUGIN_METHOD,
    MCP_LIST_CATALOG_METHOD, MCP_LIST_EFFECTIVE_PLUGINS_METHOD, MCP_LOAD_PLUGIN_SKILLS_METHOD,
    MCP_LIST_SERVERS_METHOD,
    MCP_ADD_SERVER_METHOD, MCP_REMOVE_SERVER_METHOD, MCP_RESTART_SERVERS_METHOD,
    MCP_SET_INSTRUCTIONS_METHOD, MCP_AUTHENTICATE_SERVER_METHOD, MCP_LOGOUT_ACCOUNT_METHOD,
    MCP_REMOVE_ACCOUNT_METHOD, MCP_RENAME_ACCOUNT_METHOD, MCP_NOTE_AUTH_COMPLETED_METHOD,
    MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD, MCP_UNINSTALL_PLUGIN_METHOD,
    execute_host_mcp_lifecycle_request,
    parse_host_mcp_lifecycle_request,
};
use mahayana_node_agent_coordinator::protocol::Failure;
use serde_json::json;

#[test]
fn host_mcp_lifecycle_relay_accepts_only_the_frozen_owner_commands() {
    assert_eq!(HOST_MCP_LIFECYCLE_REQUEST_EVENT_CHANNEL, "host-mcp-lifecycle-request");
    assert_eq!(MCP_RESOLVE_LIFECYCLE_GATEWAY_METHOD, "mcp.resolveLifecycleRequest");
    for method in [
        MCP_LIST_SERVERS_METHOD,
        MCP_LIST_CATALOG_METHOD,
        MCP_LIST_EFFECTIVE_PLUGINS_METHOD,
        MCP_LOAD_PLUGIN_SKILLS_METHOD,
        MCP_INSTALL_PLUGIN_METHOD,
        MCP_UNINSTALL_PLUGIN_METHOD,
        MCP_ADD_SERVER_METHOD,
        MCP_REMOVE_SERVER_METHOD,
        MCP_RESTART_SERVERS_METHOD,
        MCP_SET_INSTRUCTIONS_METHOD,
        MCP_AUTHENTICATE_SERVER_METHOD,
        MCP_LOGOUT_ACCOUNT_METHOD,
        MCP_REMOVE_ACCOUNT_METHOD,
        MCP_RENAME_ACCOUNT_METHOD,
        MCP_NOTE_AUTH_COMPLETED_METHOD,
    ] {
        let parsed = parse_host_mcp_lifecycle_request(&json!({
            "requestId": format!("mcp-{method}"),
            "method": method,
            "args": {},
        }))
        .expect("approved MCP lifecycle method");
        assert_eq!(parsed.method, method);
    }
    let denied = parse_host_mcp_lifecycle_request(&json!({
        "requestId": "mcp-denied",
        "method": "spawnLocalExecDaemon",
        "args": {},
    }))
    .expect_err("unrelated Coordinator command must be denied");
    assert_eq!(denied.code, "MCP_LIFECYCLE_RELAY_METHOD_DENIED");
}

#[test]
fn host_mcp_lifecycle_relay_preserves_request_id_and_remote_failure() {
    let success = execute_host_mcp_lifecycle_request(
        &json!({
            "requestId": "host-mcp-7",
            "method": MCP_LIST_CATALOG_METHOD,
            "args": {"forceRefresh": true},
        }),
        |method, args| {
            assert_eq!(method, MCP_LIST_CATALOG_METHOD);
            assert_eq!(args["forceRefresh"], true);
            Ok(json!([{"id":"7"}]))
        },
    )
    .expect("valid lifecycle request");
    assert_eq!(success["requestId"], "host-mcp-7");
    assert_eq!(success["ok"], true);
    assert_eq!(success["result"][0]["id"], "7");

    let failed = execute_host_mcp_lifecycle_request(
        &json!({
            "requestId": "host-mcp-8",
            "method": MCP_UNINSTALL_PLUGIN_METHOD,
            "args": {"pluginId":"8"},
        }),
        |_method, _args| Err(Failure::new("MCP_OWNER_OFFLINE", "desktop owner unavailable")),
    )
    .expect("valid lifecycle request with failed owner");
    assert_eq!(failed["requestId"], "host-mcp-8");
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["error"], "MCP_OWNER_OFFLINE: desktop owner unavailable");
}
