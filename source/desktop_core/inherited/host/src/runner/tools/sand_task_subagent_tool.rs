use std::sync::Arc;

use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{ProviderSessionError, RoutedToolDefinition};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

pub const SAND_TASK_TOOL_NAME: &str = "Task";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentLaunchRecord {
    pub id: String,
    pub subagent_type: String,
}

pub type SubagentTaskReviewCallback = Arc<
    dyn Fn(&str, &str, &str) -> Result<Option<String>, ProviderSessionError> + Send + Sync + 'static,
>;

pub trait SubagentTaskSink: Send + Sync {
    fn launch_subagent(
        &self,
        prompt: &str,
        subagent_type: &str,
        tool_call_id: &str,
    ) -> Result<SubagentLaunchRecord, ProviderSessionError>;
}

pub struct SubagentTaskToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    sink: Arc<dyn SubagentTaskSink>,
    review: Option<SubagentTaskReviewCallback>,
    allowed_subagent_types: Option<Arc<Vec<String>>>,
}

impl SubagentTaskToolBridge {
    pub fn new(delegate: Arc<dyn RoutedToolBridge>, sink: Arc<dyn SubagentTaskSink>) -> Self {
        Self {
            delegate,
            sink,
            review: None,
            allowed_subagent_types: None,
        }
    }

    pub fn with_allowed_subagent_types(mut self, allowed: Arc<Vec<String>>) -> Self {
        self.allowed_subagent_types = Some(allowed);
        self
    }

    pub fn with_review(mut self, review: SubagentTaskReviewCallback) -> Self {
        self.review = Some(review);
        self
    }
}

fn task_definition(allowed_subagent_types: Option<&[String]>) -> RoutedToolDefinition {
    let subagent_type_schema = match allowed_subagent_types {
        Some(allowed) => json!({
            "type": "string",
            "enum": allowed,
        }),
        None => json!({"type":"string","minLength":1}),
    };
    RoutedToolDefinition {
        name: SAND_TASK_TOOL_NAME.into(),
        provider_identifier: "fabushi-runner".into(),
        tool_name: SAND_TASK_TOOL_NAME.into(),
        description: Some(
            "Delegate a self-contained task to a background subagent. The child runs asynchronously through the same shipping Runner boundary and the parent is revived separately when it settles."
                .into(),
        ),
        input_schema: json!({
            "type": "object",
            "required": ["prompt"],
            "additionalProperties": false,
            "properties": {
                "prompt": {"type":"string","minLength":1},
                "subagent_type": subagent_type_schema
            }
        }),
    }
}

impl RoutedToolBridge for SubagentTaskToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.retain(|tool| {
            !tool.name.eq_ignore_ascii_case(SAND_TASK_TOOL_NAME)
                && !tool.tool_name.eq_ignore_ascii_case(SAND_TASK_TOOL_NAME)
        });
        tools.insert(
            0,
            task_definition(self.allowed_subagent_types.as_deref().map(Vec::as_slice)),
        );
        Ok(tools)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        if !tool.name.eq_ignore_ascii_case(SAND_TASK_TOOL_NAME)
            && !tool.tool_name.eq_ignore_ascii_case(SAND_TASK_TOOL_NAME)
        {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }
        let prompt = args
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| ProviderSessionError::Tool("Task requires prompt".into()))?;
        let requested_subagent_type = args
            .get("subagent_type")
            .or_else(|| args.get("subagentType"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let subagent_type = match (requested_subagent_type, self.allowed_subagent_types.as_ref()) {
            (Some(value), Some(allowed)) => allowed
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(value))
                .map(String::as_str)
                .ok_or_else(|| {
                    ProviderSessionError::Tool(format!(
                        "Task subagent type is unavailable for this turn: {value}"
                    ))
                })?,
            (None, Some(allowed)) => allowed.first().map(String::as_str).ok_or_else(|| {
                ProviderSessionError::Tool("Task has no available subagent types for this turn".into())
            })?,
            (Some(value), None) => value,
            (None, None) => "general-purpose",
        };
        if let Some(review) = self.review.as_ref() {
            if let Some(reason) = review(prompt, subagent_type, tool_call_id)? {
                return Ok(Value::String(reason));
            }
        }
        let launched = self
            .sink
            .launch_subagent(prompt, subagent_type, tool_call_id)?;
        Ok(json!({
            "subagent_id": launched.id,
            "subagent_type": launched.subagent_type,
            "status": "running"
        }))
    }
}
