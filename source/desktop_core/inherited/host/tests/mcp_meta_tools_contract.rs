use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedMcpMetaToolDefinition, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::tools::mcp_meta_tools::{
    EXTRA_LONG_TOOL_TIMEOUT_MS, LONG_TOOL_TIMEOUT_MS, SHORT_TOOL_TIMEOUT_MS,
    build_tool_call_execution_timed_out_message, create_sand_mcp_meta_tool_options,
    effective_meta_invocation_tool_name, effective_routed_tool_name, is_subagent_tool_name,
    parse_block_until_ms,
    pick_tool_call_timeout_tier_ms, suggested_tool_timeout_ms,
    tool_call_execution_guard_ms,
};
use serde_json::json;

#[test]
fn frozen_timeout_tiers_and_block_until_rules_are_preserved() {
    assert!(is_subagent_tool_name("task"));
    assert!(is_subagent_tool_name("MCP_TASK"));
    assert!(!is_subagent_tool_name("shell"));

    assert_eq!(parse_block_until_ms(&json!({"block_until_ms": 120000})), Some(120_000));
    assert_eq!(parse_block_until_ms(&json!({"block_until_ms": -1})), None);
    assert_eq!(parse_block_until_ms(&json!({"block_until_ms": "120000"})), None);

    assert_eq!(
        suggested_tool_timeout_ms("subagent", &json!({})),
        LONG_TOOL_TIMEOUT_MS
    );
    assert_eq!(
        suggested_tool_timeout_ms("github_search", &json!({})),
        SHORT_TOOL_TIMEOUT_MS
    );
    assert_eq!(
        pick_tool_call_timeout_tier_ms(EXTRA_LONG_TOOL_TIMEOUT_MS + 1),
        EXTRA_LONG_TOOL_TIMEOUT_MS
    );
    assert_eq!(
        tool_call_execution_guard_ms("github_search", &json!({}), false),
        SHORT_TOOL_TIMEOUT_MS - 60_000
    );
    assert_eq!(
        tool_call_execution_guard_ms(
            "shell",
            &json!({"block_until_ms": 20 * 60 * 1000}),
            false,
        ),
        30 * 60 * 1000 - 60_000
    );
    assert_eq!(
        tool_call_execution_guard_ms("anything", &json!({}), true),
        LONG_TOOL_TIMEOUT_MS - 60_000
    );

    let message = build_tool_call_execution_timed_out_message("shell", 1_000);
    assert!(message.contains("timed out after 1 seconds"));
    assert!(message.contains("block_until_ms"));
}

#[test]
fn mcp_descriptors_group_by_server_and_sort_tools_like_frozen_meta_options() {
    let tools = vec![
        RoutedMcpMetaToolDefinition {
            tool: RoutedToolDefinition {
                name: "github_zeta".into(),
                provider_identifier: "github".into(),
                tool_name: "zeta".into(),
                description: Some("z".into()),
                input_schema: json!({"type":"object","z":true}),
            },
            plugin: Some(json!({"slug":"github"})),
            marketplace: Some(json!({"name":"team-market"})),
            plugin_id: Some("plugin-db-1".into()),
            marketplace_id: Some("market-1".into()),
        },
        RoutedMcpMetaToolDefinition {
            tool: RoutedToolDefinition {
                name: "notion_query".into(),
                provider_identifier: "notion".into(),
                tool_name: "query".into(),
                description: None,
                input_schema: json!({"type":"object"}),
            },
            plugin: None,
            marketplace: None,
            plugin_id: None,
            marketplace_id: None,
        },
        RoutedMcpMetaToolDefinition {
            tool: RoutedToolDefinition {
                name: "github_alpha".into(),
                provider_identifier: "github".into(),
                tool_name: "alpha".into(),
                description: Some("a".into()),
                input_schema: json!({"type":"object","a":true}),
            },
            plugin: Some(json!({"slug":"github"})),
            marketplace: Some(json!({"name":"team-market"})),
            plugin_id: Some("plugin-db-1".into()),
            marketplace_id: Some("market-1".into()),
        },
    ];
    let options = create_sand_mcp_meta_tool_options(&tools);
    assert!(options.enabled);
    assert_eq!(options.mcp_descriptors.len(), 2);
    assert_eq!(options.mcp_descriptors[0].server_identifier, "github");
    assert_eq!(
        options.mcp_descriptors[0]
            .tools
            .iter()
            .map(|tool| tool.tool_name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    assert_eq!(options.mcp_descriptors[1].server_identifier, "notion");
    assert_eq!(options.mcp_descriptors[0].plugin_db_id.as_deref(), Some("plugin-db-1"));
    assert_eq!(options.mcp_descriptors[0].marketplace_id.as_deref(), Some("market-1"));
    assert_eq!(
        options.mcp_descriptors[0].plugin,
        Some(json!({"slug":"github"}))
    );
    assert_eq!(
        options.mcp_descriptors[0].marketplace,
        Some(json!({"name":"team-market"}))
    );
}

#[test]
fn routed_tool_effective_name_prefers_provider_tool_name() {
    let tool = RoutedToolDefinition {
        name: "github_search".into(),
        provider_identifier: "github".into(),
        tool_name: "search".into(),
        description: None,
        input_schema: json!({}),
    };
    assert_eq!(effective_routed_tool_name(&tool), "search");

    let fallback = RoutedToolDefinition {
        tool_name: " ".into(),
        ..tool
    };
    assert_eq!(effective_routed_tool_name(&fallback), "github_search");

    let dynamic = RoutedToolDefinition {
        name: "CallMcpTool".into(),
        provider_identifier: "mahayana-mcp-meta".into(),
        tool_name: "CallMcpTool".into(),
        description: None,
        input_schema: json!({}),
    };
    assert_eq!(
        effective_meta_invocation_tool_name(
            &dynamic,
            &json!({"server":"github","toolName":"task"})
        ),
        "task"
    );

    let _type_anchor: Option<ProviderSessionError> = None;
}
