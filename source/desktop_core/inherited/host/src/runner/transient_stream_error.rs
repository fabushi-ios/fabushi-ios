#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFailureKind {
    Timeout,
    Transport,
    RateLimit,
    Capacity,
    Server,
    Protocol,
    Authentication,
    InvalidRequest,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct TransientStreamError {
    pub kind: StreamFailureKind,
    pub message: String,
    pub retry_after_ms: Option<u64>,
}

impl TransientStreamError {
    pub fn classify(message: impl Into<String>, http_status: Option<u16>, retry_after_ms: Option<u64>) -> Self {
        let message = message.into();
        let lower = message.to_ascii_lowercase();
        let kind = match http_status {
            Some(401 | 403) => StreamFailureKind::Authentication,
            Some(408) => StreamFailureKind::Timeout,
            Some(429) => StreamFailureKind::RateLimit,
            Some(500 | 502 | 503 | 504) => StreamFailureKind::Server,
            Some(status) if (400..500).contains(&status) => StreamFailureKind::InvalidRequest,
            _ if lower.contains("cancel") => StreamFailureKind::Cancelled,
            _ if lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("etimedout")
                || lower.contains("deadline_exceeded") => StreamFailureKind::Timeout,
            _ if lower.contains("capacity") || lower.contains("overload") => StreamFailureKind::Capacity,
            _ if lower.contains("transport")
                || lower.contains("connection")
                || lower.contains("reset")
                || lower.contains("broken pipe")
                || lower.contains("econnreset")
                || lower.contains("epipe")
                || lower.contains("econnaborted")
                || lower.contains("econnrefused")
                || lower.contains("enetreset")
                || lower.contains("enetdown")
                || lower.contains("enetunreach")
                || lower.contains("ehostunreach")
                || lower.contains("eai_again")
                || lower.contains("socket hang up")
                || lower.contains("premature close")
                || lower.contains("stream closed")
                || lower.contains("closed stream")
                || lower.contains("operation was aborted")
                || lower.contains("[aborted]")
                || lower.contains("[unavailable]") => StreamFailureKind::Transport,
            _ if lower.contains("malformed") || lower.contains("invalid json") || lower.contains("protocol") => StreamFailureKind::Protocol,
            _ => StreamFailureKind::Unknown,
        };
        Self { kind, message, retry_after_ms }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            StreamFailureKind::Timeout
                | StreamFailureKind::Transport
                | StreamFailureKind::RateLimit
                | StreamFailureKind::Capacity
                | StreamFailureKind::Server
        )
    }
}
