use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Number, Value};

use crate::extensions::inference::provider_session::{ProviderSessionError, RoutedToolDefinition};
use crate::extensions::transcript::client_side_tool_v2_producer::ClientSideToolV2ProducedValue;

const CALL_TYPE: &str = "aiserver.v1.ClientSideToolV2Call";
const RESULT_TYPE: &str = "aiserver.v1.ClientSideToolV2Result";

const TOOL_RUN_TERMINAL_COMMAND_V2: i64 = 15;
const TOOL_WEB_SEARCH: i64 = 18;
const TOOL_EDIT_FILE_V2: i64 = 38;
const TOOL_READ_FILE_V2: i64 = 40;
const TOOL_LIST_MCP_RESOURCES: i64 = 44;
const TOOL_READ_MCP_RESOURCE: i64 = 45;
const TOOL_TASK_V2: i64 = 48;
const TOOL_CALL_MCP_TOOL: i64 = 49;
const TOOL_ASK_QUESTION: i64 = 51;
const TOOL_GENERATE_IMAGE: i64 = 53;
const TOOL_COMPUTER_USE: i64 = 54;
const TOOL_RECORD_SCREEN: i64 = 56;
const TOOL_WEB_FETCH: i64 = 57;
const TOOL_MCP_AUTH: i64 = 60;
const TOOL_GET_MCP_TOOLS: i64 = 63;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProjectionPhase {
    Started,
    Partial,
    Completed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectedClientSideToolV2 {
    Call {
        tool_call_id: String,
        name: String,
        raw_args: String,
        model_call_id: Option<String>,
        is_streaming: bool,
        message: Value,
        protobuf_bytes: Vec<u8>,
    },
    Result {
        tool_call_id: String,
        name: String,
        output: String,
        error: Option<String>,
        message: Value,
        protobuf_bytes: Vec<u8>,
    },
}

impl ProjectedClientSideToolV2 {
    pub fn message(&self) -> &Value {
        match self {
            Self::Call { message, .. } | Self::Result { message, .. } => message,
        }
    }

    pub fn protobuf_bytes(&self) -> &[u8] {
        match self {
            Self::Call { protobuf_bytes, .. } | Self::Result { protobuf_bytes, .. } => protobuf_bytes,
        }
    }

    pub fn into_produced_value(self) -> ClientSideToolV2ProducedValue {
        match self {
            Self::Call { tool_call_id, protobuf_bytes, .. } => {
                ClientSideToolV2ProducedValue::call(tool_call_id, protobuf_bytes)
            }
            Self::Result { tool_call_id, protobuf_bytes, .. } => {
                ClientSideToolV2ProducedValue::result(tool_call_id, protobuf_bytes)
            }
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/client_side_tool_v2_schema.rs"));

fn push_varint(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            return;
        }
    }
}

fn push_key(output: &mut Vec<u8>, field_number: u64, wire_type: u8) {
    push_varint(output, (field_number << 3) | u64::from(wire_type));
}

fn push_length_delimited(output: &mut Vec<u8>, field_number: u64, bytes: &[u8]) {
    push_key(output, field_number, 2);
    push_varint(output, bytes.len() as u64);
    output.extend_from_slice(bytes);
}

fn scalar_wire_type(scalar: u64) -> Option<u8> {
    match scalar {
        1 | 6 | 16 => Some(1),
        2 | 7 | 15 => Some(5),
        3 | 4 | 5 | 8 | 13 | 17 | 18 => Some(0),
        9 | 12 => Some(2),
        _ => None,
    }
}

fn value_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

fn value_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
}

fn encode_scalar_payload(value: &Value, scalar: u64) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    match scalar {
        1 => output.extend_from_slice(&value.as_f64()?.to_le_bytes()),
        2 => output.extend_from_slice(&(value.as_f64()? as f32).to_le_bytes()),
        3 => push_varint(&mut output, value_i64(value)? as u64),
        4 => push_varint(&mut output, value_u64(value)?),
        5 => push_varint(&mut output, value_i64(value)? as i32 as u32 as u64),
        6 => output.extend_from_slice(&value_u64(value)?.to_le_bytes()),
        7 => output.extend_from_slice(&(value_u64(value)? as u32).to_le_bytes()),
        8 => push_varint(&mut output, u64::from(value.as_bool()?)),
        9 => output.extend_from_slice(value.as_str()?.as_bytes()),
        12 => {
            if let Some(encoded) = value.as_str() {
                output.extend_from_slice(&STANDARD.decode(encoded).ok()?);
            } else {
                for byte in value.as_array()? {
                    output.push(u8::try_from(value_u64(byte)?).ok()?);
                }
            }
        }
        13 => push_varint(&mut output, value_u64(value)? as u32 as u64),
        15 => output.extend_from_slice(&(value_i64(value)? as i32).to_le_bytes()),
        16 => output.extend_from_slice(&value_i64(value)?.to_le_bytes()),
        17 => {
            let value = value_i64(value)? as i32;
            push_varint(&mut output, ((value << 1) ^ (value >> 31)) as u32 as u64);
        }
        18 => {
            let value = value_i64(value)?;
            push_varint(&mut output, ((value << 1) ^ (value >> 63)) as u64);
        }
        _ => return None,
    }
    Some(output)
}

fn encode_scalar_field(output: &mut Vec<u8>, number: u64, scalar: u64, value: &Value) -> Option<()> {
    let payload = encode_scalar_payload(value, scalar)?;
    let wire = scalar_wire_type(scalar)?;
    if wire == 2 {
        push_length_delimited(output, number, &payload);
    } else {
        push_key(output, number, wire);
        output.extend_from_slice(&payload);
    }
    Some(())
}

fn encode_enum_field(output: &mut Vec<u8>, number: u64, value: &Value) -> Option<()> {
    push_key(output, number, 0);
    push_varint(output, value_u64(value)?);
    Some(())
}

fn encode_map_key(output: &mut Vec<u8>, scalar: u64, key: &str) -> Option<()> {
    let value = match scalar {
        8 => Value::Bool(key == "true"),
        9 => Value::String(key.to_string()),
        _ => Value::Number(Number::from(key.parse::<i64>().ok()?)),
    };
    encode_scalar_field(output, 1, scalar, &value)
}

fn encode_map_value(
    output: &mut Vec<u8>,
    kind: ClientToolProtoMapValueKind,
    value: &Value,
) -> Option<()> {
    match kind {
        ClientToolProtoMapValueKind::Scalar(scalar) => encode_scalar_field(output, 2, scalar, value),
        ClientToolProtoMapValueKind::Enum => encode_enum_field(output, 2, value),
        ClientToolProtoMapValueKind::Message(type_name) => {
            let bytes = encode_proto_message(type_name, value)?;
            push_length_delimited(output, 2, &bytes);
            Some(())
        }
    }
}

fn encode_field(
    output: &mut Vec<u8>,
    field: ClientToolProtoFieldDescriptor,
    value: &Value,
) -> Option<()> {
    match field.kind {
        ClientToolProtoFieldKind::Scalar(scalar) => encode_scalar_field(output, field.number, scalar, value),
        ClientToolProtoFieldKind::Enum => encode_enum_field(output, field.number, value),
        ClientToolProtoFieldKind::Message(type_name) => {
            let bytes = encode_proto_message(type_name, value)?;
            push_length_delimited(output, field.number, &bytes);
            Some(())
        }
        ClientToolProtoFieldKind::Map { key_scalar, value: value_kind } => {
            for (key, value) in value.as_object()? {
                let mut entry = Vec::new();
                encode_map_key(&mut entry, key_scalar, key)?;
                encode_map_value(&mut entry, value_kind, value)?;
                push_length_delimited(output, field.number, &entry);
            }
            Some(())
        }
    }
}

fn encode_proto_message(type_name: &str, value: &Value) -> Option<Vec<u8>> {
    let object = value.as_object()?;
    let descriptor = client_side_tool_proto_message_descriptor(type_name)?;
    let mut output = Vec::new();
    for field in descriptor.fields {
        let Some(value) = object.get(field.json_name) else { continue };
        if value.is_null() {
            continue;
        }
        if field.repeated && !matches!(field.kind, ClientToolProtoFieldKind::Map { .. }) {
            for item in value.as_array()? {
                encode_field(&mut output, *field, item)?;
            }
        } else {
            encode_field(&mut output, *field, value)?;
        }
    }
    Some(output)
}

fn json_value_message(value: &Value) -> Value {
    match value {
        Value::Null => json!({"nullValue": 0}),
        Value::Bool(value) => json!({"boolValue": value}),
        Value::Number(value) => json!({"numberValue": value.as_f64().unwrap_or_default()}),
        Value::String(value) => json!({"stringValue": value}),
        Value::Array(values) => json!({
            "listValue": {
                "values": values.iter().map(json_value_message).collect::<Vec<_>>()
            }
        }),
        Value::Object(values) => json!({
            "structValue": {
                "fields": values.iter()
                    .map(|(key, value)| (key.clone(), json_value_message(value)))
                    .collect::<Map<_, _>>()
            }
        }),
    }
}

fn protobuf_struct(value: &Value) -> Value {
    json!({
        "fields": value.as_object()
            .map(|object| object.iter()
                .map(|(key, value)| (key.clone(), json_value_message(value)))
                .collect::<Map<_, _>>())
            .unwrap_or_default()
    })
}

fn call_message(
    tool: i64,
    call_id: &str,
    name: &str,
    raw_args: Value,
    model_call_id: &str,
    streaming: bool,
    params: Option<(&str, Value)>,
) -> Option<ProjectedClientSideToolV2> {
    if call_id.is_empty() || name.is_empty() {
        return None;
    }
    let raw_args_string = serde_json::to_string(&raw_args).ok()?;
    let mut message = json!({
        "tool": tool,
        "toolCallId": call_id,
        "name": name,
        "rawArgs": raw_args_string,
        "isStreaming": streaming
    });
    if !model_call_id.is_empty() {
        message["modelCallId"] = Value::String(model_call_id.to_string());
    }
    if let Some((case, value)) = params {
        message[case] = value;
    }
    let protobuf_bytes = encode_proto_message(CALL_TYPE, &message)?;
    Some(ProjectedClientSideToolV2::Call {
        tool_call_id: call_id.to_string(),
        name: name.to_string(),
        raw_args: raw_args_string,
        model_call_id: (!model_call_id.is_empty()).then(|| model_call_id.to_string()),
        is_streaming: streaming,
        message,
        protobuf_bytes,
    })
}

fn result_message(
    tool: i64,
    call_id: &str,
    name: &str,
    result: Option<(&str, Value)>,
    error: Option<String>,
) -> Option<ProjectedClientSideToolV2> {
    if call_id.is_empty() || name.is_empty() || (result.is_none() && error.is_none()) {
        return None;
    }
    let mut message = json!({"tool": tool, "toolCallId": call_id});
    let mut output = String::new();
    if let Some((case, value)) = result {
        output = serde_json::to_string(&value).unwrap_or_default();
        message[case] = value;
    }
    if let Some(error) = error.as_ref() {
        message["error"] = json!({
            "clientVisibleErrorMessage": error,
            "modelVisibleErrorMessage": error
        });
    }
    let protobuf_bytes = encode_proto_message(RESULT_TYPE, &message)?;
    Some(ProjectedClientSideToolV2::Result {
        tool_call_id: call_id.to_string(),
        name: name.to_string(),
        output,
        error,
        message,
        protobuf_bytes,
    })
}

fn string_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn failure_text(value: &Value, fallback: &str) -> String {
    if let Some(value) = value.as_str().map(str::trim).filter(|value| !value.is_empty()) {
        return value.to_string();
    }
    string_field(value, &["errorMessage", "error", "reason", "message"])
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn shell_call(
    phase: ToolProjectionPhase,
    call_id: &str,
    args: &Value,
    model_call_id: &str,
    surface_name: &str,
) -> Option<ProjectedClientSideToolV2> {
    let command = string_field(args, &["command"])?;
    let cwd = string_field(args, &["workingDirectory", "cwd"]).unwrap_or_default();
    let is_background = args.get("isBackground")
        .or_else(|| args.get("is_background"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let skip_approval = args.get("skipApproval").and_then(Value::as_bool).unwrap_or(false);
    call_message(
        TOOL_RUN_TERMINAL_COMMAND_V2,
        call_id,
        if surface_name.is_empty() { "Shell" } else { surface_name },
        json!({"command": command, "cwd": cwd}),
        model_call_id,
        phase == ToolProjectionPhase::Partial,
        Some(("runTerminalCommandV2Params", json!({
            "command": command,
            "cwd": cwd,
            "isBackground": is_background,
            "requireUserApproval": !skip_approval
        }))),
    )
}

fn shell_result(call_id: &str, args: &Value, value: &Value, surface_name: &str) -> Option<ProjectedClientSideToolV2> {
    let name = if surface_name.is_empty() { "Shell" } else { surface_name };
    if value.is_string() {
        return result_message(TOOL_RUN_TERMINAL_COMMAND_V2, call_id, name, None, Some(failure_text(value, "Command rejected")));
    }
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("failure");
    if kind == "permissionDenied" {
        return result_message(
            TOOL_RUN_TERMINAL_COMMAND_V2,
            call_id,
            name,
            None,
            Some(format!("Permission denied: {}", failure_text(value, "Command permission denied"))),
        );
    }
    if matches!(kind, "spawnError" | "error") {
        return result_message(TOOL_RUN_TERMINAL_COMMAND_V2, call_id, name, None, Some(failure_text(value, "Command could not be started")));
    }
    let stdout = value.get("stdout").and_then(Value::as_str).unwrap_or_default();
    let stderr = value.get("stderr").and_then(Value::as_str).unwrap_or_default();
    let output = string_field(value, &["interleavedOutput"])
        .map(str::to_string)
        .unwrap_or_else(|| [stdout, stderr].into_iter().filter(|value| !value.is_empty()).collect::<Vec<_>>().join("\n"));
    let aborted = value.get("aborted").and_then(Value::as_bool).unwrap_or(false);
    let ended_reason = if kind == "success" { 1 } else if kind == "rejected" { 0 } else if kind == "timeout" { 5 } else if aborted { 2 } else { 3 };
    let is_background = args.get("isBackground")
        .or_else(|| args.get("is_background"))
        .and_then(Value::as_bool)
        .unwrap_or(false) || kind == "backgrounded";
    let mut result = json!({
        "output": output,
        "outputRaw": output,
        "exitCode": value.get("exitCode").and_then(Value::as_i64).unwrap_or_default(),
        "poppedOutIntoBackground": is_background,
        "isRunningInBackground": is_background,
        "resultingWorkingDirectory": string_field(value, &["workingDirectory"]).unwrap_or_default(),
        "endedReason": ended_reason
    });
    if let Some(code) = value.get("exitCode").and_then(Value::as_i64) {
        result["exitCodeV2"] = Value::Number(Number::from(code));
    }
    if kind == "rejected" {
        result["rejected"] = Value::Bool(true);
    }
    result_message(TOOL_RUN_TERMINAL_COMMAND_V2, call_id, name, Some(("runTerminalCommandV2Result", result)), None)
}

fn read_call(
    phase: ToolProjectionPhase,
    call_id: &str,
    args: &Value,
    model_call_id: &str,
    surface_name: &str,
) -> Option<ProjectedClientSideToolV2> {
    let path = string_field(args, &["path"])?;
    call_message(
        TOOL_READ_FILE_V2,
        call_id,
        if surface_name.is_empty() { "Read" } else { surface_name },
        args.clone(),
        model_call_id,
        phase == ToolProjectionPhase::Partial,
        Some(("readFileV2Params", json!({
            "targetFile": path,
            "offset": args.get("offset").cloned().unwrap_or(Value::Null),
            "limit": args.get("limit").cloned().unwrap_or(Value::Null),
            "enableLineNumbers": args.get("includeLineNumbers").cloned().unwrap_or(Value::Null)
        }))),
    )
}

fn read_result(call_id: &str, value: &Value, surface_name: &str) -> Option<ProjectedClientSideToolV2> {
    let name = if surface_name.is_empty() { "Read" } else { surface_name };
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or("error");
    if kind != "success" {
        return result_message(TOOL_READ_FILE_V2, call_id, name, None, Some(failure_text(value, &format!("Tool failed: {kind}"))));
    }
    let output = value.get("output")?;
    if output.get("kind").and_then(Value::as_str) != Some("content") {
        return result_message(
            TOOL_READ_FILE_V2,
            call_id,
            name,
            None,
            Some("Read completed with non-text content; bytes remain available in ordinary transcript transport".into()),
        );
    }
    let contents = output.get("content").and_then(Value::as_str).unwrap_or_default();
    result_message(
        TOOL_READ_FILE_V2,
        call_id,
        name,
        Some(("readFileV2Result", json!({
            "contents": contents,
            "numCharactersInRequestedRange": contents.encode_utf16().count(),
            "totalLinesInFile": value.get("totalLines").and_then(Value::as_i64).unwrap_or_default()
        }))),
        None,
    )
}

fn task_call(phase: ToolProjectionPhase, call_id: &str, args: &Value, model_call_id: &str) -> Option<ProjectedClientSideToolV2> {
    let prompt = string_field(args, &["prompt"])?;
    let description = string_field(args, &["description"]).unwrap_or_default();
    call_message(
        TOOL_TASK_V2,
        call_id,
        "Task",
        args.clone(),
        model_call_id,
        phase == ToolProjectionPhase::Partial,
        Some(("taskV2Params", json!({
            "description": description,
            "prompt": prompt,
            "subagentType": string_field(args, &["subagentType", "subagent_type"]).unwrap_or_default(),
            "model": args.get("model").cloned().unwrap_or(Value::Null),
            "name": description,
            "mode": args.get("mode").cloned().unwrap_or_else(|| Value::Number(Number::from(0)))
        }))),
    )
}

fn task_result(call_id: &str, value: &Value) -> Option<ProjectedClientSideToolV2> {
    let agent_id = string_field(value, &["agentId", "subagent_id", "id"])?;
    result_message(
        TOOL_TASK_V2,
        call_id,
        "Task",
        Some(("taskV2Result", json!({
            "agentId": agent_id,
            "isBackground": value.get("isBackground").and_then(Value::as_bool).unwrap_or(true),
            "cloudAgentBcId": value.get("cloudAgentBcId").cloned().unwrap_or(Value::Null)
        }))),
        None,
    )
}

fn mcp_call(
    phase: ToolProjectionPhase,
    call_id: &str,
    provider_identifier: &str,
    tool_name: &str,
    display_name: &str,
    args: &Value,
    model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if tool_name.trim().is_empty() {
        return None;
    }
    call_message(
        TOOL_CALL_MCP_TOOL,
        call_id,
        if display_name.trim().is_empty() { "MCP" } else { display_name },
        args.clone(),
        model_call_id,
        phase == ToolProjectionPhase::Partial,
        Some(("callMcpToolParams", json!({
            "server": provider_identifier,
            "toolName": tool_name,
            "toolArgs": protobuf_struct(args)
        }))),
    )
}

fn mcp_result(
    call_id: &str,
    provider_identifier: &str,
    tool_name: &str,
    display_name: &str,
    value: &Value,
) -> Option<ProjectedClientSideToolV2> {
    result_message(
        TOOL_CALL_MCP_TOOL,
        call_id,
        if display_name.trim().is_empty() { "MCP" } else { display_name },
        Some(("callMcpToolResult", json!({
            "server": provider_identifier,
            "toolName": tool_name,
            "result": protobuf_struct(value)
        }))),
        None,
    )
}

fn terminal_result<'a>(value: &'a Value) -> (&'a str, &'a Value) {
    if let Some(case) = value.get("case").and_then(Value::as_str) {
        return (case, value.get("value").unwrap_or(value));
    }
    if let Some(kind) = value.get("kind").and_then(Value::as_str) {
        return (kind, value.get("value").unwrap_or(value));
    }
    if value.get("rejected").and_then(Value::as_bool) == Some(true) {
        return ("rejected", value);
    }
    if value.get("isError").and_then(Value::as_bool) == Some(true)
        || value.get("error").is_some()
    {
        return ("error", value);
    }
    ("success", value)
}

fn completed_error(tool: i64, call_id: &str, name: &str, value: &Value, fallback: &str) -> Option<ProjectedClientSideToolV2> {
    result_message(tool, call_id, name, None, Some(failure_text(value, fallback)))
}

fn edit_projection(
    phase: ToolProjectionPhase,
    call_id: &str,
    args: &Value,
    result: Option<Result<Value, String>>,
    model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        let path = string_field(args, &["path"])?;
        let mut params = json!({"relativeWorkspacePath": path});
        if let Some(content) = args.get("streamContent").and_then(Value::as_str) {
            params["streamingContent"] = Value::String(content.to_string());
        }
        return call_message(
            TOOL_EDIT_FILE_V2, call_id, "Edit", json!({"path": path}), model_call_id,
            phase == ToolProjectionPhase::Partial, Some(("editFileV2Params", params)),
        );
    }
    let value = match result? {
        Ok(value) => value,
        Err(error) => return result_message(TOOL_EDIT_FILE_V2, call_id, "Edit", None, Some(error)),
    };
    let (case, payload) = terminal_result(&value);
    if case == "permissionDenied" {
        return completed_error(TOOL_EDIT_FILE_V2, call_id, "Edit", payload, "Permission denied");
    }
    if !matches!(case, "success" | "rejected") {
        return completed_error(TOOL_EDIT_FILE_V2, call_id, "Edit", payload, "Edit failed");
    }
    let before = payload.get("beforeFullFileContent").cloned().unwrap_or(Value::Null);
    let after = payload.get("afterFullFileContent").cloned().unwrap_or(Value::Null);
    let diff_string = string_field(payload, &["diffString"]).unwrap_or_default();
    let mut projected = json!({
        "fileWasCreated": before.is_null(),
        "rejected": case == "rejected",
        "resultForModel": string_field(payload, &["message", "reason"]).unwrap_or_default()
    });
    if !before.is_null() { projected["contentsBeforeEdit"] = before; }
    if !after.is_null() { projected["contentsAfterEdit"] = after; }
    if !diff_string.is_empty() {
        projected["diff"] = json!({"chunks":[{"diffString":diff_string}]});
    }
    result_message(TOOL_EDIT_FILE_V2, call_id, "Edit", Some(("editFileV2Result", projected)), None)
}

fn list_mcp_resources_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value, String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        return call_message(TOOL_LIST_MCP_RESOURCES, call_id, "ListMcpResources", args.clone(), model_call_id,
            phase == ToolProjectionPhase::Partial,
            Some(("listMcpResourcesParams", json!({"server": args.get("server").cloned().unwrap_or(Value::Null)}))));
    }
    let value = match result? { Ok(v) => v, Err(e) => return result_message(TOOL_LIST_MCP_RESOURCES, call_id, "ListMcpResources", None, Some(e)) };
    let (case,payload)=terminal_result(&value);
    if case != "success" { return completed_error(TOOL_LIST_MCP_RESOURCES, call_id, "ListMcpResources", payload, "List MCP resources failed"); }
    result_message(TOOL_LIST_MCP_RESOURCES, call_id, "ListMcpResources", Some(("listMcpResourcesResult", json!({"resources": payload.get("resources").cloned().unwrap_or_else(|| json!([]))}))), None)
}

fn read_mcp_resource_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value, String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        let server=string_field(args,&["server"])?;
        let uri=string_field(args,&["uri"])?;
        return call_message(TOOL_READ_MCP_RESOURCE, call_id, "ReadMcpResource", args.clone(), model_call_id,
            phase == ToolProjectionPhase::Partial,
            Some(("readMcpResourceParams", json!({"server":server,"uri":uri,"downloadPath":args.get("downloadPath").cloned().unwrap_or(Value::Null)}))));
    }
    let value=match result? {Ok(v)=>v,Err(e)=>return result_message(TOOL_READ_MCP_RESOURCE,call_id,"ReadMcpResource",None,Some(e))};
    let (case,payload)=terminal_result(&value);
    if case!="success" {return completed_error(TOOL_READ_MCP_RESOURCE,call_id,"ReadMcpResource",payload,"Read MCP resource failed")}
    result_message(TOOL_READ_MCP_RESOURCE,call_id,"ReadMcpResource",Some(("readMcpResourceResult",payload.clone())),None)
}

fn ask_question_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        let questions=args.get("questions").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(|q| {
            let options=q.get("options").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(|o| json!({
                "id":string_field(&o,&["id"]).unwrap_or_default(), "label":string_field(&o,&["label"]).unwrap_or_default()
            })).collect::<Vec<_>>();
            json!({"id":string_field(&q,&["id"]).unwrap_or_default(),"prompt":string_field(&q,&["prompt"]).unwrap_or_default(),"allowMultiple":q.get("allowMultiple").and_then(Value::as_bool).unwrap_or(false),"options":options})
        }).collect::<Vec<_>>();
        return call_message(TOOL_ASK_QUESTION,call_id,"AskQuestion",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,
            Some(("askQuestionParams",json!({"title":string_field(args,&["title"]).unwrap_or_default(),"questions":questions,"runAsync":args.get("runAsync").and_then(Value::as_bool).unwrap_or(false)}))));
    }
    let value=match result? {Ok(v)=>v,Err(e)=>return result_message(TOOL_ASK_QUESTION,call_id,"AskQuestion",None,Some(e))};
    let (case,payload)=terminal_result(&value);
    if matches!(case,"error"|"rejected") {return completed_error(TOOL_ASK_QUESTION,call_id,"AskQuestion",payload,"Question rejected")}
    let answers=payload.get("answers").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(|a| json!({
        "questionId":string_field(&a,&["questionId"]).unwrap_or_default(),
        "selectedOptionIds":a.get("selectedOptionIds").cloned().unwrap_or_else(||json!([])),
        "freeformText":a.get("freeformText").cloned().unwrap_or(Value::Null)
    })).collect::<Vec<_>>();
    result_message(TOOL_ASK_QUESTION,call_id,"AskQuestion",Some(("askQuestionResult",json!({"isAsync":case=="async","answers":answers}))),None)
}

fn mcp_auth_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        let server=string_field(args,&["serverIdentifier"])?;
        return call_message(TOOL_MCP_AUTH,call_id,"McpAuth",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,
            Some(("mcpAuthParams",json!({"serverIdentifier":server}))));
    }
    let value=match result? {Ok(v)=>v,Err(e)=>return result_message(TOOL_MCP_AUTH,call_id,"McpAuth",None,Some(e))};
    let (case,payload)=terminal_result(&value);
    if case!="success" {return completed_error(TOOL_MCP_AUTH,call_id,"McpAuth",payload,"MCP authentication failed")}
    result_message(TOOL_MCP_AUTH,call_id,"McpAuth",Some(("mcpAuthResult",json!({"success":true,"message":string_field(payload,&["serverIdentifier"]).unwrap_or_default()}))),None)
}

fn web_search_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        let term=string_field(args,&["searchTerm"])?;
        return call_message(TOOL_WEB_SEARCH,call_id,"WebSearch",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,
            Some(("webSearchParams",json!({"searchTerm":term}))));
    }
    let value=match result? {Ok(v)=>v,Err(e)=>return result_message(TOOL_WEB_SEARCH,call_id,"WebSearch",None,Some(e))};
    let (case,payload)=terminal_result(&value);
    if case=="error" {return completed_error(TOOL_WEB_SEARCH,call_id,"WebSearch",payload,"Web search failed")}
    result_message(TOOL_WEB_SEARCH,call_id,"WebSearch",Some(("webSearchResult",json!({
        "rejected":case=="rejected","isFinal":true,
        "references":payload.get("references").cloned().unwrap_or_else(||json!([]))
    }))),None)
}

fn web_fetch_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        let url=string_field(args,&["url"])?;
        return call_message(TOOL_WEB_FETCH,call_id,"WebFetch",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,
            Some(("webFetchParams",json!({"url":url}))));
    }
    let value=match result? {Ok(v)=>v,Err(e)=>return result_message(TOOL_WEB_FETCH,call_id,"WebFetch",None,Some(e))};
    let (case,payload)=terminal_result(&value);
    let url=string_field(payload,&["url"]).or_else(||string_field(args,&["url"])).unwrap_or_default();
    let projected=if case=="success" {json!({"url":url,"markdown":string_field(payload,&["markdown"]).unwrap_or_default()})}
        else {json!({"url":url,"error":failure_text(payload,"Web fetch failed")})};
    result_message(TOOL_WEB_FETCH,call_id,"WebFetch",Some(("webFetchResult",projected)),None)
}

fn computer_use_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        return call_message(TOOL_COMPUTER_USE,call_id,"Computer",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,
            Some(("computerUseParams",json!({"actions":args.get("actions").cloned().unwrap_or_else(||json!([]))}))));
    }
    match result? {Ok(v)=>result_message(TOOL_COMPUTER_USE,call_id,"Computer",Some(("computerUseResult",v)),None),Err(e)=>result_message(TOOL_COMPUTER_USE,call_id,"Computer",None,Some(e))}
}

fn generate_image_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        string_field(args,&["description"])?;
        return call_message(TOOL_GENERATE_IMAGE,call_id,"GenerateImage",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,None);
    }
    match result? {Ok(v)=>result_message(TOOL_GENERATE_IMAGE,call_id,"GenerateImage",Some(("generateImageResult",v)),None),Err(e)=>result_message(TOOL_GENERATE_IMAGE,call_id,"GenerateImage",None,Some(e))}
}

fn record_screen_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        return call_message(TOOL_RECORD_SCREEN,call_id,"RecordScreen",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,Some(("recordScreenParams",args.clone())));
    }
    match result? {Ok(v)=>result_message(TOOL_RECORD_SCREEN,call_id,"RecordScreen",Some(("recordScreenResult",v)),None),Err(e)=>result_message(TOOL_RECORD_SCREEN,call_id,"RecordScreen",None,Some(e))}
}

fn get_mcp_tools_projection(
    phase: ToolProjectionPhase, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if phase != ToolProjectionPhase::Completed {
        return call_message(TOOL_GET_MCP_TOOLS,call_id,"GetMcpTools",args.clone(),model_call_id,phase==ToolProjectionPhase::Partial,Some(("getMcpToolsParams",json!({
            "server":args.get("server").cloned().unwrap_or(Value::Null),"toolName":args.get("toolName").cloned().unwrap_or(Value::Null),"pattern":args.get("pattern").cloned().unwrap_or(Value::Null)
        }))));
    }
    let value=match result? {Ok(v)=>v,Err(e)=>return result_message(TOOL_GET_MCP_TOOLS,call_id,"GetMcpTools",None,Some(e))};
    let (case,payload)=terminal_result(&value);
    if case!="success" {return completed_error(TOOL_GET_MCP_TOOLS,call_id,"GetMcpTools",payload,"Get MCP tools failed")}
    result_message(TOOL_GET_MCP_TOOLS,call_id,"GetMcpTools",Some(("getMcpToolsResult",json!({
        "content":string_field(payload,&["content"]).unwrap_or_default(),"outputFilePath":payload.get("outputFilePath").cloned().unwrap_or(Value::Null)
    }))),None)
}

fn project_remaining_tool_case(
    phase: ToolProjectionPhase, case: &str, call_id: &str, args: &Value,
    result: Option<Result<Value,String>>, model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    match case {
        "Edit"|"editToolCall" => edit_projection(phase,call_id,args,result,model_call_id),
        "ListMcpResources"|"listMcpResourcesToolCall" => list_mcp_resources_projection(phase,call_id,args,result,model_call_id),
        "ReadMcpResource"|"readMcpResourceToolCall" => read_mcp_resource_projection(phase,call_id,args,result,model_call_id),
        "AskQuestion"|"askQuestionToolCall" => ask_question_projection(phase,call_id,args,result,model_call_id),
        "McpAuth"|"mcpAuthToolCall" => mcp_auth_projection(phase,call_id,args,result,model_call_id),
        "WebSearch"|"webSearchToolCall" => web_search_projection(phase,call_id,args,result,model_call_id),
        "WebFetch"|"webFetchToolCall" => web_fetch_projection(phase,call_id,args,result,model_call_id),
        "Computer"|"computerUseToolCall" => computer_use_projection(phase,call_id,args,result,model_call_id),
        "GenerateImage"|"generateImageToolCall" => generate_image_projection(phase,call_id,args,result,model_call_id),
        "RecordScreen"|"recordScreenToolCall" => record_screen_projection(phase,call_id,args,result,model_call_id),
        "GetMcpTools"|"getMcpToolsToolCall" => get_mcp_tools_projection(phase,call_id,args,result,model_call_id),
        _ => None,
    }
}

pub fn project_generated_tool_case(
    phase: ToolProjectionPhase,
    case: &str,
    call_id: &str,
    args: &Value,
    result: Option<Result<Value, String>>,
    model_call_id: &str,
    surface_name: &str,
) -> Option<ProjectedClientSideToolV2> {
    match case {
        "shellToolCall" => match phase {
            ToolProjectionPhase::Started | ToolProjectionPhase::Partial => shell_call(phase, call_id, args, model_call_id, surface_name),
            ToolProjectionPhase::Completed => match result? {
                Ok(value) => shell_result(call_id, args, &value, surface_name),
                Err(error) => result_message(TOOL_RUN_TERMINAL_COMMAND_V2, call_id, if surface_name.is_empty() { "Shell" } else { surface_name }, None, Some(error)),
            },
        },
        "readToolCall" => match phase {
            ToolProjectionPhase::Started | ToolProjectionPhase::Partial => read_call(phase, call_id, args, model_call_id, surface_name),
            ToolProjectionPhase::Completed => match result? {
                Ok(value) => read_result(call_id, &value, surface_name),
                Err(error) => result_message(TOOL_READ_FILE_V2, call_id, if surface_name.is_empty() { "Read" } else { surface_name }, None, Some(error)),
            },
        },
        "taskToolCall" => match phase {
            ToolProjectionPhase::Started | ToolProjectionPhase::Partial => task_call(phase, call_id, args, model_call_id),
            ToolProjectionPhase::Completed => match result? {
                Ok(value) => task_result(call_id, &value),
                Err(error) => result_message(TOOL_TASK_V2, call_id, "Task", None, Some(error)),
            },
        },
        "mcpToolCall" => {
            let provider = string_field(args, &["serverIdentifier", "providerIdentifier", "server"]).unwrap_or_default();
            let tool_name = string_field(args, &["toolName", "name"]).unwrap_or_default();
            let nested_args = args.get("args").unwrap_or(args);
            match phase {
                ToolProjectionPhase::Started | ToolProjectionPhase::Partial => mcp_call(phase, call_id, provider, tool_name, string_field(args, &["name"]).unwrap_or("MCP"), nested_args, model_call_id),
                ToolProjectionPhase::Completed => match result? {
                    Ok(value) => mcp_result(call_id, provider, tool_name, string_field(args, &["name"]).unwrap_or("MCP"), &value),
                    Err(error) => result_message(TOOL_CALL_MCP_TOOL, call_id, "MCP", None, Some(error)),
                },
            }
        }
        other => project_remaining_tool_case(phase, other, call_id, args, result, model_call_id),
    }
}

fn is_internal_provider(provider: &str) -> bool {
    provider.is_empty()
        || provider.starts_with("mahayana")
        || provider.starts_with("fabushi")
        || provider.starts_with("sand")
}

pub fn project_routed_tool_call(
    phase: ToolProjectionPhase,
    tool: &RoutedToolDefinition,
    args: &Value,
    call_id: &str,
    model_call_id: &str,
    result: Option<Result<Value, String>>,
) -> Option<ProjectedClientSideToolV2> {
    if call_id.is_empty() {
        return None;
    }
    let effective = if tool.tool_name.trim().is_empty() { tool.name.as_str() } else { tool.tool_name.as_str() };
    match effective {
        "Shell" | "ExternalShell" => match phase {
            ToolProjectionPhase::Started | ToolProjectionPhase::Partial => shell_call(phase, call_id, args, model_call_id, effective),
            ToolProjectionPhase::Completed => match result? {
                Ok(value) => shell_result(call_id, args, &value, effective),
                Err(error) => result_message(TOOL_RUN_TERMINAL_COMMAND_V2, call_id, effective, None, Some(error)),
            },
        },
        "Read" | "ExternalRead" => match phase {
            ToolProjectionPhase::Started | ToolProjectionPhase::Partial => read_call(phase, call_id, args, model_call_id, effective),
            ToolProjectionPhase::Completed => match result? {
                Ok(value) => read_result(call_id, &value, effective),
                Err(error) => result_message(TOOL_READ_FILE_V2, call_id, effective, None, Some(error)),
            },
        },
        "Task" => match phase {
            ToolProjectionPhase::Started | ToolProjectionPhase::Partial => task_call(phase, call_id, args, model_call_id),
            ToolProjectionPhase::Completed => match result? {
                Ok(value) => task_result(call_id, &value),
                Err(error) => result_message(TOOL_TASK_V2, call_id, "Task", None, Some(error)),
            },
        },
        _ if !is_internal_provider(&tool.provider_identifier) => match phase {
            ToolProjectionPhase::Started | ToolProjectionPhase::Partial => {
                mcp_call(phase, call_id, &tool.provider_identifier, effective, &tool.name, args, model_call_id)
            }
            ToolProjectionPhase::Completed => match result? {
                Ok(value) => mcp_result(call_id, &tool.provider_identifier, effective, &tool.name, &value),
                Err(error) => result_message(TOOL_CALL_MCP_TOOL, call_id, &tool.name, None, Some(error)),
            },
        },
        _ => project_generated_tool_case(phase, effective, call_id, args, result, model_call_id, effective),
    }
}

fn override_call_raw_args(
    projected: ProjectedClientSideToolV2,
    raw_arguments: &str,
) -> Option<ProjectedClientSideToolV2> {
    let ProjectedClientSideToolV2::Call {
        tool_call_id, name, model_call_id, is_streaming, mut message, ..
    } = projected else { return None };
    message["rawArgs"] = Value::String(raw_arguments.to_string());
    let protobuf_bytes = encode_proto_message(CALL_TYPE, &message)?;
    Some(ProjectedClientSideToolV2::Call {
        tool_call_id,
        name,
        raw_args: raw_arguments.to_string(),
        model_call_id,
        is_streaming,
        message,
        protobuf_bytes,
    })
}

fn partial_identity(tool: &RoutedToolDefinition) -> Option<(i64, &str)> {
    let effective = if tool.tool_name.trim().is_empty() { tool.name.as_str() } else { tool.tool_name.as_str() };
    if !is_internal_provider(&tool.provider_identifier) {
        return Some((TOOL_CALL_MCP_TOOL, if tool.name.trim().is_empty() { "MCP" } else { tool.name.as_str() }));
    }
    match effective {
        "Shell"|"ExternalShell" => Some((TOOL_RUN_TERMINAL_COMMAND_V2,effective)),
        "Read"|"ExternalRead" => Some((TOOL_READ_FILE_V2,effective)),
        "Task" => Some((TOOL_TASK_V2,"Task")),
        "Edit"|"editToolCall" => Some((TOOL_EDIT_FILE_V2,"Edit")),
        "ListMcpResources"|"listMcpResourcesToolCall" => Some((TOOL_LIST_MCP_RESOURCES,"ListMcpResources")),
        "ReadMcpResource"|"readMcpResourceToolCall" => Some((TOOL_READ_MCP_RESOURCE,"ReadMcpResource")),
        "AskQuestion"|"askQuestionToolCall" => Some((TOOL_ASK_QUESTION,"AskQuestion")),
        "McpAuth"|"mcpAuthToolCall" => Some((TOOL_MCP_AUTH,"McpAuth")),
        "WebSearch"|"webSearchToolCall" => Some((TOOL_WEB_SEARCH,"WebSearch")),
        "WebFetch"|"webFetchToolCall" => Some((TOOL_WEB_FETCH,"WebFetch")),
        "Computer"|"computerUseToolCall" => Some((TOOL_COMPUTER_USE,"Computer")),
        "GenerateImage"|"generateImageToolCall" => Some((TOOL_GENERATE_IMAGE,"GenerateImage")),
        "RecordScreen"|"recordScreenToolCall" => Some((TOOL_RECORD_SCREEN,"RecordScreen")),
        "GetMcpTools"|"getMcpToolsToolCall" => Some((TOOL_GET_MCP_TOOLS,"GetMcpTools")),
        _ => None,
    }
}

pub fn project_routed_tool_partial_raw(
    phase: ToolProjectionPhase,
    tool: &RoutedToolDefinition,
    raw_arguments: &str,
    call_id: &str,
    model_call_id: &str,
) -> Option<ProjectedClientSideToolV2> {
    if !matches!(phase, ToolProjectionPhase::Started | ToolProjectionPhase::Partial)
        || call_id.trim().is_empty()
    {
        return None;
    }
    if let Ok(args) = serde_json::from_str::<Value>(raw_arguments) {
        if let Some(projected) = project_routed_tool_call(
            phase, tool, &args, call_id, model_call_id, None,
        ) {
            return override_call_raw_args(projected, raw_arguments);
        }
    }
    let (tool_kind, name) = partial_identity(tool)?;
    let projected = call_message(
        tool_kind,
        call_id,
        name,
        Value::Object(Map::new()),
        model_call_id,
        phase == ToolProjectionPhase::Partial,
        None,
    )?;
    override_call_raw_args(projected, raw_arguments)
}

pub fn project_basic_tool_call(
    phase: ToolProjectionPhase,
    call_id: &str,
    name: &str,
    raw_args: &str,
    model_call_id: &str,
    result: Option<Result<&str, &str>>,
) -> Option<ProjectedClientSideToolV2> {
    if call_id.is_empty() || name.is_empty() {
        return None;
    }
    let args = serde_json::from_str::<Value>(raw_args).unwrap_or_else(|_| Value::Object(Map::new()));
    match phase {
        ToolProjectionPhase::Started | ToolProjectionPhase::Partial => call_message(
            0,
            call_id,
            name,
            args,
            model_call_id,
            phase == ToolProjectionPhase::Partial,
            None,
        ),
        ToolProjectionPhase::Completed => match result? {
            Ok(output) => {
                let mut projected = result_message(0, call_id, name, None, Some(output.to_string()))?;
                if let ProjectedClientSideToolV2::Result { error, output: stored, .. } = &mut projected {
                    *stored = output.to_string();
                    *error = None;
                }
                Some(projected)
            }
            Err(error) => result_message(0, call_id, name, None, Some(error.to_string())),
        },
    }
}

pub fn projection_error(error: &ProviderSessionError) -> String {
    error.to_string()
}
