//! First-party automation journal adapter. No inferred compatibility with vendor RPCs.
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use reqwest::blocking::Client;
use serde_json::Value;
use url::Url;

use crate::extensions::auth::auth_service::{FabushiCoreCredential, FabushiCoreCredentialProvider};
use crate::extensions::auth::credential_renewer::configured_product_backend_url;
use super::backend_transport::{AutomationsBackendError, AutomationsBackendTransport};
use super::sand_automation_fire_consumer::BackendAutomationFire;

const PROTOCOL: &str = "fabushi.automation.v1";
const MAX_RESPONSE_BYTES: u64 = 1_048_576;

pub struct NativeAutomationsBackendTransport {
    base: Url,
    credentials: Arc<dyn FabushiCoreCredentialProvider>,
    identity: (String, String, String, String),
    http: Client,
}

fn identity(credential: &FabushiCoreCredential) -> (String, String, String, String) {
    (credential.issuer().to_string(), credential.user_id.clone(), credential.session_id.clone(), credential.device_id.clone())
}
fn auth_error() -> AutomationsBackendError {
    AutomationsBackendError::Auth("Native automation account/session changed or is unavailable; recreate its runtime.".into())
}
fn invalid_response() -> AutomationsBackendError {
    AutomationsBackendError::Decode("Invalid native automation journal response.".into())
}

impl NativeAutomationsBackendTransport {
    /// Start only after native account readiness. A new session needs a new runtime;
    /// old pending work must be reconciled under its original account.
    pub fn for_current_account(base: &str, credentials: Arc<dyn FabushiCoreCredentialProvider>) -> Result<Self, AutomationsBackendError> {
        let validated = configured_product_backend_url(Some(base), None)
            .map_err(|_| AutomationsBackendError::Backend("Native automation origin is not approved.".into()))?;
        let base = Url::parse(&validated)
            .map_err(|_| AutomationsBackendError::Backend("Invalid native automation origin.".into()))?;
        let credential = credentials.get_fabushi_core_credential().map_err(|_| auth_error())?;
        credential.authorize_origin(base.as_str()).map_err(|_| auth_error())?;
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build().map_err(|_| AutomationsBackendError::Transport("Could not create native automation client.".into()))?;
        Ok(Self { base, credentials, identity: identity(&credential), http })
    }

    fn credential(&self) -> Result<FabushiCoreCredential, AutomationsBackendError> {
        let credential = self.credentials.get_fabushi_core_credential().map_err(|_| auth_error())?;
        credential.authorize_origin(self.base.as_str()).map_err(|_| auth_error())?;
        if identity(&credential) != self.identity { return Err(auth_error()); }
        Ok(credential)
    }

    pub(super) fn ensure_current_account(&self) -> Result<(), AutomationsBackendError> {
        self.credential().map(|_| ())
    }

    pub(super) fn request_native_json(&self, method: reqwest::Method, path: &str, body: Option<&Value>, max_bytes: u64) -> Result<Value, AutomationsBackendError> {
        if !path.starts_with("/v1/") || path.starts_with("//") || path.contains("/../") || max_bytes > 8*1_048_576 {
            return Err(invalid_response());
        }
        let credential = self.credential()?;
        let url = self.base.join(path).map_err(|_| invalid_response())?;
        let mut request = self.http.request(method,url)
            .bearer_auth(credential.access_token())
            .header("x-fabushi-device-id", &credential.device_id)
            .header("accept", "application/json");
        if let Some(body)=body { request=request.json(body); }
        let response=request.send()
            .map_err(|_| AutomationsBackendError::Transport("Native automation request failed; outcome may be unknown.".into()))?;
        let status=response.status().as_u16();let mut bytes=Vec::new();
        response.take(max_bytes+1).read_to_end(&mut bytes)
            .map_err(|_| AutomationsBackendError::Transport("Native automation response was lost; reconcile before replay.".into()))?;
        self.credential()?;
        if status!=200 {return Err(AutomationsBackendError::Status{path:path.split('?').next().unwrap_or(path).into(),status});}
        if bytes.len() as u64>max_bytes {return Err(invalid_response());}
        serde_json::from_slice(&bytes).map_err(|_| invalid_response())
    }
}

impl AutomationsBackendTransport for NativeAutomationsBackendTransport {
    fn post_json(&self, path: &str, body: &Value) -> Result<Value, AutomationsBackendError> {
        let native_path = match path {
            "/sand/automation-events/poll" => "/v1/automation-events/poll",
            "/sand/automation-runs/complete" => "/v1/automation-runs/complete",
            _ => return Err(AutomationsBackendError::Backend("Native automation operation is not supported by this journal adapter.".into())),
        };
        if native_path.ends_with("/complete") {
            let run = body.get("runUuid").and_then(Value::as_str).unwrap_or_default();
            if run.is_empty() || run.len() > 200 || !matches!(body.get("status").and_then(Value::as_str), Some("succeeded" | "failed")) {
                return Err(AutomationsBackendError::Backend("Invalid native automation completion request.".into()));
            }
        }
        let value = self.request_native_json(reqwest::Method::POST,native_path,Some(body),MAX_RESPONSE_BYTES)?;
        if value.get("protocol").and_then(Value::as_str) != Some(PROTOCOL) { return Err(invalid_response()); }
        if native_path.ends_with("/poll") {
            let events = value.get("events").and_then(Value::as_array).ok_or_else(invalid_response)?;
            if events.len() > 100 || events.iter().any(|event| BackendAutomationFire::from_value(event).is_none()) {
                return Err(invalid_response());
            }
        } else {
            let receipt = value.get("receipt").and_then(Value::as_str).ok_or_else(invalid_response)?;
            if value.get("completedAt").and_then(Value::as_u64).is_none()
                || receipt.len() != 64 || !receipt.bytes().all(|b| b.is_ascii_hexdigit())
                || value.pointer("/event/id") != body.get("runUuid")
                || value.pointer("/completion/runUuid") != body.get("runUuid")
                || value.pointer("/completion/status") != body.get("status")
                || value.pointer("/completion/errorMessage").unwrap_or(&Value::Null) != body.get("errorMessage").unwrap_or(&Value::Null)
            { return Err(invalid_response()); }
        }
        Ok(value)
    }
}
