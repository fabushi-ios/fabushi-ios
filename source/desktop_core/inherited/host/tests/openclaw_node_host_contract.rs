use mahayana_host_runtime::extensions::node_host::gateway_connection::{
    can_reuse_node_host_device_token, gateway_origin_scope, NodeHostGatewayConfig,
};
use mahayana_host_runtime::extensions::node_host::command_dispatch::{
    dispatch_node_worker_computer_command, dispatch_node_worker_computer_command_with_executor,
    ComputerCommandInput, ComputerOperation, NodeWorkerComputerExecutor,
    COMPUTER_CONTRACT_MISMATCH, COMPUTER_DRIVER_UNAVAILABLE,
};
use mahayana_host_runtime::extensions::node_host::runtime_manifest::{
    NodeHostManifest, NODE_AGENT_CLI_CLAUDE_RUN_COMMAND, NODE_DESKTOP_STREAM_COMMAND,
    NODE_CLAUDE_NATIVE_AUTH_CAPABILITY, NODE_CLAUDE_SKILLS_CAPABILITY,
    NODE_SYSTEM_RUN_COMMAND, NODE_SYSTEM_RUN_COMMANDS,
};
use mahayana_host_runtime::extensions::node_host::protocol_negotiation::{
    NodeHostProtocol, OfferedProtocolRange, negotiate_protocol,
};
use std::collections::HashSet;

const COMMAND_DISPATCH_SOURCE: &str =
    include_str!("../src/extensions/node_host/command_dispatch.rs");

struct FixtureComputerExecutor;

impl NodeWorkerComputerExecutor for FixtureComputerExecutor {
    fn capabilities(&self, generation: &str) -> Result<String, String> {
        Ok(serde_json::json!({
            "provider": { "generation": generation },
            "actions": ["mouse_move", "mouse_click", "keyboard_type", "screenshot"]
        })
        .to_string())
    }

    fn act(&self, generation: &str, params: &serde_json::Value) -> Result<String, String> {
        Ok(serde_json::json!({ "generation": generation, "params": params, "result": "fixture-executed" }).to_string())
    }

    fn snapshot(&self, generation: &str, _params: &serde_json::Value) -> Result<String, String> {
        Ok(serde_json::json!({ "generation": generation, "snapshot": "fixture-owned" }).to_string())
    }
}

#[test]
fn test_ocrm_node_host_gateway_auth_and_scope() {
    let cfg = NodeHostGatewayConfig::new("127.0.0.1", 8080, false);
    assert_eq!(cfg.to_url().unwrap(), "http://127.0.0.1:8080");

    assert_eq!(
        gateway_origin_scope("http://localhost:8080/api/v1"),
        "http://localhost:8080/api/v1"
    );
    assert_eq!(
        gateway_origin_scope("https://gateway.example.com:443/ws"),
        "https://gateway.example.com/ws"
    );

    // Matching scope with token
    let saved = "http://127.0.0.1:8080/ws";
    let candidates = ["http://127.0.0.1:8080/ws", "http://127.0.0.1:8080/ws/"];
    assert!(can_reuse_node_host_device_token(
        Some(saved),
        &candidates,
        Some("node-tok-1234")
    ));

    // Mismatched scope
    let mismatched = ["http://127.0.0.1:8080/other", "http://192.168.1.1:8080/ws"];
    assert!(!can_reuse_node_host_device_token(
        Some(saved),
        &mismatched,
        Some("node-tok-1234")
    ));

    // Missing token
    assert!(!can_reuse_node_host_device_token(
        Some(saved),
        &candidates,
        None
    ));
}

#[test]
fn test_ocrm_node_host_protocol_negotiation_is_range_fenced() {
    let offered = OfferedProtocolRange::new(3, 4).unwrap();
    assert_eq!(negotiate_protocol(3, offered), Ok(NodeHostProtocol::V3));
    assert_eq!(negotiate_protocol(4, offered), Ok(NodeHostProtocol::V4));
    assert!(negotiate_protocol(2, offered).is_err());
}

#[test]
fn test_ocrm_node_host_computer_command() {
    // Close operation succeeds even if driver unavailable
    let close_input = ComputerCommandInput {
        operation: ComputerOperation::Close,
        provider_generation: None,
        action_params_json: None,
    };
    assert!(dispatch_node_worker_computer_command(&close_input, None).is_ok());

    // Capabilities operation requires driver
    let caps_input = ComputerCommandInput {
        operation: ComputerOperation::Capabilities,
        provider_generation: None,
        action_params_json: None,
    };
    assert_eq!(
        dispatch_node_worker_computer_command(&caps_input, None),
        Err(COMPUTER_DRIVER_UNAVAILABLE)
    );
    assert_eq!(
        dispatch_node_worker_computer_command(&caps_input, Some("generation-42")),
        Err(COMPUTER_DRIVER_UNAVAILABLE)
    );

    // Act operation with matching generation
    let act_input = ComputerCommandInput {
        operation: ComputerOperation::Act,
        provider_generation: Some("generation-42".into()),
        action_params_json: Some(r#"{"action":"mouse_click"}"#.to_string()),
    };
    assert_eq!(
        dispatch_node_worker_computer_command(&act_input, Some("generation-42")),
        Err(COMPUTER_DRIVER_UNAVAILABLE)
    );

    // Act operation with mismatched generation
    let act_mismatch = ComputerCommandInput {
        operation: ComputerOperation::Act,
        provider_generation: Some("generation-99".into()),
        action_params_json: None,
    };
    let executor = FixtureComputerExecutor;
    assert_eq!(
        dispatch_node_worker_computer_command_with_executor(
            &act_mismatch,
            Some("generation-42"),
            &executor,
        ),
        Err(COMPUTER_CONTRACT_MISMATCH.to_string())
    );
    assert_eq!(
        dispatch_node_worker_computer_command_with_executor(
            &ComputerCommandInput {
                operation: ComputerOperation::Act,
                provider_generation: None,
                action_params_json: None,
            },
            Some("generation-42"),
            &executor,
        ),
        Err(COMPUTER_CONTRACT_MISMATCH.to_string())
    );
    assert_eq!(
        dispatch_node_worker_computer_command_with_executor(
            &ComputerCommandInput {
                operation: ComputerOperation::Snapshot,
                provider_generation: Some("   ".into()),
                action_params_json: None,
            },
            Some("generation-42"),
            &executor,
        ),
        Err(COMPUTER_CONTRACT_MISMATCH.to_string())
    );
    let capabilities = dispatch_node_worker_computer_command_with_executor(
        &caps_input,
        Some("generation-42"),
        &executor,
    )
    .expect("fixture capabilities should be delegated");
    assert!(capabilities.contains("\"generation\":\"generation-42\""));
    let action = dispatch_node_worker_computer_command_with_executor(
        &act_input,
        Some("generation-42"),
        &executor,
    )
    .expect("fixture action should be delegated");
    assert!(action.contains("fixture-executed"));
    let snapshot = dispatch_node_worker_computer_command_with_executor(
        &ComputerCommandInput {
            operation: ComputerOperation::Snapshot,
            provider_generation: Some("generation-42".into()),
            action_params_json: None,
        },
        Some("generation-42"),
        &executor,
    )
    .expect("fixture snapshot should be delegated");
    assert!(snapshot.contains("fixture-owned"));
    assert_eq!(
        dispatch_node_worker_computer_command_with_executor(
            &ComputerCommandInput {
                operation: ComputerOperation::Act,
                provider_generation: Some("generation-42".into()),
                action_params_json: Some("{".into()),
            },
            Some("generation-42"),
            &executor,
        ),
        Err("COMPUTER_INVALID_ACTION: action parameters are invalid JSON".into())
    );

    assert!(!COMMAND_DISPATCH_SOURCE.contains("fake-png-snapshot"));
    assert!(!COMMAND_DISPATCH_SOURCE.contains("{\"result\": \"executed\"}"));
}

#[test]
fn test_ocrm_node_host_runtime_manifest() {
    let available_commands = [
        NODE_AGENT_CLI_CLAUDE_RUN_COMMAND,
        NODE_DESKTOP_STREAM_COMMAND,
        NODE_SYSTEM_RUN_COMMAND,
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<HashSet<_>>();
    let manifest = NodeHostManifest::build(&available_commands, true, true, None);
    assert!(manifest.advertised_commands.iter().any(|c| c == NODE_AGENT_CLI_CLAUDE_RUN_COMMAND));
    assert!(manifest.advertised_commands.iter().any(|c| c == NODE_DESKTOP_STREAM_COMMAND));
    assert!(manifest.advertised_commands.iter().any(|c| c == NODE_SYSTEM_RUN_COMMAND));
    assert!(manifest
        .capabilities
        .iter()
        .any(|capability| capability == NODE_CLAUDE_SKILLS_CAPABILITY));
    assert!(manifest
        .capabilities
        .iter()
        .any(|capability| capability == NODE_CLAUDE_NATIVE_AUTH_CAPABILITY));

    // With allowlist filter
    let mut allowlist = HashSet::new();
    allowlist.insert(NODE_SYSTEM_RUN_COMMAND.to_string());
    let filtered_manifest = NodeHostManifest::build(&available_commands, true, true, Some(&allowlist));
    assert_eq!(filtered_manifest.advertised_commands, vec![NODE_SYSTEM_RUN_COMMAND.to_string()]);
}

#[test]
fn test_ocrm_node_host_manifest_uses_current_system_command_names() {
    let available_commands = NODE_SYSTEM_RUN_COMMANDS
        .iter()
        .map(|command| (*command).to_string())
        .chain(["node.system_run".to_string()])
        .collect::<HashSet<_>>();

    let manifest = NodeHostManifest::build(&available_commands, false, false, None);

    assert_eq!(
        manifest.advertised_commands,
        vec![
            "system.run".to_string(),
            "system.run.prepare".to_string(),
            "system.which".to_string(),
        ]
    );
    assert!(!manifest
        .advertised_commands
        .iter()
        .any(|command| command.starts_with("node.")));
    assert!(manifest
        .capabilities
        .contains(&"system_run.execution_context".to_string()));
}

#[test]
fn test_ocrm_node_host_manifest_never_advertises_unavailable_commands() {
    let manifest = NodeHostManifest::build(&HashSet::new(), true, true, None);
    assert!(manifest.advertised_commands.is_empty());
    assert!(manifest.capabilities.is_empty());
}

#[test]
fn test_ocrm_node_host_manifest_normalizes_plugin_capabilities() {
    let manifest = NodeHostManifest::build_with_plugin_manifest(
        &HashSet::new(),
        &[],
        &["  plugin.camera  ".into(), " ".into(), "plugin.camera".into()],
        false,
        false,
        false,
        None,
    );
    assert_eq!(manifest.capabilities, vec!["plugin.camera".to_string()]);
}

#[test]
fn test_ocrm_node_host_manifest_composes_plugins_and_ephemeral_surface() {
    let available_commands = [NODE_SYSTEM_RUN_COMMAND.to_string()]
        .into_iter()
        .collect::<HashSet<_>>();
    let manifest = NodeHostManifest::build_with_plugin_manifest(
        &available_commands,
        &[
            "plugin.command".into(),
            "screen.snapshot".into(),
            "  ".into(),
        ],
        &["plugin.capability".into(), "computer".into()],
        false,
        false,
        true,
        None,
    );

    assert_eq!(
        manifest.advertised_commands,
        vec!["plugin.command".to_string(), "system.run".to_string()]
    );
    assert!(manifest
        .capabilities
        .contains(&"plugin.capability".to_string()));
    assert!(!manifest.capabilities.contains(&"computer".to_string()));
    assert!(manifest.is_ephemeral);
}

#[test]
fn test_ocrm_node_host_manifest_rejects_unbounded_plugin_entries() {
    let manifest = NodeHostManifest::build_with_plugin_manifest(
        &HashSet::new(),
        &["plugin.ok".into(), "plugin\nunsafe".into(), "x".repeat(257)],
        &["cap.ok".into(), "cap\tunsafe".into(), "y".repeat(257)],
        false,
        false,
        false,
        None,
    );
    assert_eq!(manifest.advertised_commands, vec!["plugin.ok".to_string()]);
    assert_eq!(manifest.capabilities, vec!["cap.ok".to_string()]);
}

#[test]
fn test_ocrm_node_host_system_run_dispatch_and_truncation() {
    use mahayana_host_runtime::extensions::node_host::command_dispatch::node_system_run::*;

    // 1. system.which
    let which_res = dispatch_system_which(&SystemWhichInput {
        command: "sh".into(),
        path_env: None,
    })
    .expect("system.which should resolve");
    assert!(which_res.found);
    assert!(which_res.resolved_path.is_some());

    let which_empty = dispatch_system_which(&SystemWhichInput {
        command: "non_existent_binary_12345".into(),
        path_env: None,
    })
    .expect("system.which handles missing binary");
    assert!(!which_empty.found);

    // 2. system.notify
    let notify_res = dispatch_system_notify(&SystemNotifyInput {
        title: "Test Alert".into(),
        message: "Operation completed successfully".into(),
        level: Some("info".into()),
        timeout_ms: Some(3000),
    })
    .expect("system.notify should deliver");
    assert_eq!(notify_res["status"], "delivered");

    assert!(dispatch_system_notify(&SystemNotifyInput {
        title: "  ".into(),
        message: "".into(),
        level: None,
        timeout_ms: None,
    })
    .is_err());

    // 3. system.run.prepare
    let prep = dispatch_system_run_prepare(&SystemRunPrepareInput {
        command: "echo".into(),
        args: vec!["hello".into(), "world".into()],
        cwd: None,
    })
    .expect("prepare non-sensitive command");
    assert!(!prep.requires_approval);
    assert_eq!(prep.command_display, "echo hello world");

    let prep_sensitive = dispatch_system_run_prepare(&SystemRunPrepareInput {
        command: "rm".into(),
        args: vec!["-rf".into(), "/tmp/test".into()],
        cwd: None,
    })
    .expect("prepare sensitive command");
    assert!(prep_sensitive.requires_approval);
    assert!(prep_sensitive.approval_id.is_some());

    // 4. Approval validation
    let run_input = SystemRunCommandInput {
        command: "rm".into(),
        args: vec!["-rf".into(), "/tmp/test".into()],
        cwd: None,
        env: std::collections::HashMap::new(),
        timeout_ms: Some(10000),
        max_output_bytes: Some(1024),
        approval_id: None,
    };
    // Missing approval plan
    assert!(dispatch_system_run(&run_input, None, 1000).is_err());

    // Valid approval plan
    let valid_plan = ExecApprovalPlan {
        plan_hash: prep_sensitive.plan_hash.clone(),
        approved: true,
        approved_by: Some("admin".into()),
        expires_at_ms: Some(2000),
    };
    assert!(dispatch_system_run(&run_input, Some(&valid_plan), 1500).is_ok());

    // Expired approval plan
    assert!(dispatch_system_run(&run_input, Some(&valid_plan), 2500).is_err());

    // 5. Output truncation
    let mut big_output = "A".repeat(500);
    let was_truncated = truncate_command_output(&mut big_output, 100);
    assert!(was_truncated);
    assert!(big_output.contains("[OUTPUT TRUNCATED: byte limit reached]"));
}

#[test]
fn test_ocrm_node_host_browser_and_canvas_dispatch() {
    use mahayana_host_runtime::extensions::node_host::command_dispatch::node_browser_canvas::*;

    // 1. Browser URL safety & SSRF guards
    assert!(validate_browser_proxy_url("https://example.com/api").is_ok());
    assert!(validate_browser_proxy_url("ftp://example.com").is_err());
    assert!(validate_browser_proxy_url("http://169.254.169.254/latest/meta-data").is_err());
    assert!(validate_browser_proxy_url("http://metadata.google.internal/computeMetadata/v1").is_err());

    // 2. Browser proxy actions
    let nav_res = dispatch_browser_proxy(&BrowserProxyInput {
        action: BrowserProxyAction::Navigate,
        url: Some("https://example.com".into()),
        selector: None,
        script: None,
        cookies: None,
        timeout_ms: Some(5000),
    })
    .expect("browser navigate");
    assert_eq!(nav_res.status, "navigated");

    let shot_res = dispatch_browser_proxy(&BrowserProxyInput {
        action: BrowserProxyAction::Screenshot,
        url: None,
        selector: None,
        script: None,
        cookies: None,
        timeout_ms: None,
    })
    .expect("browser screenshot");
    assert_eq!(shot_res.status, "captured");
    assert!(shot_res.screenshot_base64.is_some());

    // 3. Canvas presentation
    let canvas_res = dispatch_canvas_present(&CanvasPresentInput {
        title: "Test Canvas".into(),
        content: "<h1>Hello World</h1>".into(),
        mode: Some(CanvasPresentMode::Modal),
        width: Some(800),
        height: Some(600),
        session_token: None,
    })
    .expect("canvas present");
    assert!(canvas_res.presented);
    assert_eq!(canvas_res.mode, "modal");
    assert!(canvas_res.canvas_id.starts_with("canvas-"));

    assert!(dispatch_canvas_present(&CanvasPresentInput {
        title: "".into(),
        content: "hello".into(),
        mode: None,
        width: None,
        height: None,
        session_token: None,
    })
    .is_err());
}

#[test]
fn test_ocrm_node_host_unified_command_dispatcher() {
    use mahayana_host_runtime::extensions::node_host::command_dispatch::*;

    let executor = FixtureComputerExecutor;
    let ctx = NodeHostCommandContext {
        current_generation: Some("generation-42".into()),
        computer_executor: Some(&executor),
        approval_plan: None,
        now_ms: 1000,
    };

    // 1. system.which via unified router
    let res = dispatch_node_host_command("system.which", Some(r#"{"command":"sh"}"#), &ctx)
        .expect("unified system.which");
    assert_eq!(res["found"], true);

    // 2. device.status via unified router
    let status_res = dispatch_node_host_command("device.status", None, &ctx)
        .expect("unified device.status");
    assert_eq!(status_res["hostname"], "node-host-device");

    // 3. computer.capabilities via unified router
    let caps_res = dispatch_node_host_command(
        "computer.capabilities",
        Some(r#"{"providerGeneration":"generation-42"}"#),
        &ctx,
    )
    .expect("unified computer.capabilities");
    assert_eq!(caps_res["provider"]["generation"], "generation-42");

    // 4. Unknown command
    assert!(dispatch_node_host_command("unknown.unsupported.command", None, &ctx).is_err());
}
