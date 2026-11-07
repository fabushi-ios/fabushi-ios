use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use url::Url;
use uuid::Uuid;

use crate::cursor_backend::{create_cursor_checksum, resolve_sand_ghost_mode_header};
use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, get_configured_backend_url, sand_box_namespace, sand_client_version,
    system_now_ms,
};
use crate::extensions::auth::extension::HostAuthExtension;

pub const CONNECT_UNARY_CONTENT_TYPE: &str = "application/proto";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandConnectError {
    #[error("Sand Connect authentication failed: {0}")]
    Authentication(String),
    #[error("Sand Connect configuration failed: {0}")]
    Configuration(String),
    #[error("Sand Connect transport failed: {0}")]
    Transport(String),
    #[error("Sand Connect {path} returned HTTP {status}: {body}")]
    Status {
        path: String,
        status: u16,
        body: String,
    },
    #[error("Sand Connect protobuf decode failed: {0}")]
    Decode(String),
}

pub trait SandConnectAuth: Send + Sync {
    fn access_token(&self) -> Result<String, String>;
    fn machine_id(&self) -> Result<String, String>;
    fn backend_url(&self) -> Result<String, String> {
        get_configured_backend_url().map_err(|error| error.to_string())
    }
}

impl SandConnectAuth for HostAuthExtension {
    fn access_token(&self) -> Result<String, String> {
        self.get_access_token().map_err(|error| error.to_string())
    }

    fn machine_id(&self) -> Result<String, String> {
        self.get_machine_id().map_err(|error| error.to_string())
    }
}

pub struct SandConnectUnaryClient {
    auth: Arc<dyn SandConnectAuth>,
    client: Client,
}

impl SandConnectUnaryClient {
    pub fn new(auth: Arc<dyn SandConnectAuth>) -> Result<Self, SandConnectError> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| SandConnectError::Transport(error.to_string()))?;
        Ok(Self { auth, client })
    }

    pub fn unary<Request, Response>(
        &self,
        service: &str,
        method: &str,
        request: &Request,
    ) -> Result<Response, SandConnectError>
    where
        Request: Message,
        Response: Message + Default,
    {
        let backend_url = self
            .auth
            .backend_url()
            .map_err(SandConnectError::Configuration)?;
        let access_token = self
            .auth
            .access_token()
            .map_err(SandConnectError::Authentication)?;
        let machine_id = self
            .auth
            .machine_id()
            .map_err(SandConnectError::Authentication)?;
        let ghost_mode =
            resolve_sand_ghost_mode_header(&backend_url, &access_token, &machine_id);
        let base = Url::parse(&backend_url)
            .map_err(|error| SandConnectError::Configuration(error.to_string()))?;
        let path = format!("/{service}/{method}");
        let url = base
            .join(&path)
            .map_err(|error| SandConnectError::Configuration(error.to_string()))?;
        let body = request.encode_to_vec();

        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, CONNECT_UNARY_CONTENT_TYPE)
            .header(ACCEPT, CONNECT_UNARY_CONTENT_TYPE)
            .header("connect-protocol-version", "1")
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(
                "x-cursor-checksum",
                create_cursor_checksum(&machine_id, system_now_ms()),
            )
            .header("x-cursor-client-type", SAND_CLIENT_TYPE)
            .header("x-cursor-client-version", sand_client_version())
            .header("x-sand-box-namespace", sand_box_namespace())
            .header("x-ghost-mode", ghost_mode)
            .header("x-request-id", Uuid::new_v4().to_string())
            .body(body)
            .send()
            .map_err(|error| SandConnectError::Transport(error.to_string()))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .map_err(|error| SandConnectError::Transport(error.to_string()))?;
        if !status.is_success() {
            return Err(SandConnectError::Status {
                path,
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).chars().take(500).collect(),
            });
        }
        Response::decode(bytes.as_ref())
            .map_err(|error| SandConnectError::Decode(error.to_string()))
    }
}
