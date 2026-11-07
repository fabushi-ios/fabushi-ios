use std::ffi::{c_char, CString};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("Not initialized: {0}")]
    NotInitialized(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Operation failed: {0}")]
    OperationFailed(String),
    #[error("Panic in Rust core: {0}")]
    Panic(String),
}

impl BridgeError {
    pub fn error_code(&self) -> &'static str {
        match self {
            BridgeError::InvalidArguments(_) => "INVALID_ARGUMENTS",
            BridgeError::NotInitialized(_) => "NOT_INITIALIZED",
            BridgeError::SerializationError(_) => "SERIALIZATION_ERROR",
            BridgeError::OperationFailed(_) => "OPERATION_FAILED",
            BridgeError::Panic(_) => "HOST_PANIC",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDetail>,
}

pub fn make_success_json<T: Serialize>(result: &T) -> String {
    let resp = JsonRpcResponse {
        ok: true,
        result: Some(result),
        error: None,
    };
    serde_json::to_string(&resp).unwrap_or_else(|_| "{\"ok\":true}".to_string())
}

pub fn make_error_json(code: &str, message: &str) -> String {
    let resp: JsonRpcResponse<serde_json::Value> = JsonRpcResponse {
        ok: false,
        result: None,
        error: Some(ErrorDetail {
            code: code.to_string(),
            message: message.to_string(),
        }),
    };
    serde_json::to_string(&resp).unwrap_or_else(|_| {
        format!(
            "{{\"ok\":false,\"error\":{{\"code\":\"{}\",\"message\":\"{}\"}}}}",
            code, message
        )
    })
}

pub fn make_error_json_ptr(code: &str, message: &str) -> *mut c_char {
    let json = make_error_json(code, message);
    CString::new(json)
        .unwrap_or_else(|_| CString::new("{\"ok\":false}").unwrap())
        .into_raw()
}
