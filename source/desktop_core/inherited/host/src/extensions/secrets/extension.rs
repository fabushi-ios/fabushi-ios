use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::extensions::extension_ids_generated::HostExtensionId;
use crate::extensions::forever_box::forever_box_service::ForeverBoxService;

use super::secrets_service::{
    BoxSecretsApplier, BoxSecretsApplierOptions, BoxSecretsApplyError,
    BoxSecretsLog, BoxSecretsSetError, BoxSecretsStatus,
};

pub const SECRETS_DEPENDENCIES: &[HostExtensionId] = &[HostExtensionId::ForeverBox];

pub fn secrets_extension_id() -> HostExtensionId {
    HostExtensionId::Secrets
}

pub struct HostSecretsExtension {
    service: BoxSecretsApplier,
}

impl HostSecretsExtension {
    pub fn new(service: BoxSecretsApplier) -> Self {
        Self { service }
    }

    pub fn set_secrets(
        &self,
        secrets: BTreeMap<String, String>,
    ) -> Result<BoxSecretsStatus, BoxSecretsSetError> {
        self.service.set_secrets(secrets)
    }

    pub fn get_status(&self) -> BoxSecretsStatus {
        self.service.get_status()
    }

    pub fn stop(&self) {
        self.service.stop();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsGatewayError {
    BadRequest(String),
    Internal(String),
}

fn status_json(status: BoxSecretsStatus) -> Value {
    json!({
        "keys": status.keys,
        "isApplied": status.is_applied,
        "lastAppliedAtMs": status.last_applied_at_ms,
    })
}

fn decode_secrets(args: &Value) -> Result<BTreeMap<String, String>, SecretsGatewayError> {
    let secrets = args
        .get("secrets")
        .and_then(Value::as_object)
        .ok_or_else(|| SecretsGatewayError::BadRequest(
            "setBoxSecrets requires a secrets object".into(),
        ))?;
    let mut decoded = BTreeMap::new();
    for (key, value) in secrets {
        let Some(value) = value.as_str() else {
            return Err(SecretsGatewayError::BadRequest(format!(
                "setBoxSecrets secret {key} must be a string"
            )));
        };
        decoded.insert(key.clone(), value.to_string());
    }
    Ok(decoded)
}

/// Frozen Grok Host gateway surface for the Secrets extension.
///
/// The service remains Host-owned and this boundary only decodes/encodes the
/// public command contract. Unknown methods are deliberately not claimed so
/// the wider Host gateway can continue normal dispatch.
pub fn dispatch_secrets_gateway_call(
    extension: &HostSecretsExtension,
    method: &str,
    args: &Value,
) -> Option<Result<Value, SecretsGatewayError>> {
    match method {
        "setBoxSecrets" => {
            let secrets = match decode_secrets(args) {
                Ok(secrets) => secrets,
                Err(error) => return Some(Err(error)),
            };
            Some(
                extension
                    .set_secrets(secrets)
                    .map(status_json)
                    .map_err(|error| match error {
                        BoxSecretsSetError::Validation(message) => {
                            SecretsGatewayError::BadRequest(message)
                        }
                        BoxSecretsSetError::Io(error) => {
                            SecretsGatewayError::Internal(error.to_string())
                        }
                    }),
            )
        }
        "getBoxSecretsStatus" => Some(Ok(status_json(extension.get_status()))),
        _ => None,
    }
}

pub fn start_secrets_extension(
    box_service: Arc<ForeverBoxService>,
    log: BoxSecretsLog,
) -> HostSecretsExtension {
    let apply_box = Arc::clone(&box_service);
    let mut options = BoxSecretsApplierOptions::new(Arc::new(move |update| {
        apply_box
            .apply_environment(update)
            .map_err(|error| BoxSecretsApplyError::Retryable(error.to_string()))
    }));
    options.log = log;
    let service = BoxSecretsApplier::new(options);
    if let Err(error) = service.apply_persisted() {
        eprintln!("box secrets: failed to load persisted secrets: {error}");
    }
    HostSecretsExtension::new(service)
}
