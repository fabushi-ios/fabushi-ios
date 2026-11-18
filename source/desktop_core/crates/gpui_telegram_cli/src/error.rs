use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Usage error: {0}")]
    Usage(String),

    #[error("Unknown action or subcommand: {0}")]
    UnknownCommand(String),

    #[error("Input validation error: {0}")]
    Validation(String),

    #[error("Authentication required or rejected: {0}")]
    Unauthorized(String),

    #[error("Approval required for '{action}' (gate_id={gate_id}): {message}")]
    ApprovalRequired {
        action: String,
        gate_id: String,
        expires_at_ms: u64,
        message: String,
    },

    #[error("Action denied or forbidden: {0}")]
    Forbidden(String),

    #[error("HTTP/network error: {0}")]
    Http(String),

    #[error("Server returned {status}: {body}")]
    ServerError { status: u16, body: Value },

    #[error("Execution error: {0}")]
    Execution(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) | Self::UnknownCommand(_) | Self::Validation(_) => 2,
            Self::Unauthorized(_) => 3,
            Self::ApprovalRequired { .. } | Self::Forbidden(_) => 4,
            Self::Http(_) | Self::ServerError { .. } | Self::Execution(_) => 1,
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Usage(msg) => json!({
                "success": false,
                "error": "usage_error",
                "message": msg,
            }),
            Self::UnknownCommand(msg) => json!({
                "success": false,
                "error": "unknown_command",
                "message": msg,
            }),
            Self::Validation(msg) => json!({
                "success": false,
                "error": "validation_error",
                "message": msg,
            }),
            Self::Unauthorized(msg) => json!({
                "success": false,
                "error": "unauthorized",
                "message": msg,
            }),
            Self::ApprovalRequired {
                action,
                gate_id,
                expires_at_ms,
                message,
            } => json!({
                "success": false,
                "error": "Approval required",
                "action": action,
                "gate_id": gate_id,
                "expires_at_ms": expires_at_ms,
                "message": message,
            }),
            Self::Forbidden(msg) => json!({
                "success": false,
                "error": "forbidden",
                "message": msg,
            }),
            Self::Http(msg) => json!({
                "success": false,
                "error": "http_error",
                "message": msg,
            }),
            Self::ServerError { status, body } => json!({
                "success": false,
                "status": status,
                "error": body.get("error").cloned().unwrap_or_else(|| json!("server_error")),
                "message": body.get("message").cloned().unwrap_or_else(|| json!(format!("Server returned status {status}"))),
                "body": body,
            }),
            Self::Execution(msg) => json!({
                "success": false,
                "error": "execution_error",
                "message": msg,
            }),
        }
    }
}
