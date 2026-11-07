//! Ordered node.invoke.input delivery with bounded pre-handler buffering.
//!
//! A sequence gap is observable but not rejected by the upstream contract;
//! the next expected sequence advances to the received sequence plus one.
//! Input received before the side-effect owner registers is buffered up to
//! 64 KiB, after which the invocation is failed closed.

use std::collections::VecDeque;

pub const MAX_PENDING_INPUT_BYTES: usize = 64 * 1024;

pub struct NodeInvokeInputBuffer {
    next_input_seq: u64,
    pending: VecDeque<String>,
    pending_bytes: usize,
    input_failed: bool,
    handler: Option<Box<dyn FnMut(String) + Send>>,
}

impl Default for NodeInvokeInputBuffer {
    fn default() -> Self {
        Self {
            next_input_seq: 0,
            pending: VecDeque::new(),
            pending_bytes: 0,
            input_failed: false,
            handler: None,
        }
    }
}

impl NodeInvokeInputBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_input_seq(&self) -> u64 {
        self.next_input_seq
    }

    pub fn input_failed(&self) -> bool {
        self.input_failed
    }

    pub fn dispatch(&mut self, seq: u64, payload_json: String) -> bool {
        if self.input_failed || seq < self.next_input_seq {
            return false;
        }
        self.next_input_seq = seq.saturating_add(1);
        if let Some(handler) = self.handler.as_mut() {
            handler(payload_json);
            return true;
        }
        let payload_bytes = payload_json.len();
        if self.pending_bytes.saturating_add(payload_bytes) > MAX_PENDING_INPUT_BYTES {
            self.input_failed = true;
            self.pending.clear();
            self.pending_bytes = 0;
            return false;
        }
        self.pending_bytes += payload_bytes;
        self.pending.push_back(payload_json);
        true
    }

    pub fn register_handler<F>(&mut self, mut handler: F)
    where
        F: FnMut(String) + Send + 'static,
    {
        if self.input_failed {
            return;
        }
        self.handler = Some(Box::new(move |payload| handler(payload)));
        while let Some(payload) = self.pending.pop_front() {
            self.pending_bytes = self.pending_bytes.saturating_sub(payload.len());
            if let Some(handler) = self.handler.as_mut() {
                handler(payload);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NodeInvokeInputBuffer, MAX_PENDING_INPUT_BYTES};
    use std::sync::{Arc, Mutex};

    #[test]
    fn buffers_until_handler_and_accepts_sequence_gaps() {
        let mut buffer = NodeInvokeInputBuffer::new();
        assert!(buffer.dispatch(0, "first".into()));
        assert!(buffer.dispatch(2, "gap".into()));
        assert_eq!(buffer.next_input_seq(), 3);
        let received = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&received);
        buffer.register_handler(move |payload| {
            target.lock().expect("test lock").push(payload);
        });
        assert_eq!(
            *received.lock().expect("test lock"),
            vec!["first".to_string(), "gap".to_string()]
        );
    }

    #[test]
    fn rejects_duplicates_and_fails_closed_on_buffer_overflow() {
        let mut buffer = NodeInvokeInputBuffer::new();
        assert!(buffer.dispatch(1, "accepted".into()));
        assert!(!buffer.dispatch(1, "duplicate".into()));
        assert!(!buffer.dispatch(2, "x".repeat(MAX_PENDING_INPUT_BYTES)));
        assert!(buffer.input_failed());
        assert!(!buffer.dispatch(3, "late".into()));
    }
}
