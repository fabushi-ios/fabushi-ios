//! Bounded reconnect classification for the Host-owned NodeHost transport.
//!
//! This module owns no socket, timer, credential, session, or retry task. It
//! only converts a transport outcome into a fail-closed reconnect decision so
//! the eventual lifecycle caller cannot treat configuration/auth failures as
//! transient network loss.

use std::time::Duration;

use super::gateway_transport::NodeHostGatewayTransportError;

const BASE_RETRY_DELAY_MS: u64 = 250;
const MAX_RETRY_DELAY_MS: u64 = 30_000;
const MAX_RETRY_ATTEMPTS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHostReconnectClass {
    RetryableTransport,
    TerminalConfiguration,
    TerminalIdentity,
    TerminalProtocol,
    TerminalAuthentication,
    RetryBudgetExhausted,
}

impl NodeHostReconnectClass {
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::RetryableTransport)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHostReconnectDecision {
    Retry {
        attempt: u32,
        delay: Duration,
    },
    Pause {
        class: NodeHostReconnectClass,
    },
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NodeHostReconnectPolicy {
    attempts: u32,
}

impl NodeHostReconnectPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Reset retry history only after a validated hello promotes a session.
    pub fn on_hello(&mut self) {
        self.attempts = 0;
    }

    pub fn observe(
        &mut self,
        error: &NodeHostGatewayTransportError,
    ) -> NodeHostReconnectDecision {
        let class = classify_transport_error(error);
        if !class.is_retryable() {
            return NodeHostReconnectDecision::Pause { class };
        }
        if self.attempts >= MAX_RETRY_ATTEMPTS {
            return NodeHostReconnectDecision::Pause {
                class: NodeHostReconnectClass::RetryBudgetExhausted,
            };
        }
        self.attempts = self.attempts.saturating_add(1);
        let exponent = self.attempts.saturating_sub(1).min(16);
        let multiplier = 1_u64 << exponent;
        let delay_ms = BASE_RETRY_DELAY_MS
            .saturating_mul(multiplier)
            .min(MAX_RETRY_DELAY_MS);
        NodeHostReconnectDecision::Retry {
            attempt: self.attempts,
            delay: Duration::from_millis(delay_ms),
        }
    }
}

pub fn classify_transport_error(
    error: &NodeHostGatewayTransportError,
) -> NodeHostReconnectClass {
    match error {
        NodeHostGatewayTransportError::Io(_)
        | NodeHostGatewayTransportError::WebSocket(_) => {
            NodeHostReconnectClass::RetryableTransport
        }
        NodeHostGatewayTransportError::Config(_) => NodeHostReconnectClass::TerminalConfiguration,
        NodeHostGatewayTransportError::Credential(_) => NodeHostReconnectClass::TerminalIdentity,
        NodeHostGatewayTransportError::Protocol(message) => {
            let message = message.to_ascii_lowercase();
            if message.contains("closed")
                || message.contains("timed out")
                || message.contains("timeout")
            {
                NodeHostReconnectClass::RetryableTransport
            } else if message.contains("auth")
                || message.contains("token")
                || message.contains("pairing")
                || message.contains("permission")
            {
                NodeHostReconnectClass::TerminalAuthentication
            } else {
                NodeHostReconnectClass::TerminalProtocol
            }
        }
    }
}
