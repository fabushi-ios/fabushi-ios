use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use mahayana_host_runtime::r#box::box_transfer::TransferBox;
use mahayana_host_runtime::ports::r#box::SAND_BOX_NOT_READY_MESSAGE;
use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use serde_json::{Value, json};
use mahayana_host_runtime::runner::tools::sand_file_transfer_tools::{
    CopyFromBoxArgs, CopyToBoxArgs, FileTransferController, FileTransferExecutor,
    SandFileTransferToolBridge, UserComputerHandle, copy_file_from_box, copy_file_to_box,
    file_transfer_tool_definitions, format_bytes,
};

#[derive(Debug, Default)]
struct MemoryBox {
    files: Mutex<HashMap<String, Vec<u8>>>,
}

impl TransferBox<()> for MemoryBox {
    type Error = io::Error;

    fn download_file(
        &self,
        _ctx: &(),
        _agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing"))
    }

    fn upload_file(
        &self,
        _ctx: &(),
        _agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }
}

fn controller() -> (
    Arc<MemoryBox>,
    Arc<MemoryBox>,
    FileTransferController<MemoryBox, MemoryBox>,
) {
    let agent = Arc::new(MemoryBox::default());
    let computer = Arc::new(MemoryBox::default());
    computer
        .files
        .lock()
        .unwrap()
        .insert("/Users/me/report.pdf".into(), vec![0, 1, 2, 255]);
    let controller = FileTransferController {
        agent_box: agent.clone(),
        user_computers: vec![
            UserComputerHandle {
                id: "mac".into(),
                label: "My Mac".into(),
                connected: true,
                box_: computer.clone(),
            },
            UserComputerHandle {
                id: "old".into(),
                label: "Old Mac".into(),
                connected: false,
                box_: Arc::new(MemoryBox::default()),
            },
        ],
        default_computer_id: Some("mac".into()),
        computer_agent_id: "computer-agent".into(),
        box_id: "box-agent".into(),
        box_preparing: false,
    };
    (agent, computer, controller)
}

#[test]
fn frozen_byte_formatting_is_binary_and_human_readable() {
    assert_eq!(format_bytes(0), "0 bytes");
    assert_eq!(format_bytes(1_023), "1023 bytes");
    assert_eq!(format_bytes(1_024), "1 KB");
    assert_eq!(format_bytes(1_536), "1.5 KB");
    assert_eq!(format_bytes(10 * 1_024), "10 KB");
    assert_eq!(format_bytes(2 * 1_024 * 1_024), "2 MB");
}

#[test]
fn copy_to_box_preserves_bytes_and_uses_uploads_default() {
    let (agent, _computer, controller) = controller();
    let message = copy_file_to_box(
        &(),
        &CopyToBoxArgs {
            computer_path: "/Users/me/report.pdf".into(),
            box_path: None,
            computer: None,
        },
        &controller,
    )
    .expect("copy to box");
    assert_eq!(
        agent
            .files
            .lock()
            .unwrap()
            .get("/workspace/uploads/report.pdf")
            .cloned(),
        Some(vec![0, 1, 2, 255])
    );
    assert!(message.contains("from My Mac into your box"));
    assert!(message.contains("(4 bytes)"));
}

#[test]
fn copy_from_box_resolves_workspace_path_and_default_filename() {
    let (agent, computer, controller) = controller();
    agent
        .files
        .lock()
        .unwrap()
        .insert("/workspace/out/report.csv".into(), b"a,b\n1,2\n".to_vec());
    let message = copy_file_from_box(
        &(),
        &CopyFromBoxArgs {
            box_path: "out/report.csv".into(),
            computer_path: None,
            computer: Some("mac".into()),
        },
        &controller,
    )
    .expect("copy from box");
    assert_eq!(
        computer.files.lock().unwrap().get("report.csv").cloned(),
        Some(b"a,b\n1,2\n".to_vec())
    );
    assert!(message.contains("/workspace/out/report.csv"));
    assert!(message.contains("at report.csv"));
}

#[test]
fn unavailable_computers_and_preparing_box_fail_closed() {
    let (_agent, _computer, mut controller) = controller();
    let unknown = controller
        .resolve_computer_or_throw(Some("missing"))
        .unwrap_err()
        .to_string();
    assert!(unknown.contains("Unknown computer \"missing\""));
    assert!(unknown.contains("old (offline)"));

    controller.box_preparing = true;
    assert_eq!(
        controller.assert_box_ready().unwrap_err().to_string(),
        SAND_BOX_NOT_READY_MESSAGE
    );
}

#[derive(Debug)]
struct AgentTag;
#[derive(Debug)]
struct UserTag;

#[derive(Debug)]
struct TaggedMemoryBox<Tag> {
    files: Mutex<HashMap<String, Vec<u8>>>,
    marker: std::marker::PhantomData<Tag>,
}

impl<Tag> Default for TaggedMemoryBox<Tag> {
    fn default() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            marker: std::marker::PhantomData,
        }
    }
}

impl<Tag> TransferBox<()> for TaggedMemoryBox<Tag> {
    type Error = io::Error;

    fn download_file(
        &self,
        _ctx: &(),
        _agent_id: &str,
        path: &str,
    ) -> Result<Vec<u8>, Self::Error> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing"))
    }

    fn upload_file(
        &self,
        _ctx: &(),
        _agent_id: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), Self::Error> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }
}

#[test]
fn agent_and_user_computers_can_use_distinct_transfer_box_types() {
    let agent = Arc::new(TaggedMemoryBox::<AgentTag>::default());
    let computer = Arc::new(TaggedMemoryBox::<UserTag>::default());
    computer
        .files
        .lock()
        .unwrap()
        .insert("/Users/me/distinct.bin".into(), vec![9, 8, 7, 6]);
    let controller = FileTransferController {
        agent_box: Arc::clone(&agent),
        user_computers: vec![UserComputerHandle {
            id: "mac".into(),
            label: "My Mac".into(),
            connected: true,
            box_: Arc::clone(&computer),
        }],
        default_computer_id: Some("mac".into()),
        computer_agent_id: "computer-agent".into(),
        box_id: "box-agent".into(),
        box_preparing: false,
    };

    copy_file_to_box(
        &(),
        &CopyToBoxArgs {
            computer_path: "/Users/me/distinct.bin".into(),
            box_path: Some("distinct.bin".into()),
            computer: None,
        },
        &controller,
    )
    .expect("heterogeneous copy to box");

    assert_eq!(
        agent
            .files
            .lock()
            .unwrap()
            .get("/workspace/distinct.bin")
            .cloned(),
        Some(vec![9, 8, 7, 6])
    );
}


struct EmptyToolBridge;
impl RoutedToolBridge for EmptyToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool(format!("unexpected tool {}", tool.name)))
    }
}

#[derive(Default)]
struct FakeFileTransferExecutor {
    calls: Mutex<Vec<String>>,
}
impl FileTransferExecutor for FakeFileTransferExecutor {
    fn copy_to_box(
        &self,
        args: CopyToBoxArgs,
        tool_call_id: &str,
    ) -> Result<String, ProviderSessionError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("to:{tool_call_id}:{}", args.computer_path));
        Ok("copied in".into())
    }

    fn copy_from_box(
        &self,
        args: CopyFromBoxArgs,
        tool_call_id: &str,
    ) -> Result<String, ProviderSessionError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("from:{tool_call_id}:{}", args.box_path));
        Ok("copied out".into())
    }
}

#[test]
fn runner_file_transfer_bridge_exposes_frozen_tools_and_preserves_tool_call_scope() {
    let executor = Arc::new(FakeFileTransferExecutor::default());
    let bridge = SandFileTransferToolBridge::new(
        Arc::new(EmptyToolBridge),
        executor.clone(),
    );
    let tools = bridge.list_tools().expect("file transfer tools");
    let definitions = file_transfer_tool_definitions();
    assert_eq!(tools.len(), definitions.len());
    let copy_to = tools
        .iter()
        .find(|tool| tool.name == "CopyToBox")
        .expect("CopyToBox");
    let result = bridge
        .call_tool(
            copy_to,
            json!({"computer_path":"/Users/me/input.csv"}),
            "tool-copy-7",
        )
        .expect("copy tool result");
    assert_eq!(result, Value::String("copied in".into()));
    assert_eq!(
        executor.calls.lock().unwrap().as_slice(),
        ["to:tool-copy-7:/Users/me/input.csv"]
    );
}
