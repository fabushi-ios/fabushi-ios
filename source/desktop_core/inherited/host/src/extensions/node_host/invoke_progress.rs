//! Bounded node.invoke progress framing.
//!
//! This module owns only progress-frame sequencing and UTF-8-safe chunking.
//! Gateway delivery, backpressure, credentials, and invocation settlement stay
//! with their existing transport and invocation owners.

use std::time::{Duration, Instant};

pub const PROGRESS_CHUNK_BYTES: usize = 16 * 1024;
pub const MIN_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(250);
pub const MAX_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInvokeProgressFrame {
    pub invoke_id: String,
    pub node_id: String,
    pub seq: u64,
    pub chunk: String,
}

#[derive(Debug)]
pub struct NodeInvokeProgressWriter {
    invoke_id: String,
    node_id: String,
    next_seq: u64,
    heartbeat_interval: Duration,
    last_progress_at: Instant,
    recurring_heartbeats: bool,
    stopped: bool,
}

impl NodeInvokeProgressWriter {
    pub fn new(
        invoke_id: impl Into<String>,
        node_id: impl Into<String>,
        idle_timeout: Duration,
        now: Instant,
    ) -> Self {
        let half_timeout = idle_timeout / 2;
        Self {
            invoke_id: invoke_id.into(),
            node_id: node_id.into(),
            next_seq: 0,
            heartbeat_interval: half_timeout
                .max(MIN_HEARTBEAT_INTERVAL)
                .min(MAX_HEARTBEAT_INTERVAL),
            last_progress_at: now,
            recurring_heartbeats: false,
            stopped: false,
        }
    }

    pub fn write(&mut self, text: &str, now: Instant) -> Vec<NodeInvokeProgressFrame> {
        if self.stopped || text.is_empty() {
            return Vec::new();
        }
        self.last_progress_at = now;
        let mut remaining = text;
        let mut frames = Vec::new();
        while !remaining.is_empty() {
            let chunk_len = utf8_prefix_len(remaining, PROGRESS_CHUNK_BYTES);
            if chunk_len == 0 {
                break;
            }
            let (chunk, rest) = remaining.split_at(chunk_len);
            frames.push(self.frame(chunk));
            remaining = rest;
        }
        frames
    }

    pub fn start_heartbeats(&mut self) {
        self.recurring_heartbeats = true;
    }

    pub fn stop_heartbeats(&mut self) {
        self.recurring_heartbeats = false;
    }

    pub fn heartbeat_due(&self, now: Instant) -> bool {
        self.recurring_heartbeats
            && !self.stopped
            && now.duration_since(self.last_progress_at) >= self.heartbeat_interval
    }

    pub fn queue_heartbeat(&mut self, now: Instant) -> Option<NodeInvokeProgressFrame> {
        if !self.heartbeat_due(now) {
            return None;
        }
        self.last_progress_at = now;
        Some(self.frame(""))
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        self.recurring_heartbeats = false;
    }

    pub fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval
    }

    pub fn should_reschedule_heartbeat(&self) -> bool {
        self.recurring_heartbeats && !self.stopped
    }

    fn frame(&mut self, chunk: &str) -> NodeInvokeProgressFrame {
        let frame = NodeInvokeProgressFrame {
            invoke_id: self.invoke_id.clone(),
            node_id: self.node_id.clone(),
            seq: self.next_seq,
            chunk: chunk.to_string(),
        };
        self.next_seq = self.next_seq.saturating_add(1);
        frame
    }
}

fn utf8_prefix_len(text: &str, max_bytes: usize) -> usize {
    if text.len() <= max_bytes {
        return text.len();
    }
    text.char_indices()
        .take_while(|(offset, character)| offset.saturating_add(character.len_utf8()) <= max_bytes)
        .map(|(offset, character)| offset + character.len_utf8())
        .last()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{NodeInvokeProgressWriter, MAX_HEARTBEAT_INTERVAL, MIN_HEARTBEAT_INTERVAL};
    use std::time::{Duration, Instant};

    #[test]
    fn chunks_utf8_without_splitting_and_sequences_frames() {
        let now = Instant::now();
        let mut writer = NodeInvokeProgressWriter::new("invoke", "node", Duration::from_secs(2), now);
        let frames = writer.write("héllo", now);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].seq, 0);
        assert_eq!(frames[0].chunk, "héllo");
        assert_eq!(writer.write("next", now).first().map(|frame| frame.seq), Some(1));
    }

    #[test]
    fn heartbeat_interval_is_bounded_and_stop_is_terminal() {
        let now = Instant::now();
        let mut writer = NodeInvokeProgressWriter::new("invoke", "node", Duration::from_millis(1), now);
        assert_eq!(writer.heartbeat_interval(), MIN_HEARTBEAT_INTERVAL);
        assert!(!writer.heartbeat_due(now + MIN_HEARTBEAT_INTERVAL));
        assert!(writer.queue_heartbeat(now + MIN_HEARTBEAT_INTERVAL).is_none());
        writer.start_heartbeats();
        assert!(writer.queue_heartbeat(now).is_none());
        assert_eq!(writer.queue_heartbeat(now + MIN_HEARTBEAT_INTERVAL).expect("heartbeat").chunk, "");
        writer.stop();
        assert!(!writer.heartbeat_due(now + MAX_HEARTBEAT_INTERVAL));
        assert!(writer.write("late", now + MAX_HEARTBEAT_INTERVAL).is_empty());
    }
}
