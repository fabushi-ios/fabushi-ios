use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::protocol::{
    CoordinatorFrame, Failure, LifecyclePhase, ReplyOutcome, ShutdownReason,
    COORDINATOR_CANCELLED, COORDINATOR_PROTOCOL_VERSION,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererPortPhase {
    AwaitingHello,
    Serving,
    Settled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerAction {
    Post(CoordinatorFrame),
    Dispatch { request_id: String, method: String, args: Value },
    Abort { request_id: String },
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RendererPortSettlement {
    ShutdownRequested,
    PortClosed,
    ProtocolBreach(String),
}

#[derive(Debug)]
pub struct RendererPortServer {
    phase: RendererPortPhase,
    in_flight: HashMap<String, String>,
    seen_request_ids: HashSet<String>,
    settlement: Option<RendererPortSettlement>,
}

impl Default for RendererPortServer {
    fn default() -> Self {
        Self {
            phase: RendererPortPhase::AwaitingHello,
            in_flight: HashMap::new(),
            seen_request_ids: HashSet::new(),
            settlement: None,
        }
    }
}

impl RendererPortServer {
    pub fn phase(&self) -> RendererPortPhase { self.phase }
    pub fn settlement(&self) -> Option<&RendererPortSettlement> { self.settlement.as_ref() }
    pub fn in_flight_count(&self) -> usize { self.in_flight.len() }

    pub fn handle_value(&mut self, value: Value) -> Vec<ServerAction> {
        if self.phase == RendererPortPhase::Settled {
            return Vec::new();
        }
        match serde_json::from_value::<CoordinatorFrame>(value) {
            Ok(frame) => self.handle_frame(frame),
            Err(error) => self.breach(format!("invalid coordinator frame: {error}")),
        }
    }

    pub fn handle_frame(&mut self, frame: CoordinatorFrame) -> Vec<ServerAction> {
        if self.phase == RendererPortPhase::Settled {
            return Vec::new();
        }

        match (&self.phase, frame) {
            (_, CoordinatorFrame::Lifecycle { phase: LifecyclePhase::Shutdown, .. }) => {
                self.settle(RendererPortSettlement::ShutdownRequested)
            }
            (_, CoordinatorFrame::Reply { .. } | CoordinatorFrame::Event { .. }) => {
                self.breach("client posted a server-direction frame".into())
            }
            (_, CoordinatorFrame::Lifecycle { phase: LifecyclePhase::Ready, .. }) => {
                self.breach("client posted a server-direction ready frame".into())
            }
            (RendererPortPhase::AwaitingHello, CoordinatorFrame::Lifecycle {
                phase: LifecyclePhase::Hello,
                protocol_version,
                ..
            }) => {
                if protocol_version != Some(COORDINATOR_PROTOCOL_VERSION) {
                    return self.breach(format!(
                        "hello.protocolVersion {:?} is not supported {}",
                        protocol_version, COORDINATOR_PROTOCOL_VERSION
                    ));
                }
                self.phase = RendererPortPhase::Serving;
                vec![ServerAction::Post(CoordinatorFrame::ready())]
            }
            (RendererPortPhase::AwaitingHello, other) => {
                self.breach(format!("{} frame before hello", frame_kind(&other)))
            }
            (RendererPortPhase::Serving, CoordinatorFrame::Lifecycle { phase: LifecyclePhase::Hello, .. }) => {
                self.breach("hello repeated on a live session".into())
            }
            (RendererPortPhase::Serving, CoordinatorFrame::Request { request_id, method, args }) => {
                if request_id.trim().is_empty() {
                    return self.breach("requestId must not be empty".into());
                }
                if !self.seen_request_ids.insert(request_id.clone()) {
                    return self.breach(format!("request.requestId {request_id} reused"));
                }
                self.in_flight.insert(request_id.clone(), method.clone());
                vec![ServerAction::Dispatch { request_id, method, args }]
            }
            (RendererPortPhase::Serving, CoordinatorFrame::Cancel { request_id }) => {
                if self.in_flight.remove(&request_id).is_none() {
                    return Vec::new();
                }
                vec![
                    ServerAction::Abort { request_id: request_id.clone() },
                    ServerAction::Post(CoordinatorFrame::Reply {
                        request_id,
                        outcome: ReplyOutcome::Failed {
                            failure: Failure::new(COORDINATOR_CANCELLED, "request cancelled"),
                        },
                    }),
                ]
            }
            (RendererPortPhase::Settled, _) => Vec::new(),
        }
    }

    pub fn complete_request(&mut self, request_id: &str, outcome: ReplyOutcome) -> Vec<ServerAction> {
        if self.phase != RendererPortPhase::Serving || self.in_flight.remove(request_id).is_none() {
            return Vec::new();
        }
        vec![ServerAction::Post(CoordinatorFrame::Reply {
            request_id: request_id.to_string(),
            outcome,
        })]
    }

    pub fn post_event(&self, family: impl Into<String>, payload: Value) -> Option<ServerAction> {
        (self.phase == RendererPortPhase::Serving).then(|| ServerAction::Post(CoordinatorFrame::Event {
            family: family.into(),
            payload,
        }))
    }

    pub fn handle_port_closed(&mut self) -> Vec<ServerAction> {
        if self.phase == RendererPortPhase::Settled {
            return Vec::new();
        }
        self.settle(RendererPortSettlement::PortClosed)
    }

    fn breach(&mut self, detail: String) -> Vec<ServerAction> {
        let shutdown = ServerAction::Post(CoordinatorFrame::shutdown(
            ShutdownReason::ProtocolError,
            Some(detail.clone()),
        ));
        let mut actions = vec![shutdown];
        for request_id in self.in_flight.keys() {
            actions.push(ServerAction::Abort { request_id: request_id.clone() });
        }
        self.in_flight.clear();
        self.phase = RendererPortPhase::Settled;
        self.settlement = Some(RendererPortSettlement::ProtocolBreach(detail));
        actions.push(ServerAction::Close);
        actions
    }

    fn settle(&mut self, settlement: RendererPortSettlement) -> Vec<ServerAction> {
        let mut actions = Vec::new();
        for request_id in self.in_flight.keys() {
            actions.push(ServerAction::Abort { request_id: request_id.clone() });
        }
        self.in_flight.clear();
        self.phase = RendererPortPhase::Settled;
        self.settlement = Some(settlement);
        actions.push(ServerAction::Close);
        actions
    }
}

fn frame_kind(frame: &CoordinatorFrame) -> &'static str {
    match frame {
        CoordinatorFrame::Lifecycle { .. } => "lifecycle",
        CoordinatorFrame::Request { .. } => "request",
        CoordinatorFrame::Cancel { .. } => "cancel",
        CoordinatorFrame::Reply { .. } => "reply",
        CoordinatorFrame::Event { .. } => "event",
    }
}
