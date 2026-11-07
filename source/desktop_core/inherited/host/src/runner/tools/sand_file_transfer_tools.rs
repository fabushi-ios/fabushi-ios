use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

use crate::r#box::box_transfer::{
    BoxTransferError, SAND_BOX_UPLOADS_DIR, TransferBox, TransferEndpoint,
    resolve_box_workspace_path, transfer_file_between_boxes,
};
use crate::ports::r#box::SAND_BOX_NOT_READY_MESSAGE;

#[derive(Debug, Clone)]
pub struct UserComputerHandle<BoxType> {
    pub id: String,
    pub label: String,
    pub connected: bool,
    pub box_: Arc<BoxType>,
}

#[derive(Debug, Clone)]
pub struct FileTransferController<AgentBoxType, UserBoxType> {
    pub agent_box: Arc<AgentBoxType>,
    pub user_computers: Vec<UserComputerHandle<UserBoxType>>,
    pub default_computer_id: Option<String>,
    pub computer_agent_id: String,
    pub box_id: String,
    pub box_preparing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyToBoxArgs {
    pub computer_path: String,
    pub box_path: Option<String>,
    pub computer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyFromBoxArgs {
    pub box_path: String,
    pub computer_path: Option<String>,
    pub computer: Option<String>,
}

pub fn format_bytes(bytes: usize) -> String {
    if bytes < 1_024 {
        return format!("{bytes} bytes");
    }
    let units = ["KB", "MB", "GB"];
    let mut value = bytes as f64 / 1_024.0;
    let mut unit_index = 0usize;
    while value >= 1_024.0 && unit_index < units.len() - 1 {
        value /= 1_024.0;
        unit_index += 1;
    }
    if value >= 10.0 || value.fract() == 0.0 {
        format!("{value:.0} {}", units[unit_index])
    } else {
        format!("{value:.1} {}", units[unit_index])
    }
}

fn posix_basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

impl<AgentBoxType, UserBoxType> FileTransferController<AgentBoxType, UserBoxType> {
    pub fn resolve_computer_or_throw(
        &self,
        computer_id: Option<&str>,
    ) -> Result<&UserComputerHandle<UserBoxType>, BoxTransferError> {
        let requested = computer_id.map(str::trim).filter(|value| !value.is_empty());
        let resolved = if let Some(id) = requested {
            self.user_computers
                .iter()
                .find(|computer| computer.id == id && computer.connected)
        } else if let Some(default_id) = self.default_computer_id.as_deref() {
            self.user_computers
                .iter()
                .find(|computer| computer.id == default_id && computer.connected)
        } else {
            let mut connected = self.user_computers.iter().filter(|computer| computer.connected);
            let first = connected.next();
            if connected.next().is_none() { first } else { None }
        };
        if let Some(handle) = resolved {
            return Ok(handle);
        }

        let known = if self.user_computers.is_empty() {
            "none connected".to_string()
        } else {
            self.user_computers
                .iter()
                .map(|computer| {
                    if computer.connected {
                        computer.id.clone()
                    } else {
                        format!("{} (offline)", computer.id)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        match requested {
            None => Err(BoxTransferError(format!(
                "No computer is connected right now (the Fabushi desktop app must be open and online to transfer files). Connected computers: {known}."
            ))),
            Some(id) => Err(BoxTransferError(format!(
                "Unknown computer \"{id}\". Connected computers: {known}."
            ))),
        }
    }

    pub fn assert_box_ready(&self) -> Result<(), BoxTransferError> {
        if self.box_preparing {
            Err(BoxTransferError(SAND_BOX_NOT_READY_MESSAGE.to_string()))
        } else {
            Ok(())
        }
    }
}

pub fn copy_file_to_box<Ctx, AgentBoxType, UserBoxType>(
    context: &Ctx,
    args: &CopyToBoxArgs,
    controller: &FileTransferController<AgentBoxType, UserBoxType>,
) -> Result<String, BoxTransferError>
where
    AgentBoxType: TransferBox<Ctx>,
    UserBoxType: TransferBox<Ctx>,
{
    controller.assert_box_ready()?;
    let computer = controller.resolve_computer_or_throw(args.computer.as_deref())?;
    let box_path = resolve_box_workspace_path(
        args.box_path
            .as_deref()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                format!("{SAND_BOX_UPLOADS_DIR}/{}", posix_basename(&args.computer_path))
            })
            .as_str(),
    );
    let bytes = transfer_file_between_boxes(
        context,
        TransferEndpoint {
            box_: computer.box_.as_ref(),
            agent_id: &controller.computer_agent_id,
            path: &args.computer_path,
            label: &computer.label,
        },
        TransferEndpoint {
            box_: controller.agent_box.as_ref(),
            agent_id: &controller.box_id,
            path: &box_path,
            label: "your box",
        },
        None,
    )?;
    Ok(format!(
        "Copied {} from {} into your box at {} ({}). Open it with Shell.",
        args.computer_path,
        computer.label,
        box_path,
        format_bytes(bytes),
    ))
}

pub fn copy_file_from_box<Ctx, AgentBoxType, UserBoxType>(
    context: &Ctx,
    args: &CopyFromBoxArgs,
    controller: &FileTransferController<AgentBoxType, UserBoxType>,
) -> Result<String, BoxTransferError>
where
    AgentBoxType: TransferBox<Ctx>,
    UserBoxType: TransferBox<Ctx>,
{
    controller.assert_box_ready()?;
    let computer = controller.resolve_computer_or_throw(args.computer.as_deref())?;
    let box_path = resolve_box_workspace_path(&args.box_path);
    let computer_path = args
        .computer_path
        .clone()
        .unwrap_or_else(|| posix_basename(&box_path));
    let bytes = transfer_file_between_boxes(
        context,
        TransferEndpoint {
            box_: controller.agent_box.as_ref(),
            agent_id: &controller.box_id,
            path: &box_path,
            label: "your box",
        },
        TransferEndpoint {
            box_: computer.box_.as_ref(),
            agent_id: &controller.computer_agent_id,
            path: &computer_path,
            label: &computer.label,
        },
        None,
    )?;
    Ok(format!(
        "Copied {} from your box to {} at {} ({}). The user can open it there with ExternalShell.",
        box_path,
        computer.label,
        computer_path,
        format_bytes(bytes),
    ))
}


pub const COPY_TO_BOX_TOOL_NAME: &str = "CopyToBox";
pub const COPY_FROM_BOX_TOOL_NAME: &str = "CopyFromBox";
pub const FILE_TRANSFER_TOOL_PROVIDER: &str = "fabushi-runner";

pub trait FileTransferExecutor: Send + Sync {
    fn copy_to_box(
        &self,
        args: CopyToBoxArgs,
        tool_call_id: &str,
    ) -> Result<String, ProviderSessionError>;

    fn copy_from_box(
        &self,
        args: CopyFromBoxArgs,
        tool_call_id: &str,
    ) -> Result<String, ProviderSessionError>;
}

#[derive(Clone)]
pub struct SandFileTransferToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    executor: Arc<dyn FileTransferExecutor>,
}

impl SandFileTransferToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        executor: Arc<dyn FileTransferExecutor>,
    ) -> Self {
        Self { delegate, executor }
    }
}

pub fn file_transfer_tool_definitions() -> Vec<RoutedToolDefinition> {
    vec![
        RoutedToolDefinition {
            name: COPY_TO_BOX_TOOL_NAME.into(),
            provider_identifier: FILE_TRANSFER_TOOL_PROVIDER.into(),
            tool_name: COPY_TO_BOX_TOOL_NAME.into(),
            description: Some(
                "Copy a file from the user's computer into your box, verbatim. Use this to bring a user's file onto your box so you can work on it with Shell or Read."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["computer_path"],
                "properties": {
                    "computer_path": {"type": "string", "minLength": 1},
                    "box_path": {"type": "string", "minLength": 1},
                    "computer": {"type": "string", "minLength": 1}
                }
            }),
        },
        RoutedToolDefinition {
            name: COPY_FROM_BOX_TOOL_NAME.into(),
            provider_identifier: FILE_TRANSFER_TOOL_PROVIDER.into(),
            tool_name: COPY_FROM_BOX_TOOL_NAME.into(),
            description: Some(
                "Copy a file from your box out to the user's computer, verbatim. Use this to hand the user a file you generated or downloaded in the box."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["box_path"],
                "properties": {
                    "box_path": {"type": "string", "minLength": 1},
                    "computer_path": {"type": "string", "minLength": 1},
                    "computer": {"type": "string", "minLength": 1}
                }
            }),
        },
    ]
}

fn required_string(args: &Value, key: &str, tool: &str) -> Result<String, ProviderSessionError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| ProviderSessionError::Tool(format!("{tool} requires {key}")))
}

fn optional_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

impl RoutedToolBridge for SandFileTransferToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        let names = file_transfer_tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<BTreeSet<_>>();
        tools.retain(|tool| !names.contains(&tool.name) && !names.contains(&tool.tool_name));
        let mut ours = file_transfer_tool_definitions();
        ours.extend(tools);
        Ok(ours)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let effective = if matches!(tool.name.as_str(), COPY_TO_BOX_TOOL_NAME | COPY_FROM_BOX_TOOL_NAME) {
            tool.name.as_str()
        } else {
            tool.tool_name.as_str()
        };
        match effective {
            COPY_TO_BOX_TOOL_NAME => {
                if !args.is_object() {
                    return Err(ProviderSessionError::Tool(
                        "CopyToBox arguments must be an object".into(),
                    ));
                }
                let result = self.executor.copy_to_box(
                    CopyToBoxArgs {
                        computer_path: required_string(&args, "computer_path", COPY_TO_BOX_TOOL_NAME)?,
                        box_path: optional_string(&args, "box_path"),
                        computer: optional_string(&args, "computer"),
                    },
                    tool_call_id,
                )?;
                Ok(Value::String(result))
            }
            COPY_FROM_BOX_TOOL_NAME => {
                if !args.is_object() {
                    return Err(ProviderSessionError::Tool(
                        "CopyFromBox arguments must be an object".into(),
                    ));
                }
                let result = self.executor.copy_from_box(
                    CopyFromBoxArgs {
                        box_path: required_string(&args, "box_path", COPY_FROM_BOX_TOOL_NAME)?,
                        computer_path: optional_string(&args, "computer_path"),
                        computer: optional_string(&args, "computer"),
                    },
                    tool_call_id,
                )?;
                Ok(Value::String(result))
            }
            _ => self.delegate.call_tool(tool, args, tool_call_id),
        }
    }
}
