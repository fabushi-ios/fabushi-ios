use serde_json::{Map, Number, Value, json};

const CONVERSATION_MESSAGE_TYPE: &str = "aiserver.v1.ConversationMessage";

#[derive(Debug, Clone, Copy)]
enum CloudAgentProtoMapValueKind {
    Scalar(u64),
    Enum,
    Message(&'static str),
}

#[derive(Debug, Clone, Copy)]
enum CloudAgentProtoFieldKind {
    Scalar(u64),
    Enum,
    Message(&'static str),
    Map {
        key_scalar: u64,
        value: CloudAgentProtoMapValueKind,
    },
}

#[derive(Debug, Clone, Copy)]
struct CloudAgentProtoFieldDescriptor {
    number: u64,
    proto_name: &'static str,
    json_name: &'static str,
    kind: CloudAgentProtoFieldKind,
    repeated: bool,
    optional: bool,
    oneof: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
struct CloudAgentProtoMessageDescriptor {
    fields: &'static [CloudAgentProtoFieldDescriptor],
}

include!(concat!(env!("OUT_DIR"), "/cloud_agent_trace_schema.rs"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudAgentProtoDecodeError {
    MissingDescriptor,
    Truncated,
    VarintOverflow,
    LengthOverflow,
    InvalidWireType,
    WireTypeMismatch,
    InvalidUtf8,
    UnsupportedScalar,
    InvalidFloat,
}

impl std::fmt::Display for CloudAgentProtoDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

pub fn convert_generated_conversation_messages_to_no_preamble_trace(
    conversation: &[Vec<u8>],
) -> Result<Vec<Value>, String> {
    let mut output = Vec::new();
    for bytes in conversation {
        let message = decode_message(bytes, CONVERSATION_MESSAGE_TYPE)
            .map_err(|error| format!("decode generated ConversationMessage: {error}"))?;
        let object = message.as_object().ok_or_else(|| {
            "generated ConversationMessage decoder did not return an object".to_string()
        })?;
        let message_type = object.get("type").and_then(Value::as_i64).unwrap_or(0);
        let role = match message_type {
            1 => "user",
            2 => "assistant",
            _ => "unknown",
        };

        let mut trace = Map::new();
        trace.insert("role".into(), Value::String(role.into()));
        if let Some(text) = non_empty_string(object.get("text")) {
            trace.insert("text".into(), Value::String(text.to_string()));
        }
        if let Some(thinking) = object
            .get("thinking")
            .and_then(Value::as_object)
            .and_then(|thinking| non_empty_string(thinking.get("text")))
        {
            trace.insert("thinking".into(), Value::String(thinking.to_string()));
        }

        let tool_results = object
            .get("toolResults")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if message_type == 2 && !tool_results.is_empty() {
            let calls = tool_results
                .iter()
                .filter_map(Value::as_object)
                .map(project_tool_call)
                .collect::<Vec<_>>();
            if !calls.is_empty() {
                trace.insert("tool_calls".into(), Value::Array(calls));
            }
        }
        output.push(Value::Object(trace));

        if message_type == 2 {
            for tool_result in tool_results.iter().filter_map(Value::as_object) {
                output.push(Value::Object(project_tool_result(tool_result)));
            }
        }
    }
    Ok(output)
}

fn project_tool_call(tool_result: &Map<String, Value>) -> Value {
    let mut call = Map::new();
    apply_tool_identity_and_args(&mut call, tool_result);
    apply_tool_call_timestamps(&mut call, tool_result);
    Value::Object(call)
}

fn project_tool_result(tool_result: &Map<String, Value>) -> Map<String, Value> {
    let mut tool = Map::new();
    tool.insert("role".into(), Value::String("tool".into()));
    apply_tool_identity_and_args(&mut tool, tool_result);
    if let Some(result) = extract_tool_result_content(tool_result) {
        tool.insert("tool_result".into(), result);
    }
    apply_tool_call_timestamps(&mut tool, tool_result);
    tool
}

fn apply_tool_identity_and_args(
    target: &mut Map<String, Value>,
    tool_result: &Map<String, Value>,
) {
    if let Some(tool_call_id) = truthy_string(tool_result.get("toolCallId")) {
        target.insert(
            "tool_call_id".into(),
            Value::String(tool_call_id.to_string()),
        );
    }
    if let Some(tool_name) = truthy_string(tool_result.get("toolName")) {
        target.insert("tool_name".into(), Value::String(tool_name.to_string()));
    }
    let args = truthy_string(tool_result.get("rawArgs"))
        .or_else(|| truthy_string(tool_result.get("args")));
    if let Some(args) = args {
        target.insert("tool_args".into(), parse_json_or_string(args));
    }
}

fn extract_tool_result_content(tool_result: &Map<String, Value>) -> Option<Value> {
    if let Some(content) = truthy_string(tool_result.get("content")) {
        return Some(parse_json_or_string(content));
    }
    let result = tool_result.get("result")?.as_object()?;
    let (case, value) = result.iter().next()?;
    Some(json!({
        "resultType": case,
        "value": value,
    }))
}

fn apply_tool_call_timestamps(
    target: &mut Map<String, Value>,
    tool_result: &Map<String, Value>,
) {
    let started = integer_value(tool_result.get("startedAtMs"));
    let completed = integer_value(tool_result.get("completedAtMs"));
    if let Some(started) = started {
        target.insert("started_at_ms".into(), integer_json(started));
    }
    if let Some(completed) = completed {
        target.insert("completed_at_ms".into(), integer_json(completed));
    }
    if let (Some(started), Some(completed)) = (started, completed) {
        target.insert(
            "duration_ms".into(),
            integer_json(completed - started),
        );
    }
}

fn non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
}

fn truthy_string(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn parse_json_or_string(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

fn integer_value(value: Option<&Value>) -> Option<i128> {
    let number = value?.as_number()?;
    number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from))
}

fn integer_json(value: i128) -> Value {
    if let Ok(value) = i64::try_from(value) {
        Value::Number(Number::from(value))
    } else if let Ok(value) = u64::try_from(value) {
        Value::Number(Number::from(value))
    } else {
        Value::Number(
            Number::from_f64(value as f64)
                .expect("finite protobuf integer converts to finite f64"),
        )
    }
}

fn decode_message(
    bytes: &[u8],
    type_name: &str,
) -> Result<Value, CloudAgentProtoDecodeError> {
    let descriptor =
        cloud_agent_proto_message_descriptor(type_name).ok_or(CloudAgentProtoDecodeError::MissingDescriptor)?;
    let mut object = Map::new();
    initialize_generated_defaults(&mut object, descriptor);

    let mut position = 0usize;
    while position < bytes.len() {
        let tag = read_varint(bytes, &mut position)?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        let Some(field) = descriptor
            .fields
            .iter()
            .find(|field| field.number == field_number)
        else {
            skip_wire_value(bytes, &mut position, wire_type)?;
            continue;
        };

        if let Some(oneof) = field.oneof {
            for sibling in descriptor
                .fields
                .iter()
                .filter(|candidate| candidate.oneof == Some(oneof))
            {
                object.remove(sibling.json_name);
            }
        }

        match field.kind {
            CloudAgentProtoFieldKind::Map { key_scalar, value } => {
                let entry = read_length_delimited(bytes, &mut position, wire_type)?;
                let (key, value) = decode_map_entry(entry, key_scalar, value)?;
                let target = object
                    .entry(field.json_name.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                let map = target
                    .as_object_mut()
                    .ok_or(CloudAgentProtoDecodeError::WireTypeMismatch)?;
                map.insert(key, value);
            }
            CloudAgentProtoFieldKind::Scalar(scalar)
                if field.repeated && wire_type == 2 && scalar_is_packable(scalar) =>
            {
                let packed = read_length_delimited(bytes, &mut position, wire_type)?;
                let values = decode_packed_scalars(packed, scalar)?;
                append_repeated_values(&mut object, field.json_name, values)?;
            }
            CloudAgentProtoFieldKind::Enum if field.repeated && wire_type == 2 => {
                let packed = read_length_delimited(bytes, &mut position, wire_type)?;
                let values = decode_packed_enums(packed)?;
                append_repeated_values(&mut object, field.json_name, values)?;
            }
            _ => {
                let value = decode_field_value(bytes, &mut position, wire_type, field.kind)?;
                if field.repeated {
                    append_repeated_values(&mut object, field.json_name, vec![value])?;
                } else {
                    object.insert(field.json_name.to_string(), value);
                }
            }
        }
    }
    Ok(Value::Object(object))
}

fn initialize_generated_defaults(
    object: &mut Map<String, Value>,
    descriptor: CloudAgentProtoMessageDescriptor,
) {
    for field in descriptor.fields {
        if matches!(field.kind, CloudAgentProtoFieldKind::Map { .. }) {
            object.insert(field.json_name.into(), Value::Object(Map::new()));
        } else if field.repeated {
            object.insert(field.json_name.into(), Value::Array(Vec::new()));
        } else if field.oneof.is_some() || field.optional {
            continue;
        } else {
            match field.kind {
                CloudAgentProtoFieldKind::Scalar(scalar) => {
                    object.insert(field.json_name.into(), scalar_default(scalar));
                }
                CloudAgentProtoFieldKind::Enum => {
                    object.insert(field.json_name.into(), Value::Number(Number::from(0)));
                }
                CloudAgentProtoFieldKind::Message(_) | CloudAgentProtoFieldKind::Map { .. } => {}
            }
        }
    }
}

fn scalar_default(scalar: u64) -> Value {
    match scalar {
        8 => Value::Bool(false),
        9 => Value::String(String::new()),
        12 => Value::Array(Vec::new()),
        _ => Value::Number(Number::from(0)),
    }
}

fn append_repeated_values(
    object: &mut Map<String, Value>,
    json_name: &str,
    values: Vec<Value>,
) -> Result<(), CloudAgentProtoDecodeError> {
    let target = object
        .entry(json_name.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let array = target
        .as_array_mut()
        .ok_or(CloudAgentProtoDecodeError::WireTypeMismatch)?;
    array.extend(values);
    Ok(())
}

fn decode_field_value(
    bytes: &[u8],
    position: &mut usize,
    wire_type: u8,
    kind: CloudAgentProtoFieldKind,
) -> Result<Value, CloudAgentProtoDecodeError> {
    match kind {
        CloudAgentProtoFieldKind::Scalar(scalar) => {
            decode_scalar(bytes, position, wire_type, scalar)
        }
        CloudAgentProtoFieldKind::Enum => {
            if wire_type != 0 {
                return Err(CloudAgentProtoDecodeError::WireTypeMismatch);
            }
            Ok(Value::Number(Number::from(read_varint(bytes, position)?)))
        }
        CloudAgentProtoFieldKind::Message(child_type) => {
            let child = read_length_delimited(bytes, position, wire_type)?;
            decode_message(child, child_type)
        }
        CloudAgentProtoFieldKind::Map { .. } => {
            Err(CloudAgentProtoDecodeError::WireTypeMismatch)
        }
    }
}

fn decode_map_entry(
    bytes: &[u8],
    key_scalar: u64,
    value_kind: CloudAgentProtoMapValueKind,
) -> Result<(String, Value), CloudAgentProtoDecodeError> {
    let mut position = 0usize;
    let mut key = map_key_default(key_scalar);
    let mut value = map_value_default(value_kind);
    while position < bytes.len() {
        let tag = read_varint(bytes, &mut position)?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        match field_number {
            1 => {
                let decoded = decode_scalar(bytes, &mut position, wire_type, key_scalar)?;
                key = map_key_string(&decoded);
            }
            2 => {
                value = match value_kind {
                    CloudAgentProtoMapValueKind::Scalar(scalar) => {
                        decode_scalar(bytes, &mut position, wire_type, scalar)?
                    }
                    CloudAgentProtoMapValueKind::Enum => {
                        if wire_type != 0 {
                            return Err(CloudAgentProtoDecodeError::WireTypeMismatch);
                        }
                        Value::Number(Number::from(read_varint(bytes, &mut position)?))
                    }
                    CloudAgentProtoMapValueKind::Message(child_type) => {
                        let child = read_length_delimited(bytes, &mut position, wire_type)?;
                        decode_message(child, child_type)?
                    }
                };
            }
            _ => skip_wire_value(bytes, &mut position, wire_type)?,
        }
    }
    Ok((key, value))
}

fn map_key_default(scalar: u64) -> String {
    match scalar {
        8 => "false".into(),
        9 => String::new(),
        _ => "0".into(),
    }
}

fn map_value_default(kind: CloudAgentProtoMapValueKind) -> Value {
    match kind {
        CloudAgentProtoMapValueKind::Scalar(scalar) => scalar_default(scalar),
        CloudAgentProtoMapValueKind::Enum => Value::Number(Number::from(0)),
        CloudAgentProtoMapValueKind::Message(_) => Value::Object(Map::new()),
    }
}

fn map_key_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

fn decode_packed_scalars(
    bytes: &[u8],
    scalar: u64,
) -> Result<Vec<Value>, CloudAgentProtoDecodeError> {
    let wire_type = scalar_wire_type(scalar)?;
    let mut position = 0usize;
    let mut output = Vec::new();
    while position < bytes.len() {
        output.push(decode_scalar(bytes, &mut position, wire_type, scalar)?);
    }
    Ok(output)
}

fn decode_packed_enums(bytes: &[u8]) -> Result<Vec<Value>, CloudAgentProtoDecodeError> {
    let mut position = 0usize;
    let mut output = Vec::new();
    while position < bytes.len() {
        output.push(Value::Number(Number::from(read_varint(
            bytes,
            &mut position,
        )?)));
    }
    Ok(output)
}

fn scalar_is_packable(scalar: u64) -> bool {
    !matches!(scalar, 9 | 12)
}

fn scalar_wire_type(scalar: u64) -> Result<u8, CloudAgentProtoDecodeError> {
    match scalar {
        1 | 6 | 16 => Ok(1),
        2 | 7 | 15 => Ok(5),
        3 | 4 | 5 | 8 | 13 | 17 | 18 => Ok(0),
        9 | 12 => Ok(2),
        _ => Err(CloudAgentProtoDecodeError::UnsupportedScalar),
    }
}

fn decode_scalar(
    bytes: &[u8],
    position: &mut usize,
    wire_type: u8,
    scalar: u64,
) -> Result<Value, CloudAgentProtoDecodeError> {
    let expected = scalar_wire_type(scalar)?;
    if wire_type != expected {
        return Err(CloudAgentProtoDecodeError::WireTypeMismatch);
    }
    match scalar {
        1 => {
            let raw = read_fixed_64(bytes, position)?;
            number_from_f64(f64::from_bits(raw))
        }
        2 => {
            let raw = read_fixed_32(bytes, position)?;
            number_from_f64(f32::from_bits(raw) as f64)
        }
        3 => Ok(integer_json(read_varint(bytes, position)? as i64 as i128)),
        4 => Ok(Value::Number(Number::from(read_varint(bytes, position)?))),
        5 => {
            let raw = read_varint(bytes, position)? as u32;
            Ok(Value::Number(Number::from(raw as i32)))
        }
        6 => Ok(Value::Number(Number::from(read_fixed_64(bytes, position)?))),
        7 => Ok(Value::Number(Number::from(read_fixed_32(bytes, position)?))),
        8 => Ok(Value::Bool(read_varint(bytes, position)? != 0)),
        9 => {
            let value = read_length_delimited(bytes, position, wire_type)?;
            let value = std::str::from_utf8(value)
                .map_err(|_| CloudAgentProtoDecodeError::InvalidUtf8)?;
            Ok(Value::String(value.to_string()))
        }
        12 => {
            let value = read_length_delimited(bytes, position, wire_type)?;
            Ok(Value::Array(
                value
                    .iter()
                    .map(|byte| Value::Number(Number::from(*byte)))
                    .collect(),
            ))
        }
        13 => Ok(Value::Number(Number::from(
            read_varint(bytes, position)? as u32,
        ))),
        15 => {
            let raw = read_fixed_32(bytes, position)?;
            Ok(Value::Number(Number::from(i32::from_le_bytes(
                raw.to_le_bytes(),
            ))))
        }
        16 => {
            let raw = read_fixed_64(bytes, position)?;
            Ok(integer_json(i64::from_le_bytes(raw.to_le_bytes()) as i128))
        }
        17 => {
            let raw = read_varint(bytes, position)?;
            let value = ((raw >> 1) as i64) ^ (-((raw & 1) as i64));
            Ok(Value::Number(Number::from(value as i32)))
        }
        18 => {
            let raw = read_varint(bytes, position)?;
            let value = ((raw >> 1) as i64) ^ (-((raw & 1) as i64));
            Ok(Value::Number(Number::from(value)))
        }
        _ => Err(CloudAgentProtoDecodeError::UnsupportedScalar),
    }
}

fn number_from_f64(value: f64) -> Result<Value, CloudAgentProtoDecodeError> {
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or(CloudAgentProtoDecodeError::InvalidFloat)
}

fn read_length_delimited<'a>(
    bytes: &'a [u8],
    position: &mut usize,
    wire_type: u8,
) -> Result<&'a [u8], CloudAgentProtoDecodeError> {
    if wire_type != 2 {
        return Err(CloudAgentProtoDecodeError::WireTypeMismatch);
    }
    let length: usize = read_varint(bytes, position)?
        .try_into()
        .map_err(|_| CloudAgentProtoDecodeError::LengthOverflow)?;
    let end = position
        .checked_add(length)
        .ok_or(CloudAgentProtoDecodeError::LengthOverflow)?;
    let value = bytes
        .get(*position..end)
        .ok_or(CloudAgentProtoDecodeError::Truncated)?;
    *position = end;
    Ok(value)
}

fn read_fixed_32(
    bytes: &[u8],
    position: &mut usize,
) -> Result<u32, CloudAgentProtoDecodeError> {
    let end = position
        .checked_add(4)
        .ok_or(CloudAgentProtoDecodeError::LengthOverflow)?;
    let value = bytes
        .get(*position..end)
        .ok_or(CloudAgentProtoDecodeError::Truncated)?;
    *position = end;
    Ok(u32::from_le_bytes(
        value
            .try_into()
            .map_err(|_| CloudAgentProtoDecodeError::Truncated)?,
    ))
}

fn read_fixed_64(
    bytes: &[u8],
    position: &mut usize,
) -> Result<u64, CloudAgentProtoDecodeError> {
    let end = position
        .checked_add(8)
        .ok_or(CloudAgentProtoDecodeError::LengthOverflow)?;
    let value = bytes
        .get(*position..end)
        .ok_or(CloudAgentProtoDecodeError::Truncated)?;
    *position = end;
    Ok(u64::from_le_bytes(
        value
            .try_into()
            .map_err(|_| CloudAgentProtoDecodeError::Truncated)?,
    ))
}

fn skip_wire_value(
    bytes: &[u8],
    position: &mut usize,
    wire_type: u8,
) -> Result<(), CloudAgentProtoDecodeError> {
    match wire_type {
        0 => {
            let _ = read_varint(bytes, position)?;
            Ok(())
        }
        1 => {
            let _ = read_fixed_64(bytes, position)?;
            Ok(())
        }
        2 => {
            let _ = read_length_delimited(bytes, position, wire_type)?;
            Ok(())
        }
        5 => {
            let _ = read_fixed_32(bytes, position)?;
            Ok(())
        }
        _ => Err(CloudAgentProtoDecodeError::InvalidWireType),
    }
}

fn read_varint(
    bytes: &[u8],
    position: &mut usize,
) -> Result<u64, CloudAgentProtoDecodeError> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes
            .get(*position)
            .ok_or(CloudAgentProtoDecodeError::Truncated)?;
        *position += 1;
        if shift == 63 && byte > 1 {
            return Err(CloudAgentProtoDecodeError::VarintOverflow);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(CloudAgentProtoDecodeError::VarintOverflow)
}
