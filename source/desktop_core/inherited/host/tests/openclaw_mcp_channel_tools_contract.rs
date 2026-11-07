//! OpenClaw MCP Channel Bridge & Tools Stdio Server Contract Test Suite
//!
//! Verifies OCRM-RESP-MCP-CHANNEL-BRIDGE-001 & OCRM-RESP-MCP-TOOLS-STDIO-SERVER-001:
//! - Claude Desktop permission reply regex parsing & shortcode validation
//! - Bounded event queueing (capped at 1,000 items)
//! - Pending approval TTL tracking and expiration sweep
//! - Stdio MCP tools server lifecycle work tracking and close draining
//! - Tool policy allow/deny filtering

use mahayana_host_runtime::extensions::mcp::channel_bridge::*;
use mahayana_host_runtime::extensions::mcp::tools_stdio_server::*;

#[test]
fn test_ocrm_mcp_claude_permission_reply_parsing() {
    // Valid yes/no with 5-letter alphabet [a-km-z] (excludes 'l')
    assert_eq!(
        parse_claude_permission_reply("yes abcde"),
        Some((true, "abcde".to_string()))
    );
    assert_eq!(
        parse_claude_permission_reply("NO xyzwk"),
        Some((false, "xyzwk".to_string()))
    );
    assert_eq!(
        parse_claude_permission_reply("  Yes   mnpqr  "),
        Some((true, "mnpqr".to_string()))
    );

    // Invalid: contains excluded letter 'l'
    assert_eq!(parse_claude_permission_reply("yes abcle"), None);

    // Invalid: length not 5
    assert_eq!(parse_claude_permission_reply("yes abcdef"), None);
    assert_eq!(parse_claude_permission_reply("yes abcd"), None);

    // Invalid: contains numbers or punctuation
    assert_eq!(parse_claude_permission_reply("yes abc1e"), None);
    assert_eq!(parse_claude_permission_reply("maybe abcde"), None);
}

#[test]
fn test_ocrm_mcp_queue_bounding_and_approval_sweep() {
    let mut state = McpChannelBridgeState::new();

    // Push 1,005 events -> should be clamped to MCP_QUEUE_LIMIT (1,000)
    for i in 0..1005 {
        state.push_event(McpQueueEvent {
            id: format!("evt-{i}"),
            event_type: "chat.message".to_string(),
            timestamp_ms: 1000 + i as u64,
            payload: serde_json::json!({ "seq": i }),
        });
    }

    assert_eq!(state.events.len(), MCP_QUEUE_LIMIT);
    // Oldest 5 (evt-0 through evt-4) should have been evicted
    assert_eq!(state.events.front().unwrap().id, "evt-5");
    assert_eq!(state.events.back().unwrap().id, "evt-1004");

    // Approvals sweep
    let now = 10_000_000;
    state.approvals.push(McpPendingApproval {
        approval_id: "appr-valid".to_string(),
        channel: "discord".to_string(),
        session_key: "sess-1".to_string(),
        code: Some("abcde".to_string()),
        tracked_at_ms: now,
        expires_at_ms: now + 60_000, // Valid
    });
    state.approvals.push(McpPendingApproval {
        approval_id: "appr-expired".to_string(),
        channel: "telegram".to_string(),
        session_key: "sess-2".to_string(),
        code: Some("xyzmn".to_string()),
        tracked_at_ms: now - 100_000,
        expires_at_ms: now - 1_000, // Expired
    });

    let pruned = state.sweep_expired_approvals(now);
    assert_eq!(pruned, 1);
    assert_eq!(state.approvals.len(), 1);
    assert_eq!(state.approvals[0].approval_id, "appr-valid");
}

#[test]
fn test_ocrm_mcp_tools_server_lifecycle() {
    let lifecycle = McpToolsServerLifecycle::new();
    assert!(!lifecycle.is_closing());
    assert_eq!(lifecycle.active_count(), 0);

    // Enter request succeeds before close
    assert!(lifecycle.enter_request().is_ok());
    assert_eq!(lifecycle.active_count(), 1);
    lifecycle.exit_request();
    assert_eq!(lifecycle.active_count(), 0);

    // Initiate close
    lifecycle.close();
    assert!(lifecycle.is_closing());

    // Enter request fails after close
    assert_eq!(lifecycle.enter_request(), Err("Connection closed"));
}

#[test]
fn test_ocrm_mcp_tool_policy_filtering() {
    let tools = vec![
        McpToolDescriptor {
            name: "memory_recall".to_string(),
            description: Some("Recall memory".to_string()),
            input_schema: serde_json::json!({}),
        },
        McpToolDescriptor {
            name: "memory_store".to_string(),
            description: Some("Store memory".to_string()),
            input_schema: serde_json::json!({}),
        },
        McpToolDescriptor {
            name: "bash_exec".to_string(),
            description: Some("Execute bash command".to_string()),
            input_schema: serde_json::json!({}),
        },
    ];

    let policy = McpToolPolicy {
        allow: vec!["memory_*".to_string(), "memory_recall".to_string(), "memory_store".to_string()],
        deny: vec!["bash_exec".to_string()],
    };

    let filtered = filter_tools_for_mcp(&tools, &policy);
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].name, "memory_recall");
    assert_eq!(filtered[1].name, "memory_store");
}
