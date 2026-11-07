use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use crate::extensions::session::production::ProductionSessionWorkers;
use crate::runner::routed_provider_runtime::RoutedToolBridge;
use crate::sand_multitask::SAND_MULTITASK_TODO_DESCRIPTION;

pub const SAND_MULTITASK_TODO_TOOL_NAME: &str = "TodoWrite";
pub const SAND_MULTITASK_TODOS_KV: &str = "multitaskTodos";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultitaskTodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl MultitaskTodoStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultitaskTodoItem {
    pub id: String,
    pub content: String,
    pub status: MultitaskTodoStatus,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

pub trait MultitaskTodoState: Send + Sync {
    fn load_todos(&self) -> Result<Vec<MultitaskTodoItem>, String>;
    fn store_todos(&self, todos: &[MultitaskTodoItem]) -> Result<(), String>;
}

pub struct ProductionMultitaskTodoState {
    sessions: Arc<ProductionSessionWorkers>,
    agent_id: String,
}

impl ProductionMultitaskTodoState {
    pub fn new(sessions: Arc<ProductionSessionWorkers>, agent_id: impl Into<String>) -> Self {
        Self {
            sessions,
            agent_id: agent_id.into(),
        }
    }
}

impl MultitaskTodoState for ProductionMultitaskTodoState {
    fn load_todos(&self) -> Result<Vec<MultitaskTodoItem>, String> {
        let Some(raw) = self
            .sessions
            .read_agent_kv_projection(&self.agent_id, SAND_MULTITASK_TODOS_KV)?
        else {
            return Ok(Vec::new());
        };
        serde_json::from_str(&raw)
            .map_err(|error| format!("could not decode durable multitask todos: {error}"))
    }

    fn store_todos(&self, todos: &[MultitaskTodoItem]) -> Result<(), String> {
        let raw = serde_json::to_string(todos)
            .map_err(|error| format!("could not encode durable multitask todos: {error}"))?;
        let wrote = self.sessions.write_agent_kv_projection(
            &self.agent_id,
            SAND_MULTITASK_TODOS_KV,
            &raw,
        )?;
        if !wrote {
            return Err("multitask todo write was rejected by session admission".into());
        }
        Ok(())
    }
}

pub struct SandMultitaskTodoToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    state: Arc<dyn MultitaskTodoState>,
}

impl SandMultitaskTodoToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        state: Arc<dyn MultitaskTodoState>,
    ) -> Self {
        Self { delegate, state }
    }
}

impl RoutedToolBridge for SandMultitaskTodoToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.retain(|tool| {
            tool.name != SAND_MULTITASK_TODO_TOOL_NAME
                && tool.tool_name != SAND_MULTITASK_TODO_TOOL_NAME
        });
        tools.insert(0, multitask_todo_definition());
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if tool.name != SAND_MULTITASK_TODO_TOOL_NAME
            && tool.tool_name != SAND_MULTITASK_TODO_TOOL_NAME
        {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }
        let todos = apply_multitask_todo_update(self.state.as_ref(), &args)?;
        Ok(Value::String(render_multitask_todo_success(&todos)))
    }
}

pub fn multitask_todo_definition() -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: SAND_MULTITASK_TODO_TOOL_NAME.into(),
        provider_identifier: "fabushi-runner".into(),
        tool_name: SAND_MULTITASK_TODO_TOOL_NAME.into(),
        description: Some(SAND_MULTITASK_TODO_DESCRIPTION.into()),
        input_schema: json!({
            "type": "object",
            "required": ["todos", "merge"],
            "additionalProperties": false,
            "properties": {
                "todos": {
                    "type": "array",
                    "minItems": 2,
                    "description": "Array of TODO items to update or create",
                    "items": {
                        "type": "object",
                        "required": ["id", "content", "status"],
                        "additionalProperties": false,
                        "properties": {
                            "id": {"type":"string","description":"Unique identifier for the TODO item"},
                            "content": {"type":"string","description":"The description/content of the todo item"},
                            "status": {"type":"string","enum":["pending","in_progress","completed","cancelled"]}
                        }
                    }
                },
                "merge": {
                    "type":"boolean",
                    "description":"Whether to merge the todos with the existing todos. If true, the todos will be merged into the existing todos based on the id field. If false, the new todos will replace the existing todos."
                }
            }
        }),
    }
}

struct TodoPatch {
    id: String,
    content: Option<String>,
    status: MultitaskTodoStatus,
}

pub fn apply_multitask_todo_update(
    state: &dyn MultitaskTodoState,
    args: &Value,
) -> Result<Vec<MultitaskTodoItem>, ProviderSessionError> {
    let object = args.as_object().ok_or_else(|| tool_error(
        "TodoWrite arguments must be an object",
    ))?;
    let merge = parse_merge(object.get("merge"))?;
    let values = object
        .get("todos")
        .and_then(Value::as_array)
        .ok_or_else(|| tool_error("TodoWrite requires a todos array"))?;
    let patches = values
        .iter()
        .map(parse_patch)
        .collect::<Result<Vec<_>, _>>()?;
    let now = now_ms();

    let mut todos = state.load_todos().map_err(tool_error)?;
    if merge {
        for patch in patches {
            if let Some(existing) = todos.iter_mut().find(|todo| todo.id == patch.id) {
                if let Some(content) = patch.content {
                    existing.content = content;
                }
                if existing.content.is_empty() {
                    return Err(tool_error(
                        "Invalid argument: must provide 'content' for new todos items",
                    ));
                }
                existing.status = patch.status;
                existing.updated_at_ms = now;
            } else {
                let content = patch.content.unwrap_or_default();
                if content.is_empty() {
                    return Err(tool_error(
                        "Invalid argument: must provide 'content' for new todos items",
                    ));
                }
                todos.push(MultitaskTodoItem {
                    id: patch.id,
                    content,
                    status: patch.status,
                    created_at_ms: now,
                    updated_at_ms: now,
                });
            }
        }
    } else {
        todos = patches
            .into_iter()
            .map(|patch| MultitaskTodoItem {
                id: patch.id,
                content: patch.content.unwrap_or_default(),
                status: patch.status,
                created_at_ms: now,
                updated_at_ms: now,
            })
            .collect();
    }
    state.store_todos(&todos).map_err(tool_error)?;
    Ok(todos)
}

fn parse_patch(value: &Value) -> Result<TodoPatch, ProviderSessionError> {
    let object = value.as_object().ok_or_else(|| tool_error(
        "TodoWrite todo entries must be objects",
    ))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| tool_error("TodoWrite todo.id must be a string"))?
        .to_string();
    let content = match object.get("content") {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Null) | None => None,
        Some(_) => return Err(tool_error("TodoWrite todo.content must be a string")),
    };
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .and_then(MultitaskTodoStatus::parse)
        .ok_or_else(|| tool_error(
            "TodoWrite todo.status must be pending, in_progress, completed, or cancelled",
        ))?;
    Ok(TodoPatch {
        id,
        content,
        status,
    })
}

fn parse_merge(value: Option<&Value>) -> Result<bool, ProviderSessionError> {
    match value {
        None => Ok(true),
        Some(Value::Bool(value)) => Ok(*value),
        Some(Value::String(value)) if value.eq_ignore_ascii_case("true") || value == "1" => Ok(true),
        Some(Value::String(value)) if value.eq_ignore_ascii_case("false") || value == "0" => Ok(false),
        Some(Value::Number(value)) if value.as_i64() == Some(1) => Ok(true),
        Some(Value::Number(value)) if value.as_i64() == Some(0) => Ok(false),
        _ => Err(tool_error("TodoWrite merge must be a boolean")),
    }
}

pub fn render_multitask_todo_success(todos: &[MultitaskTodoItem]) -> String {
    let mut message = String::from(
        "Successfully updated TODOs. Make sure to follow and update your TODO list as you make progress. Cancel and add new TODO tasks as needed when the user makes a correction or follow-up request.",
    );
    if todos.iter().any(|todo| todo.status == MultitaskTodoStatus::Pending)
        && todos.iter().all(|todo| todo.status != MultitaskTodoStatus::InProgress)
    {
        message.push_str(
            " No TODOs are marked in-progress, make sure to mark them before starting the next.",
        );
    }
    if todos.iter().filter(|todo| todo.status.is_finished()).count() > 20 {
        message.push_str(
            "\n\n<system_reminder>You have many finished todos. Consider cleaning up old ones.</system_reminder>",
        );
    }
    message.push_str("\n\nHere are the latest contents of your todo list:");
    for todo in todos {
        message.push_str(&format!(
            "\n- **{}**: {} (id: {})",
            todo.status.as_str().to_ascii_uppercase(),
            todo.content,
            todo.id,
        ));
    }
    message
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn tool_error(message: impl Into<String>) -> ProviderSessionError {
    ProviderSessionError::Tool(message.into())
}
