use std::error::Error;
use std::io;

use thiserror::Error;

use crate::ports::r#box::{
    SandBoxDaemonUnreachableError, SandBoxNoMonitorAvailableError,
};

const NETWORK_ERRNOS: &[&str] = &[
    "ECONNRESET",
    "EPIPE",
    "ECONNABORTED",
    "EHOSTUNREACH",
    "ENETUNREACH",
    "ENETDOWN",
    "ENETRESET",
];

#[derive(Debug, Error)]
pub enum GatewayCommandError {
    #[error("unknown gateway method: {0}")]
    UnknownMethod(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Internal(String),
}

impl GatewayCommandError {
    pub fn status(&self) -> u16 {
        match self {
            Self::UnknownMethod(_) => 404,
            Self::BadRequest(_) => 400,
            Self::Unauthorized(_) => 401,
            Self::Forbidden(_) => 403,
            Self::Conflict(_) => 409,
            Self::Internal(_) => 500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayCommandErrorClassification {
    pub reason: &'static str,
    pub error_class: String,
    pub errno: Option<String>,
}

fn known_errno_in_text(text: &str) -> Option<String> {
    const KNOWN: &[&str] = &[
        "ECONNREFUSED",
        "ENOTFOUND",
        "EAI_AGAIN",
        "ETIMEDOUT",
        "ECONNRESET",
        "EPIPE",
        "ECONNABORTED",
        "EHOSTUNREACH",
        "ENETUNREACH",
        "ENETDOWN",
        "ENETRESET",
    ];
    let upper = text.to_ascii_uppercase();
    KNOWN
        .iter()
        .find(|errno| upper.contains(*errno))
        .map(|errno| (*errno).to_string())
}

fn io_errno(error: &io::Error) -> Option<String> {
    let symbolic = match error.kind() {
        io::ErrorKind::ConnectionRefused => Some("ECONNREFUSED"),
        io::ErrorKind::TimedOut => Some("ETIMEDOUT"),
        io::ErrorKind::ConnectionReset => Some("ECONNRESET"),
        io::ErrorKind::BrokenPipe => Some("EPIPE"),
        io::ErrorKind::ConnectionAborted => Some("ECONNABORTED"),
        _ => None,
    };
    symbolic
        .map(str::to_string)
        .or_else(|| known_errno_in_text(&error.to_string()))
}

fn find_system_errno(error: &(dyn Error + 'static)) -> Option<String> {
    let mut current = Some(error);
    while let Some(node) = current {
        if let Some(io_error) = node.downcast_ref::<io::Error>() {
            if let Some(errno) = io_errno(io_error) {
                return Some(errno);
            }
        }
        if let Some(errno) = known_errno_in_text(&node.to_string()) {
            return Some(errno);
        }
        current = node.source();
    }
    None
}

fn has_timeout_semantics(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(node) = current {
        if node
            .downcast_ref::<io::Error>()
            .is_some_and(|value| value.kind() == io::ErrorKind::TimedOut)
        {
            return true;
        }
        let lower = node.to_string().to_ascii_lowercase();
        if lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("deadline exceeded")
        {
            return true;
        }
        current = node.source();
    }
    false
}

fn error_class(error: &(dyn Error + 'static)) -> String {
    if error.downcast_ref::<SandBoxDaemonUnreachableError>().is_some() {
        "SandBoxDaemonUnreachableError".into()
    } else if error.downcast_ref::<SandBoxNoMonitorAvailableError>().is_some() {
        "SandBoxNoMonitorAvailableError".into()
    } else if error.downcast_ref::<GatewayCommandError>().is_some() {
        "GatewayCommandError".into()
    } else if error.downcast_ref::<io::Error>().is_some() {
        "io::Error".into()
    } else {
        "Error".into()
    }
}

pub fn classify_gateway_command_error(
    error: &(dyn Error + 'static),
) -> GatewayCommandErrorClassification {
    let error_class = error_class(error);
    let errno = find_system_errno(error);

    if let Some(error) = error.downcast_ref::<SandBoxDaemonUnreachableError>() {
        let reason = match error.outcome.as_str() {
            "refused" => "daemon_refused",
            "timeout" => "daemon_timeout",
            _ => "daemon_crash",
        };
        return GatewayCommandErrorClassification {
            reason,
            error_class,
            errno,
        };
    }

    if error.downcast_ref::<SandBoxNoMonitorAvailableError>().is_some() {
        return GatewayCommandErrorClassification {
            reason: "no_monitor",
            error_class,
            errno,
        };
    }

    let reason = match errno.as_deref() {
        Some("ECONNREFUSED") => "refused",
        Some("ENOTFOUND") | Some("EAI_AGAIN") => "dns",
        Some("ETIMEDOUT") => "timeout",
        Some(errno) if NETWORK_ERRNOS.contains(&errno) => "network",
        _ if has_timeout_semantics(error) => "timeout",
        _ => "application",
    };

    GatewayCommandErrorClassification {
        reason,
        error_class,
        errno,
    }
}
