use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

impl BridgeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            http_status: None,
        }
    }

    pub fn with_http_status(
        code: impl Into<String>,
        message: impl Into<String>,
        http_status: u16,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            http_status: Some(http_status),
        }
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new("INVALID_PARAMS", msg)
    }

    pub fn invalid_json(msg: impl Into<String>) -> Self {
        Self::new("INVALID_JSON", msg)
    }

    pub fn invalid_arguments(msg: impl Into<String>) -> Self {
        Self::new("INVALID_ARGUMENTS", msg)
    }

    pub fn null_host() -> Self {
        Self::new("INVALID_ARGUMENTS", "TelegramAppHost pointer is null")
    }

    pub fn unknown_command(command: &str) -> Self {
        Self::new(
            "unknown_command",
            format!("Unsupported C-ABI RPC command: {command}"),
        )
    }

    pub fn panic_recovered(msg: impl Into<String>) -> Self {
        Self::new("HOST_PANIC", msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new("INTERNAL_ERROR", msg)
    }

    pub fn to_envelope_string(&self) -> String {
        json!({
            "ok": false,
            "error": self
        })
        .to_string()
    }
}

pub fn ok_envelope(result: Value) -> String {
    json!({
        "ok": true,
        "result": result
    })
    .to_string()
}
