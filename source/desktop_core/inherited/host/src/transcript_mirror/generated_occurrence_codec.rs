use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{Map, Number, Value};

use super::transcript_occurrence_deriver::{
    DecodedTranscriptStep, DecodedTranscriptTurn, DecodedUserMessage,
    TranscriptOccurrenceCodec,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedToolProjection {
    pub name_override: Option<String>,
    pub input: Value,
    pub result: Option<Value>,
}

pub trait GeneratedToolJsonProjection: Send + Sync {
    fn project(
        &self,
        tool_field_number: u64,
        tool_message: &[u8],
    ) -> Result<Option<GeneratedToolProjection>, String>;
}


include!(concat!(env!("OUT_DIR"), "/agent_tool_json_schema.rs"));

#[derive(Debug, Clone, Copy, Default)]
pub struct CanonicalGeneratedToolJsonProjection;

impl GeneratedToolJsonProjection for CanonicalGeneratedToolJsonProjection {
    fn project(
        &self,
        tool_field_number: u64,
        tool_message: &[u8],
    ) -> Result<Option<GeneratedToolProjection>, String> {
        let Some(descriptor) = generated_agent_tool_call_descriptor(tool_field_number) else {
            return Ok(None);
        };
        let mut input = Value::Object(Map::new());
        let mut result = None;
        let mut position = 0usize;
        while position < tool_message.len() {
            let tag = read_varint(tool_message, &mut position)?;
            let field_number = tag >> 3;
            let wire_type = (tag & 0x07) as u8;
            match field_number {
                1 if descriptor.args_type.is_some() => {
                    let bytes = read_length_delimited(tool_message, &mut position, wire_type)?;
                    input = decode_generated_json_message(
                        bytes,
                        descriptor.args_type.expect("checked args type"),
                    )?;
                }
                2 if descriptor.result_type.is_some() => {
                    let bytes = read_length_delimited(tool_message, &mut position, wire_type)?;
                    result = Some(decode_generated_json_message(
                        bytes,
                        descriptor.result_type.expect("checked result type"),
                    )?);
                }
                _ => skip_wire_value(tool_message, &mut position, wire_type)?,
            }
        }
        let name_override = (tool_field_number == 15)
            .then(|| {
                input
                    .get("toolName")
                    .or_else(|| input.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .flatten();
        Ok(Some(GeneratedToolProjection {
            name_override,
            input,
            result,
        }))
    }
}


pub fn encode_generated_agent_tool_step(
    tool_name: &str,
    input: &Value,
    result: Option<&Value>,
    tool_call_id: &str,
    started_at_ms: Option<u64>,
    completed_at_ms: Option<u64>,
) -> Result<Vec<u8>, String> {
    let normalized = normalize_generated_tool_name(tool_name);
    let (field_number, descriptor) = (1u64..=77)
        .filter_map(|field_number| {
            generated_agent_tool_call_descriptor(field_number)
                .map(|descriptor| (field_number, descriptor))
        })
        .find(|(_, descriptor)| descriptor.static_name == normalized)
        .ok_or_else(|| format!("canonical generated tool descriptor is missing for {tool_name}"))?;

    let mut wrapper = Vec::new();
    if let Some(args_type) = descriptor.args_type {
        let args = encode_generated_json_message(input, args_type)?;
        write_length_delimited(&mut wrapper, 1, &args);
    }
    if let (Some(result), Some(result_type)) = (result, descriptor.result_type) {
        let result = encode_generated_json_message(result, result_type)?;
        write_length_delimited(&mut wrapper, 2, &result);
    }

    let mut tool_call = Vec::new();
    write_length_delimited(&mut tool_call, field_number, &wrapper);
    if !tool_call_id.trim().is_empty() {
        write_length_delimited(&mut tool_call, 57, tool_call_id.as_bytes());
    }
    if let Some(started_at_ms) = started_at_ms {
        write_key(&mut tool_call, 59, 0);
        write_varint(&mut tool_call, started_at_ms);
    }
    if let Some(completed_at_ms) = completed_at_ms {
        write_key(&mut tool_call, 60, 0);
        write_varint(&mut tool_call, completed_at_ms);
    }

    let mut step = Vec::new();
    write_length_delimited(&mut step, 2, &tool_call);
    Ok(step)
}

fn normalize_generated_tool_name(name: &str) -> String {
    let trimmed = name.trim();
    match trimmed {
        "Shell" | "ExternalShell" => return "shell".into(),
        "Read" | "ExternalRead" => return "read".into(),
        "Task" => return "task".into(),
        "Computer" => return "computer_use".into(),
        "GenerateImage" => return "generate_image".into(),
        "RecordScreen" => return "record_screen".into(),
        "ListMcpResources" => return "list_mcp_resources".into(),
        "ReadMcpResource" => return "read_mcp_resource".into(),
        "AskQuestion" => return "ask_question".into(),
        "McpAuth" => return "mcp_auth".into(),
        "WebSearch" => return "web_search".into(),
        "WebFetch" => return "web_fetch".into(),
        "GetMcpTools" => return "get_mcp_tools".into(),
        "SendMessage" => return "send_message".into(),
        _ => {}
    }
    let base = trimmed
        .strip_suffix("ToolCall")
        .or_else(|| trimmed.strip_suffix("_tool_call"))
        .unwrap_or(trimmed);
    let mut output = String::new();
    for (index, ch) in base.chars().enumerate() {
        if ch == '-' || ch == ' ' {
            if !output.ends_with('_') {
                output.push('_');
            }
            continue;
        }
        if ch.is_ascii_uppercase() {
            if index > 0 && !output.ends_with('_') {
                output.push('_');
            }
            output.push(ch.to_ascii_lowercase());
        } else {
            output.push(ch);
        }
    }
    output
}

fn encode_generated_json_message(value: &Value, type_name: &str) -> Result<Vec<u8>, String> {
    let owned;
    let value = match type_name {
        "google.protobuf.Value" => {
            owned = generated_value_message(value);
            &owned
        }
        "google.protobuf.Struct" => {
            let object = value
                .as_object()
                .ok_or_else(|| "generated google.protobuf.Struct JSON must be an object".to_string())?;
            owned = Value::Object(Map::from_iter([(
                "fields".to_string(),
                Value::Object(
                    object
                        .iter()
                        .map(|(key, value)| (key.clone(), generated_value_message(value)))
                        .collect(),
                ),
            )]));
            &owned
        }
        "google.protobuf.ListValue" => {
            let values = value
                .as_array()
                .ok_or_else(|| "generated google.protobuf.ListValue JSON must be an array".to_string())?;
            owned = Value::Object(Map::from_iter([(
                "values".to_string(),
                Value::Array(values.iter().map(generated_value_message).collect()),
            )]));
            &owned
        }
        "google.protobuf.Timestamp" => {
            let timestamp = value
                .as_str()
                .ok_or_else(|| "generated google.protobuf.Timestamp JSON must be an RFC3339 string".to_string())?;
            let parsed = chrono::DateTime::parse_from_rfc3339(timestamp)
                .map_err(|error| format!("invalid generated Timestamp JSON: {error}"))?
                .with_timezone(&chrono::Utc);
            owned = serde_json::json!({
                "seconds": parsed.timestamp().to_string(),
                "nanos": parsed.timestamp_subsec_nanos(),
            });
            &owned
        }
        _ => value,
    };

    let descriptor = generated_agent_tool_json_message_descriptor(type_name)
        .ok_or_else(|| format!("canonical generated JSON descriptor is missing for {type_name}"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("generated protobuf JSON for {type_name} must be an object"))?;
    let mut output = Vec::new();
    for field in descriptor.fields {
        let Some(value) = object.get(field.json_name) else {
            continue;
        };
        if value.is_null() && !matches!(field.kind, AgentToolJsonFieldKind::Message("google.protobuf.Value")) {
            continue;
        }
        if field.repeated && !matches!(field.kind, AgentToolJsonFieldKind::Map { .. }) {
            let values = value
                .as_array()
                .ok_or_else(|| format!("generated repeated field {} must be an array", field.json_name))?;
            for value in values {
                encode_generated_field(&mut output, *field, value)?;
            }
        } else {
            encode_generated_field(&mut output, *field, value)?;
        }
    }
    Ok(output)
}

fn generated_value_message(value: &Value) -> Value {
    match value {
        Value::Null => serde_json::json!({"nullValue": "NULL_VALUE"}),
        Value::Bool(value) => serde_json::json!({"boolValue": value}),
        Value::Number(value) => serde_json::json!({"numberValue": value}),
        Value::String(value) => serde_json::json!({"stringValue": value}),
        Value::Array(values) => serde_json::json!({
            "listValue": values,
        }),
        Value::Object(values) => serde_json::json!({
            "structValue": values,
        }),
    }
}

fn encode_generated_field(
    output: &mut Vec<u8>,
    field: AgentToolJsonFieldDescriptor,
    value: &Value,
) -> Result<(), String> {
    match field.kind {
        AgentToolJsonFieldKind::Scalar(scalar) => {
            encode_generated_scalar_field(output, field.number, scalar, value)
        }
        AgentToolJsonFieldKind::Enum(values) => {
            write_key(output, field.number, 0);
            write_varint(output, generated_enum_number(value, values)? as u64);
            Ok(())
        }
        AgentToolJsonFieldKind::Message(type_name) => {
            let bytes = encode_generated_json_message(value, type_name)?;
            write_length_delimited(output, field.number, &bytes);
            Ok(())
        }
        AgentToolJsonFieldKind::Map { key_scalar, value: value_kind } => {
            let object = value
                .as_object()
                .ok_or_else(|| format!("generated map field {} must be an object", field.json_name))?;
            for (key, value) in object {
                let mut entry = Vec::new();
                encode_generated_map_key(&mut entry, key_scalar, key)?;
                encode_generated_map_value(&mut entry, value_kind, value)?;
                write_length_delimited(output, field.number, &entry);
            }
            Ok(())
        }
    }
}

fn encode_generated_map_key(
    output: &mut Vec<u8>,
    scalar: u64,
    key: &str,
) -> Result<(), String> {
    let value = match scalar {
        8 => Value::Bool(match key {
            "true" => true,
            "false" => false,
            _ => return Err(format!("invalid generated bool map key {key}")),
        }),
        9 => Value::String(key.to_string()),
        _ => Value::String(key.to_string()),
    };
    encode_generated_scalar_field(output, 1, scalar, &value)
}

fn encode_generated_map_value(
    output: &mut Vec<u8>,
    kind: AgentToolJsonMapValueKind,
    value: &Value,
) -> Result<(), String> {
    match kind {
        AgentToolJsonMapValueKind::Scalar(scalar) => {
            encode_generated_scalar_field(output, 2, scalar, value)
        }
        AgentToolJsonMapValueKind::Enum(values) => {
            write_key(output, 2, 0);
            write_varint(output, generated_enum_number(value, values)? as u64);
            Ok(())
        }
        AgentToolJsonMapValueKind::Message(type_name) => {
            let bytes = encode_generated_json_message(value, type_name)?;
            write_length_delimited(output, 2, &bytes);
            Ok(())
        }
    }
}

fn generated_enum_number(
    value: &Value,
    values: &'static [(i64, &'static str)],
) -> Result<i64, String> {
    if let Some(value) = value.as_i64() {
        return Ok(value);
    }
    if let Some(value) = value.as_u64() {
        return i64::try_from(value).map_err(|_| "generated enum value exceeds i64".to_string());
    }
    let name = value
        .as_str()
        .ok_or_else(|| "generated enum JSON must be a name or integer".to_string())?;
    values
        .iter()
        .find(|(_, candidate)| *candidate == name)
        .map(|(number, _)| *number)
        .ok_or_else(|| format!("unknown generated enum value {name}"))
}

fn encode_generated_scalar_field(
    output: &mut Vec<u8>,
    field_number: u64,
    scalar: u64,
    value: &Value,
) -> Result<(), String> {
    let wire = scalar_wire_type(scalar)?;
    write_key(output, field_number, wire);
    match scalar {
        1 => output.extend_from_slice(&generated_f64(value)?.to_bits().to_le_bytes()),
        2 => output.extend_from_slice(&(generated_f64(value)? as f32).to_bits().to_le_bytes()),
        3 => write_varint(output, generated_i64(value)? as u64),
        4 => write_varint(output, generated_u64(value)?),
        5 => write_varint(output, generated_i64(value)? as i32 as u32 as u64),
        6 => output.extend_from_slice(&generated_u64(value)?.to_le_bytes()),
        7 => output.extend_from_slice(&(generated_u64(value)? as u32).to_le_bytes()),
        8 => write_varint(
            output,
            u64::from(value.as_bool().ok_or_else(|| "generated bool JSON must be boolean".to_string())?),
        ),
        9 => {
            let text = value
                .as_str()
                .ok_or_else(|| "generated string JSON must be a string".to_string())?;
            write_varint(output, text.len() as u64);
            output.extend_from_slice(text.as_bytes());
        }
        12 => {
            let encoded = value
                .as_str()
                .ok_or_else(|| "generated bytes JSON must be base64 text".to_string())?;
            let bytes = STANDARD
                .decode(encoded)
                .map_err(|error| format!("invalid generated base64 bytes JSON: {error}"))?;
            write_varint(output, bytes.len() as u64);
            output.extend_from_slice(&bytes);
        }
        13 => write_varint(output, generated_u64(value)? as u32 as u64),
        15 => output.extend_from_slice(&(generated_i64(value)? as i32).to_le_bytes()),
        16 => output.extend_from_slice(&generated_i64(value)?.to_le_bytes()),
        17 => {
            let value = generated_i64(value)? as i32;
            write_varint(output, ((value << 1) ^ (value >> 31)) as u32 as u64);
        }
        18 => {
            let value = generated_i64(value)?;
            write_varint(output, ((value << 1) ^ (value >> 63)) as u64);
        }
        _ => return Err(format!("unsupported generated protobuf scalar {scalar}")),
    }
    Ok(())
}

fn generated_i64(value: &Value) -> Result<i64, String> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
        .ok_or_else(|| "generated integer JSON is not an i64".to_string())
}

fn generated_u64(value: &Value) -> Result<u64, String> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str().and_then(|value| value.parse::<u64>().ok()))
        .ok_or_else(|| "generated integer JSON is not a u64".to_string())
}

fn generated_f64(value: &Value) -> Result<f64, String> {
    if let Some(value) = value.as_f64() {
        return Ok(value);
    }
    match value.as_str() {
        Some("NaN") => Ok(f64::NAN),
        Some("Infinity") => Ok(f64::INFINITY),
        Some("-Infinity") => Ok(f64::NEG_INFINITY),
        Some(value) => value
            .parse::<f64>()
            .map_err(|error| format!("invalid generated float JSON: {error}")),
        None => Err("generated float JSON is not numeric".into()),
    }
}

fn write_key(output: &mut Vec<u8>, field_number: u64, wire_type: u8) {
    write_varint(output, (field_number << 3) | u64::from(wire_type));
}

fn write_length_delimited(output: &mut Vec<u8>, field_number: u64, bytes: &[u8]) {
    write_key(output, field_number, 2);
    write_varint(output, bytes.len() as u64);
    output.extend_from_slice(bytes);
}

fn write_varint(output: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn decode_generated_json_message(bytes: &[u8], type_name: &str) -> Result<Value, String> {
    let descriptor = generated_agent_tool_json_message_descriptor(type_name)
        .ok_or_else(|| format!("canonical generated JSON descriptor is missing for {type_name}"))?;
    let mut object = Map::new();
    let mut position = 0usize;
    while position < bytes.len() {
        let tag = read_varint(bytes, &mut position)?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        let Some(field) = descriptor
            .fields
            .iter()
            .find(|field| field.number == field_number)
            .copied()
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
            AgentToolJsonFieldKind::Map { key_scalar, value } => {
                let entry = read_length_delimited(bytes, &mut position, wire_type)?;
                let (key, value) = decode_generated_map_entry(entry, key_scalar, value)?;
                let target = object
                    .entry(field.json_name.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                target
                    .as_object_mut()
                    .ok_or_else(|| "generated protobuf map projection collided with non-map field".to_string())?
                    .insert(key, value);
            }
            AgentToolJsonFieldKind::Scalar(scalar)
                if field.repeated && wire_type == 2 && scalar_is_packable(scalar) =>
            {
                let packed = read_length_delimited(bytes, &mut position, wire_type)?;
                let mut packed_position = 0usize;
                while packed_position < packed.len() {
                    let value = decode_generated_scalar(
                        packed,
                        &mut packed_position,
                        scalar_wire_type(scalar)?,
                        scalar,
                    )?;
                    append_generated_repeated(&mut object, field.json_name, value)?;
                }
            }
            AgentToolJsonFieldKind::Enum(values) if field.repeated && wire_type == 2 => {
                let packed = read_length_delimited(bytes, &mut position, wire_type)?;
                let mut packed_position = 0usize;
                while packed_position < packed.len() {
                    let number = read_varint(packed, &mut packed_position)? as i64;
                    append_generated_repeated(
                        &mut object,
                        field.json_name,
                        generated_enum_json(number, values),
                    )?;
                }
            }
            _ => {
                let value = decode_generated_field_value(bytes, &mut position, wire_type, field.kind)?;
                if field.repeated {
                    append_generated_repeated(&mut object, field.json_name, value)?;
                } else {
                    object.insert(field.json_name.to_string(), value);
                }
            }
        }
    }
    project_well_known_json(type_name, Value::Object(object))
}

fn project_well_known_json(type_name: &str, value: Value) -> Result<Value, String> {
    match type_name {
        "google.protobuf.Struct" => Ok(value
            .get("fields")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()))),
        "google.protobuf.ListValue" => Ok(value
            .get("values")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()))),
        "google.protobuf.Value" => {
            let object = value
                .as_object()
                .ok_or_else(|| "generated google.protobuf.Value was not an object".to_string())?;
            if object.contains_key("nullValue") {
                Ok(Value::Null)
            } else if let Some(value) = object.get("numberValue") {
                Ok(value.clone())
            } else if let Some(value) = object.get("stringValue") {
                Ok(value.clone())
            } else if let Some(value) = object.get("boolValue") {
                Ok(value.clone())
            } else if let Some(value) = object.get("structValue") {
                Ok(value.clone())
            } else if let Some(value) = object.get("listValue") {
                Ok(value.clone())
            } else {
                Ok(Value::Null)
            }
        }
        "google.protobuf.Timestamp" => {
            let seconds = value
                .get("seconds")
                .and_then(Value::as_str)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or_default();
            let nanos = value
                .get("nanos")
                .and_then(Value::as_i64)
                .unwrap_or_default()
                .clamp(0, 999_999_999) as u32;
            let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos)
                .ok_or_else(|| "generated Timestamp is outside the supported UTC range".to_string())?;
            let formatted = if nanos == 0 {
                timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string()
            } else {
                let digits = if nanos % 1_000_000 == 0 {
                    3
                } else if nanos % 1_000 == 0 {
                    6
                } else {
                    9
                };
                let fraction = &format!("{nanos:09}")[..digits];
                format!("{}.{fraction}Z", timestamp.format("%Y-%m-%dT%H:%M:%S"))
            };
            Ok(Value::String(formatted))
        }
        _ => Ok(value),
    }
}

fn append_generated_repeated(
    object: &mut Map<String, Value>,
    json_name: &str,
    value: Value,
) -> Result<(), String> {
    let target = object
        .entry(json_name.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    target
        .as_array_mut()
        .ok_or_else(|| "generated repeated projection collided with non-array field".to_string())?
        .push(value);
    Ok(())
}

fn decode_generated_field_value(
    bytes: &[u8],
    position: &mut usize,
    wire_type: u8,
    kind: AgentToolJsonFieldKind,
) -> Result<Value, String> {
    match kind {
        AgentToolJsonFieldKind::Scalar(scalar) => {
            decode_generated_scalar(bytes, position, wire_type, scalar)
        }
        AgentToolJsonFieldKind::Enum(values) => {
            if wire_type != 0 {
                return Err("generated enum wire type mismatch".into());
            }
            Ok(generated_enum_json(read_varint(bytes, position)? as i64, values))
        }
        AgentToolJsonFieldKind::Message(type_name) => {
            let child = read_length_delimited(bytes, position, wire_type)?;
            decode_generated_json_message(child, type_name)
        }
        AgentToolJsonFieldKind::Map { .. } => {
            Err("generated map field reached scalar decoder".into())
        }
    }
}

fn decode_generated_map_entry(
    bytes: &[u8],
    key_scalar: u64,
    value_kind: AgentToolJsonMapValueKind,
) -> Result<(String, Value), String> {
    let mut position = 0usize;
    let mut key = generated_map_key_default(key_scalar);
    let mut value = generated_map_value_default(value_kind);
    while position < bytes.len() {
        let tag = read_varint(bytes, &mut position)?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        match field_number {
            1 => {
                let decoded = decode_generated_scalar(bytes, &mut position, wire_type, key_scalar)?;
                key = generated_map_key_string(&decoded);
            }
            2 => {
                value = match value_kind {
                    AgentToolJsonMapValueKind::Scalar(scalar) => {
                        decode_generated_scalar(bytes, &mut position, wire_type, scalar)?
                    }
                    AgentToolJsonMapValueKind::Enum(values) => {
                        if wire_type != 0 {
                            return Err("generated map enum wire type mismatch".into());
                        }
                        generated_enum_json(read_varint(bytes, &mut position)? as i64, values)
                    }
                    AgentToolJsonMapValueKind::Message(type_name) => {
                        let child = read_length_delimited(bytes, &mut position, wire_type)?;
                        decode_generated_json_message(child, type_name)?
                    }
                };
            }
            _ => skip_wire_value(bytes, &mut position, wire_type)?,
        }
    }
    Ok((key, value))
}

fn generated_map_key_default(scalar: u64) -> String {
    match scalar {
        8 => "false".into(),
        9 => String::new(),
        _ => "0".into(),
    }
}

fn generated_map_value_default(kind: AgentToolJsonMapValueKind) -> Value {
    match kind {
        AgentToolJsonMapValueKind::Scalar(scalar) => generated_scalar_default(scalar),
        AgentToolJsonMapValueKind::Enum(values) => generated_enum_json(0, values),
        AgentToolJsonMapValueKind::Message(_) => Value::Object(Map::new()),
    }
}

fn generated_map_key_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

fn generated_enum_json(number: i64, values: &'static [(i64, &'static str)]) -> Value {
    values
        .iter()
        .find(|(candidate, _)| *candidate == number)
        .map(|(_, name)| Value::String((*name).to_string()))
        .unwrap_or_else(|| Value::Number(Number::from(number)))
}

fn generated_scalar_default(scalar: u64) -> Value {
    match scalar {
        8 => Value::Bool(false),
        3 | 4 | 6 | 16 | 18 => Value::String("0".into()),
        9 | 12 => Value::String(String::new()),
        _ => Value::Number(Number::from(0)),
    }
}

fn scalar_is_packable(scalar: u64) -> bool {
    !matches!(scalar, 9 | 12)
}

fn scalar_wire_type(scalar: u64) -> Result<u8, String> {
    match scalar {
        1 | 6 | 16 => Ok(1),
        2 | 7 | 15 => Ok(5),
        3 | 4 | 5 | 8 | 13 | 17 | 18 => Ok(0),
        9 | 12 => Ok(2),
        _ => Err(format!("unsupported generated protobuf scalar {scalar}")),
    }
}

fn decode_generated_scalar(
    bytes: &[u8],
    position: &mut usize,
    wire_type: u8,
    scalar: u64,
) -> Result<Value, String> {
    let expected = scalar_wire_type(scalar)?;
    if wire_type != expected {
        return Err(format!(
            "generated protobuf scalar wire mismatch: scalar={scalar} expected={expected} actual={wire_type}"
        ));
    }
    match scalar {
        1 => number_from_f64(f64::from_bits(read_fixed_64(bytes, position)?)),
        2 => number_from_f64(f32::from_bits(read_fixed_32(bytes, position)?) as f64),
        3 => Ok(Value::String((read_varint(bytes, position)? as i64).to_string())),
        4 => Ok(Value::String(read_varint(bytes, position)?.to_string())),
        5 => Ok(Value::Number(Number::from(read_varint(bytes, position)? as u32 as i32))),
        6 => Ok(Value::String(read_fixed_64(bytes, position)?.to_string())),
        7 => Ok(Value::Number(Number::from(read_fixed_32(bytes, position)?))),
        8 => Ok(Value::Bool(read_varint(bytes, position)? != 0)),
        9 => {
            let value = read_length_delimited(bytes, position, wire_type)?;
            let text = std::str::from_utf8(value)
                .map_err(|_| "generated protobuf string is invalid UTF-8".to_string())?;
            Ok(Value::String(text.to_string()))
        }
        12 => {
            let value = read_length_delimited(bytes, position, wire_type)?;
            Ok(Value::String(STANDARD.encode(value)))
        }
        13 => Ok(Value::Number(Number::from(read_varint(bytes, position)? as u32))),
        15 => Ok(Value::Number(Number::from(i32::from_le_bytes(
            read_fixed_32(bytes, position)?.to_le_bytes(),
        )))),
        16 => Ok(Value::String(i64::from_le_bytes(
            read_fixed_64(bytes, position)?.to_le_bytes(),
        ).to_string())),
        17 => {
            let raw = read_varint(bytes, position)?;
            let value = ((raw >> 1) as i64) ^ (-((raw & 1) as i64));
            Ok(Value::Number(Number::from(value as i32)))
        }
        18 => {
            let raw = read_varint(bytes, position)?;
            let value = ((raw >> 1) as i64) ^ (-((raw & 1) as i64));
            Ok(Value::String(value.to_string()))
        }
        _ => Err(format!("unsupported generated protobuf scalar {scalar}")),
    }
}

fn number_from_f64(value: f64) -> Result<Value, String> {
    if value.is_nan() {
        return Ok(Value::String("NaN".into()));
    }
    if value == f64::INFINITY {
        return Ok(Value::String("Infinity".into()));
    }
    if value == f64::NEG_INFINITY {
        return Ok(Value::String("-Infinity".into()));
    }
    Number::from_f64(value)
        .map(Value::Number)
        .ok_or_else(|| "generated protobuf float could not be projected to JSON".to_string())
}

fn read_length_delimited<'a>(
    bytes: &'a [u8],
    position: &mut usize,
    wire_type: u8,
) -> Result<&'a [u8], String> {
    if wire_type != 2 {
        return Err("generated protobuf length-delimited wire type mismatch".into());
    }
    read_bytes(bytes, position)
}

fn read_fixed_32(bytes: &[u8], position: &mut usize) -> Result<u32, String> {
    let end = position
        .checked_add(4)
        .ok_or_else(|| "generated protobuf fixed32 overflow".to_string())?;
    let value = bytes
        .get(*position..end)
        .ok_or_else(|| "truncated generated protobuf fixed32".to_string())?;
    *position = end;
    Ok(u32::from_le_bytes(
        value.try_into().map_err(|_| "truncated generated protobuf fixed32".to_string())?,
    ))
}

fn read_fixed_64(bytes: &[u8], position: &mut usize) -> Result<u64, String> {
    let end = position
        .checked_add(8)
        .ok_or_else(|| "generated protobuf fixed64 overflow".to_string())?;
    let value = bytes
        .get(*position..end)
        .ok_or_else(|| "truncated generated protobuf fixed64".to_string())?;
    *position = end;
    Ok(u64::from_le_bytes(
        value.try_into().map_err(|_| "truncated generated protobuf fixed64".to_string())?,
    ))
}

fn skip_wire_value(data: &[u8], position: &mut usize, wire_type: u8) -> Result<(), String> {
    match wire_type {
        0 => {
            let _ = read_varint(data, position)?;
        }
        1 => {
            let _ = read_fixed_64(data, position)?;
        }
        2 => {
            let _ = read_bytes(data, position)?;
        }
        5 => {
            let _ = read_fixed_32(data, position)?;
        }
        _ => return Err(format!("unsupported generated protobuf wire type {wire_type}")),
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RejectGeneratedToolJsonProjection;

impl GeneratedToolJsonProjection for RejectGeneratedToolJsonProjection {
    fn project(
        &self,
        _tool_field_number: u64,
        _tool_message: &[u8],
    ) -> Result<Option<GeneratedToolProjection>, String> {
        Ok(None)
    }
}

pub struct GeneratedTranscriptOccurrenceCodec<Projection> {
    tool_json: Projection,
}

impl<Projection> GeneratedTranscriptOccurrenceCodec<Projection> {
    pub fn new(tool_json: Projection) -> Self {
        Self { tool_json }
    }

    pub fn tool_json_projection(&self) -> &Projection {
        &self.tool_json
    }
}

impl<Projection> TranscriptOccurrenceCodec
    for GeneratedTranscriptOccurrenceCodec<Projection>
where
    Projection: GeneratedToolJsonProjection,
{
    fn decode_turn(&self, bytes: &[u8]) -> Result<DecodedTranscriptTurn, String> {
        let mut selected = DecodedTranscriptTurn::Undefined;
        for field in length_delimited_fields(bytes)? {
            match field.number {
                1 => {
                    let mut user_message = Vec::new();
                    let mut steps = Vec::new();
                    for nested in length_delimited_fields(field.bytes)? {
                        match nested.number {
                            1 => user_message = nested.bytes.to_vec(),
                            2 => steps.push(nested.bytes.to_vec()),
                            _ => {}
                        }
                    }
                    selected = DecodedTranscriptTurn::Agent {
                        user_message,
                        steps,
                    };
                }
                2 => selected = DecodedTranscriptTurn::Shell,
                _ => {}
            }
        }
        Ok(selected)
    }

    fn decode_user_message(&self, bytes: &[u8]) -> Result<DecodedUserMessage, String> {
        let mut text = String::new();
        let mut text_blob_id = None;
        for field in length_delimited_fields(bytes)? {
            match field.number {
                1 => text = String::from_utf8_lossy(field.bytes).into_owned(),
                18 => text_blob_id = Some(field.bytes.to_vec()),
                _ => {}
            }
        }
        Ok(DecodedUserMessage { text, text_blob_id })
    }

    fn decode_step(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, String> {
        let mut selected = DecodedTranscriptStep::Undefined;
        for field in length_delimited_fields(bytes)? {
            selected = match field.number {
                1 => DecodedTranscriptStep::Assistant {
                    text: decode_text_message(field.bytes)?,
                },
                2 => self.decode_tool_call(field.bytes)?,
                3 => DecodedTranscriptStep::Thinking {
                    text: decode_text_message(field.bytes)?,
                },
                _ => selected,
            };
        }
        Ok(selected)
    }
}

impl<Projection> GeneratedTranscriptOccurrenceCodec<Projection>
where
    Projection: GeneratedToolJsonProjection,
{
    fn decode_tool_call(&self, bytes: &[u8]) -> Result<DecodedTranscriptStep, String> {
        let mut selected: Option<(u64, &[u8])> = None;
        for field in length_delimited_fields(bytes)? {
            if generated_tool_name(field.number).is_some() {
                selected = Some((field.number, field.bytes));
            }
        }
        let Some((field_number, tool_message)) = selected else {
            return Ok(DecodedTranscriptStep::Undefined);
        };
        let static_name = generated_tool_name(field_number)
            .ok_or_else(|| format!("unsupported generated tool field {field_number}"))?;
        let projection = self
            .tool_json
            .project(field_number, tool_message)?
            .ok_or_else(|| {
                format!(
                    "canonical generated tool JSON projection is required for {static_name}"
                )
            })?;
        let name = if field_number == 15 {
            projection
                .name_override
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "mcp".to_string())
        } else {
            static_name.to_string()
        };
        Ok(DecodedTranscriptStep::Tool {
            name,
            input: projection.input,
            result: projection.result,
        })
    }
}

fn decode_text_message(bytes: &[u8]) -> Result<String, String> {
    let mut text = String::new();
    for field in length_delimited_fields(bytes)? {
        if field.number == 1 {
            text = String::from_utf8_lossy(field.bytes).into_owned();
        }
    }
    Ok(text)
}

#[derive(Clone, Copy)]
struct LengthDelimitedField<'a> {
    number: u64,
    bytes: &'a [u8],
}

fn length_delimited_fields(data: &[u8]) -> Result<Vec<LengthDelimitedField<'_>>, String> {
    let mut position = 0usize;
    let mut fields = Vec::new();
    while position < data.len() {
        let tag = read_varint(data, &mut position)?;
        let field_number = tag >> 3;
        let wire_type = (tag & 0x07) as u8;
        if field_number == 0 {
            return Err("protobuf field number 0 is invalid".into());
        }
        if wire_type == 2 {
            let bytes = read_bytes(data, &mut position)?;
            fields.push(LengthDelimitedField {
                number: field_number,
                bytes,
            });
        } else {
            skip_field(data, &mut position, wire_type, field_number)?;
        }
    }
    Ok(fields)
}

fn read_varint(data: &[u8], position: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *data
            .get(*position)
            .ok_or_else(|| "truncated protobuf varint".to_string())?;
        *position += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("protobuf varint exceeds 64 bits".into())
}

fn read_bytes<'a>(data: &'a [u8], position: &mut usize) -> Result<&'a [u8], String> {
    let length: usize = read_varint(data, position)?
        .try_into()
        .map_err(|_| "protobuf length does not fit usize".to_string())?;
    let end = position
        .checked_add(length)
        .ok_or_else(|| "protobuf length overflow".to_string())?;
    let value = data
        .get(*position..end)
        .ok_or_else(|| "truncated protobuf length-delimited field".to_string())?;
    *position = end;
    Ok(value)
}

fn skip_field(
    data: &[u8],
    position: &mut usize,
    wire_type: u8,
    field_number: u64,
) -> Result<(), String> {
    match wire_type {
        0 => {
            let _ = read_varint(data, position)?;
        }
        1 => {
            *position = position
                .checked_add(8)
                .ok_or_else(|| "protobuf fixed64 overflow".to_string())?;
            if *position > data.len() {
                return Err("truncated protobuf fixed64".into());
            }
        }
        2 => {
            let _ = read_bytes(data, position)?;
        }
        3 => loop {
            let tag = read_varint(data, position)?;
            let nested_field = tag >> 3;
            let nested_wire = (tag & 0x07) as u8;
            if nested_wire == 4 {
                if nested_field != field_number {
                    return Err("protobuf group end field mismatch".into());
                }
                break;
            }
            skip_field(data, position, nested_wire, nested_field)?;
        },
        4 => return Err("unexpected protobuf end-group field".into()),
        5 => {
            *position = position
                .checked_add(4)
                .ok_or_else(|| "protobuf fixed32 overflow".to_string())?;
            if *position > data.len() {
                return Err("truncated protobuf fixed32".into());
            }
        }
        _ => return Err(format!("unsupported protobuf wire type {wire_type}")),
    }
    Ok(())
}

pub fn generated_tool_name(field_number: u64) -> Option<&'static str> {
    generated_agent_tool_call_descriptor(field_number).map(|descriptor| descriptor.static_name)
}
