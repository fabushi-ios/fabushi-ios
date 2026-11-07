use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use url::Url;
use uuid::Uuid;

pub use crate::cursor_backend::create_cursor_checksum;
use crate::cursor_backend::resolve_sand_ghost_mode_header;
use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, get_configured_backend_url, sand_box_namespace, sand_client_version,
};
use crate::extensions::auth::extension::HostAuthExtension;

use super::box_lifecycle_service::{
    BoxLifecycleClient, BoxLifecycleFuture, BoxRunState, RecreateSandBoxRequest,
    RecreateSandBoxResponse,
};
use super::extension::BoxLifecycleClientFactory;

pub const GET_SAND_BOX_RUN_STATE_PATH: &str =
    "/aiserver.v1.GrokBotService/GetSandBoxRunState";
pub const RECREATE_SAND_BOX_PATH: &str =
    "/aiserver.v1.GrokBotService/RecreateSandBox";
pub const BOX_LIFECYCLE_RPC_TIMEOUT_MS: u64 = 10_000;

pub trait BoxLifecycleAuth: Send + Sync {
    fn get_access_token(&self) -> Result<String, String>;
    fn get_machine_id(&self) -> Result<String, String>;
}

impl BoxLifecycleAuth for HostAuthExtension {
    fn get_access_token(&self) -> Result<String, String> {
        HostAuthExtension::get_access_token(self).map_err(|error| error.to_string())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        HostAuthExtension::get_machine_id(self).map_err(|error| error.to_string())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProductionBoxLifecycleError {
    #[error("box lifecycle auth failed: {0}")]
    Auth(String),
    #[error("invalid box lifecycle backend URL: {0}")]
    InvalidBackendUrl(String),
    #[error("box lifecycle transport failed: {0}")]
    Transport(String),
    #[error("box lifecycle backend returned HTTP {status}: {body}")]
    HttpStatus { status: u16, body: String },
    #[error("box lifecycle protobuf response was invalid: {0}")]
    InvalidProto(String),
}

pub struct ProductionBoxLifecycleClient<Auth> {
    auth: Arc<Auth>,
    backend_url: String,
    client: Client,
}

impl<Auth> ProductionBoxLifecycleClient<Auth>
where
    Auth: BoxLifecycleAuth + 'static,
{
    pub fn new(
        auth: Arc<Auth>,
        backend_url: impl Into<String>,
    ) -> Result<Self, ProductionBoxLifecycleError> {
        let backend_url = backend_url.into();
        Url::parse(&backend_url)
            .map_err(|error| ProductionBoxLifecycleError::InvalidBackendUrl(error.to_string()))?;
        let client = Client::builder()
            .timeout(Duration::from_millis(BOX_LIFECYCLE_RPC_TIMEOUT_MS))
            .build()
            .map_err(|error| ProductionBoxLifecycleError::Transport(error.to_string()))?;
        Ok(Self {
            auth,
            backend_url,
            client,
        })
    }

    fn unary(&self, path: &str, body: Vec<u8>) -> Result<Vec<u8>, ProductionBoxLifecycleError> {
        let base = Url::parse(&self.backend_url)
            .map_err(|error| ProductionBoxLifecycleError::InvalidBackendUrl(error.to_string()))?;
        let url = base
            .join(path)
            .map_err(|error| ProductionBoxLifecycleError::InvalidBackendUrl(error.to_string()))?;
        let access_token = self
            .auth
            .get_access_token()
            .map_err(ProductionBoxLifecycleError::Auth)?;
        let machine_id = self
            .auth
            .get_machine_id()
            .map_err(ProductionBoxLifecycleError::Auth)?;
        let checksum = create_cursor_checksum(&machine_id, system_now_ms());
        let ghost_mode =
            resolve_sand_ghost_mode_header(&self.backend_url, &access_token, &machine_id);

        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "application/proto")
            .header("connect-protocol-version", "1")
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header("x-cursor-checksum", checksum)
            .header("x-cursor-client-type", SAND_CLIENT_TYPE)
            .header("x-cursor-client-version", sand_client_version())
            .header("x-sand-box-namespace", sand_box_namespace())
            // Shared Cursor backend parity resolves privacy first and falls back
            // to the privacy-safe ghost mode on lookup failure.
            .header("x-ghost-mode", ghost_mode)
            .header("x-request-id", Uuid::new_v4().to_string())
            .body(body)
            .send()
            .map_err(|error| ProductionBoxLifecycleError::Transport(error.to_string()))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .map_err(|error| ProductionBoxLifecycleError::Transport(error.to_string()))?
            .to_vec();
        if !status.is_success() {
            return Err(ProductionBoxLifecycleError::HttpStatus {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).chars().take(512).collect(),
            });
        }
        Ok(bytes)
    }

    fn get_run_state(&self) -> Result<BoxRunState, ProductionBoxLifecycleError> {
        let bytes = self.unary(GET_SAND_BOX_RUN_STATE_PATH, Vec::new())?;
        Ok(BoxRunState {
            image_update_available: decode_optional_bool_field(&bytes, 2)?.unwrap_or(false),
        })
    }

    fn recreate(
        &self,
        request: &RecreateSandBoxRequest,
    ) -> Result<RecreateSandBoxResponse, ProductionBoxLifecycleError> {
        let mut body = Vec::new();
        if request.preserve_data {
            body.extend_from_slice(&[0x08, 0x01]);
        }
        if request.force {
            body.extend_from_slice(&[0x10, 0x01]);
        }
        let bytes = self.unary(RECREATE_SAND_BOX_PATH, body)?;
        Ok(RecreateSandBoxResponse {
            started: decode_optional_bool_field(&bytes, 1)?.unwrap_or(false),
            reason: decode_optional_string_field(&bytes, 2)?,
        })
    }
}

impl<Auth, Signal> BoxLifecycleClient<Signal> for ProductionBoxLifecycleClient<Auth>
where
    Auth: BoxLifecycleAuth + 'static,
{
    type Error = ProductionBoxLifecycleError;

    fn get_sand_box_run_state<'a>(
        &'a self,
        _signal: &'a Signal,
    ) -> BoxLifecycleFuture<'a, Result<BoxRunState, Self::Error>> {
        Box::pin(async move { self.get_run_state() })
    }

    fn recreate_sand_box<'a>(
        &'a self,
        request: RecreateSandBoxRequest,
    ) -> BoxLifecycleFuture<'a, Result<RecreateSandBoxResponse, Self::Error>> {
        Box::pin(async move { self.recreate(&request) })
    }
}

#[derive(Debug, Clone)]
pub struct ProductionBoxLifecycleClientFactory {
    backend_url: String,
}

impl ProductionBoxLifecycleClientFactory {
    pub fn new(backend_url: impl Into<String>) -> Self {
        Self {
            backend_url: backend_url.into(),
        }
    }

    pub fn from_process_env() -> Result<Self, ProductionBoxLifecycleError> {
        let backend_url = get_configured_backend_url()
            .map_err(|error| ProductionBoxLifecycleError::InvalidBackendUrl(error.to_string()))?;
        Ok(Self::new(backend_url))
    }

    pub fn backend_url(&self) -> &str {
        &self.backend_url
    }
}

impl<Auth> BoxLifecycleClientFactory<Auth> for ProductionBoxLifecycleClientFactory
where
    Auth: BoxLifecycleAuth + 'static,
{
    type Client = ProductionBoxLifecycleClient<Auth>;

    fn create_sand_cursor_backend_client(&self, auth: Arc<Auth>) -> Self::Client {
        ProductionBoxLifecycleClient::new(auth, self.backend_url.clone())
            .expect("validated production box lifecycle backend URL")
    }
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn decode_varint(input: &[u8], cursor: &mut usize) -> Result<u64, ProductionBoxLifecycleError> {
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
    Err(ProductionBoxLifecycleError::InvalidProto(
        "malformed varint".into(),
    ))
}

fn field_span(
    input: &[u8],
    cursor: &mut usize,
    wire_type: u8,
) -> Result<(usize, usize), ProductionBoxLifecycleError> {
    match wire_type {
        0 => {
            let start = *cursor;
            let _ = decode_varint(input, cursor)?;
            Ok((start, *cursor))
        }
        1 => {
            let start = *cursor;
            *cursor = cursor.saturating_add(8);
            if *cursor > input.len() {
                return Err(ProductionBoxLifecycleError::InvalidProto(
                    "truncated fixed64 field".into(),
                ));
            }
            Ok((start, *cursor))
        }
        2 => {
            let length = usize::try_from(decode_varint(input, cursor)?).map_err(|_| {
                ProductionBoxLifecycleError::InvalidProto("length overflow".into())
            })?;
            let start = *cursor;
            *cursor = cursor.saturating_add(length);
            if *cursor > input.len() {
                return Err(ProductionBoxLifecycleError::InvalidProto(
                    "truncated length-delimited field".into(),
                ));
            }
            Ok((start, *cursor))
        }
        5 => {
            let start = *cursor;
            *cursor = cursor.saturating_add(4);
            if *cursor > input.len() {
                return Err(ProductionBoxLifecycleError::InvalidProto(
                    "truncated fixed32 field".into(),
                ));
            }
            Ok((start, *cursor))
        }
        other => Err(ProductionBoxLifecycleError::InvalidProto(format!(
            "unsupported wire type {other}"
        ))),
    }
}

fn decode_optional_bool_field(
    input: &[u8],
    wanted_field: u64,
) -> Result<Option<bool>, ProductionBoxLifecycleError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted_field && wire == 0 {
            return Ok(Some(decode_varint(input, &mut cursor)? != 0));
        }
        let _ = field_span(input, &mut cursor, wire)?;
    }
    Ok(None)
}

fn decode_optional_string_field(
    input: &[u8],
    wanted_field: u64,
) -> Result<Option<String>, ProductionBoxLifecycleError> {
    let mut cursor = 0usize;
    while cursor < input.len() {
        let key = decode_varint(input, &mut cursor)?;
        let field = key >> 3;
        let wire = (key & 0x07) as u8;
        if field == wanted_field && wire == 2 {
            let length = usize::try_from(decode_varint(input, &mut cursor)?).map_err(|_| {
                ProductionBoxLifecycleError::InvalidProto("length overflow".into())
            })?;
            let end = cursor.saturating_add(length);
            if end > input.len() {
                return Err(ProductionBoxLifecycleError::InvalidProto(
                    "truncated string field".into(),
                ));
            }
            let value = std::str::from_utf8(&input[cursor..end])
                .map_err(|_| ProductionBoxLifecycleError::InvalidProto(
                    "string field is not UTF-8".into(),
                ))?
                .to_string();
            return Ok((!value.is_empty()).then_some(value));
        }
        let _ = field_span(input, &mut cursor, wire)?;
    }
    Ok(None)
}
