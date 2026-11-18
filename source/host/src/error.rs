use gpui_telegram_client::HttpClientError;
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
        Self::new("invalid_params", msg)
    }

    pub fn invalid_json(msg: impl Into<String>) -> Self {
        Self::new("invalid_json", msg)
    }

    pub fn invalid_arguments(msg: impl Into<String>) -> Self {
        Self::new("invalid_arguments", msg)
    }

    pub fn null_host() -> Self {
        Self::new("invalid_arguments", "TelegramAppHost pointer is null")
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self::with_http_status("unauthorized", msg, 401)
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::with_http_status("forbidden", msg, 403)
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::with_http_status("not_found", msg, 404)
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::with_http_status("conflict", msg, 409)
    }

    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self::with_http_status("rate_limited", msg, 429)
    }

    pub fn network(msg: impl Into<String>) -> Self {
        Self::new("network_error", msg)
    }

    pub fn storage(msg: impl Into<String>) -> Self {
        Self::new("storage_error", msg)
    }

    pub fn crypto(msg: impl Into<String>) -> Self {
        Self::new("crypto_error", msg)
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
        Self::new("internal_error", msg)
    }

    pub fn from_http_error(err: HttpClientError) -> Self {
        match err {
            HttpClientError::Unauthorized(msg) => Self::unauthorized(msg),
            HttpClientError::Conflict(msg) => Self::conflict(msg),
            HttpClientError::Proxy(msg) => Self::network(format!("Proxy error: {msg}")),
            HttpClientError::Serialization(e) => Self::invalid_json(e.to_string()),
            HttpClientError::Network(e) => Self::network(e.to_string()),
            HttpClientError::Api { status, message } => match status {
                401 => Self::unauthorized(message),
                403 => Self::forbidden(message),
                404 => Self::not_found(message),
                409 => Self::conflict(message),
                429 => Self::rate_limited(message),
                400 | 422 => Self::with_http_status("invalid_params", message, status),
                _ => Self::with_http_status("network_error", message, status),
            },
        }
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
