//! Candidate selection policy for the node-host Gateway transport.
//!
//! This module owns only candidate ordering and failover admission. The
//! Gateway client remains the owner of sockets, TLS, edge-auth headers,
//! requests, and stop/drain. Keeping this state separate prevents a late
//! close from an abandoned candidate from switching a connection that already
//! selected a winner.

use super::gateway_connection::NodeHostGatewayConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateCloseDecision {
    Ignore,
    Stop,
    Advance { next_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeHostGatewayCandidateRouter {
    candidate_count: usize,
    current_index: usize,
    winner_selected: bool,
    stopped: bool,
}

impl NodeHostGatewayCandidateRouter {
    pub fn new(candidates: &[NodeHostGatewayConfig]) -> Result<Self, String> {
        if candidates.is_empty() {
            return Err("node host gateway candidate list cannot be empty".into());
        }
        for candidate in candidates {
            candidate.validate()?;
        }
        Ok(Self {
            candidate_count: candidates.len(),
            current_index: 0,
            winner_selected: candidates.len() == 1,
            stopped: false,
        })
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }

    pub fn winner_selected(&self) -> bool {
        self.winner_selected
    }

    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn on_hello(&mut self) -> bool {
        if self.stopped {
            return false;
        }
        self.winner_selected = true;
        true
    }

    pub fn on_close(&mut self, phase_pre_hello: bool, connect_request_sent: bool) -> CandidateCloseDecision {
        if self.stopped || self.winner_selected {
            return CandidateCloseDecision::Ignore;
        }
        if !phase_pre_hello || connect_request_sent {
            return CandidateCloseDecision::Stop;
        }
        let next_index = self.current_index.saturating_add(1);
        if next_index >= self.candidate_count {
            return CandidateCloseDecision::Stop;
        }
        self.current_index = next_index;
        CandidateCloseDecision::Advance { next_index }
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }
}

pub fn format_gateway_candidate_url(
    gateway: &NodeHostGatewayConfig,
    context_path: Option<&str>,
) -> Result<String, String> {
    gateway.validate()?;
    if let Some(path) = context_path {
        if path.len() > 2048
            || path.chars().any(|character| character.is_control())
            || path.contains('?')
            || path.contains('#')
            || path.contains('\\')
        {
            return Err("node host gateway context path is unsafe or too long".into());
        }
    }
    let host = if gateway.host.contains(':')
        && !(gateway.host.starts_with('[') && gateway.host.ends_with(']'))
    {
        format!("[{}]", gateway.host)
    } else {
        gateway.host.clone()
    };
    let scheme = if gateway.tls { "wss" } else { "ws" };
    let path = context_path
        .or(gateway.context_path.as_deref())
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != "/")
        .map(|path| {
            if path.starts_with('/') {
                path.to_string()
            } else {
                format!("/{path}")
            }
        })
        .unwrap_or_default();
    Ok(format!("{scheme}://{host}:{}{path}", gateway.port))
}

#[cfg(test)]
mod tests {
    use super::{
        format_gateway_candidate_url, CandidateCloseDecision, NodeHostGatewayCandidateRouter,
    };
    use crate::extensions::node_host::gateway_connection::NodeHostGatewayConfig;

    #[test]
    fn empty_candidates_are_rejected_and_urls_preserve_ipv6_and_path() {
        assert!(NodeHostGatewayCandidateRouter::new(&[]).is_err());
        let config = NodeHostGatewayConfig::new("2001:db8::1", 18789, true);
        assert_eq!(
            format_gateway_candidate_url(&config, Some("gateway/v1")).expect("candidate URL"),
            "wss://[2001:db8::1]:18789/gateway/v1"
        );
        let configured_path =
            NodeHostGatewayConfig::new("127.0.0.1", 18789, false).with_context_path("stored");
        assert_eq!(
            format_gateway_candidate_url(&configured_path, None).expect("candidate URL"),
            "ws://127.0.0.1:18789/stored"
        );
    }

    #[test]
    fn only_unaccepted_pre_hello_candidates_fail_over() {
        let config = NodeHostGatewayConfig::new("127.0.0.1", 18789, false);
        let mut router = NodeHostGatewayCandidateRouter::new(&[
            config.clone(),
            config.clone(),
            config,
        ])
        .expect("candidate list should be accepted");
        assert_eq!(
            router.on_close(true, false),
            CandidateCloseDecision::Advance { next_index: 1 }
        );
        assert!(router.on_hello());
        assert_eq!(router.on_close(true, false), CandidateCloseDecision::Ignore);
    }

    #[test]
    fn sent_connect_request_stops_instead_of_switching_candidate() {
        let config = NodeHostGatewayConfig::new("127.0.0.1", 18789, false);
        let mut router = NodeHostGatewayCandidateRouter::new(&[config.clone(), config])
            .expect("candidate list should be accepted");
        assert_eq!(router.on_close(true, true), CandidateCloseDecision::Stop);
        router.stop();
        assert_eq!(router.on_close(true, false), CandidateCloseDecision::Ignore);
    }

    #[test]
    fn candidate_admission_rejects_unsafe_endpoint_components() {
        let empty_host = NodeHostGatewayConfig::new("  ", 18789, true);
        assert!(NodeHostGatewayCandidateRouter::new(&[empty_host]).is_err());

        let unsafe_path = NodeHostGatewayConfig::new("gateway.example", 18789, true)
            .with_context_path("/gateway?token=leak");
        assert!(NodeHostGatewayCandidateRouter::new(&[unsafe_path]).is_err());

        let unsafe_host = NodeHostGatewayConfig::new("gateway@example", 18789, true);
        assert!(format_gateway_candidate_url(&unsafe_host, None).is_err());

        let host_with_port = NodeHostGatewayConfig::new("gateway.example:443", 18789, true);
        assert!(NodeHostGatewayCandidateRouter::new(&[host_with_port]).is_err());

        let plaintext_public = NodeHostGatewayConfig::new("gateway.example", 18789, false);
        assert!(NodeHostGatewayCandidateRouter::new(&[plaintext_public]).is_err());
    }
}
