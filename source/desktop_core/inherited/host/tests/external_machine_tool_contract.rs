use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_external_machine_tools::{
    EXTERNAL_SHELL_CANCEL_TOOL_NAME, EXTERNAL_SHELL_HISTORY_TOOL_NAME,
    EXTERNAL_SHELL_STATUS_TOOL_NAME, ExternalMachineExecutor, ExternalMachineReadArgs,
    ExternalMachineShellArgs,
};
use mahayana_host_runtime::runner::tools::turn_toolset::{
    TurnToolsetDependencies, TurnToolsetRole, build_turn_toolset,
};
use mahayana_host_runtime::sand_activity::{
    SAND_EXTERNAL_READ_TOOL_NAME, SAND_EXTERNAL_SHELL_TOOL_NAME,
};
use serde_json::{Value, json};

struct BaseBridge;

impl RoutedToolBridge for BaseBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(vec![RoutedToolDefinition {
            name: "base_tool".into(),
            provider_identifier: "base".into(),
            tool_name: "base_tool".into(),
            description: None,
            input_schema: json!({"type":"object"}),
        }])
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Ok(Value::String("base".into()))
    }
}

#[derive(Default)]
struct FakeExternalMachine {
    calls: Mutex<Vec<String>>,
}

impl ExternalMachineExecutor for FakeExternalMachine {
    fn execute_shell(&self, args: &ExternalMachineShellArgs) -> Result<Value, ProviderSessionError> {
        self.calls.lock().expect("calls").push(format!(
            "shell:{}:{}:{}", args.command, args.working_directory, args.tool_call_id
        ));
        Ok(json!({"stdout":"ok","exitCode":0}))
    }

    fn execute_read(&self, args: &ExternalMachineReadArgs) -> Result<Value, ProviderSessionError> {
        self.calls.lock().expect("calls").push(format!(
            "read:{}:{:?}:{:?}:{}", args.path, args.offset, args.limit, args.tool_call_id
        ));
        Ok(json!({"success":{"path":args.path,"content":"hello"}}))
    }

    fn operation_status(&self, operation_id: &str) -> Result<Value, ProviderSessionError> {
        self.calls.lock().expect("calls").push(format!("status:{operation_id}"));
        Ok(json!({"operationId":operation_id,"status":"running"}))
    }

    fn operation_history(&self) -> Result<Value, ProviderSessionError> {
        self.calls.lock().expect("calls").push("history".into());
        Ok(json!([{"operationId":"shell-call","status":"completed"}]))
    }

    fn cancel_operation(&self, operation_id: &str) -> Result<Value, ProviderSessionError> {
        self.calls.lock().expect("calls").push(format!("cancel:{operation_id}"));
        Ok(json!({"operationId":operation_id,"cancelRequested":true}))
    }
}

#[test]
fn turn_toolset_consumes_external_shell_and_read_executor() {
    let executor = Arc::new(FakeExternalMachine::default());
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            external_machine_executor: Some(executor.clone()),
            ..TurnToolsetDependencies::default()
        },
    );
    let tools = bridge.list_tools().expect("tools");
    assert!(tools.iter().any(|tool| tool.name == SAND_EXTERNAL_SHELL_TOOL_NAME));
    assert!(tools.iter().any(|tool| tool.name == SAND_EXTERNAL_READ_TOOL_NAME));
    assert!(tools.iter().any(|tool| tool.name == EXTERNAL_SHELL_STATUS_TOOL_NAME));
    assert!(tools.iter().any(|tool| tool.name == EXTERNAL_SHELL_HISTORY_TOOL_NAME));
    assert!(tools.iter().any(|tool| tool.name == EXTERNAL_SHELL_CANCEL_TOOL_NAME));
    assert!(tools.iter().any(|tool| tool.name == "base_tool"));

    let shell = tools.iter().find(|tool| tool.name == SAND_EXTERNAL_SHELL_TOOL_NAME).unwrap();
    let shell_result = bridge.call_tool(
        shell,
        json!({"command":"pwd","workingDirectory":"/tmp"}),
        "shell-call",
    ).expect("shell result");
    assert_eq!(shell_result["exitCode"], 0);

    let read = tools.iter().find(|tool| tool.name == SAND_EXTERNAL_READ_TOOL_NAME).unwrap();
    let read_result = bridge.call_tool(
        read,
        json!({"path":"/tmp/a.txt","offset":2,"limit":5}),
        "read-call",
    ).expect("read result");
    assert_eq!(read_result["success"]["content"], "hello");

    let status = tools.iter().find(|tool| tool.name == EXTERNAL_SHELL_STATUS_TOOL_NAME).unwrap();
    let status_result = bridge
        .call_tool(status, json!({"operationId":"shell-call"}), "status-call")
        .expect("status result");
    assert_eq!(status_result["status"], "running");

    let history = tools.iter().find(|tool| tool.name == EXTERNAL_SHELL_HISTORY_TOOL_NAME).unwrap();
    let history_result = bridge.call_tool(history, json!({}), "history-call").expect("history");
    assert_eq!(history_result[0]["operationId"], "shell-call");

    let cancel = tools.iter().find(|tool| tool.name == EXTERNAL_SHELL_CANCEL_TOOL_NAME).unwrap();
    let cancel_result = bridge
        .call_tool(cancel, json!({"operationId":"shell-call"}), "cancel-call")
        .expect("cancel result");
    assert_eq!(cancel_result["cancelRequested"], true);

    assert_eq!(
        executor.calls.lock().expect("calls").as_slice(),
        &[
            "shell:pwd:/tmp:shell-call".to_string(),
            "read:/tmp/a.txt:Some(2):Some(5):read-call".to_string(),
            "status:shell-call".to_string(),
            "history".to_string(),
            "cancel:shell-call".to_string(),
        ]
    );
}

#[test]
fn external_shell_review_blocks_before_executor_side_effect() {
    let executor = Arc::new(FakeExternalMachine::default());
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            external_machine_executor: Some(executor.clone()),
            external_shell_review: Some(Arc::new(|_| Ok(Some("blocked by auto-review".into())))),
            ..TurnToolsetDependencies::default()
        },
    );
    let tools = bridge.list_tools().expect("tools");
    let shell = tools.iter().find(|tool| tool.name == SAND_EXTERNAL_SHELL_TOOL_NAME).unwrap();
    let result = bridge.call_tool(
        shell,
        json!({"command":"rm -rf /tmp/nope"}),
        "blocked-call",
    ).expect("review result");
    assert_eq!(result, Value::String("blocked by auto-review".into()));
    assert!(executor.calls.lock().expect("calls").is_empty());
}


#[test]
fn box_scoped_subagent_does_not_receive_external_machine_tools() {
    let executor = Arc::new(FakeExternalMachine::default());
    let bridge = build_turn_toolset(
        Arc::new(BaseBridge),
        TurnToolsetDependencies {
            role: TurnToolsetRole {
                is_subagent_runner: true,
                is_box_scoped_subagent: true,
                ..TurnToolsetRole::default()
            },
            external_machine_executor: Some(executor),
            ..TurnToolsetDependencies::default()
        },
    );
    let tools = bridge.list_tools().expect("tools");
    assert!(tools.iter().all(|tool| tool.name != SAND_EXTERNAL_SHELL_TOOL_NAME));
    assert!(tools.iter().all(|tool| tool.name != SAND_EXTERNAL_READ_TOOL_NAME));
    assert!(tools.iter().all(|tool| tool.name != EXTERNAL_SHELL_STATUS_TOOL_NAME));
    assert!(tools.iter().all(|tool| tool.name != EXTERNAL_SHELL_HISTORY_TOOL_NAME));
    assert!(tools.iter().all(|tool| tool.name != EXTERNAL_SHELL_CANCEL_TOOL_NAME));
}
