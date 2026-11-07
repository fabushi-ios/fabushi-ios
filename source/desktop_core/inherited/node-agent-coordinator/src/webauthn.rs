use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::Error as _,
    ser::Error as _,
};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct WebAuthnCeremony {
    pub kind: String,
    pub origin: String,
    /// Grok Bot 0.18 carries ceremony-specific fields at the top level.
    /// Internally we keep those extension fields in a Value so new fields can
    /// pass through without changing this crate, but the wire format remains
    /// flat and compatible with the frozen reference.
    pub payload: Value,
}

impl Serialize for WebAuthnCeremony {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut object = match &self.payload {
            Value::Object(object) => object.clone(),
            Value::Null => Map::new(),
            _ => {
                return Err(S::Error::custom(
                    "WebAuthn ceremony extension payload must be a JSON object",
                ));
            }
        };
        object.insert("kind".into(), Value::String(self.kind.clone()));
        object.insert("origin".into(), Value::String(self.origin.clone()));
        Value::Object(object).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WebAuthnCeremony {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let Value::Object(mut object) = value else {
            return Err(D::Error::custom("WebAuthn ceremony must be a JSON object"));
        };
        let kind = match object.remove("kind") {
            Some(Value::String(value)) if !value.is_empty() => value,
            _ => return Err(D::Error::custom("WebAuthn ceremony kind is required")),
        };
        let origin = match object.remove("origin") {
            Some(Value::String(value)) if !value.is_empty() => value,
            _ => return Err(D::Error::custom("WebAuthn ceremony origin is required")),
        };

        // Accept the previous Fabushi-only nested representation on read so
        // persisted/test fixtures migrate forward, but always emit the frozen
        // Grok-compatible flat representation on write.
        let payload = if object.len() == 1
            && object
                .get("payload")
                .is_some_and(|value| value.is_object())
        {
            object.remove("payload").unwrap_or(Value::Null)
        } else {
            Value::Object(object)
        };

        Ok(Self {
            kind,
            origin,
            payload,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedWebAuthnConsent {
    pub approved: bool,
    pub prompt_id: Option<String>,
    pub window_handle: Option<u64>,
}

impl ApprovedWebAuthnConsent {
    pub fn approved() -> Self {
        Self {
            approved: true,
            prompt_id: None,
            window_handle: None,
        }
    }

    pub fn declined() -> Self {
        Self {
            approved: false,
            prompt_id: None,
            window_handle: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebAuthnSignerError {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WebAuthnSignerResult {
    Success { credential_json: Value },
    Failed { error: WebAuthnSignerError },
}

pub trait WebAuthnSigner {
    fn sign(
        &mut self,
        ceremony: &WebAuthnCeremony,
        approved: Option<&ApprovedWebAuthnConsent>,
    ) -> WebAuthnSignerResult;
}

#[derive(Debug, Default)]
pub struct RejectingWebAuthnSigner;

impl WebAuthnSigner for RejectingWebAuthnSigner {
    fn sign(
        &mut self,
        _ceremony: &WebAuthnCeremony,
        _approved: Option<&ApprovedWebAuthnConsent>,
    ) -> WebAuthnSignerResult {
        WebAuthnSignerResult::Failed {
            error: WebAuthnSignerError {
                name: "NotAllowedError".into(),
                code: Some("webauthn_unavailable".into()),
                message: "WebAuthn signing requires the platform signer adapter".into(),
            },
        }
    }
}

pub mod provider;
pub mod signer;
