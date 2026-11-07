use std::sync::Arc;

use crate::cursor_backend::{resolve_sand_ghost_mode_header, send_cursor_unary};
use crate::extensions::auth::extension::HostAuthExtension;

pub const AI_AVAILABLE_MODELS_PATH: &str = "/aiserver.v1.AiService/AvailableModels";
pub const AVAILABLE_MODELS_SCOPE_USER_AVAILABLE: u64 = 1;
pub const MODEL_CATALOG_FETCH_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandModelCatalogValue {
    pub value: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandModelCatalogParameterType {
    Boolean,
    Enum,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandModelCatalogParameter {
    pub id: String,
    pub name: Option<String>,
    pub parameter_type: SandModelCatalogParameterType,
    pub values: Vec<SandModelCatalogValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandModelCatalogParameterValue {
    pub id: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandModelCatalogEntry {
    pub id: String,
    pub display_name: Option<String>,
    pub aliases: Vec<String>,
    pub params: Vec<SandModelCatalogParameter>,
    pub variants: Vec<Vec<SandModelCatalogParameterValue>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelCatalogFetchError {
    #[error("Cloud Agent model catalog authentication failed: {0}")]
    Auth(String),
    #[error("Cloud Agent model catalog backend failed: {0}")]
    Backend(String),
    #[error("Cloud Agent model catalog protobuf was invalid: {0}")]
    InvalidProto(String),
}

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

fn push_varint_field(output: &mut Vec<u8>, field: u64, value: u64) {
    push_varint(output, field << 3);
    push_varint(output, value);
}

pub fn encode_available_models_request() -> Vec<u8> {
    let mut output = Vec::with_capacity(6);
    push_varint_field(&mut output, 5, 1);
    push_varint_field(&mut output, 7, 1);
    push_varint_field(&mut output, 10, AVAILABLE_MODELS_SCOPE_USER_AVAILABLE);
    output
}

fn read_varint(input: &[u8], cursor: &mut usize) -> Result<u64, ModelCatalogFetchError> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    while *cursor < input.len() && shift < 64 {
        let byte = input[*cursor];
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(ModelCatalogFetchError::InvalidProto("malformed varint".into()))
}

fn read_bytes<'a>(
    input: &'a [u8],
    cursor: &mut usize,
) -> Result<&'a [u8], ModelCatalogFetchError> {
    let length = usize::try_from(read_varint(input, cursor)?)
        .map_err(|_| ModelCatalogFetchError::InvalidProto("length overflow".into()))?;
    let end = cursor.saturating_add(length);
    if end > input.len() {
        return Err(ModelCatalogFetchError::InvalidProto(
            "truncated length-delimited field".into(),
        ));
    }
    let value = &input[*cursor..end];
    *cursor = end;
    Ok(value)
}

fn read_string(
    input: &[u8],
    cursor: &mut usize,
) -> Result<String, ModelCatalogFetchError> {
    let bytes = read_bytes(input, cursor)?;
    String::from_utf8(bytes.to_vec())
        .map_err(|_| ModelCatalogFetchError::InvalidProto("string field is not UTF-8".into()))
}

fn skip_field(
    input: &[u8],
    cursor: &mut usize,
    wire: u8,
) -> Result<(), ModelCatalogFetchError> {
    match wire {
        0 => {
            let _ = read_varint(input, cursor)?;
        }
        1 => {
            *cursor = cursor.saturating_add(8);
        }
        2 => {
            let _ = read_bytes(input, cursor)?;
        }
        5 => {
            *cursor = cursor.saturating_add(4);
        }
        other => {
            return Err(ModelCatalogFetchError::InvalidProto(format!(
                "unsupported wire type {other}"
            )));
        }
    }
    if *cursor > input.len() {
        return Err(ModelCatalogFetchError::InvalidProto(
            "truncated protobuf field".into(),
        ));
    }
    Ok(())
}

fn clean_optional(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn parse_parameter_value(
    input: &[u8],
) -> Result<SandModelCatalogValue, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut value = String::new();
    let mut display_name = None;
    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        match (field, wire) {
            (1, 2) => value = read_string(input, &mut cursor)?,
            (2, 2) => display_name = clean_optional(read_string(input, &mut cursor)?),
            _ => skip_field(input, &mut cursor, wire)?,
        }
    }
    Ok(SandModelCatalogValue { value, display_name })
}

fn parse_parameter_values(
    input: &[u8],
) -> Result<Vec<SandModelCatalogValue>, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut values = Vec::new();
    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        if field == 1 && wire == 2 {
            values.push(parse_parameter_value(read_bytes(input, &mut cursor)?)?);
        } else {
            skip_field(input, &mut cursor, wire)?;
        }
    }
    Ok(values)
}

fn parse_parameter_type(
    input: &[u8],
) -> Result<Option<(SandModelCatalogParameterType, Vec<SandModelCatalogValue>)>, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut result = None;
    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        match (field, wire) {
            (1, 2) => {
                let values = parse_parameter_values(read_bytes(input, &mut cursor)?)?;
                result = Some((SandModelCatalogParameterType::Boolean, values));
            }
            (2, 2) => {
                let values = parse_parameter_values(read_bytes(input, &mut cursor)?)?;
                result = Some((SandModelCatalogParameterType::Enum, values));
            }
            _ => skip_field(input, &mut cursor, wire)?,
        }
    }
    Ok(result)
}

fn parse_parameter_definition(
    input: &[u8],
) -> Result<Option<SandModelCatalogParameter>, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut id = String::new();
    let mut name = None;
    let mut parameter = None;
    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        match (field, wire) {
            (1, 2) => id = read_string(input, &mut cursor)?,
            (2, 2) => name = clean_optional(read_string(input, &mut cursor)?),
            (4, 2) => parameter = parse_parameter_type(read_bytes(input, &mut cursor)?)?,
            _ => skip_field(input, &mut cursor, wire)?,
        }
    }
    let Some((parameter_type, values)) = parameter else {
        return Ok(None);
    };
    if values.is_empty() {
        return Ok(None);
    }
    Ok(Some(SandModelCatalogParameter {
        id,
        name,
        parameter_type,
        values,
    }))
}

fn parse_variant_parameter_value(
    input: &[u8],
) -> Result<SandModelCatalogParameterValue, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut id = String::new();
    let mut value = String::new();
    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        match (field, wire) {
            (1, 2) => id = read_string(input, &mut cursor)?,
            (2, 2) => value = read_string(input, &mut cursor)?,
            _ => skip_field(input, &mut cursor, wire)?,
        }
    }
    Ok(SandModelCatalogParameterValue { id, value })
}

fn parse_variant(
    input: &[u8],
) -> Result<Vec<SandModelCatalogParameterValue>, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut values = Vec::new();
    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        if field == 1 && wire == 2 {
            values.push(parse_variant_parameter_value(read_bytes(input, &mut cursor)?)?);
        } else {
            skip_field(input, &mut cursor, wire)?;
        }
    }
    Ok(values)
}

fn parse_available_model(
    input: &[u8],
) -> Result<Option<SandModelCatalogEntry>, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut name = String::new();
    let mut display_name = None;
    let mut aliases = Vec::new();
    let mut params = Vec::new();
    let mut variants = Vec::new();

    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        match (field, wire) {
            (1, 2) => name = read_string(input, &mut cursor)?,
            (17, 2) => display_name = clean_optional(read_string(input, &mut cursor)?),
            (29, 2) => {
                if let Some(parameter) =
                    parse_parameter_definition(read_bytes(input, &mut cursor)?)?
                {
                    params.push(parameter);
                }
            }
            (30, 2) => variants.push(parse_variant(read_bytes(input, &mut cursor)?)?),
            (37, 2) => aliases.push(read_string(input, &mut cursor)?),
            _ => skip_field(input, &mut cursor, wire)?,
        }
    }

    if name.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(SandModelCatalogEntry {
        id: name,
        display_name,
        aliases,
        params,
        variants,
    }))
}

pub fn decode_available_models_response(
    input: &[u8],
) -> Result<Vec<SandModelCatalogEntry>, ModelCatalogFetchError> {
    let mut cursor = 0;
    let mut models = Vec::new();
    while cursor < input.len() {
        let key = read_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 7) as u8;
        if field == 2 && wire == 2 {
            if let Some(model) = parse_available_model(read_bytes(input, &mut cursor)?)? {
                models.push(model);
            }
        } else {
            skip_field(input, &mut cursor, wire)?;
        }
    }
    Ok(models)
}

pub fn fetch_sand_model_catalog(
    backend_url: &str,
    auth: Arc<HostAuthExtension>,
) -> Result<Vec<SandModelCatalogEntry>, ModelCatalogFetchError> {
    let access_token = auth
        .get_access_token()
        .map_err(|error| ModelCatalogFetchError::Auth(error.to_string()))?;
    let machine_id = auth
        .get_machine_id()
        .map_err(|error| ModelCatalogFetchError::Auth(error.to_string()))?;
    let ghost_mode = resolve_sand_ghost_mode_header(backend_url, &access_token, &machine_id);
    let response = send_cursor_unary(
        backend_url,
        &access_token,
        &machine_id,
        AI_AVAILABLE_MODELS_PATH,
        &encode_available_models_request(),
        MODEL_CATALOG_FETCH_TIMEOUT_MS,
        ghost_mode,
    )
    .map_err(|error| ModelCatalogFetchError::Backend(error.to_string()))?;
    decode_available_models_response(&response)
}
