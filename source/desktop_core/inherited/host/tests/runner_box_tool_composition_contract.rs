use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::box_tool_access::{
    RUNNER_BOX_READ_TOOL_NAME, RUNNER_BOX_SHELL_TOOL_NAME, RUNNER_BOX_TOOL_PROVIDER,
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxShellRequest, RunnerBoxToolBridge,
    RunnerBoxWriteRequest,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use serde_json::{Value, json};

struct Upstream {
    collide: bool,
}

impl RoutedToolBridge for Upstream {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![RoutedToolDefinition {
            name: if self.collide { "Shell" } else { "SearchPlugins" }.into(),
            provider_identifier: "host".into(),
            tool_name: "search".into(),
            description: None,
            input_schema: json!({"type":"object"}),
        }])
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({"upstream":tool.name,"args":args}))
    }
}

#[derive(Default)]
struct BoxPort {
    shells: Mutex<Vec<RunnerBoxShellRequest>>,
    reads: Mutex<Vec<RunnerBoxReadRequest>>,
    writes: Mutex<Vec<RunnerBoxWriteRequest>>,
}

impl RunnerBoxResourcePort for BoxPort {
    fn execute_shell(
        &self,
        request: RunnerBoxShellRequest,
    ) -> Result<Value, ProviderSessionError> {
        self.shells.lock().expect("shell calls").push(request.clone());
        Ok(json!({"kind":"success","command":request.command}))
    }

    fn execute_read(
        &self,
        request: RunnerBoxReadRequest,
    ) -> Result<Value, ProviderSessionError> {
        self.reads.lock().expect("read calls").push(request.clone());
        Ok(json!({"kind":"success","path":request.path}))
    }

    fn execute_write(
        &self,
        request: RunnerBoxWriteRequest,
    ) -> Result<(), ProviderSessionError> {
        self.writes.lock().expect("write calls").push(request);
        Ok(())
    }
}

#[test]
fn runner_box_bridge_merges_frozen_shell_and_read_with_host_tools() {
    let box_port = Arc::new(BoxPort::default());
    let bridge = RunnerBoxToolBridge::new(
        Arc::new(Upstream { collide: false }),
        box_port.clone(),
    );

    let tools = bridge.list_tools().expect("tool list");
    assert!(tools.iter().any(|tool| tool.name == "SearchPlugins"));
    let shell = tools
        .iter()
        .find(|tool| tool.name == RUNNER_BOX_SHELL_TOOL_NAME)
        .expect("Shell definition");
    let read = tools
        .iter()
        .find(|tool| tool.name == RUNNER_BOX_READ_TOOL_NAME)
        .expect("Read definition");
    assert_eq!(shell.provider_identifier, RUNNER_BOX_TOOL_PROVIDER);
    assert_eq!(read.provider_identifier, RUNNER_BOX_TOOL_PROVIDER);
    assert_eq!(shell.input_schema["required"][0], "command");
    assert_eq!(read.input_schema["required"][0], "path");

    let shell_result = bridge
        .call_tool(
            shell,
            json!({"command":"pwd","workingDirectory":"/workspace/project"}),
            "shell-call",
        )
        .expect("Shell call");
    assert_eq!(shell_result["command"], "pwd");
    assert_eq!(
        box_port.shells.lock().expect("shell calls").as_slice(),
        &[RunnerBoxShellRequest {
            command: "pwd".into(),
            working_directory: "/workspace/project".into(),
            tool_call_id: "shell-call".into(),
            is_background: false,
            block_until_ms: None,
        }]
    );

    let read_result = bridge
        .call_tool(
            read,
            json!({"path":"/workspace/a.txt","offset":2,"limit":8,"encodingHint":"utf-8"}),
            "read-call",
        )
        .expect("Read call");
    assert_eq!(read_result["path"], "/workspace/a.txt");
    assert_eq!(
        box_port.reads.lock().expect("read calls").as_slice(),
        &[RunnerBoxReadRequest {
            path: "/workspace/a.txt".into(),
            tool_call_id: "read-call".into(),
            offset: Some(2),
            limit: Some(8),
            encoding_hint: Some("utf-8".into()),
        }]
    );
}

#[test]
fn runner_box_bridge_preserves_explicit_background_and_positive_block_until_semantics() {
    let box_port = Arc::new(BoxPort::default());
    let bridge = RunnerBoxToolBridge::new(
        Arc::new(Upstream { collide: false }),
        box_port.clone(),
    );
    let tools = bridge.list_tools().expect("tool list");
    let shell = tools
        .iter()
        .find(|tool| tool.name == RUNNER_BOX_SHELL_TOOL_NAME)
        .expect("Shell definition");

    assert_eq!(shell.input_schema["properties"]["is_background"]["type"], "boolean");
    assert_eq!(shell.input_schema["properties"]["block_until_ms"]["type"], "integer");
    assert_eq!(shell.input_schema["properties"]["block_until_ms"]["minimum"], 0);

    bridge
        .call_tool(
            shell,
            json!({
                "command":"sleep 10",
                "is_background":true,
                "block_until_ms":750
            }),
            "background-shell-call",
        )
        .expect("background Shell call");
    bridge
        .call_tool(
            shell,
            json!({
                "command":"sleep 10",
                "is_background":false,
                "block_until_ms":75
            }),
            "timed-shell-call",
        )
        .expect("timed Shell call");

    assert_eq!(
        box_port.shells.lock().expect("shell calls").as_slice(),
        &[
            RunnerBoxShellRequest {
                command: "sleep 10".into(),
                working_directory: "/workspace".into(),
                tool_call_id: "background-shell-call".into(),
                is_background: true,
                block_until_ms: Some(750),
            },
            RunnerBoxShellRequest {
                command: "sleep 10".into(),
                working_directory: "/workspace".into(),
                tool_call_id: "timed-shell-call".into(),
                is_background: false,
                block_until_ms: Some(75),
            },
        ]
    );
}

#[test]
fn runner_box_bridge_preserves_upstream_dispatch_and_fails_closed_on_name_collision() {
    let box_port = Arc::new(BoxPort::default());
    let bridge = RunnerBoxToolBridge::new(
        Arc::new(Upstream { collide: false }),
        box_port.clone(),
    );
    let upstream_tool = bridge
        .list_tools()
        .expect("tool list")
        .into_iter()
        .find(|tool| tool.name == "SearchPlugins")
        .expect("upstream tool");
    let upstream = bridge
        .call_tool(&upstream_tool, json!({"q":"calendar"}), "host-call")
        .expect("upstream dispatch");
    assert_eq!(upstream["upstream"], "SearchPlugins");
    assert!(box_port.shells.lock().expect("shell calls").is_empty());
    assert!(box_port.reads.lock().expect("read calls").is_empty());

    let colliding = RunnerBoxToolBridge::new(
        Arc::new(Upstream { collide: true }),
        box_port,
    );
    let error = colliding
        .list_tools()
        .expect_err("Host tools must not shadow Runner box tools");
    assert!(error.to_string().contains("collides"));
}

#[test]
fn runner_box_bridge_validates_model_arguments_before_host_resource_execution() {
    let box_port = Arc::new(BoxPort::default());
    let bridge = RunnerBoxToolBridge::new(
        Arc::new(Upstream { collide: false }),
        box_port.clone(),
    );
    let tools = bridge.list_tools().expect("tool list");
    let shell = tools.iter().find(|tool| tool.name == "Shell").expect("Shell");
    let read = tools.iter().find(|tool| tool.name == "Read").expect("Read");

    assert!(bridge.call_tool(shell, json!({"command":"   "}), "bad").is_err());
    assert!(bridge.call_tool(read, json!({"path":""}), "bad").is_err());
    assert!(bridge.call_tool(read, json!({"path":"/x","limit":-1}), "bad").is_err());
    assert!(box_port.shells.lock().expect("shell calls").is_empty());
    assert!(box_port.reads.lock().expect("read calls").is_empty());
}

#[test]
fn runner_box_shell_review_fences_side_effect_before_resource_execution() {
    let box_port = Arc::new(BoxPort::default());
    let seen = Arc::new(Mutex::new(Vec::<RunnerBoxShellRequest>::new()));
    let seen_review = Arc::clone(&seen);
    let bridge = RunnerBoxToolBridge::new(
        Arc::new(Upstream { collide: false }),
        box_port.clone(),
    )
    .with_shell_review(Arc::new(move |request| {
        seen_review.lock().expect("review calls").push(request.clone());
        Ok(Some("Auto-review blocked shell".into()))
    }));
    let tools = bridge.list_tools().expect("tool list");
    let shell = tools
        .iter()
        .find(|tool| tool.name == RUNNER_BOX_SHELL_TOOL_NAME)
        .expect("Shell definition");
    let result = bridge
        .call_tool(
            shell,
            json!({"command":"curl example.com","workingDirectory":"/workspace"}),
            "shell-review-call",
        )
        .expect("review denial is a tool result");
    assert_eq!(result, Value::String("Auto-review blocked shell".into()));
    assert_eq!(seen.lock().expect("review calls").len(), 1);
    assert!(box_port.shells.lock().expect("shell calls").is_empty());
}
