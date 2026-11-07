use std::sync::{Arc, Mutex};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::box_tool_access::{
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxShellRequest, RunnerBoxWriteRequest,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_generate_image_tool::{
    GenerateImageToolExecutor, GeneratedImageToolOutput, GENERATE_IMAGE_TOOL_NAME,
    SandGenerateImageToolBridge,
};
use mahayana_host_runtime::runner::tools::turn_toolset::{
    TurnToolsetDependencies, TurnToolsetRole, build_turn_toolset,
};
use mahayana_host_runtime::runner::shell_terminal_watch::{
    ShellTerminalPollRead, TerminalReadResult,
};
use serde_json::{Value, json};

struct EmptyBridge;
impl RoutedToolBridge for EmptyBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }
    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("unexpected delegate".into()))
    }
}

struct FakeGenerator;
impl GenerateImageToolExecutor for FakeGenerator {
    fn generate(
        &self,
        description: &str,
        references: &[(String, String)],
    ) -> Result<GeneratedImageToolOutput, ProviderSessionError> {
        assert!(description.contains("mountain"));
        assert!(references.is_empty());
        Ok(GeneratedImageToolOutput {
            persisted_path: "/tmp/hash.png".into(),
            image_data_base64: "AQID".into(),
        })
    }
}

#[derive(Default)]
struct FakeBox {
    writes: Mutex<Vec<(String, Vec<u8>, String)>>,
}
impl RunnerBoxResourcePort for FakeBox {
    fn execute_shell(
        &self,
        _request: RunnerBoxShellRequest,
    ) -> Result<Value, ProviderSessionError> {
        unreachable!()
    }
    fn execute_read(
        &self,
        _request: RunnerBoxReadRequest,
    ) -> Result<Value, ProviderSessionError> {
        Ok(json!({"kind":"fileNotFound"}))
    }
    fn execute_write(
        &self,
        request: RunnerBoxWriteRequest,
    ) -> Result<(), ProviderSessionError> {
        self.writes.lock().unwrap().push((
            request.path,
            request.data,
            request.tool_call_id,
        ));
        Ok(())
    }
    fn poll_background_shell_terminal(&self, _shell_id: &str) -> ShellTerminalPollRead {
        ShellTerminalPollRead::Snapshot {
            output_path: "/tmp/fake-terminal-output".into(),
            result: TerminalReadResult::FileNotFound,
        }
    }
}

#[test]
fn generate_image_executes_provider_and_writes_box_asset() {
    let box_resources = Arc::new(FakeBox::default());
    let bridge: Arc<dyn RoutedToolBridge> = Arc::new(SandGenerateImageToolBridge::new(
        Arc::new(EmptyBridge),
        Arc::new(FakeGenerator),
        box_resources.clone(),
    ));
    let tools = bridge.list_tools().expect("tools");
    let tool = tools
        .iter()
        .find(|tool| tool.name == GENERATE_IMAGE_TOOL_NAME)
        .expect("GenerateImage");
    let result = bridge
        .call_tool(
            tool,
            json!({
                "description":"A detailed mountain landscape under moonlight",
                "aspect_ratio":"16:9"
            }),
            "image-call",
        )
        .expect("generate");
    assert_eq!(result["success"]["filePath"], "/workspace/assets/hash.png");
    assert_eq!(
        box_resources.writes.lock().unwrap().as_slice(),
        &[(
            "/workspace/assets/hash.png".to_string(),
            vec![1, 2, 3],
            "image-call".to_string()
        )]
    );
}

#[test]
fn generate_image_is_parent_only() {
    let bridge = build_turn_toolset(
        Arc::new(EmptyBridge),
        TurnToolsetDependencies {
            role: TurnToolsetRole {
                is_subagent_runner: true,
                is_box_scoped_subagent: true,
                ..TurnToolsetRole::default()
            },
            box_resources: Some(Arc::new(FakeBox::default())),
            generate_image_executor: Some(Arc::new(FakeGenerator)),
            ..TurnToolsetDependencies::default()
        },
    );
    assert!(
        bridge
            .list_tools()
            .expect("tools")
            .iter()
            .all(|tool| tool.name != GENERATE_IMAGE_TOOL_NAME)
    );
}
