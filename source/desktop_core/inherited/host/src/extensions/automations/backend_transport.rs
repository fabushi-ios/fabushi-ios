use std::sync::Arc;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;
use url::Url;

use crate::extensions::auth::credential_renewer::{
    SAND_CLIENT_TYPE, sand_box_namespace, sand_client_version,
};
use crate::extensions::auth::extension::HostAuthExtension;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutomationsBackendError {
    #[error("sand automations backend {path} returned {status}")]
    Status { path: String, status: u16 },
    #[error("sand automations backend URL invalid: {0}")]
    Backend(String),
    #[error("sand automations auth failed: {0}")]
    Auth(String),
    #[error("sand automations transport failed: {0}")]
    Transport(String),
    #[error("sand automations response decode failed: {0}")]
    Decode(String),
}

impl AutomationsBackendError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub trait AutomationsBackendTransport: Send + Sync {
    fn post_json(&self, path: &str, body: &Value) -> Result<Value, AutomationsBackendError>;
}

pub struct ReqwestAutomationsBackendTransport {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    client: Client,
}

impl ReqwestAutomationsBackendTransport {
    pub fn new(
        backend_url: impl Into<String>,
        auth: Arc<HostAuthExtension>,
    ) -> Result<Self, String> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| error.to_string())?;
        Ok(Self {
            backend_url: backend_url.into(),
            auth,
            client,
        })
    }
}

impl AutomationsBackendTransport for ReqwestAutomationsBackendTransport {
    fn post_json(&self, path: &str, body: &Value) -> Result<Value, AutomationsBackendError> {
        let token = self
            .auth
            .get_access_token()
            .map_err(|error| AutomationsBackendError::Auth(error.to_string()))?;
        let base = Url::parse(&self.backend_url)
            .map_err(|error| AutomationsBackendError::Backend(error.to_string()))?;
        let url = base
            .join(path)
            .map_err(|error| AutomationsBackendError::Backend(error.to_string()))?;

        let response = self
            .client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .header("x-cursor-client-type", SAND_CLIENT_TYPE)
            .header("x-cursor-client-version", sand_client_version())
            .header("x-sand-box-namespace", sand_box_namespace())
            .json(body)
            .send()
            .map_err(|error| AutomationsBackendError::Transport(error.to_string()))?;

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(AutomationsBackendError::Status {
                path: path.to_string(),
                status,
            });
        }

        response
            .json::<Value>()
            .map_err(|error| AutomationsBackendError::Decode(error.to_string()))
    }
}
