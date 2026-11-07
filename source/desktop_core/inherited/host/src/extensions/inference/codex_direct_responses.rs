use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CodexDirectUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl CodexDirectUsage {
    fn add(self, next: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.saturating_add(next.input_tokens),
            output_tokens: self.output_tokens.saturating_add(next.output_tokens),
            cache_read_tokens: self.cache_read_tokens.saturating_add(next.cache_read_tokens),
            cache_write_tokens: self.cache_write_tokens.saturating_add(next.cache_write_tokens),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexDirectTool {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
    pub source: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexDirectOptions {
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub instructions: String,
    pub input: Vec<Value>,
    pub tools: Vec<CodexDirectTool>,
    pub max_steps: usize,
}

impl CodexDirectOptions {
    pub fn new(
        model: impl Into<String>,
        instructions: impl Into<String>,
        input: Vec<Value>,
    ) -> Self {
        Self {
            model: model.into(),
            reasoning_effort: None,
            instructions: instructions.into(),
            input,
            tools: Vec::new(),
            max_steps: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexDirectResult {
    pub text: String,
    pub response_id: String,
    pub usage: CodexDirectUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexDirectCheckpoint {
    pub input: Vec<Value>,
    pub text: String,
    pub response_id: String,
    pub usage: CodexDirectUsage,
    pub completed_steps: usize,
    pub tool_calls_completed: usize,
}

#[derive(Debug, Error)]
pub enum CodexDirectError {
    #[error("Codex direct transport failed: {0}")]
    Transport(String),
    #[error("Codex direct protocol failed: {0}")]
    Protocol(String),
    #[error("Codex direct tool failed: {0}")]
    Tool(String),
    #[error("Codex provider refusal: {0:?}")]
    ProviderRefusal(CodexProviderRefusal),
    #[error("Codex direct request cancelled: {0}")]
    Cancelled(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexProviderRefusal {
    pub category: String,
    pub explanation: Option<String>,
    pub continuation_message: Option<String>,
    pub error_type: Option<String>,
}

fn read_bounded_text(value: Option<&Value>, max_bytes: usize) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty() && text.len() <= max_bytes).then(|| text.to_string())
}

pub fn parse_codex_provider_refusal(value: &Value) -> Option<CodexProviderRefusal> {
    let error = value
        .get("error")
        .filter(|value| value.is_object())
        .unwrap_or(value);
    if error.get("code").and_then(Value::as_str) != Some("misalignment_policy_violation") {
        return None;
    }
    let misalignment = error.get("misalignment")?.as_object()?;
    Some(CodexProviderRefusal {
        category: "misalignment".into(),
        explanation: read_bounded_text(misalignment.get("detailed_explanation"), 64 * 1024),
        continuation_message: misalignment
            .get("steer")
            .and_then(Value::as_object)
            .and_then(|steer| read_bounded_text(steer.get("message"), 1024)),
        error_type: read_bounded_text(misalignment.get("error_type"), 256),
    })
}

pub trait CodexDirectTransport {
    fn stream_response(
        &mut self,
        request: &Value,
        on_event: &mut dyn FnMut(Value) -> Result<(), CodexDirectError>,
        should_cancel: &dyn Fn() -> bool,
    ) -> Result<(), CodexDirectError>;
}

fn usage_of(response: &Value) -> CodexDirectUsage {
    let usage = response.get("usage").and_then(Value::as_object);
    let details = usage
        .and_then(|usage| usage.get("input_tokens_details"))
        .and_then(Value::as_object);
    CodexDirectUsage {
        input_tokens: usage
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_read_tokens: details
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cache_write_tokens: 0,
    }
}

fn request_tools(tools: &[CodexDirectTool]) -> Option<Vec<Value>> {
    if tools.is_empty() {
        return None;
    }
    Some(
        tools
            .iter()
            .map(|tool| {
                let mut value = json!({
                    "type": "function",
                    "name": tool.name,
                    "parameters": tool.parameters,
                    "strict": false,
                });
                if let Some(description) = tool.description.as_deref() {
                    value["description"] = Value::String(description.to_string());
                }
                value
            })
            .collect(),
    )
}

fn safe_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| {
        json!({
            "isError": true,
            "error": format!("failed to serialize tool output: {error}")
        })
        .to_string()
    })
}

fn output_items(completed: &Value, observed: &[Value]) -> Vec<Value> {
    completed
        .get("output")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .cloned()
        .unwrap_or_else(|| observed.to_vec())
}

fn retain_executable_function_calls(output: &mut Vec<Value>, calls: &[Value]) {
    let executable_call_ids = calls
        .iter()
        .filter_map(|call| call.get("call_id").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    output.retain(|item| {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return true;
        }
        item.get("call_id")
            .and_then(Value::as_str)
            .is_some_and(|call_id| executable_call_ids.contains(call_id))
    });
}

fn append_balanced_tool_step(
    input: &mut Vec<Value>,
    output: Vec<Value>,
    results: Vec<Value>,
) -> Result<(), CodexDirectError> {
    let mut results_by_call_id = results
        .into_iter()
        .filter_map(|result| {
            result
                .get("call_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .map(|call_id| (call_id, result))
        })
        .collect::<HashMap<_, _>>();

    for item in output {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            input.push(item);
            continue;
        }
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CodexDirectError::Protocol(
                    "executable function_call is missing call_id during replay".into(),
                )
            })?
            .to_string();
        let result = results_by_call_id.remove(&call_id).ok_or_else(|| {
            CodexDirectError::Protocol(format!(
                "function_call {call_id} has no matching function_call_output during replay"
            ))
        })?;
        input.push(item);
        input.push(result);
    }

    if !results_by_call_id.is_empty() {
        return Err(CodexDirectError::Protocol(
            "tool step produced a function_call_output without a matching function_call".into(),
        ));
    }
    Ok(())
}

fn parse_dsml_compat_calls(text: &str) -> Option<Vec<(String, Value)>> {
    const CALLS_OPEN: &str = "<｜｜DSML｜｜ calls>";
    const CALLS_CLOSE: &str = "</｜｜DSML｜｜ calls>";
    const INVOKE_OPEN: &str = "<｜｜DSML｜｜ invoke name=\"";
    const INVOKE_CLOSE: &str = "</｜｜DSML｜｜ invoke>";
    const PARAM_OPEN: &str = "<｜｜DSML｜｜ parameter name=\"";
    const PARAM_CLOSE: &str = "</｜｜DSML｜｜ parameter>";

    let trimmed = text.trim();
    let mut rest = trimmed
        .strip_prefix(CALLS_OPEN)?
        .strip_suffix(CALLS_CLOSE)?
        .trim();
    let mut calls = Vec::new();

    while !rest.is_empty() {
        let after_open = rest.strip_prefix(INVOKE_OPEN)?;
        let name_end = after_open.find("\">")?;
        let name = &after_open[..name_end];
        if name.is_empty()
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        {
            return None;
        }

        let body = &after_open[name_end + 2..];
        let invoke_end = body.find(INVOKE_CLOSE)?;
        let mut params = body[..invoke_end].trim();
        let mut arguments = serde_json::Map::new();

        while !params.is_empty() {
            let after_param_open = params.strip_prefix(PARAM_OPEN)?;
            let param_name_end = after_param_open.find('"')?;
            let param_name = &after_param_open[..param_name_end];
            if param_name.is_empty()
                || !param_name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
                || arguments.contains_key(param_name)
            {
                return None;
            }

            let after_name = &after_param_open[param_name_end + 1..];
            let tag_end = after_name.find('>')?;
            let attributes = after_name[..tag_end].trim();
            if !attributes.is_empty() && attributes != "string=\"true\"" {
                return None;
            }

            let value_and_tail = &after_name[tag_end + 1..];
            let value_end = value_and_tail.find(PARAM_CLOSE)?;
            let value = &value_and_tail[..value_end];
            arguments.insert(param_name.to_string(), Value::String(value.to_string()));
            params = value_and_tail[value_end + PARAM_CLOSE.len()..].trim();
        }

        if name == "SendMessage" && arguments.len() == 1 {
            if let Some(message) = arguments
                .remove("message")
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
            {
                arguments.insert("type".into(), Value::String("text".into()));
                arguments.insert("content".into(), Value::String(message));
            }
        }

        calls.push((name.to_string(), Value::Object(arguments)));
        rest = body[invoke_end + INVOKE_CLOSE.len()..].trim();
    }

    (!calls.is_empty()).then_some(calls)
}

fn dsml_compat_function_calls(
    text: &str,
    tools: &[CodexDirectTool],
    step: usize,
) -> Option<Vec<Value>> {
    let parsed = parse_dsml_compat_calls(text)?;
    if parsed.iter().any(|(name, _)| !tools.iter().any(|tool| tool.name == *name)) {
        return None;
    }
    Some(
        parsed
            .into_iter()
            .enumerate()
            .map(|(index, (name, arguments))| {
                json!({
                    "type": "function_call",
                    "name": name,
                    "call_id": format!("dsml-step-{step}-call-{index}"),
                    "arguments": safe_json(&arguments),
                })
            })
            .collect(),
    )
}

pub fn run_codex_direct_responses(
    transport: &mut dyn CodexDirectTransport,
    options: &CodexDirectOptions,
    execute_tool: &mut dyn FnMut(
        &CodexDirectTool,
        Value,
        &str,
    ) -> Result<Value, CodexDirectError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
) -> Result<CodexDirectResult, CodexDirectError> {
    let mut ignore_checkpoint =
        |_checkpoint: &CodexDirectCheckpoint| Ok(());
    run_codex_direct_responses_with_lifecycle(
        transport,
        options,
        None,
        execute_tool,
        on_text_delta,
        &mut ignore_checkpoint,
        &|| false,
    )
}

pub fn run_codex_direct_responses_with_cancel(
    transport: &mut dyn CodexDirectTransport,
    options: &CodexDirectOptions,
    execute_tool: &mut dyn FnMut(
        &CodexDirectTool,
        Value,
        &str,
    ) -> Result<Value, CodexDirectError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    should_cancel: &dyn Fn() -> bool,
) -> Result<CodexDirectResult, CodexDirectError> {
    let mut ignore_checkpoint =
        |_checkpoint: &CodexDirectCheckpoint| Ok(());
    run_codex_direct_responses_with_lifecycle(
        transport,
        options,
        None,
        execute_tool,
        on_text_delta,
        &mut ignore_checkpoint,
        should_cancel,
    )
}

pub fn run_codex_direct_responses_with_lifecycle(
    transport: &mut dyn CodexDirectTransport,
    options: &CodexDirectOptions,
    resume_from: Option<&CodexDirectCheckpoint>,
    execute_tool: &mut dyn FnMut(
        &CodexDirectTool,
        Value,
        &str,
    ) -> Result<Value, CodexDirectError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), CodexDirectError>,
    should_cancel: &dyn Fn() -> bool,
) -> Result<CodexDirectResult, CodexDirectError> {
    run_codex_direct_responses_with_lifecycle_and_tool_step_reminder(
        transport,
        options,
        resume_from,
        execute_tool,
        on_text_delta,
        on_checkpoint,
        should_cancel,
        None,
    )
}

pub fn run_codex_direct_responses_with_lifecycle_and_tool_step_reminder(
    transport: &mut dyn CodexDirectTransport,
    options: &CodexDirectOptions,
    resume_from: Option<&CodexDirectCheckpoint>,
    execute_tool: &mut dyn FnMut(
        &CodexDirectTool,
        Value,
        &str,
    ) -> Result<Value, CodexDirectError>,
    on_text_delta: &mut dyn FnMut(&str, &str),
    on_checkpoint: &mut dyn FnMut(
        &CodexDirectCheckpoint,
    ) -> Result<(), CodexDirectError>,
    should_cancel: &dyn Fn() -> bool,
    tool_step_reminder: Option<&(dyn Fn(&[(String, Value)]) -> Option<String> + Send + Sync)>,
) -> Result<CodexDirectResult, CodexDirectError> {
    let max_steps = options.max_steps.max(1);
    let tools_by_name = options
        .tools
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect::<HashMap<_, _>>();
    let declared_tools = request_tools(&options.tools);
    let (
        mut input,
        mut text,
        mut response_id,
        mut total_usage,
        mut completed_steps,
        mut tool_calls_completed,
    ) = match resume_from {
        Some(checkpoint) => (
            checkpoint.input.clone(),
            checkpoint.text.clone(),
            checkpoint.response_id.clone(),
            checkpoint.usage,
            checkpoint.completed_steps,
            checkpoint.tool_calls_completed,
        ),
        None => (
            options.input.clone(),
            String::new(),
            String::new(),
            CodexDirectUsage::default(),
            0,
            0,
        ),
    };

    if completed_steps >= max_steps {
        return Err(CodexDirectError::Protocol(format!(
            "provider resume checkpoint already exhausted Fabushi's {max_steps}-step tool limit"
        )));
    }

    for _step in completed_steps..max_steps {
        if should_cancel() {
            return Err(CodexDirectError::Cancelled(
                "Runner cancelled before provider dispatch".into(),
            ));
        }
        let mut request = json!({
            "model": options.model,
            "instructions": options.instructions,
            "input": input,
            "include": ["reasoning.encrypted_content"],
            "stream": true,
            "store": false,
        });
        if let Some(tools) = declared_tools.as_ref() {
            request["tools"] = Value::Array(tools.clone());
            request["tool_choice"] = Value::String("auto".into());
            request["parallel_tool_calls"] = Value::Bool(true);
        }
        if let Some(effort) = options.reasoning_effort.as_deref() {
            request["reasoning"] = json!({ "effort": effort, "summary": "auto" });
        }

        let step_text_start = text.len();
        let mut completed: Option<Value> = None;
        let mut observed_output = Vec::new();
        transport.stream_response(&request, &mut |event| {
            if should_cancel() {
                return Err(CodexDirectError::Cancelled(
                    "Runner cancelled the provider stream".into(),
                ));
            }
            match event.get("type").and_then(Value::as_str) {
                Some("response.output_text.delta") => {
                    if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                        text.push_str(delta);
                        on_text_delta(delta, &text);
                    }
                }
                Some("response.output_item.done") => {
                    if let Some(item) = event.get("item").filter(|value| value.is_object()) {
                        observed_output.push(item.clone());
                    }
                }
                Some("response.completed") => {
                    completed = event.get("response").filter(|value| value.is_object()).cloned();
                }
                Some("response.failed") | Some("error") => {
                    let failure = event
                        .get("response")
                        .and_then(|value| value.get("error"))
                        .or_else(|| event.get("error"))
                        .cloned()
                        .unwrap_or(event);
                    if let Some(refusal) = parse_codex_provider_refusal(&failure) {
                        return Err(CodexDirectError::ProviderRefusal(refusal));
                    }
                    return Err(CodexDirectError::Protocol(format!(
                        "provider returned failure: {}",
                        safe_json(&failure).chars().take(4096).collect::<String>()
                    )));
                }
                _ => {}
            }
            Ok(())
        }, should_cancel)?;

        let completed = completed.ok_or_else(|| {
            CodexDirectError::Protocol(
                "response stream ended without response.completed".into(),
            )
        })?;
        if let Some(id) = completed.get("id").and_then(Value::as_str) {
            response_id = id.to_string();
        }
        total_usage = total_usage.add(usage_of(&completed));

        let mut output = output_items(&completed, &observed_output);
        let mut calls = output
            .iter()
            .filter(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call")
                    && item.get("name").and_then(Value::as_str).is_some()
                    && item.get("call_id").and_then(Value::as_str).is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        if calls.is_empty() {
            let step_text = &text[step_text_start..];
            if let Some(dsml_calls) =
                dsml_compat_function_calls(step_text, &options.tools, completed_steps)
            {
                text.truncate(step_text_start);
                output = dsml_calls.clone();
                calls = dsml_calls;
            }
        }
        if calls.is_empty() {
            return Ok(CodexDirectResult {
                text,
                response_id,
                usage: total_usage,
            });
        }

        // Only replay tool calls for which this Host will append a matching
        // function_call_output. Provider streams can occasionally include an
        // incomplete function_call item alongside the completed calls. Feeding
        // that orphan back into the next request corrupts the conversation and
        // makes the Router reject it as an assistant tool call without a
        // corresponding tool result.
        retain_executable_function_calls(&mut output, &calls);

        let calls_in_step = calls.len();
        let mut results = Vec::new();
        let mut observed_tool_calls = Vec::with_capacity(calls_in_step);
        for call in calls {
            if should_cancel() {
                return Err(CodexDirectError::Cancelled(
                    "Runner cancelled before tool execution".into(),
                ));
            }
            let name = call.get("name").and_then(Value::as_str).unwrap_or("");
            let call_id = call.get("call_id").and_then(Value::as_str).unwrap_or("");
            let arguments = call
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let args = serde_json::from_str::<Value>(arguments)
                .unwrap_or_else(|_| json!({}));
            observed_tool_calls.push((name.to_string(), args.clone()));
            let output = match tools_by_name.get(name).copied() {
                None => json!({
                    "isError": true,
                    "error": format!("Unknown Fabushi tool: {name}")
                }),
                Some(tool) => match execute_tool(tool, args, call_id) {
                    Ok(value) => value,
                    Err(error) => json!({
                        "isError": true,
                        "error": error.to_string()
                    }),
                },
            };
            results.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": safe_json(&output),
            }));
        }

        // Keep each executed call adjacent to its result. The first-party
        // Responses adapter maps every function_call item to an assistant
        // tool_calls message. If parallel calls are replayed as
        // [call1, call2, result1, result2], that adapter produces two adjacent
        // assistant tool-call messages and the upstream Router correctly
        // rejects the first one as lacking an immediately following tool
        // result. Atomic [call1, result1, call2, result2] replay preserves the
        // Responses meaning while satisfying the downstream chat transcript
        // invariant.
        append_balanced_tool_step(&mut input, output, results)?;
        if let Some(reminder) =
            tool_step_reminder.and_then(|hook| hook(&observed_tool_calls))
        {
            input.push(json!({
                "role": "user",
                "content": reminder,
            }));
        }
        completed_steps = completed_steps.saturating_add(1);
        tool_calls_completed =
            tool_calls_completed.saturating_add(calls_in_step);
        on_checkpoint(&CodexDirectCheckpoint {
            input: input.clone(),
            text: text.clone(),
            response_id: response_id.clone(),
            usage: total_usage,
            completed_steps,
            tool_calls_completed,
        })?;
    }

    Err(CodexDirectError::Protocol(format!(
        "provider exceeded Fabushi's {max_steps}-step tool limit"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_tool_calls_are_replayed_as_adjacent_call_result_pairs() {
        let mut input = vec![json!({ "role": "user", "content": "inspect both" })];
        let output = vec![
            json!({
                "type": "reasoning",
                "encrypted_content": "opaque",
            }),
            json!({
                "type": "function_call",
                "name": "ReadFile",
                "call_id": "call-a",
                "arguments": "{}",
            }),
            json!({
                "type": "function_call",
                "name": "ListFiles",
                "call_id": "call-b",
                "arguments": "{}",
            }),
        ];
        let results = vec![
            json!({
                "type": "function_call_output",
                "call_id": "call-a",
                "output": "A",
            }),
            json!({
                "type": "function_call_output",
                "call_id": "call-b",
                "output": "B",
            }),
        ];

        append_balanced_tool_step(&mut input, output, results).unwrap();

        assert_eq!(
            input.iter().map(|item| {
                (
                    item.get("type").and_then(Value::as_str),
                    item.get("call_id").and_then(Value::as_str),
                )
            }).collect::<Vec<_>>(),
            vec![
                (None, None),
                (Some("reasoning"), None),
                (Some("function_call"), Some("call-a")),
                (Some("function_call_output"), Some("call-a")),
                (Some("function_call"), Some("call-b")),
                (Some("function_call_output"), Some("call-b")),
            ]
        );
    }

    #[test]
    fn incomplete_provider_tool_calls_are_not_replayed_without_results() {
        let mut output = vec![
            json!({
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "working"}],
            }),
            json!({
                "type": "function_call",
                "name": "ReadFile",
                "call_id": "call-complete",
                "arguments": "{}",
            }),
            json!({
                "type": "function_call",
                "name": "RunShell",
                "arguments": "{}",
            }),
        ];
        let calls = vec![output[1].clone()];

        retain_executable_function_calls(&mut output, &calls);

        assert_eq!(output.len(), 2);
        assert_eq!(
            output[1].get("call_id").and_then(Value::as_str),
            Some("call-complete")
        );
        assert!(
            output.iter().all(|item| {
                item.get("type").and_then(Value::as_str) != Some("function_call")
                    || item.get("call_id").and_then(Value::as_str) == Some("call-complete")
            }),
            "no orphan function_call may be replayed without a matching result"
        );
    }

    #[test]
    fn provider_refusal_parser_preserves_review_fields_without_inventing_native_ids() {
        let refusal = parse_codex_provider_refusal(&json!({
            "error": {
                "code": "misalignment_policy_violation",
                "misalignment": {
                    "detailed_explanation": "The request needs review.",
                    "steer": { "message": "Continue only the requested task." },
                    "error_type": "future_category"
                }
            }
        }))
        .expect("fixed refusal envelope");

        assert_eq!(refusal.category, "misalignment");
        assert_eq!(refusal.explanation.as_deref(), Some("The request needs review."));
        assert_eq!(
            refusal.continuation_message.as_deref(),
            Some("Continue only the requested task.")
        );
        assert_eq!(refusal.error_type.as_deref(), Some("future_category"));
    }

    #[test]
    fn provider_refusal_parser_rejects_other_error_codes() {
        assert!(parse_codex_provider_refusal(&json!({
            "error": {
                "code": "invalid_prompt",
                "misalignment": {
                    "detailed_explanation": "not a review"
                }
            }
        }))
        .is_none());
    }
}
