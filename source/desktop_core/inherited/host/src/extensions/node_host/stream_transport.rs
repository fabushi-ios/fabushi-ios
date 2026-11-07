//! Transport-neutral state for an auxiliary node-host byte stream.
//!
//! Socket creation, TLS/edge credentials, byte writes, and timers remain with
//! the existing Gateway transport owner. This module only makes framing,
//! backpressure, and close decisions explicit and testable.

use std::time::{Duration, Instant};

pub const STREAM_MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const STREAM_PAUSE_BUFFERED_BYTES: usize = 4 * 1024 * 1024;
pub const STREAM_CLOSE_ACK_TIMEOUT: Duration = Duration::from_secs(5);
pub const STREAM_CLOSE_FLUSH_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStreamCloseTrigger {
    OwnerAbort,
    TargetClose,
    TargetError,
    WebSocketClose,
    WebSocketError,
    SendError,
    InvalidFrame,
    SpliceUnavailable,
    StartupError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayFrameDecision {
    Forward(Vec<u8>),
    RejectText,
    IgnoreClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetDataDecision {
    Forward { bytes: usize, pause_source: bool },
    IgnoreClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetCloseDecision {
    CloseNow,
    WaitForCloseAck { deadline: Instant },
    WaitForFlush { deadline: Instant },
    IgnoreClosed,
}

#[derive(Debug, Default)]
pub struct NodeStreamSpliceState {
    settled: bool,
    target_readable_ended: bool,
    paused: bool,
    forwarded_bytes: u64,
    trigger: Option<NodeStreamCloseTrigger>,
}

impl NodeStreamSpliceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_gateway_frame(&mut self, binary: bool, payload: &[u8]) -> GatewayFrameDecision {
        if self.settled || self.target_readable_ended {
            return GatewayFrameDecision::IgnoreClosed;
        }
        if !binary || payload.len() > STREAM_MAX_PAYLOAD_BYTES {
            self.finish(NodeStreamCloseTrigger::InvalidFrame);
            return GatewayFrameDecision::RejectText;
        }
        GatewayFrameDecision::Forward(payload.to_vec())
    }

    pub fn on_target_data(
        &mut self,
        bytes: usize,
        websocket_buffered_bytes: usize,
    ) -> TargetDataDecision {
        if self.settled || self.target_readable_ended {
            return TargetDataDecision::IgnoreClosed;
        }
        self.forwarded_bytes = self.forwarded_bytes.saturating_add(bytes as u64);
        self.paused = websocket_buffered_bytes > STREAM_PAUSE_BUFFERED_BYTES;
        TargetDataDecision::Forward {
            bytes,
            pause_source: self.paused,
        }
    }

    pub fn on_target_drain(&mut self, websocket_buffered_bytes: usize) -> bool {
        if self.settled {
            return false;
        }
        if websocket_buffered_bytes <= STREAM_PAUSE_BUFFERED_BYTES {
            self.paused = false;
        }
        !self.paused
    }

    pub fn on_target_close(
        &mut self,
        readable_ended: bool,
        websocket_open: bool,
        websocket_buffered_bytes: usize,
        now: Instant,
    ) -> TargetCloseDecision {
        if self.settled {
            return TargetCloseDecision::IgnoreClosed;
        }
        self.target_readable_ended = true;
        self.trigger.get_or_insert(NodeStreamCloseTrigger::TargetClose);
        if !readable_ended || !websocket_open {
            self.settled = true;
            return TargetCloseDecision::CloseNow;
        }
        if self.forwarded_bytes == 0 || websocket_buffered_bytes == 0 {
            return TargetCloseDecision::WaitForCloseAck {
                deadline: now + STREAM_CLOSE_ACK_TIMEOUT,
            };
        }
        TargetCloseDecision::WaitForFlush {
            deadline: now + STREAM_CLOSE_FLUSH_TIMEOUT,
        }
    }

    pub fn finish(&mut self, trigger: NodeStreamCloseTrigger) {
        self.trigger.get_or_insert(trigger);
        self.settled = true;
        self.paused = false;
    }

    pub fn settled(&self) -> bool {
        self.settled
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn forwarded_bytes(&self) -> u64 {
        self.forwarded_bytes
    }

    pub fn trigger(&self) -> Option<NodeStreamCloseTrigger> {
        self.trigger
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GatewayFrameDecision, NodeStreamCloseTrigger, NodeStreamSpliceState,
        TargetCloseDecision, TargetDataDecision, STREAM_CLOSE_FLUSH_TIMEOUT,
        STREAM_PAUSE_BUFFERED_BYTES,
    };
    use std::time::Instant;

    #[test]
    fn binary_frames_forward_and_text_frames_fail_closed() {
        let mut state = NodeStreamSpliceState::new();
        assert_eq!(state.on_gateway_frame(true, b"ok"), GatewayFrameDecision::Forward(b"ok".to_vec()));
        assert_eq!(state.on_gateway_frame(false, b"text"), GatewayFrameDecision::RejectText);
        assert_eq!(state.trigger(), Some(NodeStreamCloseTrigger::InvalidFrame));
        assert_eq!(state.on_gateway_frame(true, b"late"), GatewayFrameDecision::IgnoreClosed);
    }

    #[test]
    fn target_backpressure_and_flush_close_are_bounded() {
        let now = Instant::now();
        let mut state = NodeStreamSpliceState::new();
        assert_eq!(
            state.on_target_data(10, STREAM_PAUSE_BUFFERED_BYTES + 1),
            TargetDataDecision::Forward { bytes: 10, pause_source: true }
        );
        assert!(state.paused());
        assert!(state.on_target_drain(0));
        assert_eq!(
            state.on_target_close(true, true, 1, now),
            TargetCloseDecision::WaitForFlush { deadline: now + STREAM_CLOSE_FLUSH_TIMEOUT }
        );
    }
}
