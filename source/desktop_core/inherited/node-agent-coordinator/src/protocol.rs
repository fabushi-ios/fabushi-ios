use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const COORDINATOR_PROTOCOL_VERSION: u32 = 1;
pub const COORDINATOR_CANCELLED: &str = "COORDINATOR_CANCELLED";
pub const COORDINATOR_UNKNOWN_METHOD: &str = "COORDINATOR_UNKNOWN_METHOD";
pub const COORDINATOR_DISCONNECTED: &str = "COORDINATOR_DISCONNECTED";
pub const COORDINATOR_HOST_RESTARTED: &str = "COORDINATOR_HOST_RESTARTED";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LifecyclePhase {
    Hello,
    Ready,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShutdownReason {
    Requested,
    ProtocolError,
    PortClosed,
    HostRestarted,
    Crash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub code: String,
    pub message: String,
    #[serde(rename = "transportKind", default, skip_serializing_if = "Option::is_none")]
    pub transport_kind: Option<String>,
}

impl Failure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), transport_kind: None }
    }

    pub fn with_transport_kind(
        code: impl Into<String>,
        message: impl Into<String>,
        transport_kind: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            transport_kind: Some(transport_kind.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ReplyOutcome {
    Ok { value: Value },
    Failed { failure: Failure },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CoordinatorFrame {
    Lifecycle {
        phase: LifecyclePhase,
        #[serde(rename = "protocolVersion", skip_serializing_if = "Option::is_none")]
        protocol_version: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<ShutdownReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Request {
        #[serde(rename = "requestId")]
        request_id: String,
        method: String,
        args: Value,
    },
    Cancel {
        #[serde(rename = "requestId")]
        request_id: String,
    },
    Reply {
        #[serde(rename = "requestId")]
        request_id: String,
        outcome: ReplyOutcome,
    },
    Event {
        family: String,
        payload: Value,
    },
}

impl CoordinatorFrame {
    pub fn hello() -> Self {
        Self::Lifecycle {
            phase: LifecyclePhase::Hello,
            protocol_version: Some(COORDINATOR_PROTOCOL_VERSION),
            reason: None,
            detail: None,
        }
    }

    pub fn ready() -> Self {
        Self::Lifecycle {
            phase: LifecyclePhase::Ready,
            protocol_version: Some(COORDINATOR_PROTOCOL_VERSION),
            reason: None,
            detail: None,
        }
    }

    pub fn shutdown(reason: ShutdownReason, detail: impl Into<Option<String>>) -> Self {
        Self::Lifecycle {
            phase: LifecyclePhase::Shutdown,
            protocol_version: None,
            reason: Some(reason),
            detail: detail.into(),
        }
    }
}
